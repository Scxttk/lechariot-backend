#!/usr/bin/env bash
# nightly.sh — kompletter nächtlicher smartshop-Lauf: sync (alle
# aktiven Regionen aus Supabase: fetch + push pro PLZ), danach watch check.
# Schlägt sync fehl (Tabelle leer/unerreichbar, alle Filialen
# gescheitert), fällt das Skript auf den alten Einzel-PLZ-Pfad zurück
# (fetch --all-stores + push mit ZIP), damit die Pipeline nie dunkel geht.
# Gedacht als Ziel für den launchd-Agent (scripts/de.smartshop.nightly.plist),
# läuft aber genauso von Hand oder aus cron.
#
# Konfiguration kommt aus ~/.config/smartshop/env (siehe scripts/env.example).
# Pflicht dort: ZIP (Bootstrap-Fallback), SUPABASE_URL, SUPABASE_SERVICE_KEY.
# Optional: DB, SMARTSHOP_BIN, REWE_CERT, REWE_KEY, NTFY_TOPIC.
#
# Logs: ~/Library/Logs/smartshop/nightly-JJJJMMTT-HHMMSS.log,
# es werden die letzten 14 Läufe aufbewahrt.
#
# Aufruf:
#   nightly.sh             normaler Lauf
#   nightly.sh --dry-run   Testlauf: Scratch-DB, `push --dry-run`, kein Upload
#
# Exit-Code: 0 bei Erfolg, !=0 bei Fehler (Konfiguration, fetch oder push).
# Watchlist-Treffer ändern den Exit-Code nicht, lösen aber (falls NTFY_TOPIC
# gesetzt ist) eine ntfy-Benachrichtigung aus.

set -euo pipefail

ENV_FILE="${SMARTSHOP_ENV_FILE:-$HOME/.config/smartshop/env}"
LOG_DIR="$HOME/Library/Logs/smartshop"
KEEP_LOGS=14

DRY_RUN=0
if [ "${1:-}" = "--dry-run" ]; then
    DRY_RUN=1
    shift
fi
if [ $# -gt 0 ]; then
    echo "nightly.sh: unbekanntes Argument: $1 (erlaubt: --dry-run)" >&2
    exit 2
fi

mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/nightly-$(date '+%Y%m%d-%H%M%S').log"

log() {
    printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$LOG_FILE"
}

# ntfy-Benachrichtigung — wird still übersprungen, wenn NTFY_TOPIC leer ist.
notify() {
    local title="$1" body="$2"
    [ -n "${NTFY_TOPIC:-}" ] || return 0
    curl -fsS --max-time 15 \
        -H "Title: $title" \
        -d "$body" \
        "https://ntfy.sh/$NTFY_TOPIC" >/dev/null 2>&1 \
        || log "WARNUNG: ntfy-Benachrichtigung fehlgeschlagen."
}

fail() {
    log "FEHLER: $*"
    notify "smartshop: Nightly fehlgeschlagen" "$* (Log: $LOG_FILE)"
    exit 1
}

# Alte Logs aufräumen: nur die neuesten $KEEP_LOGS behalten.
rotate_logs() {
    local old
    # Dateinamen enthalten den Zeitstempel, Sortierung nach Name = nach Alter.
    old=$(find "$LOG_DIR" -maxdepth 1 -name 'nightly-*.log' | sort -r | tail -n +"$((KEEP_LOGS + 1))") || true
    [ -n "$old" ] && printf '%s\n' "$old" | xargs rm -f --
    return 0
}
rotate_logs

# --- Konfiguration laden -----------------------------------------------------

[ -f "$ENV_FILE" ] || fail "Konfigurationsdatei fehlt: $ENV_FILE (Vorlage: scripts/env.example)."
# shellcheck disable=SC1090
. "$ENV_FILE"

[ -n "${ZIP:-}" ] || fail "ZIP ist in $ENV_FILE nicht gesetzt."

DB="${DB:-$HOME/.local/share/smartshop/smartshop.db}"
if [ "$DRY_RUN" -eq 1 ]; then
    DB="$(mktemp -d)/smartshop-dry-run.db"
    log "Dry-Run: benutze Scratch-DB $DB"
fi
mkdir -p "$(dirname "$DB")"

# Binary neu bauen, damit der Lauf nie ein veraltetes Installat benutzt
# (~/.cargo/bin/smartshop hinkt sonst nach jeder Code-Änderung hinterher).
# Schlägt der Build fehl, brechen wir ab statt mit altem Binary weiterzulaufen.
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -f "$REPO_DIR/Cargo.toml" ]; then
    CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
    command -v "$CARGO" >/dev/null 2>&1 || CARGO="cargo"
    command -v "$CARGO" >/dev/null 2>&1 \
        || fail "cargo nicht gefunden — Rebuild vor dem Sync nicht möglich."
    log "Baue smartshop neu (cargo install --path $REPO_DIR --locked)..."
    "$CARGO" install --path "$REPO_DIR" --locked >>"$LOG_FILE" 2>&1 \
        || fail "cargo install fehlgeschlagen — Abbruch, um nicht mit altem Binary zu laufen (Details: $LOG_FILE)."
else
    log "WARNUNG: Repo nicht gefunden ($REPO_DIR/Cargo.toml fehlt) — benutze vorhandenes Binary."
fi

# Binary finden: SMARTSHOP_BIN aus der Env-Datei, sonst PATH, sonst ~/.cargo/bin
# (launchd startet mit minimalem PATH, cargo install landet dort).
if [ -z "${SMARTSHOP_BIN:-}" ]; then
    if command -v smartshop >/dev/null 2>&1; then
        SMARTSHOP_BIN="$(command -v smartshop)"
    elif [ -x "$HOME/.cargo/bin/smartshop" ]; then
        SMARTSHOP_BIN="$HOME/.cargo/bin/smartshop"
    else
        fail "smartshop-Binary nicht gefunden (weder im PATH noch in ~/.cargo/bin; SMARTSHOP_BIN setzen)."
    fi
fi
[ -x "$SMARTSHOP_BIN" ] || fail "smartshop-Binary nicht ausführbar: $SMARTSHOP_BIN"

# sync liest die Filialliste auch im Dry-Run aus Supabase.
[ -n "${SUPABASE_URL:-}" ] || fail "SUPABASE_URL ist in $ENV_FILE nicht gesetzt."
[ -n "${SUPABASE_SERVICE_KEY:-}" ] || fail "SUPABASE_SERVICE_KEY ist in $ENV_FILE nicht gesetzt."
export SUPABASE_URL="${SUPABASE_URL:-}" SUPABASE_SERVICE_KEY="${SUPABASE_SERVICE_KEY:-}"

# launchd startet mit minimalem PATH; smartshop ruft das `rewerse`-Binary aber
# über den PATH auf (Command::new("rewerse")). Cargo-/Go-bin voranstellen, damit
# der Rewe-Scraper es auch im nächtlichen Lauf findet.
export PATH="$HOME/.cargo/bin:$HOME/go/bin:$PATH"

# Rewe-Zertifikat ist optional; ohne läuft --all-stores durch, Rewe erscheint
# dann als Fehler in der Zusammenfassung (siehe docs/rewe-cert.md).
CERT_ARGS=()
[ -n "${REWE_CERT:-}" ] && CERT_ARGS+=(--cert "$REWE_CERT")
[ -n "${REWE_KEY:-}" ] && CERT_ARGS+=(--key "$REWE_KEY")

# Kommando ausführen, Ausgabe zeilenweise mit Zeitstempel loggen.
run_step() {
    local name="$1"
    shift
    log "Schritt '$name': $*"
    if "$@" 2>&1 | while IFS= read -r line; do log "  $line"; done; then
        log "Schritt '$name' erfolgreich."
    else
        fail "Schritt '$name' fehlgeschlagen (Exit-Code ${PIPESTATUS[0]:-?})."
    fi
}

# --- Pipeline ----------------------------------------------------------------

log "Start Nightly-Lauf (Fallback-PLZ $ZIP, DB $DB, dry-run=$DRY_RUN)"

# Die Nightly soll ALLE gewählten Filialen schaffen — der CLI-Default (25)
# ist ein Schutz für manuelle Läufe, würde hier aber Filialen auf die nächste
# Nacht verschieben (dieselbe Falle wie 2026-07-22 mit den Regionen: 15 aktiv,
# 5 blieben pro Lauf liegen). MAX_BRANCHES in der env übersteuert bei Bedarf.
SYNC_ARGS=(sync --db "$DB" --max-branches "${MAX_BRANCHES:-100}" ${CERT_ARGS[@]+"${CERT_ARGS[@]}"})
[ "$DRY_RUN" -eq 1 ] && SYNC_ARGS+=(--dry-run)

log "Schritt 'sync': ${SYNC_ARGS[*]}"
if "$SMARTSHOP_BIN" "${SYNC_ARGS[@]}" 2>&1 | while IFS= read -r line; do log "  $line"; done; then
    log "Schritt 'sync' erfolgreich."
else
    log "WARNUNG: sync fehlgeschlagen (Exit-Code ${PIPESTATUS[0]:-?}) — Fallback auf Einzel-PLZ $ZIP."
    run_step "fetch (Fallback)" "$SMARTSHOP_BIN" fetch --all-stores --zip "$ZIP" --db "$DB" \
        ${CERT_ARGS[@]+"${CERT_ARGS[@]}"}
    PUSH_ARGS=(push --db "$DB")
    [ "$DRY_RUN" -eq 1 ] && PUSH_ARGS+=(--dry-run)
    run_step "push (Fallback)" "$SMARTSHOP_BIN" "${PUSH_ARGS[@]}"
fi

# Watchlist prüfen: Exit-Code 1 = Treffer -> Benachrichtigung (kein Fehler).
log "Schritt 'watch check':"
if WATCH_OUT="$("$SMARTSHOP_BIN" watch check --db "$DB" 2>&1)"; then
    log "  Keine Watchlist-Treffer."
else
    rc=$?
    if [ "$rc" -eq 1 ]; then
        log "  Watchlist-Treffer:"
        printf '%s\n' "$WATCH_OUT" | while IFS= read -r line; do log "    $line"; done
        notify "smartshop: neue Deals" "$WATCH_OUT"
    else
        fail "watch check fehlgeschlagen (Exit-Code $rc): $WATCH_OUT"
    fi
fi

log "Nightly-Lauf erfolgreich beendet."
