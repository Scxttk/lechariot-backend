-- Le Chariot – Migration: On-Demand-Regionen (Phase 2.5)

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

alter table public.offers add column if not exists region text;

update public.offers set region = '01219' where region is null;

alter table public.offers drop constraint if exists offers_market_product_valid_from_key;
create unique index if not exists offers_market_product_valid_region_key
    on public.offers (market, product, valid_from, region);

create index if not exists offers_region_idx on public.offers (region);

insert into public.regions (plz) values ('01219') on conflict do nothing;
