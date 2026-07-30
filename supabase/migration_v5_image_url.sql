-- ============================================================
-- Le Chariot – Schema v5 (Produktbild statt/neben Emoji)
-- Im Supabase SQL Editor ausführen. Rückwärtskompatibel:
-- die neue Spalte ist optional, das Emoji bleibt als Fallback.
-- ============================================================

alter table public.offers
    add column if not exists image_url text;  -- Produktbild-URL vom Scraper; Emoji bleibt Fallback
