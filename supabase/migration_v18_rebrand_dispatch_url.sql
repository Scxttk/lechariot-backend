-- Le Chariot – Migration v18: Dispatch-URL auf das umbenannte Repo zeigen lassen

create or replace function public.trigger_branch_scrape()
returns trigger
language plpgsql
security definer
set search_path = public, extensions, net, vault
as $$
declare
  pat            text;
  last_dispatch  timestamptz;
begin
  -- Der Sync selbst schreibt am Ende seine Zeile (`last_synced` gesetzt).
  -- Läuft er für eine Filiale, die noch niemand angefordert hatte, ist das
  -- ein INSERT — ohne diese Bremse löste die Fertigmeldung einen neuen Lauf
  -- aus, der wieder eine Fertigmeldung schreibt.
  if new.last_synced is not null then
    return new;
  end if;
  -- Cooldown PRO Filiale: dedupliziert nur wiederholte Anfragen für DIESELBE
  -- Filiale. Kein globales Budget, also kann die Bremse nie die Erst-Anfrage
  -- einer anderen, frischen Filiale unterdrücken.
  select last_dispatch_at into last_dispatch
  from public.branch_dispatch_log
  where market_id = new.market_id;
  if last_dispatch is not null
     and last_dispatch > now() - interval '10 minutes' then
    raise notice 'trigger_branch_scrape: Filiale % vor % dispatcht (< 10 min Cooldown), übersprungen',
      new.market_id, now() - last_dispatch;
    return new;
  end if;
  select decrypted_secret into pat
  from vault.decrypted_secrets
  where name = 'github_pat';
  if pat is null then
    raise warning 'trigger_branch_scrape: Vault secret github_pat missing, no scrape dispatched for branch %', new.market_id;
    return new;
  end if;
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
      'inputs', jsonb_build_object('market_id', new.market_id)
    )
  );
  insert into public.branch_dispatch_log (market_id, last_dispatch_at)
  values (new.market_id, now())
  on conflict (market_id) do update set last_dispatch_at = excluded.last_dispatch_at;
  return new;
exception when others then
  -- Die Anforderung darf nie daran scheitern, dass der Dispatch scheitert.
  raise warning 'trigger_branch_scrape: dispatch failed for branch %: %', new.market_id, sqlerrm;
  return new;
end;
$$;

revoke execute on function public.trigger_branch_scrape() from public, anon, authenticated;
