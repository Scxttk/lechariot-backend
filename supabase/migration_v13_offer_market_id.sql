-- ============================================================
-- Le Chariot – Migration v13: Angebote gehören der Filiale
--
-- Bisher war der Schlüssel eines Angebots (market, product, valid_from,
-- region) — also „REWE in 01219". Das ist falsch, sobald eine PLZ mehr als
-- eine Filiale derselben Kette hat: beide schreiben auf denselben Schlüssel
-- und überschreiben sich gegenseitig. Grundlage ist die Messung in der
-- Geschäftslogik: Kaufland hat in 12 Regionen 12 verschiedene Prospekte,
-- die Angebote hängen an der Filiale, nicht an der Region.
--
-- Ab hier trägt jede Zeile ihre `market_id` — dieselbe Filial-ID, die auch
-- `markets.market_id` und `branches.market_id` tragen und über die gescrapt
-- wird. Die Kette (`market`) bleibt daneben stehen: die App filtert damit.
--
-- ------------------------------------------------------------
-- Warum `region` im Schlüssel BLEIBT (und nicht, wie im Laufplan skizziert,
-- sofort herausfällt)
-- ------------------------------------------------------------
-- Am 2026-07-25 gegen die Live-Datenbank gemessen: vier Filialen stehen
-- unter MEHREREN PLZ, weil der Store-Finder sie für jede Nachbar-PLZ erneut
-- als nächste meldet — Penny 531053 in 01067/01069/01099/01139, REWE 565005
-- in 01217+01219, Penny 4030829 in 01217+01219, EDEKA 024508 in
-- 04626+08451. Zusammen 2.889 der 38.097 Zeilen.
--
-- Fiele `region` jetzt aus dem Schlüssel, würden diese Zeilen zu je einer
-- verschmelzen — und die verbliebene Zeile trüge die PLZ der zuletzt
-- gesyncten Region. Die App filtert aber bis Phase 12 `region=eq.<PLZ>`:
-- Penny wäre danach in drei von vier Dresdner Regionen unsichtbar, und der
-- nächtliche Lauf würde die Zuordnung bei jedem Durchgang neu würfeln.
--
-- Der Schlüssel wird deshalb (market_id, product, valid_from, region):
--   * Zwei Filialen DERSELBEN Kette in DERSELBEN PLZ haben verschiedene
--     market_id und überschreiben sich nicht mehr — das ist der Zweck.
--   * Eine Filiale in mehreren Regionen behält ihre Zeile je Region, die
--     App bleibt funktionsfähig.
-- In Phase 12 fällt `region` ersatzlos weg (die App fragt dann nach
-- Filialen); unter `nulls not distinct` schrumpft der Schlüssel dabei von
-- allein auf (market_id, product, valid_from).
--
-- ACHTUNG, REIHENFOLGE: Diese Migration und der neue Binary gehören
-- zusammen. Der alte Binary upsertet mit `on_conflict=market,product,
-- valid_from,region` — dieser Unique-Index existiert danach nicht mehr,
-- PostgREST antwortet mit 42P10. Der neue Binary wiederum braucht den neuen
-- Index. Also: PR mergen und die Migration im selben Zeitfenster ausführen,
-- am besten nicht kurz vor 06:30 (Nightly).
--
-- Idempotent – kann gefahrlos erneut laufen.
-- ============================================================

-- ------------------------------------------------------------
-- Teil 1: Spalte + Backfill auf `offers`
-- ------------------------------------------------------------

alter table public.offers add column if not exists market_id text;

-- Backfill über das bisherige Markt-Verzeichnis: `markets` hält seit v3 pro
-- (Kette, PLZ) genau die eine Filiale, aus der gescrapt wurde — also exakt
-- die Filiale, der die Bestandszeilen gehören. Gegengeprüft am 2026-07-25:
-- für alle 38.097 Zeilen existiert die passende markets-Zeile, es bleibt
-- keine ohne market_id zurück.
update public.offers o
   set market_id = m.market_id
  from public.markets m
 where o.market_id is null
   and m.chain = o.market
   and m.plz   = o.region;

-- Ohne market_id kann keine Zeile mehr gepusht werden — sonst stünde wieder
-- die Kette statt der Filiale im Schlüssel. Nach dem Backfill oben ist die
-- Spalte vollständig, also gilt ab jetzt NOT NULL.
alter table public.offers alter column market_id set not null;

-- ------------------------------------------------------------
-- Teil 2: Schlüsselwechsel auf `offers`
-- ------------------------------------------------------------

-- `nulls not distinct`, damit Zeilen ohne valid_from nicht bei jedem Lauf
-- dupliziert werden (dieselbe Absicherung wie in v7 für price_history) —
-- und damit der Schlüssel in Phase 12 mit region = null weiter greift.
create unique index if not exists offers_branch_product_valid_key
    on public.offers (market_id, product, valid_from, region) nulls not distinct;

-- Alter Schlüssel weg — sonst würden zwei Filialen derselben Kette in
-- derselben PLZ weiterhin aufeinander schreiben, und genau das ist der
-- Grund für diese Migration.
drop index if exists public.offers_market_product_valid_region_key;
alter table public.offers drop constraint if exists offers_market_product_valid_from_key;

create index if not exists offers_market_id_idx on public.offers (market_id);

-- ------------------------------------------------------------
-- Teil 3: dasselbe für `price_history`
-- ------------------------------------------------------------
--
-- Die Historie wird im selben Push geschrieben und trug bisher denselben
-- Regionsschlüssel. Bliebe sie darauf, würden zwei Filialen einer PLZ ihre
-- Preisverläufe gegenseitig überschreiben — der Chart im Angebots-Sheet
-- zeigte dann eine Mischkurve aus zwei Läden.
--
-- Auch hier gegengeprüft: alle 62.793 Zeilen lassen sich auflösen.
alter table public.price_history add column if not exists market_id text;

update public.price_history h
   set market_id = m.market_id
  from public.markets m
 where h.market_id is null
   and m.chain = h.market
   and m.plz   = h.region;

alter table public.price_history alter column market_id set not null;

alter table public.price_history drop constraint if exists price_history_week_key;

do $$
begin
    alter table public.price_history
        add constraint price_history_branch_week_key
        unique nulls not distinct (market_id, product, valid_from, region);
exception
    when duplicate_object then null;
end $$;

create index if not exists price_history_market_id_idx on public.price_history (market_id);
