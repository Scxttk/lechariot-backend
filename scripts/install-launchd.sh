#!/usr/bin/env bash
# install-launchd.sh — installiert oder entfernt den nächtlichen
# lechariot-launchd-Agent (de.lechariot.nightly, täglich 06:30).
#
#   scripts/install-launchd.sh              installieren + aktivieren
#   scripts/install-launchd.sh uninstall    deaktivieren + entfernen
#
# Voraussetzung: ~/.config/lechariot/env existiert (siehe scripts/env.example).

set -euo pipefail

LABEL="de.lechariot.nightly"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="$SCRIPT_DIR/$LABEL.plist"
TARGET="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG_DIR="$HOME/Library/Logs/lechariot"
DOMAIN="gui/$(id -u)"

uninstall() {
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null \
        && echo "Agent $LABEL gestoppt." \
        || echo "Agent $LABEL war nicht geladen."
    if [ -f "$TARGET" ]; then
        rm "$TARGET"
        echo "Entfernt: $TARGET"
    fi
    echo "Deinstallation abgeschlossen."
}

install() {
    [ -f "$TEMPLATE" ] || { echo "Vorlage fehlt: $TEMPLATE" >&2; exit 1; }
    if [ ! -f "$HOME/.config/lechariot/env" ]; then
        echo "WARNUNG: ~/.config/lechariot/env fehlt — der nächtliche Lauf wird" >&2
        echo "fehlschlagen. Vorlage: $SCRIPT_DIR/env.example" >&2
    fi

    mkdir -p "$HOME/Library/LaunchAgents" "$LOG_DIR"

    sed -e "s|__SCRIPT__|$SCRIPT_DIR/nightly.sh|g" \
        -e "s|__LOGDIR__|$LOG_DIR|g" \
        "$TEMPLATE" >"$TARGET"
    plutil -lint "$TARGET"

    # Alte Instanz entladen, falls vorhanden, dann neu laden.
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    launchctl bootstrap "$DOMAIN" "$TARGET"
    launchctl enable "$DOMAIN/$LABEL"

    echo "Installiert: $TARGET"
    echo "Der Agent läuft täglich um 06:30. Sofort testen mit:"
    echo "  launchctl kickstart $DOMAIN/$LABEL"
    echo "Logs: $LOG_DIR/"
}

case "${1:-install}" in
    install)   install ;;
    uninstall) uninstall ;;
    *) echo "Aufruf: $0 [install|uninstall]" >&2; exit 2 ;;
esac
