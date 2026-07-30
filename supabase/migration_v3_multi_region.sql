-- ============================================================
-- Le Chariot – Migration v3: Multi-Region-Sync
-- Erweitert `regions` um Anforderungs-Metadaten, erlaubt anon
-- INSERT (Region anfordern), legt Markt-Verzeichnis `markets` an.
-- Idempotent – kann gefahrlos erneut laufen.
-- ============================================================

-- regions: wann angefordert, ob aktiv gesynct wird
alter table public.regions add column if not exists requested_at timestamptz default now();
alter table public.regions add column if not exists active boolean not null default true;

-- PLZ muss aus genau 5 Ziffern bestehen
do $$
begin
    alter table public.regions
        add constraint regions_plz_format check (plz ~ '^[0-9]{5}$');
exception
    when duplicate_object then null;
end $$;

-- anon darf Regionen anfordern (nur INSERT, kein UPDATE/DELETE)
drop policy if exists "Anon insert" on public.regions;
create policy "Anon insert" on public.regions
    for insert with check (true);

-- Markt-Verzeichnis: welcher Markt wurde pro Kette+PLZ gefunden
create table if not exists public.markets (
    chain       text,
    branch_name text,
    market_id   text,
    plz         text,
    updated_at  timestamptz default now(),
    primary key (chain, plz)
);

alter table public.markets enable row level security;

drop policy if exists "Public read" on public.markets;
create policy "Public read" on public.markets
    for select using (true);

drop policy if exists "Service write" on public.markets;
create policy "Service write" on public.markets
    for all using (auth.role() = 'service_role');
