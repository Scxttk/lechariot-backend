-- Le Chariot – Komplettes Setup (Schema v1 + v2 in einem)

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
    regular_price double precision,
    base_price    double precision,
    base_unit     text,
    ean           text,
    brand         text,
    source        text default 'marktguru',
    unique (market, product, valid_from)
);

alter table public.offers enable row level security;

drop policy if exists "Public read" on public.offers;
create policy "Public read" on public.offers
    for select using (true);

drop policy if exists "Service write" on public.offers;
create policy "Service write" on public.offers
    for all using (auth.role() = 'service_role');

create index if not exists offers_market_idx  on public.offers (market);
create index if not exists offers_valid_idx   on public.offers (valid_from, valid_until);
create index if not exists offers_product_idx on public.offers (lower(product));
create index if not exists offers_ean_idx     on public.offers (ean) where ean is not null;
