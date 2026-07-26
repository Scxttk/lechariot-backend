-- Migration v19 — Gebiets-Anforderung: das Verzeichnis wächst beim ersten
-- Nutzer, nicht erst am Sonntag
--
-- ============================================================
-- Das Problem
-- ============================================================
--
-- Im Verzeichnis liegen Kaufland (787) und Penny (2121) bundesweit — je ein
-- Request holt ganz Deutschland. Die übrigen sechs Ketten kosten pro Gebiet
-- eine Suche je Kette und werden deshalb nur gebietsweise geholt. Welche
-- Gebiete das sind, leitet `branches-sync --from-branches` aus den GEWÄHLTEN
-- Filialen ab — und dieser Lauf hängt am Zeitplan von `branches.yml`,
-- sonntags 03:15 UTC.
--
-- Für die Pflege ist das richtig: Filialen ändern sich in Monaten. Für den
-- ERSTEN Nutzer eines Gebiets ist es falsch. Gemessen am 2026-07-26 in
-- Gößnitz (04639): Der Picker zeigte Penny und zwei Kauflands, kein Netto,
-- kein Lidl, kein REWE — obwohl es sie dort gibt. Bis zum nächsten Sonntag,
-- und auch dann nur, wenn vorher jemand eine der bundesweiten Filialen
-- gewählt hatte, damit das Gebiet überhaupt abgeleitet werden kann.
--
-- Ein Lauf für ein einzelnes Gebiet dauert **3 Minuten** (gemessen am
-- 2026-07-26: 08:33:42 → 08:36:50 UTC, danach stand Netto Gößnitz drin).
-- Es fehlt also nichts als der Auslöser.
--
-- ============================================================
-- Warum die Anforderung auf eine FILIALE lautet und nicht auf eine PLZ
-- ============================================================
--
-- Das ist die Lehre aus 94108. Der alte Regionen-Weg nahm alles an, was fünf
-- Ziffern hatte; so wurde eine Postleitzahl, die es nicht gibt, zur aktiven
-- Region und sammelte eine Woche lang 1.890 Angebote aus Nachbargebieten ein.
-- Eine Zahl lässt sich nicht prüfen — eine Filial-ID schon: entweder der
-- Store-Finder ihrer Kette hat sie geliefert und sie steht in `branches`,
-- oder es gibt sie nicht. Genau diese Prüfung macht v14 für `branch_requests`.
--
-- Die Anforderung lautet deshalb „hol das Gebiet um DIESE Filiale herum".
-- Die PLZ schlägt der Trigger selbst im Verzeichnis nach, der Client nennt
-- sie nie. Und weil Kaufland und Penny bundesweit drinstehen, hat praktisch
-- jeder bewohnte Ort einen Anker in Reichweite — in Gößnitz war es Penny.
--
-- Idempotent – kann gefahrlos erneut laufen.

create table if not exists public.area_requests (
    -- Ankerfiliale, wie in `branches.market_id`. NICHT die PLZ: die wäre vom
    -- Client frei wählbar und damit ungeprüft (siehe oben).
    market_id     text primary key,
    -- Vom Trigger aus `branches` nachgeschlagen, nie vom Client gesetzt.
    -- Hierüber stempelt der Backend-Lauf die Fertigmeldung.
    plz           text,
    requested_at  timestamptz not null default now(),
    active        boolean not null default true,
    -- Null = das Gebiet ist noch nicht gezogen (die App wartet und weist den
    -- Nutzer beim nächsten Start darauf hin), sonst Ende des letzten Laufs.
    last_synced   timestamptz
);

create index if not exists area_requests_pending_idx
    on public.area_requests (last_synced nulls first, requested_at);
create index if not exists area_requests_plz_idx
    on public.area_requests (plz);

alter table public.area_requests enable row level security;

-- Lesen darf jeder: Genau darüber erfährt die App, dass ihr Gebiet fertig
-- ist — auch nach einem Neustart, denn die Zeile überlebt die App.
drop policy if exists "Public read" on public.area_requests;
create policy "Public read" on public.area_requests
    for select using (true);

drop policy if exists "Service write" on public.area_requests;
create policy "Service write" on public.area_requests
    for all using (auth.role() = 'service_role');

-- ============================================================
-- INSERT-Policy: nur `market_id`, nur existierende Filialen
-- ============================================================
--
-- Wortgleich zur Überlegung in v14: Die Steuerspalten müssen ihre Defaults
-- behalten, sonst könnte mit dem öffentlichen anon-Key jeder die Queue
-- vergiften. `plz` gehört ausdrücklich dazu — sie ist abgeleitet, nicht
-- angegeben.
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

-- Defense in Depth auf Privilegienebene (deckt auch spätere Policy-Fehler):
-- ein Insert, der eine andere Spalte benennt, scheitert schon an
-- „permission denied for column".
revoke insert on public.area_requests from anon, authenticated;
grant  insert (market_id) on public.area_requests to anon, authenticated;

-- ============================================================
-- Gebiet nachschlagen
-- ============================================================
--
-- BEFORE INSERT, damit `plz` gesetzt ist, bevor der Dispatch-Trigger läuft —
-- der braucht sie. Eine Ankerfiliale ohne PLZ im Verzeichnis kommt vor
-- (einzelne Finder liefern keine); dann bleibt `plz` null und der Dispatch
-- unterbleibt mit einer Warnung, statt ein leeres Gebiet zu scrapen.
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

-- ============================================================
-- Trigger-Selbstschutz: Cooldown PRO GEBIET
-- ============================================================
--
-- Wie in v11 und v14 gilt: `revoke execute` stoppt einen Trigger NICHT —
-- Postgres prüft das EXECUTE-Recht einmalig bei CREATE TRIGGER. Das
-- Ratenlimit muss im Funktionskörper sitzen.
--
-- Der Cooldown zählt pro **Gebiet**, nicht pro Ankerfiliale: Zwei Nutzer
-- desselben Ortes wählen verschiedene Anker, meinen aber dasselbe Gebiet.
-- Ohne das liefen für 04639 zwei identische Drei-Minuten-Läufe.
--
-- 30 Minuten statt der 10 aus v14, weil ein Gebiets-Lauf länger dauert
-- (~3 min gegen ~40 s) und weil Filialen sich in Monaten ändern, nicht in
-- Stunden — ein zweiter Lauf am selben Tag hat nichts Neues zu holen.
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

-- ============================================================
-- Verbleibendes Risiko
-- ============================================================
--
-- Der Cooldown bremst nur Wiederholungen desselben Gebiets. Mit dem anon-Key
-- lassen sich weiterhin viele VERSCHIEDENE Gebiete je einmal anfordern —
-- gedeckelt durch das Verzeichnis: Angefordert werden kann nur, was in
-- `branches` steht (Stand 2026-07-26: 3.171 Zeilen), und die verteilen sich
-- auf deutlich weniger Gebiete. Ein Angreifer könnte also GitHub-Actions-
-- Minuten verbrennen, keine fremden Daten lesen oder schreiben.
--
-- Sollte das je gebraucht werden, ist die nächste Stufe ein globales Budget
-- (n Dispatches pro Stunde über alle Gebiete) im selben Funktionskörper —
-- bewusst noch nicht gebaut, solange es keinen Nutzer außer den Testern gibt.

-- ============================================================
-- Gegenprobe nach dem Ausführen
-- ============================================================
-- 1) Struktur:
-- select count(*) from public.area_requests;                      -- 0
--
-- 2) Erfundene Filiale wird abgelehnt (mit dem publishable Key der App!):
--    insert into public.area_requests (market_id) values ('GIBTSNICHT');
--    -- erwartet: 42501 (new row violates row-level security policy)
--
-- 3) Echte Filiale: PLZ wird nachgeschlagen und ein Lauf gestartet.
--    insert into public.area_requests (market_id) values ('531032');
--    select market_id, plz, last_synced from public.area_requests;
--    -- erwartet: plz = '04639', last_synced = null,
--    --           und in Actions läuft "Filialverzeichnis auffrischen"
