# lechariot

Scrapes offers from German supermarkets into SQLite, then makes them
searchable, comparable and watchable — and pushes them to Supabase for the
[iOS app](https://github.com/Scxttk/lechariot-app). Nine chains: REWE, Penny,
Kaufland, Lidl, Netto, ALDI Nord, ALDI Süd, EDEKA, NORMA. The CLI speaks
German.

## Building

Rust (edition 2024):

```sh
cargo build --release   # target/release/lechariot
```

REWE additionally needs the `rewerse` CLI and a client certificate
([docs/rewe-cert.md](docs/rewe-cert.md)). Netto, ALDI Süd and EDEKA shell out
to the system `curl`, because Akamai blocks reqwest — `curl` has to be on the
`PATH`. Lidl needs `pdftotext` (poppler-utils).

Every command takes `--db <path>` (default `lechariot.db`); the database is
created and migrated on open.

## Quick start

```sh
lechariot fetch --all-stores --zip 50667   # scrape every chain for a ZIP
lechariot search Butter                    # what's on offer
lechariot compare Milch                    # same product across markets
lechariot serve --port 8080                # dashboard + read-only JSON API
```

Full command reference: [docs/cli.md](docs/cli.md).

| Command | Does |
|---|---|
| `fetch` | scrape and store offers |
| `search` / `compare` / `stats` | query what's stored |
| `history` / `deals` | prices over time, price drops |
| `watch` | watchlist; `watch check` exits 1 on a hit (cron-friendly) |
| `list` | shopping list, `list suggest` finds the cheapest market per item |
| `export` | JSON or CSV |
| `serve` | web dashboard + JSON API |
| `push` | upload offers to Supabase |
| `sync-regions` | full pipeline per ZIP, driven by `public.regions` |
| `branches-sync` | fill `public.branches`, the store directory the app searches |

## Supabase

`push` and `sync-regions` need:

```sh
export SUPABASE_URL="https://xyz.supabase.co"
export SUPABASE_SERVICE_KEY="…"   # service role key, not the anon key
```

`push --region <ZIP>` upserts on `market_id,product,valid_from,region` in
batches of 100 and deletes outdated weeks **per branch** first — offers belong
to a store, not to a chain in a ZIP code. Offers without a price are skipped.

`sync-regions` reads the active regions (oldest request first) and runs the
whole pipeline per ZIP; a failing region doesn't abort the run. `--market-id`
scrapes one specific store instead of "whatever the finder returns for this
ZIP" — that distinction is real: on 2026-07-25 the three REWE stores in 01067
published three different flyers in the same week, and a Coca-Cola cost €0.75
at two of them and €1.49 at the third.

`branches-sync` answers a different question than "which store do we scrape":
it fills the directory the app searches for *nearby* stores. Kaufland (787)
and Penny (2120) publish theirs in one request and are fetched nationwide;
every other chain is fetched per area, so the directory grows where the app is
used. Refreshed weekly by `.github/workflows/branches.yml`.

Migrations live under [`supabase/`](supabase/) and are run by hand in the SQL
editor. For the regular sync, [`scripts/nightly.sh`](scripts/nightly.sh) does
sync-regions plus the watch check in one go; on macOS
`scripts/install-launchd.sh` installs the nightly agent
([docs/automation.md](docs/automation.md)).

### Enrichment

Every pushed offer is deterministically enriched (`src/enrich.rs`): `category`
is one of 15 fixed values rather than the raw scraper category, and `emoji` a
match from a curated keyword table, falling back to the category's default —
never null. The app hardcodes this list, so changes ship with an app update
(regression test in `tests/enrich.rs`).

| | | | | | |
|---|---|---|---|---|---|
| Obst & Gemüse | 🥬 | Getränke | 🥤 | Drogerie | 🧴 |
| Molkerei & Eier | 🥛 | Alkohol | 🍺 | Haushalt | 🧽 |
| Fleisch & Wurst | 🥩 | Vorräte & Kochen | 🥫 | Tierbedarf | 🐾 |
| Fisch | 🐟 | Backwaren | 🥖 | Kinder | 🧸 |
| Tiefkühl | ❄️ | Süßes & Snacks | 🍬 | Sonstiges | 🛒 |

`image_url` carries the retailer's own image URL as-is and the app loads it
straight from their CDN — hotlinking freely accessible content instead of
hosting copies, which also saves all image egress. If a CDN rotates paths or
blocks hotlinking, the app falls back to the emoji. Mirroring into the
`offer-images` bucket still exists as `push --mirror-images`.

## Scrapers

| Chain | Auth | Market | Offers |
|---|---|---|---|
| REWE | TLS cert + `rewerse` | per store (ZIP) | ~323 |
| Penny | – | per store (ZIP) | ~540 |
| Kaufland | – | per store (ZIP) | varies |
| Lidl | needs `pdftotext` | sales region | ~382 |
| Netto | curl | per store (ZIP) | ~190 |
| ALDI Nord | – | national* | ~240 |
| ALDI Süd | curl | national* | ~75 |
| EDEKA | curl | per store (ZIP) | varies |
| NORMA | – | national* | ~220 |

Counts from a live fetch for 50667 on 2026-07-17 (REWE: 01219 on 2026-07-18);
they move every week. Kaufland and EDEKA returned nothing for that ZIP — both
are region-dependent.

\* The *offers* are national, the *presence* isn't: `find_market` asks the
chain's own store finder and only registers the chain if there's a store
within 15 km. If the finder itself fails, the sync falls back to the national
placeholder with a WARN.

**Lidl** comes from Lidl's own weekly leaflet — a PDF with a real text layer,
read via `pdftotext -bbox-layout`, plus the online-shop articles from the
leaflet JSON. Struck-through prices, page-accurate validity windows,
categories; no API key, no LLM. It replaced the third-party marktguru API on
2026-07-31 after covering it to 96 %. What that cost: marktguru was the only
Lidl source carrying product images, so until crops are extracted from the
leaflet pages every Lidl offer falls back to its emoji.
`LIDL_SOURCE=prospekt-llm` reads the same text layer through GitHub Models to
pick up tiles the geometry misses — an add-on, not a replacement: every price
must appear verbatim in the page text and pass the arithmetic check, so the
model can miss rows but cannot invent them
([docs/scrapers](docs/scrapers/README.md)).

## HTTP API and dashboard

`lechariot serve` delivers a server-rendered HTML dashboard (no JavaScript,
CSS embedded) at `/` and read-only JSON under `/api`.

| Endpoint | Parameters |
|---|---|
| `/markets`, `/stats`, `/deals`, `/watches`, `/list` | – (`deals` takes `since`) |
| `/offers` | `q` (required), `max_price`, `market` |
| `/compare`, `/history` | `q` (required) |
| `/watches/check`, `/list/suggest` | – |

Errors come back as `{"error": "…"}` with status 400 or 500. Both the API and
the dashboard are **unauthenticated** — only run this on a trusted network.
The watchlist forms are the only thing that writes.

## Database

SQLite in WAL mode; tables `markets`, `offers`, `price_history`, `watches`,
`shopping_list`, `uploaded_images`. The schema version lives in
`PRAGMA user_version` and `db::open()` migrates on open — a schema change
bumps `SCHEMA_VERSION` and adds a step in `migrate()` (`src/db.rs`).

Every push also writes its rows to the Supabase table `price_history` (not the
local one of the same name) as a permanent weekly snapshot, keyed
`(market, product, region, valid_from)`. A failure there only warns; the
offers push never fails because of it.

## Docs

- [docs/cli.md](docs/cli.md) — every command in full
- [docs/rewe-cert.md](docs/rewe-cert.md) — the REWE TLS certificate
- [docs/ci.md](docs/ci.md) — GitHub Actions, nightly sync, migrations
- [docs/automation.md](docs/automation.md) · [docs/cron.md](docs/cron.md) — running it unattended
