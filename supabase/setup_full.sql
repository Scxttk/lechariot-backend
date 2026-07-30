-- ============================================================
-- Le Chariot – Komplettes Setup (Schema v1 + v2 in einem)
-- Für frische Supabase-Projekte: einmalig im SQL Editor ausführen.
-- Idempotent – kann gefahrlos erneut ausgeführt werden.
-- ============================================================

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

    -- Schema v2: Normal-/Grundpreis + Matching-Felder
    regular_price double precision,          -- Normalpreis ("statt"-Preis)
    base_price    double precision,          -- Grundpreis, z.B. €/kg
    base_unit     text,                      -- Einheit des Grundpreises ("kg", "l")
    ean           text,                      -- GTIN/EAN für Produkt-Matching
    brand         text,                      -- Marke, getrennt vom Produktnamen
    source        text default 'marktguru',  -- Datenquelle

    -- Verhindert Duplikate bei täglichen Runs
    unique (market, product, valid_from)
);

-- Row Level Security aktivieren
alter table public.offers enable row level security;

-- Öffentliches Lesen (für die iOS App mit anon key)
drop policy if exists "Public read" on public.offers;
create policy "Public read" on public.offers
    for select using (true);

-- Nur Service Role darf schreiben (für den Sync)
drop policy if exists "Service write" on public.offers;
create policy "Service write" on public.offers
    for all using (auth.role() = 'service_role');

-- Indizes für schnelle Abfragen der iOS App
create index if not exists offers_market_idx  on public.offers (market);
create index if not exists offers_valid_idx   on public.offers (valid_from, valid_until);
create index if not exists offers_product_idx on public.offers (lower(product));
create index if not exists offers_ean_idx     on public.offers (ean) where ean is not null;
