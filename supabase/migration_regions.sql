-- ============================================================
-- Le Chariot – Migration: On-Demand-Regionen (Phase 2.5)
-- Neue Tabelle `regions` (Sync-Cache pro PLZ) + `region`-Spalte
-- auf `offers`. Idempotent – kann gefahrlos erneut laufen.
-- ============================================================

-- Region-Cache: welche PLZ wurde wann zuletzt gesynct
create table if not exists public.regions (
    plz          text primary key,
    last_synced  timestamptz
);

alter table public.regions enable row level security;

drop policy if exists "Public read" on public.regions;
create policy "Public read" on public.regions
    for select using (true);

drop policy if exists "Service write" on public.regions;
create policy "Service write" on public.regions
    for all using (auth.role() = 'service_role');

-- offers: Region (PLZ), aus der die Angebote stammen
alter table public.offers add column if not exists region text;

-- Bestandsdaten stammen aus dem bisherigen Dresden-Sync (PLZ 01219)
update public.offers set region = '01219' where region is null;

-- Unique-Constraint um region erweitern: dasselbe (bundesweite) Angebot
-- darf in mehreren Regionen existieren
alter table public.offers drop constraint if exists offers_market_product_valid_from_key;
create unique index if not exists offers_market_product_valid_region_key
    on public.offers (market, product, valid_from, region);

create index if not exists offers_region_idx on public.offers (region);

-- Bisherige Sync-Region als bekannte Region eintragen
insert into public.regions (plz) values ('01219') on conflict do nothing;
