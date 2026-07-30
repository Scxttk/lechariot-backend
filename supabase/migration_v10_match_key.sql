-- v10: Begriffs-Tags aus dem Rust-Import (src/matching.rs +
-- docs/matching-woerterbuch.json). Werte: Alltagsbegriffe wie 'käse',
-- 'tomaten'; '{nonfood}' = erkanntes Non-Food; '{}' = ungetaggt
-- (Review-Liste, siehe docs/tagging.md).
alter table public.offers
  add column if not exists match_key text[] not null default '{}';

-- Kategorie-Fallback der App filtert per match_key @> '{käse}'.
create index if not exists offers_match_key_gin
  on public.offers using gin (match_key);
