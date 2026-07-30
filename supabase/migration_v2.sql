-- Le Chariot – Schema v2 (Vorbereitung für Normalpreis-Vergleich)

alter table public.offers
    add column if not exists regular_price double precision,
    add column if not exists base_price    double precision,
    add column if not exists base_unit     text,
    add column if not exists ean           text,
    add column if not exists brand         text,
    add column if not exists source        text default 'marktguru';

create index if not exists offers_ean_idx on public.offers (ean) where ean is not null;
