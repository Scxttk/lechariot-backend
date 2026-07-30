-- Le Chariot – Migration v19: Gebiets-Anforderung

create table if not exists public.area_requests (
    market_id     text primary key,
    plz           text,
    requested_at  timestamptz not null default now(),
    active        boolean not null default true,
    last_synced   timestamptz
);

create index if not exists area_requests_pending_idx
    on public.area_requests (last_synced nulls first, requested_at);
create index if not exists area_requests_plz_idx
    on public.area_requests (plz);

alter table public.area_requests enable row level security;

drop policy if exists "Public read" on public.area_requests;
create policy "Public read" on public.area_requests
    for select using (true);

drop policy if exists "Service write" on public.area_requests;
create policy "Service write" on public.area_requests
    for all using (auth.role() = 'service_role');

drop policy if exists "Anon insert" on public.area_requests;
create policy "Anon insert" on public.area_requests
    for insert
    to anon, authenticated
    with check (
        active is true
        and last_synced is null
        and plz is null
        and requested_at is not null
        and exists (
            select 1 from public.branches b
             where b.market_id = area_requests.market_id
        )
    );

revoke insert on public.area_requests from anon, authenticated;
grant  insert (market_id) on public.area_requests to anon, authenticated;

create or replace function public.area_request_resolve_plz()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
begin
  select b.plz into new.plz
  from public.branches b
  where b.market_id = new.market_id;
  return new;
end;
$$;

revoke execute on function public.area_request_resolve_plz() from public, anon, authenticated;

drop trigger if exists on_area_request_resolve on public.area_requests;
create trigger on_area_request_resolve
  before insert on public.area_requests
  for each row
  execute function public.area_request_resolve_plz();

create table if not exists public.area_dispatch_log (
    plz               text primary key,
    last_dispatch_at  timestamptz not null default now()
);

alter table public.area_dispatch_log enable row level security;

create extension if not exists pg_net with schema extensions;

create or replace function public.trigger_area_scrape()
returns trigger
language plpgsql
security definer
set search_path = public, extensions, net, vault
as $$
declare
  pat            text;
  last_dispatch  timestamptz;
begin
  -- Ohne Gebiet gibt es nichts anzustoßen (Ankerfiliale ohne PLZ).
  if new.plz is null then
    raise warning 'trigger_area_scrape: Filiale % hat keine PLZ im Verzeichnis, kein Gebiets-Lauf', new.market_id;
    return new;
  end if;
  -- Der Backend-Lauf stempelt am Ende `last_synced`. Das ist ein UPDATE und
  -- kommt hier nicht an (Trigger nur auf INSERT), aber die Bedingung bleibt
  -- als Gürtel: Wer eine fertige Zeile neu einspielt, löst nichts aus.
  if new.last_synced is not null then
    return new;
  end if;
  select last_dispatch_at into last_dispatch
  from public.area_dispatch_log
  where plz = new.plz;
  if last_dispatch is not null
     and last_dispatch > now() - interval '30 minutes' then
    raise notice 'trigger_area_scrape: Gebiet % vor % dispatcht (< 30 min Cooldown), übersprungen',
      new.plz, now() - last_dispatch;
    return new;
  end if;
  select decrypted_secret into pat
  from vault.decrypted_secrets
  where name = 'github_pat';
  if pat is null then
    raise warning 'trigger_area_scrape: Vault secret github_pat missing, no run dispatched for area %', new.plz;
    return new;
  end if;
  perform net.http_post(
    url := 'https://api.github.com/repos/Scxttk/lechariot-backend/actions/workflows/branches.yml/dispatches',
    headers := jsonb_build_object(
      'Authorization', 'Bearer ' || pat,
      'Accept', 'application/vnd.github+json',
      'X-GitHub-Api-Version', '2022-11-28',
      'User-Agent', 'lechariot-supabase-trigger',
      'Content-Type', 'application/json'
    ),
    body := jsonb_build_object(
      'ref', 'master',
      -- skip_national: Kaufland und Penny stehen schon bundesweit drin, die
      -- zwei Requests wären reine Wiederholung. Als String, weil der Workflow
      -- den Wert als Text vergleicht.
      'inputs', jsonb_build_object('area', new.plz, 'skip_national', 'true')
    )
  );
  insert into public.area_dispatch_log (plz, last_dispatch_at)
  values (new.plz, now())
  on conflict (plz) do update set last_dispatch_at = excluded.last_dispatch_at;
  return new;
exception when others then
  -- Die Anforderung darf nie daran scheitern, dass der Dispatch scheitert.
  raise warning 'trigger_area_scrape: dispatch failed for area %: %', new.plz, sqlerrm;
  return new;
end;
$$;

revoke execute on function public.trigger_area_scrape() from public, anon, authenticated;

drop trigger if exists on_area_request_insert on public.area_requests;
create trigger on_area_request_insert
  after insert on public.area_requests
  for each row
  execute function public.trigger_area_scrape();
