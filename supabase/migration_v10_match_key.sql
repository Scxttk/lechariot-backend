-- Le Chariot – Migration v10: Begriffs-Tags aus dem Rust-Import

alter table public.offers
  add column if not exists match_key text[] not null default '{}';

create index if not exists offers_match_key_gin
  on public.offers using gin (match_key);
