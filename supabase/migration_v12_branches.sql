-- Le Chariot – Migration v12: Filialverzeichnis `branches`

create table if not exists public.branches (
    market_id   text primary key,
    chain       text not null,
    name        text not null,
    street      text,
    plz         text,
    city        text,
    lat         double precision,
    lon         double precision,
    source      text,
    updated_at  timestamptz not null default now()
);

create index if not exists branches_lat_lon_idx on public.branches (lat, lon);
create index if not exists branches_plz_idx on public.branches (plz);

alter table public.branches enable row level security;

drop policy if exists "Public read" on public.branches;
create policy "Public read" on public.branches
    for select using (true);

drop policy if exists "Service write" on public.branches;
create policy "Service write" on public.branches
    for all using (auth.role() = 'service_role');

revoke insert, update, delete on public.branches from anon, authenticated;
