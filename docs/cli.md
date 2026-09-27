# CLI reference

Every command in full. For the short version, start at the [README](../README.md).

Each command takes `--db <path>` (default `lechariot.db` in the working
directory); the database is created and migrated on demand.

## Fetching

```sh
lechariot fetch --store lidl --zip 50667   # one chain
lechariot fetch --all-stores --zip 50667   # all of them, per-chain summary
```

Individual chain failures don't abort a run. `--dry-run` prints without
storing, `--notify` checks the watchlist afterwards.

### search — search offers by title

```sh
lechariot search Butter
#   [Wochen-Angebote] Butter (250-g-Packung) — 1.29 €  [2026-07-13 – 2026-07-18]
#   ...
```

Optionally `--max-price <euro>`.

### compare — price of a product across all markets

```sh
lechariot compare Milch
# MÜLLER Reine Buttermilch*
#   Penny Am Eigelstein    0.66 €  (1.32 €/kg) (je 500 g)
#   ...
```

Grouped by normalized product name, cheapest market first, with unit price
where derivable.

### stats — per-market statistics + top discounts

```sh
lechariot stats
# Angebote pro Markt:
#   Filiale                      Angebote  Gültigkeit              Ø Rabatt
#   Penny Am Eigelstein               542  2026-07-13 – 2026-07-26 33 %
#   ...
# Top 10 Rabatte:
#    1. -80 %  UNCLE SAM Herren-T-Shirt* je Stück — 3.99 € statt 19.99 € (Penny Am Eigelstein)
```

### history — price history of a product

```sh
lechariot history Butter
```

Shows, per title/market, the prices seen over time (from `price_history`).

### deals — price drops

```sh
lechariot deals            # all recorded drops
lechariot deals --since 7  # only the last 7 days
```

### watch — watchlist (cron-friendly)

```sh
lechariot watch add Kaffee --max-price 5
# Watch #1 angelegt: 'Kaffee' (bis 5.00 €)
lechariot watch list
lechariot watch check   # prints hits, exit code 1 if there are any
lechariot watch remove 1
```

`watch check` exits with **exit code 1** as soon as at least one watch matches
(0 otherwise) — see [docs/cron.md](docs/cron.md) for notifications.

### list — shopping list

```sh
lechariot list add Butter
lechariot list show
lechariot list suggest
# Butter                   0.66 € bei Penny Am Eigelstein — MÜLLER Reine Buttermilch*  (1.32 €/kg)
lechariot list remove Butter
lechariot list clear
```

`suggest` finds the cheapest matching offer per item across all markets.

### export — JSON or CSV

```sh
lechariot export --format csv --query Butter > butter.csv
lechariot export --format json --out angebote.json
```

Without `--out` the output goes to stdout; `--query` filters by title/subtitle.

### serve — web dashboard + read-only JSON API

```sh
lechariot serve --port 8080
# Web-UI läuft auf http://0.0.0.0:8080, JSON-API auf http://0.0.0.0:8080/api (DB: lechariot.db)
```

The web dashboard lives at `/`, the JSON endpoints under `/api/*`
(see below and [docs/cron.md](docs/cron.md)).

