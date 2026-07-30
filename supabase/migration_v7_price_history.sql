-- Le Chariot – Migration v7: Preis-Historie

create table if not exists public.price_history (
    id            bigint generated always as identity primary key,
    market        text,
    product       text,
    region        text,
    price         numeric,
    regular_price numeric,
    base_price    numeric,
    base_unit     text,
    unit          text,
    category      text,
    valid_from    date,
    valid_until   date,
    recorded_at   timestamptz default now()
);

do $$
begin
    alter table public.price_history
        add constraint price_history_week_key unique nulls not distinct (market, product, region, valid_from);
exception
    when duplicate_object then null;
end $$;

alter table public.price_history enable row level security;

drop policy if exists "Public read" on public.price_history;
create policy "Public read" on public.price_history
    for select using (true);

drop policy if exists "Service write" on public.price_history;
create policy "Service write" on public.price_history
    for all using (auth.role() = 'service_role');
