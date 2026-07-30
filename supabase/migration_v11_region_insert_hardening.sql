-- Le Chariot – Migration v11: On-Demand-Regionen absichern (RLS + Trigger-Selbstschutz)

drop policy if exists "Anon insert" on public.regions;
create policy "Anon insert" on public.regions
    for insert
    to anon, authenticated
    with check (
        plz ~ '^[0-9]{5}$'
        and active is true
        and last_synced is null
        and requested_at is not null
    );

revoke insert on public.regions from anon, authenticated;
grant insert (plz) on public.regions to anon, authenticated;

create table if not exists public.region_dispatch_log (
    plz               text primary key,
    last_dispatch_at  timestamptz not null default now()
);

alter table public.region_dispatch_log enable row level security;

create extension if not exists pg_net with schema extensions;

create or replace function public.trigger_region_scrape()
returns trigger
language plpgsql
security definer
set search_path = public, extensions, net, vault
as $$
declare
  pat            text;
  last_dispatch  timestamptz;
begin
  -- ----------------------------------------------------------------
  -- Cooldown PRO PLZ: schon in den letzten 10 Minuten für GENAU DIESE
  -- PLZ dispatcht? Dann nichts tun. Das dedupliziert nur wiederholte
  -- Anfragen für DIESELBE Region — es gibt KEIN globales Budget und keine
  -- Abhängigkeit von anderen PLZs, also kann diese Bremse nie die
  -- Erst-Anfrage einer anderen, frischen PLZ unterdrücken.
  -- ----------------------------------------------------------------
  select last_dispatch_at into last_dispatch
  from public.region_dispatch_log
  where plz = new.plz;
  if last_dispatch is not null
     and last_dispatch > now() - interval '10 minutes' then
    raise notice 'trigger_region_scrape: PLZ % vor % dispatcht (< 10 min Cooldown), übersprungen',
      new.plz, now() - last_dispatch;
    return new;
  end if;
  select decrypted_secret into pat
  from vault.decrypted_secrets
  where name = 'github_pat';
  if pat is null then
    raise warning 'trigger_region_scrape: Vault secret github_pat missing, no scrape dispatched for PLZ %', new.plz;
    return new;
  end if;
  -- Async fire-and-forget; result appears later in net._http_response.
  -- GitHub rejects requests without a User-Agent header.
  perform net.http_post(
    url := 'https://api.github.com/repos/Scxttk/lechariot-backend/actions/workflows/nightly.yml/dispatches',
    headers := jsonb_build_object(
      'Authorization', 'Bearer ' || pat,
      'Accept', 'application/vnd.github+json',
      'X-GitHub-Api-Version', '2022-11-28',
      'User-Agent', 'lechariot-supabase-trigger',
      'Content-Type', 'application/json'
    ),
    body := jsonb_build_object(
      'ref', 'master',
      'inputs', jsonb_build_object('plz', new.plz)
    )
  );
  -- Cooldown-Fenster für DIESE PLZ starten/erneuern. Überlebt ein
  -- delete+reinsert der Regionszeile (eigene Tabelle, eigener PK).
  insert into public.region_dispatch_log (plz, last_dispatch_at)
  values (new.plz, now())
  on conflict (plz) do update set last_dispatch_at = excluded.last_dispatch_at;
  return new;
exception when others then
  -- Never block the region insert because the dispatch failed.
  raise warning 'trigger_region_scrape: dispatch failed for PLZ %: %', new.plz, sqlerrm;
  return new;
end;
$$;

revoke execute on function public.trigger_region_scrape() from public, anon, authenticated;

drop trigger if exists on_region_insert on public.regions;
create trigger on_region_insert
  after insert on public.regions
  for each row
  execute function public.trigger_region_scrape();
