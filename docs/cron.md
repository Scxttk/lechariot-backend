# lechariot automatisieren: Cron, launchd und die JSON-API

> **macOS-Nutzer:** Der empfohlene Weg ist der fertige launchd-Agent —
> Einrichtung in [automation.md](automation.md). Diese Seite beschreibt die
> Cron-Variante und die JSON-API.

Zwei Bausteine bieten sich für Automatisierung an:

1. `scripts/nightly.sh` — nächtliche Pipeline: Angebote aller Ketten
   abrufen, nach Supabase pushen, Watchlist prüfen (Konfiguration über
   `~/.config/lechariot/env`, siehe `scripts/env.example`).
2. `lechariot watch check` — meldet Watchlist-Treffer per **Exit-Code 1**
   (0 = keine Treffer), ideal als Cron-Bedingung für Benachrichtigungen.

## Voraussetzungen

- `lechariot` gebaut und im `PATH` (oder `LECHARIOT_BIN` setzen).
- `curl` im `PATH` (Netto, ALDI Süd und EDEKA nutzen System-curl).
- Für Rewe: `rewerse` + Zertifikat, siehe [rewe-cert.md](rewe-cert.md) —
  ohne läuft `--all-stores` trotzdem durch, Rewe erscheint dann als Fehler
  in der Zusammenfassung.

Hinweis zum Fehlerverhalten: `fetch --all-stores` bricht bei einzelnen
Ketten-Fehlern **nicht** ab und endet mit Exit-Code 0, solange der Lauf als
Ganzes funktioniert. Das Lauf-Log (unter `~/Library/Logs/lechariot/`)
enthält die Zusammenfassung pro Kette.

## Crontab (Linux/macOS)

`crontab -e`, dann z. B. täglicher Abruf um 6:30 und Watchlist-Check um 7:00:

```cron
# Komplette Pipeline (fetch + push + watch check); Konfiguration liest das
# Skript aus ~/.config/lechariot/env
30 6 * * * /pfad/zu/lechariot/scripts/nightly.sh

# Watchlist prüfen: Exit-Code 1 bei Treffern -> Benachrichtigung schicken
0 7 * * * /usr/local/bin/lechariot watch check --db "$HOME/.local/share/lechariot/lechariot.db" > /tmp/lechariot-watch.txt || mail -s "lechariot: neue Deals" ich@example.com < /tmp/lechariot-watch.txt
```

Der Watch-Check nutzt den Exit-Code-Vertrag: `watch check` schreibt die
Treffer nach stdout und beendet sich mit 1, sobald mindestens ein Watch
anschlägt. `||` führt den Notifier also genau dann aus, wenn es Deals gibt.
Statt `mail` funktioniert jeder Notifier, z. B. `notify-send` (Linux-Desktop)
oder ein `curl`-POST an ntfy/Slack:

```cron
0 7 * * * lechariot watch check --db /pfad/lechariot.db > /tmp/w.txt || curl -s -d @/tmp/w.txt ntfy.sh/mein-lechariot-topic
```

Alternativ übernimmt `nightly.sh` das bereits: mit gesetztem `NTFY_TOPIC`
in `~/.config/lechariot/env` benachrichtigt es nach dem Push automatisch
bei Watchlist-Treffern und Fehlläufen.

## launchd (macOS)

Cron funktioniert auch auf macOS, launchd ist aber der native Weg und holt
verpasste Läufe nach dem Aufwachen nach. Der fertige Agent
(`de.lechariot.nightly`, täglich 06:30) liegt in `scripts/` und wird mit

```sh
scripts/install-launchd.sh
```

installiert — komplette Anleitung inklusive Logs, Benachrichtigungen und
Deinstallation in [automation.md](automation.md).

## JSON-API für Dashboards

`lechariot serve` startet eine **rein lesende** HTTP-API auf Port 8080
(änderbar mit `--port`), z. B. für ein Grafana-/Homepage-Widget oder ein
eigenes Dashboard:

```sh
lechariot serve --db "$HOME/.local/share/lechariot/lechariot.db" --port 8080
```

| Endpoint | Parameter | Liefert |
|---|---|---|
| `GET /api/markets` | – | Gespeicherte Filialen |
| `GET /api/offers` | `q` (Pflicht), `max_price`, `market` | Angebote zur Suche |
| `GET /api/compare` | `q` (Pflicht) | Preisvergleich, gruppiert pro Produkt |
| `GET /api/stats` | – | Angebote/Filiale + Top-10-Rabatte |
| `GET /api/history` | `q` (Pflicht) | Preisverlauf |
| `GET /api/deals` | `since` (Tage, optional) | Preissenkungen |
| `GET /api/watches` | – | Watchlist-Einträge |
| `GET /api/watches/check` | – | `{"hits": bool, "watches": [...]}` |
| `GET /api/list` | – | Einkaufsliste |
| `GET /api/list/suggest` | – | Günstigster Markt je Listen-Artikel |

Fehler kommen als JSON (`{"error": "..."}`) mit 400 (fehlender/ungültiger
Parameter) bzw. 500. Beispiel für ein Dashboard-Polling:

```sh
curl -s "http://localhost:8080/api/watches/check" | jq '.hits'
```

Die API bindet an `0.0.0.0` und hat **keine Authentifizierung** — im
Heimnetz betreiben oder per Reverse-Proxy/Firewall absichern. Als Dauerdienst
eignet sich wieder launchd/systemd (Programm: `lechariot serve --db ...`).
