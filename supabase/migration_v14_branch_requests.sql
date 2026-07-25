-- ============================================================
-- Smart Shop – Migration v14: Filialen anfordern (`branch_requests`)
--
-- Gegenstück zu `regions` für den Filial-Schlüssel aus v13: Die App wählt
-- eine Filiale aus dem Verzeichnis (`public.branches`) und fordert deren
-- Angebote an. Der Weg ist derselbe wie bei Regionen und existiert seit
-- Phase 2.5:
--
--   App (anon)  ──insert──▶  branch_requests
--                                 │ Trigger (security definer)
--                                 ▼
--                      GitHub workflow_dispatch (nightly.yml, inputs.market_id)
--                                 │
--                                 ▼
--                 smartshop sync-regions --market-id <ID>
--                                 │
--                                 ▼
--             offers + branch_requests.last_synced  ◀──poll── App
--
-- Alle Absicherungen aus migration_v11 wandern mit — sie waren die Antwort
-- auf eine echte Lücke (anon konnte die Sync-Queue vergiften und unbegrenzt
-- fremd bezahlte workflow_dispatch auslösen) und dürfen auf dem neuen Weg
-- nicht wieder aufgehen. Eine Absicherung kommt dazu:
--
--   **Die market_id muss im Verzeichnis stehen.** Bei Regionen kam alles
--   durch, was fünf Ziffern hat — so wurde 94108 zur aktiven Region, obwohl
--   es die PLZ nicht gibt, und sammelte eine Woche lang 1.890 Angebote aus
--   Nachbarregionen ein (siehe [[Smartshop Geschäftslogik]]). Eine Filial-ID
--   lässt sich dagegen hart prüfen: Entweder der Store-Finder der Kette hat
--   sie geliefert und sie steht in `branches`, oder es gibt sie nicht.
--
-- Idempotent – kann gefahrlos erneut laufen.
-- ============================================================

create table if not exists public.branch_requests (
    -- Filial-ID wie in `branches.market_id` und `offers.market_id`.
    market_id     text primary key,
    requested_at  timestamptz not null default now(),
    -- Wird weiter gesynct? Analog zu `regions.active`.
    active        boolean not null default true,
    -- Null = noch nie gesynct (die App wartet), sonst Ende des letzten Laufs.
    last_synced   timestamptz
);

create index if not exists branch_requests_pending_idx
    on public.branch_requests (last_synced nulls first, requested_at);

alter table public.branch_requests enable row level security;

-- Lesen darf jeder: Genau darüber pollt die App ihren Fortschritt.
drop policy if exists "Public read" on public.branch_requests;
create policy "Public read" on public.branch_requests
    for select using (true);

drop policy if exists "Service write" on public.branch_requests;
create policy "Service write" on public.branch_requests
    for all using (auth.role() = 'service_role');

-- ============================================================
-- INSERT-Policy: nur `market_id`, nur existierende Filialen
-- ============================================================
--
-- Wie in v11: Die Steuerspalten müssen ihre Defaults behalten, sonst könnte
-- mit dem öffentlichen anon-Key jeder die Queue vergiften (`active`,
-- `last_synced`, `requested_at` frei setzen). Zusätzlich muss die Filiale im
-- Verzeichnis stehen — `branches` hat „Public read", der Unterabfrage-Check
-- greift also auch für anon.
drop policy if exists "Anon insert" on public.branch_requests;
create policy "Anon insert" on public.branch_requests
    for insert
    to anon, authenticated
    with check (
        active is true
        and last_synced is null
        and requested_at is not null
        and exists (
            select 1 from public.branches b
             where b.market_id = branch_requests.market_id
        )
    );

-- Defense in Depth auf Privilegienebene (deckt auch spätere Policy-Fehler):
-- ein Insert, der eine andere Spalte benennt, scheitert schon an
-- „permission denied for column".
revoke insert on public.branch_requests from anon, authenticated;
grant  insert (market_id) on public.branch_requests to anon, authenticated;

-- ============================================================
-- Trigger-Selbstschutz: Cooldown PRO market_id
-- ============================================================
--
-- Wie in v11 gilt: `revoke execute` stoppt einen Trigger NICHT — Postgres
-- prüft das EXECUTE-Recht einmalig bei CREATE TRIGGER. Das Ratenlimit muss
-- im Funktionskörper sitzen.
--
-- Eigene Log-Tabelle, damit der Cooldown ein delete+reinsert der
-- Anforderungszeile überlebt.
create table if not exists public.branch_dispatch_log (
    market_id         text primary key,
    last_dispatch_at  timestamptz not null default now()
);

alter table public.branch_dispatch_log enable row level security;

create extension if not exists pg_net with schema extensions;

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
    url := 'https://api.github.com/repos/Scxttk/smartshop-backend/actions/workflows/nightly.yml/dispatches',
    headers := jsonb_build_object(
      'Authorization', 'Bearer ' || pat,
      'Accept', 'application/vnd.github+json',
      'X-GitHub-Api-Version', '2022-11-28',
      'User-Agent', 'smartshop-supabase-trigger',
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

drop trigger if exists on_branch_request_insert on public.branch_requests;
create trigger on_branch_request_insert
  after insert on public.branch_requests
  for each row
  execute function public.trigger_branch_scrape();

-- ============================================================
-- Verbleibendes Risiko
-- ============================================================
-- Der Cooldown bremst nur wiederholte Anfragen derselben Filiale. Mit dem
-- anon-Key lassen sich weiterhin viele VERSCHIEDENE Filialen je einmal
-- anfordern. Anders als bei den ~8.000 gültigen PLZ ist die Menge hier
-- allerdings hart begrenzt: Angefordert werden kann nur, was im Verzeichnis
-- steht (Stand 2026-07-25: 3.115 Zeilen) — und das wächst nur, wo jemand die
-- App tatsächlich benutzt. Der saubere Folgeschritt bleibt derselbe wie in
-- v11 (siehe docs/ci.md): Anfragen über eine authentifizierte,
-- ratenbegrenzte Edge Function mit Quota pro Installation.
