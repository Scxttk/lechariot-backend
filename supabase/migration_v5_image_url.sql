-- Le Chariot – Schema v5 (Produktbild statt/neben Emoji)

alter table public.offers
    add column if not exists image_url text;
