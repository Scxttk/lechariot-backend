# smartshop

A CLI tool that scrapes offers from German supermarkets, stores them in a
SQLite database, and makes them searchable, comparable, and watchable.
Eight chains are supported (Rewe, Penny, Kaufland, Lidl, Netto, ALDI Nord,
ALDI Süd, EDEKA). The CLI output itself is in German.

## Building & installing

Rust (edition 2024) is required:

```sh
cargo build --release
# Binary: target/release/smartshop
```

The Rewe scrapers additionally need the `rewerse` CLI and a client
certificate — see [docs/rewe-cert.md](docs/rewe-cert.md). Netto, ALDI Süd, and
EDEKA shell out to the system `curl` (Akamai blocks reqwest), so `curl` must be
on the `PATH`.

Every command takes `--db <path>` (default `smartshop.db` in the working
directory). The database is created and migrated on demand.

## Quick start

Fetch and store offers:

```sh
smartshop fetch --store lidl --zip 50667
# Suche Lidl-Markt für PLZ 50667...
# Markt gefunden: Lidl Deutschland (ID: LIDL_DE)
# Lade Angebote...
# 470 Angebote gefunden.
# 470 Angebote in 'smartshop.db' gespeichert.
```

All chains at once (`--all-stores`, with a per-chain summary; individual
failures don't abort the run):

```sh
smartshop fetch --all-stores --zip 50667
```

`--dry-run` only prints without storing; `--notify` checks the watchlist after
the fetch.

## Commands

### search — search offers by title

```sh
smartshop search Butter
#   [Wochen-Angebote] Butter (250-g-Packung) — 1.29 €  [2026-07-13 – 2026-07-18]
#   ...
```

Optionally `--max-price <euro>`.

### compare — price of a product across all markets

```sh
smartshop compare Milch
# MÜLLER Reine Buttermilch*
#   Penny Am Eigelstein    0.66 €  (1.32 €/kg) (je 500 g)
#   ...
```

Grouped by normalized product name, cheapest market first, with unit price
where derivable.

### stats — per-market statistics + top discounts

```sh
smartshop stats
# Angebote pro Markt:
#   Filiale                      Angebote  Gültigkeit              Ø Rabatt
#   Penny Am Eigelstein               542  2026-07-13 – 2026-07-26 33 %
#   ...
# Top 10 Rabatte:
#    1. -80 %  UNCLE SAM Herren-T-Shirt* je Stück — 3.99 € statt 19.99 € (Penny Am Eigelstein)
```

### history — price history of a product

```sh
smartshop history Butter
```

Shows, per title/market, the prices seen over time (from `price_history`).

### deals — price drops

```sh
smartshop deals            # all recorded drops
smartshop deals --since 7  # only the last 7 days
```

### watch — watchlist (cron-friendly)

```sh
smartshop watch add Kaffee --max-price 5
# Watch #1 angelegt: 'Kaffee' (bis 5.00 €)
smartshop watch list
smartshop watch check   # prints hits, exit code 1 if there are any
smartshop watch remove 1
```

`watch check` exits with **exit code 1** as soon as at least one watch matches
(0 otherwise) — see [docs/cron.md](docs/cron.md) for notifications.

### list — shopping list

```sh
smartshop list add Butter
smartshop list show
smartshop list suggest
# Butter                   0.66 € bei Penny Am Eigelstein — MÜLLER Reine Buttermilch*  (1.32 €/kg)
smartshop list remove Butter
smartshop list clear
```

`suggest` finds the cheapest matching offer per item across all markets.

### export — JSON or CSV

```sh
smartshop export --format csv --query Butter > butter.csv
smartshop export --format json --out angebote.json
```

Without `--out` the output goes to stdout; `--query` filters by title/subtitle.

### serve — web dashboard + read-only JSON API

```sh
smartshop serve --port 8080
# Web-UI läuft auf http://0.0.0.0:8080, JSON-API auf http://0.0.0.0:8080/api (DB: smartshop.db)
```

The web dashboard lives at `/`, the JSON endpoints under `/api/*`
(see below and [docs/cron.md](docs/cron.md)).

### push — Supabase push

Uploads the stored offers to the Supabase table `public.offers`
(PostgREST upsert on `market_id,product,valid_from,region`, batches of 100)
and registers the ZIP code in `public.regions`. Outdated weeks for the
respective **branch** are deleted first — offers belong to a store, not to a
chain in a ZIP code (see `supabase/migration_v13_offer_market_id.sql`).
Offers without a price are skipped.

```sh
export SUPABASE_URL="https://xyz.supabase.co"
export SUPABASE_SERVICE_KEY="…"   # service role key, not the anon key

smartshop push --region 01219                  # all markets
smartshop push --store lidl --region 01219     # a single chain
smartshop push --dry-run                       # print only, no network
```

`--region <ZIP>` is required (except with `--dry-run`). For the weekly sync
via cron, simply push after the fetch:

For the regular sync, `scripts/nightly.sh` handles sync-regions plus the
watchlist check in one go; on macOS, `scripts/install-launchd.sh` installs the
ready-made nightly agent — see [docs/automation.md](docs/automation.md).

During the push, every offer is deterministically enriched (`src/enrich.rs`):
`category` holds one of 15 fixed categories (instead of the raw scraper
category), `emoji` a matching emoji from a curated keyword table — with no
match, the category's default emoji, never null. The app hardcodes this list;
changes only go out together with an app update (regression test in
`tests/enrich.rs`).

In addition, every row carries a product image in `image_url`: the scrapers
deliver the retailer image URLs, the push mirrors them into the public
Supabase storage bucket `offer-images` (`src/storage.rs`) and writes the
stable bucket URL into the row — retailer CDNs rotate their paths weekly and
some block hotlinking. The mirroring is idempotent (object path = sha256 of
the source URL, upload with `x-upsert`); already-uploaded images are tracked
in the local table `uploaded_images`, so nightly runs only touch new images.
Failures of individual images don't abort the push — the retailer URL stays in
place, and the emoji is the last fallback in the app. `--no-mirror-images`
disables the mirroring.

| Category | Default emoji | | Category | Default emoji |
|---|---|---|---|---|
| Obst & Gemüse | 🥬 | | Alkohol | 🍺 |
| Molkerei & Eier | 🥛 | | Vorräte & Kochen | 🥫 |
| Fleisch & Wurst | 🥩 | | Drogerie | 🧴 |
| Fisch | 🐟 | | Haushalt | 🧽 |
| Backwaren | 🥖 | | Tierbedarf | 🐾 |
| Tiefkühl | ❄️ | | Kinder | 🧸 |
| Süßes & Snacks | 🍬 | | Sonstiges | 🛒 |
| Getränke | 🥤 | | | |

### sync-regions — multi-region sync

Reads all active regions from the Supabase table `public.regions`
(sorted by `requested_at`, oldest request first) and runs the full pipeline
per ZIP code: fetch all chains, report found stores to `public.markets`, push
offers with `--region <ZIP>`. The local `offers` table is cleared per region
so that no offers from other regions get pushed along.

```sh
export SUPABASE_URL="https://xyz.supabase.co"
export SUPABASE_SERVICE_KEY="…"

smartshop sync-regions                          # up to 10 regions
smartshop sync-regions --max-regions 3          # change the limit
smartshop sync-regions --dry-run                # no Supabase writes
```

Failures of individual regions don't abort the run; exit code ≠ 0 only if all
regions fail or the table is empty/unreachable. Prerequisite: the migration
`supabase/migration_v3_multi_region.sql` has been run once in the Supabase SQL
editor (see [docs/ci.md](docs/ci.md)).

#### `--market-id` — sync a single store

```sh
smartshop sync-regions --market-id 1766063      # REWE am Postplatz, 01067
```

The ZIP path asks the store finder which store is *nearest* and takes the
first hit. That answer is unique per chain and ZIP, and the offers aren't:
measured on 2026-07-25, the three REWE stores in 01067 published three
different flyers in the same week — Friedrichstadt carried 101 offers the
Postplatz store did not, and a Coca-Cola cost €0.75 at two of them and €1.49
at the third.

With `--market-id` the store is picked from the directory (`public.branches`)
instead, so there is nothing left to search: the offers are fetched for that
store, written under its `market_id` and pushed with the store's own ZIP as
`region`. Conflicts with `--only`.

### branches-sync — store directory

Fills `public.branches`, the directory the app searches to show *nearby*
stores. This is a different question from "which store do we scrape": until
now the backend kept exactly one store per chain and ZIP — whichever the
finder returned first — and threw the rest of the response away. That is where
rows like "REWE in 01219" come from: searching REWE for `01219` returns five
stores, all of them in 01257/01259/01277.

Chains differ wildly in what a full directory costs, and the command follows
that: Kaufland (787 stores) and Penny (2120) publish theirs in a single
request and are fetched nationwide; everything else is fetched per area, so
the directory grows where the app is actually used.

```sh
export SUPABASE_URL="https://xyz.supabase.co"
export SUPABASE_SERVICE_KEY="…"

smartshop branches-sync                            # Kaufland + Penny, nationwide
smartshop branches-sync --area 01219               # plus every chain around one ZIP
smartshop branches-sync --from-regions             # areas = active regions
smartshop branches-sync --area 01219 --dry-run     # no Supabase writes
```

For an area, REWE is searched twice — once by ZIP (the neighbourhood) and once
by city (the centre; that search caps at 20 hits) — and everything outside
`--radius-km` (default 25) is dropped afterwards, because each finder draws
its own boundary differently. EDEKA is the expensive one: its offer id sits
behind a redirect, so every store costs an extra request.

Failures of individual chains only warn. Prerequisite: the migration
`supabase/migration_v12_branches.sql` has been run once in the Supabase SQL
editor. Refreshed weekly by `.github/workflows/branches.yml`.

## Scraper support

| Chain | Auth required | Market | Offers (ballpark) |
|---|---|---|---|
| Rewe | yes — TLS cert + `rewerse` | store-specific (ZIP) | ~323 |
| Penny | no | store-specific (ZIP) | ~540 |
| Kaufland | no | store-specific (ZIP) | varies (not every ZIP) |
| Lidl | no | national¹ | ~470 |
| Netto | no (curl) | store-specific (ZIP) | ~190 |
| ALDI Nord | no | national¹ | ~240 |
| ALDI Süd | no (curl) | national¹ | ~75 |
| EDEKA | no (curl) | store-specific (ZIP) | varies (not every region) |

Ballpark numbers from a live fetch for ZIP 50667 (Cologne) on 2026-07-17;
actual numbers vary per week and store. Kaufland and EDEKA returned no hits
for this ZIP — both are region-dependent. The Rewe number (~323) comes from a
live fetch for ZIP 01219 (Dresden, store "REWE Supermarkt", ID 565005) on
2026-07-18.

¹ The *offers* are national, the *presence* no longer is: `find_market`
queries the chain's official store finder (Lidl: Bing Spatial Data Service
behind lidl.de, ALDI Nord/Süd: Uberall locator; ZIP geocoding via Nominatim,
`src/scrapers/store_finder.rs`). If there is a store within 15 km, it is
registered with name, ID, and coordinates; otherwise the chain is not
registered for the region at all. If the finder itself fails (network, format
change), the sync falls back to the national placeholder with a WARN. Penny
and Kaufland deliver their store coordinates anyway — `markets.lat/lon`
(migration_v8) carries them, NULL where unknown.

## HTTP API endpoints

All endpoints are `GET` and return JSON; errors as `{"error": "..."}` with
status 400 (missing/invalid parameter) or 500. Via `smartshop serve` they are
reachable under the **`/api`** prefix (e.g. `/api/offers?q=Butter`); the root
paths belong to the web dashboard.

| Endpoint | Parameters | Returns |
|---|---|---|
| `/markets` | – | Stored stores |
| `/offers` | `q` (required), `max_price`, `market` | Offers matching the search |
| `/compare` | `q` (required) | Price comparison, grouped per product |
| `/stats` | – | Offers per store + top 10 discounts |
| `/history` | `q` (required) | Price history |
| `/deals` | `since` (days, optional) | Price drops |
| `/watches` | – | Watchlist |
| `/watches/check` | – | `{"hits": bool, "watches": [...]}` |
| `/list` | – | Shopping list |
| `/list/suggest` | – | Cheapest market per list item |

The API is read-only and **unauthenticated** — only run it on a trusted
network.

## Web dashboard

`smartshop serve` additionally delivers a server-rendered HTML dashboard —
entirely without JavaScript, CSS embedded, no external assets.

| Page | Content |
|---|---|
| `/` | Overview: offers per market, validity periods, top discounts |
| `/search?q=…` | Offer search with market, price, and unit price |
| `/compare?q=…` | Price comparison per product across all markets, cheapest first |
| `/watchlist` | View, create, and remove watches (POST forms) |
| `/history?offer=…` | Price history as an inline SVG sparkline plus table |

Like the JSON API, the dashboard is unauthenticated — only run it on a
trusted network. The watchlist (create/remove) is the only thing that writes.

## Database & schema

SQLite in WAL mode. Tables: `markets`, `offers`, `price_history`, `watches`,
`shopping_list`, `uploaded_images`. The schema version lives in
`PRAGMA user_version`; `db::open()` automatically migrates to the current
version on open. Schema changes bump `SCHEMA_VERSION` and add a migration step
in `migrate()` (`src/db.rs`) — existing databases are updated in place.

The Supabase schema (tables `offers`, `regions`, `markets` plus the storage
bucket `offer-images`) lives canonically under [`supabase/`](supabase/) —
`schema.sql` + migrations; for new projects: run `setup_full.sql`, then
`migration_regions.sql`, `migration_v3_multi_region.sql`,
`migration_v4_region_trigger.sql`, `migration_v5_image_url.sql`, and
`migration_v6_storage_bucket.sql` in the SQL editor.

## Price history

In addition to the weekly `offers` table, every push writes the same rows to
the Supabase table `price_history` (not to be confused with the local SQLite
table of the same name) — as a permanent weekly snapshot so the app can later
show price histories. The upsert key is
`(market, product, region, valid_from)`: pushing the same week again updates
the rows instead of duplicating them. Rows without a price are skipped.
Failures while writing the history (e.g. missing table) only emit a warning —
the actual offers push never fails because of it.

**Manual migration:** run `supabase/migration_v7_price_history.sql` once in
the Supabase SQL editor. Until then, every push still succeeds but reports
`WARNUNG: Preis-Historie fehlgeschlagen`.

## Documentation

- [docs/rewe-cert.md](docs/rewe-cert.md) — setting up the Rewe TLS certificate
- [docs/automation.md](docs/automation.md) — nightly launchd agent (macOS)
- [docs/cron.md](docs/cron.md) — cron automation and the JSON API
- [docs/ci.md](docs/ci.md) — GitHub Actions CI, nightly sync, and migrations
- [scripts/nightly.sh](scripts/nightly.sh) — pipeline script sync-regions + watch check (with single-ZIP fallback)
