-- ============================================================
-- Le Chariot – Schema v2 (Vorbereitung für Normalpreis-Vergleich)
-- Im Supabase SQL Editor ausführen. Rückwärtskompatibel:
-- alle neuen Spalten sind optional, bestehende Uploads laufen weiter.
-- ============================================================

alter table public.offers
    add column if not exists regular_price double precision,  -- Normalpreis ("statt"-Preis, marktguru: oldPrice)
    add column if not exists base_price    double precision,  -- Grundpreis, z.B. €/kg (marktguru: referencePrice)
    add column if not exists base_unit     text,              -- Einheit des Grundpreises ("kg", "l")
    add column if not exists ean           text,              -- GTIN/EAN für Produkt-Matching über Märkte
    add column if not exists brand         text,              -- Marke, getrennt vom Produktnamen
    add column if not exists source        text default 'marktguru';  -- Datenquelle

create index if not exists offers_ean_idx on public.offers (ean) where ean is not null;
