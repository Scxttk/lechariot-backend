-- Le Chariot – Migration v17: der Schlüssel der Preis-Historie

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
