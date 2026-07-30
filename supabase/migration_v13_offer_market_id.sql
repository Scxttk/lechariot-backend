-- Le Chariot – Migration v13: Angebote gehören der Filiale

alter table public.offers add column if not exists market_id text;

update public.offers o
   set market_id = m.market_id
  from public.markets m
 where o.market_id is null
   and m.chain = o.market
   and m.plz   = o.region;

alter table public.offers alter column market_id set not null;

create unique index if not exists offers_branch_product_valid_key
    on public.offers (market_id, product, valid_from, region) nulls not distinct;

drop index if exists public.offers_market_product_valid_region_key;
alter table public.offers drop constraint if exists offers_market_product_valid_from_key;

create index if not exists offers_market_id_idx on public.offers (market_id);

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
