-- Migration v11: On-Demand-Regionen absichern (RLS + Trigger-Selbstschutz)
--
-- Schließt die Confused-Deputy-/Queue-Poisoning-Lücke der v3-Policy
-- „Anon insert" (`with check (true)`): mit dem öffentlichen anon-Key konnte
-- bislang JEDER
--   (a) beliebige Werte in die Queue-Steuerspalten `active`, `last_synced`
--       und `requested_at` schreiben (Sync-Queue vergiften), und
--   (b) über den SECURITY-DEFINER-Trigger `trigger_region_scrape` unbegrenzt
--       GitHub-`workflow_dispatch`-Aufrufe auslösen (fremder PAT bezahlt).
--
-- Diese Migration:
--   1. härtet die INSERT-Policy: nur `plz` darf gesetzt werden, die
--      Steuerspalten müssen ihre Defaults behalten (validierender
--      with-check + spaltenweises INSERT-Recht),
--   2. begrenzt den Trigger PRO PLZ (Cooldown im Funktionskörper) — das
--      dedupliziert ausschließlich wiederholte Anfragen für DIESELBE
--      Region und kann NIE die Erst-Anfrage einer anderen, frischen PLZ
--      unterdrücken (kein globales Budget, keine caller-übergreifende
--      Drosselung).
--
-- Idempotent: safe to re-run.

-- ============================================================
-- Teil 1: INSERT-Policy härten (F4/F6/F8/F9/F10 – Spalten + Row-Check)
-- ============================================================
--
-- Vorher: `with check (true)` erlaubte anon, active/last_synced/requested_at
-- frei zu setzen. Jetzt gilt:
--   - plz muss 5 Ziffern sein,
--   - active MUSS true sein (Region wird aktiv gesynct),
--   - last_synced MUSS null sein (noch nie gesynct = frische Anforderung),
--   - requested_at MUSS gesetzt sein (Anforderungszeitpunkt vorhanden).
-- Ein reiner `insert into regions (plz) values ('12345')` erfüllt das:
-- requested_at hat Default now(), active Default true, last_synced hat
-- keinen Default (=> null). Damit bleibt der legitime App-Pfad unverändert.
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

-- Zusätzlich auf Privilegienebene: anon/authenticated dürfen nur die Spalte
-- `plz` liefern. Ein Insert, der active/last_synced/requested_at benennt,
-- scheitert dann schon an „permission denied for column" — unabhängig von
-- der RLS-Policy (Defense in Depth, deckt auch spätere Policy-Fehler ab).
revoke insert on public.regions from anon, authenticated;
grant insert (plz) on public.regions to anon, authenticated;

-- ============================================================
-- Teil 2: Trigger-Selbstschutz — Cooldown PRO PLZ (F11, DB-Seite)
-- ============================================================
--
-- WICHTIG: `revoke execute on function ...` verhindert NICHT, dass der
-- Trigger feuert — Postgres prüft das EXECUTE-Recht nur EINMAL bei
-- CREATE TRIGGER (zur Definitionszeit), nicht bei jedem INSERT. Ein
-- Ratenlimit muss deshalb IM FUNKTIONSKÖRPER sitzen, nicht über GRANTs.

-- Dispatch-Protokoll: wann wurde pro PLZ zuletzt ein workflow_dispatch
-- gefeuert. Eigene Tabelle (kein Regions-Spaltenanhang), damit der Eintrag
-- ein delete+reinsert der Regionszeile ÜBERLEBT und der Cooldown auch dann
-- greift. RLS an, KEINE anon/authenticated-Policy => nur service_role und
-- die SECURITY-DEFINER-Funktion (läuft als Owner, umgeht RLS) sehen sie.
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

-- Function runs with owner rights; nobody else needs direct execute.
-- (Hinweis s. o.: das stoppt den Trigger nicht — der Cooldown oben tut es.)
revoke execute on function public.trigger_region_scrape() from public, anon, authenticated;

drop trigger if exists on_region_insert on public.regions;
create trigger on_region_insert
  after insert on public.regions
  for each row
  execute function public.trigger_region_scrape();

-- ============================================================
-- Hinweis zum verbleibenden Risiko (Distinct-PLZ-Flood)
-- ============================================================
-- Der PLZ-Cooldown bremst NUR wiederholte Anfragen derselben Region.
-- Ein Angreifer mit dem anon-Key kann weiterhin viele UNTERSCHIEDLICHE,
-- gültige PLZs einfügen und so je einmal einen Dispatch auslösen (durch
-- den PK `plz` ist die natürliche Rate zwar auf die Zahl je EINMALIG
-- angefragter PLZs begrenzt, aber ~8000 gültige deutsche PLZs sind viel).
-- Diese Distinct-PLZ-Flut lässt sich trigger-seitig NICHT abwehren, ohne
-- legitime Erst-Anfragen fremder Nutzer zu unterdrücken, weil am Trigger
-- keine Aufrufer-Identität vorliegt (die App nutzt den geteilten anon-Key).
-- Der saubere Folgeschritt steht in docs/ci.md: Regionen-Anfragen über eine
-- authentifizierte, ratenbegrenzte Edge Function routen bzw.
-- auth.role()='authenticated' + Pro-Installation-Quota verlangen.
