-- Le Chariot – Migration v16: `region` wird zu `nationwide`

delete from public.offers a
using public.offers b
where a.market_id = b.market_id
  and a.product = b.product
  and a.valid_from is not distinct from b.valid_from
  and (
        coalesce(a.created_at, '-infinity'::timestamptz)
          < coalesce(b.created_at, '-infinity'::timestamptz)
     or (coalesce(a.created_at, '-infinity'::timestamptz)
          = coalesce(b.created_at, '-infinity'::timestamptz)
         and a.id < b.id)
      );

alter table public.offers add column if not exists nationwide boolean;

update public.offers
   set nationwide = (region is null)
 where nationwide is null;

alter table public.offers alter column nationwide set default false;
alter table public.offers alter column nationwide set not null;

alter table public.price_history add column if not exists nationwide boolean;
update public.price_history
   set nationwide = (region is null)
 where nationwide is null;
alter table public.price_history alter column nationwide set default false;
alter table public.price_history alter column nationwide set not null;

create unique index if not exists offers_branch_product_valid_key2
    on public.offers (market_id, product, valid_from);

drop index if exists offers_branch_product_valid_key;

delete from public.price_history a
using public.price_history b
where a.market_id = b.market_id
  and a.product = b.product
  and a.valid_from is not distinct from b.valid_from
  and (
        coalesce(a.recorded_at, '-infinity'::timestamptz)
          < coalesce(b.recorded_at, '-infinity'::timestamptz)
     or (coalesce(a.recorded_at, '-infinity'::timestamptz)
          = coalesce(b.recorded_at, '-infinity'::timestamptz)
         and a.id < b.id)
      );

alter table public.price_history drop constraint if exists price_history_branch_week_key;
alter table public.price_history drop constraint if exists price_history_week_key;

do $$
begin
    alter table public.price_history
        add constraint price_history_branch_week_key2
        unique nulls not distinct (market_id, product, valid_from);
exception
    when duplicate_object then null;
end $$;

alter table public.offers drop column if exists region;
alter table public.price_history drop column if exists region;

drop trigger if exists on_region_insert on public.regions;
drop function if exists public.trigger_region_scrape() cascade;
drop table if exists public.region_dispatch_log;
drop table if exists public.regions;
