-- Le Chariot – Supabase-Schema

create table if not exists public.offers (
    id            bigint generated always as identity primary key,
    market        text             not null,
    product       text             not null,
    price         double precision not null,
    "loyaltyPrice" double precision,
    unit          text             default 'Stück',
    category      text,
    emoji         text,
    image_url     text,
    valid_from    date,
    valid_until   date,
    created_at    timestamptz      default now(),
    unique (market, product, valid_from)
);

alter table public.offers enable row level security;

create policy "Public read" on public.offers
    for select using (true);

create policy "Service write" on public.offers
    for all using (auth.role() = 'service_role');

create index if not exists offers_market_idx   on public.offers (market);
create index if not exists offers_valid_idx    on public.offers (valid_from, valid_until);
create index if not exists offers_product_idx  on public.offers (lower(product));
