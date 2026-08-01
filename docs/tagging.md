# Angebots-Tagging (`match_key`)

Der Push taggt jedes Angebot regelbasiert mit Alltagsbegriffen
(`src/matching.rs` + eingebettetes `docs/matching-woerterbuch.json`) und
schreibt sie in die Spalte `offers.match_key text[]`
(`supabase/migration_v10_match_key.sql`). Werte:

- `{käse}`, `{tomaten}`, … — Begriffs-Tags (auch mehrere möglich)
- `{nonfood}` — erkanntes Non-Food
- `{}` — ungetaggt → Kandidat für die Review-Liste

## Wörterbuch pflegen

Quelle ist `docs/matching-woerterbuch.json` (Sektionen `begriffe` mit
exact/suffix/prefix/block, `kategorien` Kategorie→Begriff und `marken`
Marke→Begriff bzw. `NONFOOD`). Geschrieben wird sie nie von Hand, sondern von
`docs/matching-woerterbuch-eval.py` — dort stehen die Listen mit ihrer
Begründung.

`suffix` und `prefix` sind die beiden Hälften des Kompositum-Wegs: das
bekannte Wort steht im Deutschen mal hinten („Steinofen**baguette**") und mal
vorne („**Kartoffel**taschen"). Beide greifen erst ab vier Zeichen, beide
achten auf `SUFFIX_STOP` („Reis"/„Preis", „Wein"/„Schwein") und auf die
Blockliste des Begriffs. Sie gehören pro Begriff gepflegt, nie pauschal — die
Sperrlisten in der Python-Referenz führen zu jeder Ausnahme den gemessenen
Fehltreffer mit.

Nach jeder Änderung:

1. `python3 docs/matching-woerterbuch-eval.py` — misst Abdeckung gegen die
   lokale Nightly-DB und regeneriert die JSON aus der Python-Referenz.
2. `cargo test matching` und
   `cargo test parity_with_eval_db -- --ignored --nocapture` — die Rust-Zahlen
   müssen exakt zur Python-Ausgabe passen.
3. Neu bauen — die JSON ist per `include_str!` einkompiliert.

## Review-Liste: ungetaggte Angebote der aktuellen KW

Supabase SQL-Editor (oder psql):

```sql
select market, product, valid_from, region
from public.offers
where match_key = '{}'
  and valid_until >= current_date
  and region = '01219'          -- ohne Region-Filter tauchen alte,
                                -- noch untagged Kopien anderer Regionen auf
order by market, product;
```

Per PostgREST/curl:

```sh
curl -s "$SUPABASE_URL/rest/v1/offers?match_key=eq.\{\}&valid_until=gte.$(date +%F)&region=eq.01219&select=market,product,valid_from,region" \
  -H "apikey: $SUPABASE_SERVICE_KEY" -H "Authorization: Bearer $SUPABASE_SERVICE_KEY"
```

Erwartung: ~2 % der Food-Angebote (kontextlose Flyer-Titel wie „Fix",
„Ciolino"). Neue Begriffe/Marken daraus ins Wörterbuch pflegen (Workflow
oben); geschätzt ~10–30 Einträge/Woche bis zur Konvergenz.
