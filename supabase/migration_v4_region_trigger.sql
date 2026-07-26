-- Migration v4: On-demand region scraping trigger
--
-- An INSERT into public.regions fires an async pg_net HTTP POST to GitHub's
-- workflow_dispatch API for nightly.yml, so a newly requested PLZ gets
-- scraped within minutes instead of waiting for the nightly cron.
--
-- Prerequisites (one-time, Supabase SQL editor):
--   1. A fine-grained GitHub PAT with "Actions: read and write" on
--      Scxttk/lechariot-backend (see docs/ci.md).
--   2. Store it in Vault under the name 'github_pat':
--        select vault.create_secret('<PAT>', 'github_pat');
--
-- Idempotent: safe to re-run.
--
-- Debugging: pg_net is async; responses land in net._http_response.
--   select * from net._http_response order by id desc limit 5;
-- A successful dispatch returns HTTP 204.

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

-- Function runs with owner rights; nobody else needs direct execute.
revoke execute on function public.trigger_region_scrape() from public, anon, authenticated;

drop trigger if exists on_region_insert on public.regions;
create trigger on_region_insert
  after insert on public.regions
  for each row
  execute function public.trigger_region_scrape();
