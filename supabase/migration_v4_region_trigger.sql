-- Le Chariot – Migration v4: On-Demand-Trigger für Regionen

create extension if not exists pg_net with schema extensions;

create or replace function public.trigger_region_scrape()
returns trigger
language plpgsql
security definer
set search_path = public, extensions, net, vault
as $$
declare
  pat text;
begin
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
    body := jsonb_build_object('ref', 'master')
  );
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
