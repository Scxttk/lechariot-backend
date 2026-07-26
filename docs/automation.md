# Nächtliche Automatisierung mit launchd (macOS)

Der launchd-Agent `de.lechariot.nightly` ruft jede Nacht um **06:30** die
Angebote aller Ketten ab (`fetch --all-stores`), lädt sie nach Supabase hoch
(`push --region`) und prüft danach die Watchlist. Herzstück ist
`scripts/nightly.sh`; das plist ruft nur dieses Skript auf.

## 1. Voraussetzungen

- `lechariot` installiert, z. B. mit `cargo install --path .`
  (Binary landet in `~/.cargo/bin/lechariot` — das Skript findet es dort
  automatisch).
- Supabase-Projekt mit Tabelle `public.offers` und Service-Role-Key.
- Optional: Rewe-Zertifikat (siehe [rewe-cert.md](rewe-cert.md)).

## 2. Konfigurationsdatei anlegen

Alle Einstellungen — inklusive der Secrets — liegen in
`~/.config/lechariot/env`. **Diese Datei niemals einchecken.**

```sh
mkdir -p ~/.config/lechariot
cp scripts/env.example ~/.config/lechariot/env
chmod 600 ~/.config/lechariot/env
$EDITOR ~/.config/lechariot/env
```

Pflichtwerte: `ZIP`, `SUPABASE_URL`, `SUPABASE_SERVICE_KEY`.
Optional: `DB`, `LECHARIOT_BIN`, `REWE_CERT`/`REWE_KEY`, `NTFY_TOPIC`
(alle in `scripts/env.example` erklärt).

## 3. Testlauf ohne Upload

Vor der Installation lohnt ein Probelauf. `--dry-run` benutzt eine
Wegwerf-Datenbank und übergibt `--dry-run` an `push` — es wird nichts
hochgeladen:

```sh
scripts/nightly.sh --dry-run
```

## 4. Agent installieren

```sh
scripts/install-launchd.sh
```

Das Skript rendert das plist (mit den richtigen Pfaden) nach
`~/Library/LaunchAgents/de.lechariot.nightly.plist`, prüft es mit
`plutil -lint` und aktiviert es per `launchctl bootstrap`. Sofort testen:

```sh
launchctl kickstart gui/$(id -u)/de.lechariot.nightly
```

## 5. Logs prüfen

Jeder Lauf schreibt eine eigene Logdatei mit Zeitstempeln nach
`~/Library/Logs/lechariot/` (die letzten 14 Läufe werden aufbewahrt):

```sh
ls -lt ~/Library/Logs/lechariot/
tail -f ~/Library/Logs/lechariot/nightly-*.log
```

`launchd.out.log` / `launchd.err.log` im selben Ordner fangen nur Fehler ab,
die vor dem Skript-Logging passieren (z. B. wenn bash das Skript nicht
findet).

## 6. Benachrichtigungen (optional)

Mit gesetztem `NTFY_TOPIC` in der Env-Datei schickt das Skript eine
Push-Nachricht über [ntfy.sh](https://ntfy.sh):

- bei **Fehlschlag** des Laufs (fetch oder push kaputt),
- bei **Watchlist-Treffern** nach dem Push (`watch check` mit Exit-Code 1).

Topic frei wählen (wie ein Passwort behandeln!) und die ntfy-App aufs
Handy — fertig. Ohne `NTFY_TOPIC` wird der Schritt still übersprungen.

## 7. Deinstallieren

```sh
scripts/install-launchd.sh uninstall
```

Stoppt den Agent und löscht das plist. Logs und Datenbank bleiben liegen.

## Troubleshooting

- **Mac schläft um 06:30:** kein Problem — launchd holt bei
  `StartCalendarInterval` verpasste Läufe nach dem Aufwachen nach. Der Lauf
  startet dann kurz nachdem der Mac wieder wach ist. (Ist der Mac komplett
  ausgeschaltet, entfällt der Lauf ersatzlos.)
- **„Konfigurationsdatei fehlt“ im Log:** Schritt 2 vergessen —
  `~/.config/lechariot/env` anlegen.
- **„lechariot-Binary nicht gefunden“:** `cargo install --path .` ausführen
  oder `LECHARIOT_BIN` in der Env-Datei auf das Binary zeigen lassen
  (launchd startet mit minimalem `PATH`).
- **Rewe/Kaufland/EDEKA als FEHLER in der Zusammenfassung:** einzelne
  Ketten-Fehler brechen den Lauf nicht ab; Rewe braucht ein Zertifikat,
  Kaufland eine Filiale in PLZ-Nähe.
- **Läuft der Agent überhaupt?**
  `launchctl print gui/$(id -u)/de.lechariot.nightly` zeigt Status und
  letzten Exit-Code.
- **Push schlägt fehl:** `SUPABASE_URL`/`SUPABASE_SERVICE_KEY` prüfen
  (Service-Role-Key, nicht der anon key) — Details stehen im Lauf-Log.
