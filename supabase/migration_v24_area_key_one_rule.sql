-- Le Chariot — Migration v24
-- Eine Rundungsregel, an einer Stelle, und der Lauf rechnet sie nicht nach
--
-- NOCH NICHT GEGEN DIE PRODUKTION AUSGEFÜHRT. Von Hand im SQL-Editor laufen
-- lassen, und zwar NACH dem Merge des zugehörigen PRs: Der schickt `area_key`
-- als Dispatch-Eingabe mit, und eine Eingabe, die `branches.yml` auf master
-- noch nicht deklariert, beantwortet GitHub mit 422 — `trigger_area_scrape`
-- macht daraus eine Warnung, die niemand liest, und der Lauf bleibt aus.
-- Dieselbe Reihenfolge wie bei v21.
--
-- ============================================================
-- Das Problem
-- ============================================================
--
-- Die Rasterzelle einer Anforderung wird an DREI Stellen gerechnet: im
-- Trigger (`round(lat::numeric, 1)`), in `branches::area_cell` im Rust-Lauf
-- und in `AreaRequestStore.areaKey` in der App. SQL rundet dabei eine
-- Dezimalzahl, die beiden anderen rundeten einen IEEE-Binärwert — und das ist
-- nicht dieselbe Rechnung:
--
--   `(v * 10.0).round() / 10.0` zieht einen Wert, der knapp UNTER einer
--   Halben liegt, durch die Multiplikation auf die Halbe hoch und rundet dann
--   auf.
--
-- Gemessen am 2026-07-31: für den f64 direkt unter 53,85
-- (53.849999999999994) ergab die Binärrechnung `cell:53.9`, das SQL
-- `cell:53.8`. Die App wartet dann auf einen Schlüssel, den es nicht gibt,
-- und die Fertigmeldung des Laufs (`area_key=eq.…`) trifft keine Zeile —
-- beides still. Das ist die Fehlerklasse, die dieses Projekt inzwischen
-- viermal bezahlt hat: nichts schlägt fehl, es passiert nur nichts.
--
-- Wie erreichbar ist das wirklich? Gemessen wurde es, nicht geschätzt: Über
-- den ganzen deutschen Kasten (Breite 47,0-56,0, Länge 5,0-16,0) stimmen ALLE
-- Koordinaten mit ein bis vier Nachkommastellen auf beiden Wegen überein,
-- ebenso 400 000 zufällige Doubles. Verschieden ist das Ergebnis nur für
-- f64-Werte unmittelbar neben einer x,x5-Grenze (12 solche Breiten im
-- deutschen Bereich) — die entstehen aus keiner Dezimalzahl mit höchstens 15
-- signifikanten Stellen, also aus keiner Geokodierer-Antwort. Der Fehler ist
-- damit praktisch unerreichbar; behoben wird er trotzdem, weil er still ist
-- und die Korrektur nichts kostet.
--
-- ============================================================
-- Die Korrektur
-- ============================================================
--
-- 1. EINE Definition: `public.area_key(lat, lon)`. Der Trigger ruft sie auf,
--    statt die Formel ein zweites Mal hinzuschreiben, und die App darf sie
--    aufrufen, statt sie nachzubauen (`/rest/v1/rpc/area_key`).
-- 2. EINE Regel: gerundet wird auf der Dezimaldarstellung
--    (`round(v::text::numeric, 1)`), nicht auf dem Binärwert. `v::text` ist
--    seit Postgres 12 die kürzeste Darstellung, die wieder auf denselben
--    double liest; `format!("{v}")` in Rust liefert genau dieselbe. Damit
--    runden beide Seiten dieselbe Zeichenkette — unabhängig davon, wie die
--    Gleitkomma-Multiplikation gerundet hätte.
-- 3. Der Lauf rechnet gar nicht mehr: Der Dispatch schickt `area_key` mit,
--    und `branches-sync --area-key` benutzt ihn wortwörtlich. Der
--    Sonntagslauf liest ihn ohnehin schon von der Zeile.
--
-- Bleibt die App als einzige Stelle, die den Schlüssel noch selbst bildet —
-- das ist ein eigener PR im App-Repo, siehe unten.
--
-- Idempotent — kann gefahrlos erneut laufen.

-- ============================================================
-- 1) Die eine Definition
-- ============================================================
--
-- `immutable`, damit sie in einem Index oder einer Prüfbedingung stehen
-- könnte; `strict`, damit sie ohne Koordinaten NULL liefert statt
-- 'cell:,' — der Trigger unterscheidet genau daran.
--
-- `set extra_float_digits = 1` pinnt die Textausgabe auf die kürzeste
-- Darstellung, egal was in der Sitzung eingestellt ist. Ohne diese Zeile
-- hinge die Regel an einer Sitzungsvariablen, und das ist genau die Sorte
-- stiller Abhängigkeit, die hier gerade repariert wird.
create or replace function public.area_key(lat double precision, lon double precision)
returns text
language sql
immutable
strict
set extra_float_digits = 1
as $$
  select 'cell:' || round((lat::text)::numeric, 1)::text
              || ',' || round((lon::text)::numeric, 1)::text;
$$;

comment on function public.area_key(double precision, double precision) is
    'Rasterzelle als Text, 0,1 Grad, von der Null weg gerundet — auf der '
    'kürzesten Dezimaldarstellung, nicht auf dem Binärwert. Die EINE '
    'Definition: Trigger, Lauf und App müssen dieselbe Zeichenkette bekommen, '
    'sonst wartet die App auf einen Schlüssel, den es nicht gibt.';

-- Die App darf fragen, statt zu raten. Lesend und ohne Tabellenzugriff — die
-- Funktion rechnet nur.
grant execute on function public.area_key(double precision, double precision)
    to anon, authenticated;

-- ============================================================
-- 2) Bestandszeilen auf die eine Regel ziehen
-- ============================================================
--
-- Erwartung: NULL Zeilen. Die sechs Zeilen vom 2026-07-31 tragen
-- Geokodierer-Koordinaten, und für die ist die Regel unverändert. Trifft es
-- doch eine, dann war genau sie der stille Fall — ihr Schlüssel war
-- unerreichbar, das Ändern kann also nichts kaputt machen, was nicht schon
-- kaputt war. Vorher einmal lesen, damit die Zahl bekannt ist:
--
--   select id, market_id, lat, lon, area_key, public.area_key(lat, lon) as neu
--     from public.area_requests
--    where lat is not null and lon is not null
--      and area_key is distinct from public.area_key(lat, lon);
update public.area_requests
   set area_key = public.area_key(lat, lon)
 where lat is not null
   and lon is not null
   and area_key is distinct from public.area_key(lat, lon);

-- ============================================================
-- 3) Der Trigger ruft die Funktion auf, statt die Formel zu wiederholen
-- ============================================================
--
-- Wort für Wort v22, bis auf die eine Zuweisung am Ende. Die Reihenfolge
-- bleibt: Der Schlüssel entsteht ERST, nachdem die Koordinaten geprüft (und
-- gegebenenfalls verworfen) wurden.
create or replace function public.area_request_resolve_plz()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
declare
  anchor_lat  double precision;
  anchor_lon  double precision;
  gap_km      double precision;
begin
  select b.plz, b.lat, b.lon
    into new.plz, anchor_lat, anchor_lon
  from public.branches b
  where b.market_id = new.market_id;

  if new.lat is null or new.lon is null then
    new.lat := null;
    new.lon := null;
  elsif new.lat not between 47.0 and 56.0
     or new.lon not between 5.0 and 16.0 then
    raise notice 'area_request_resolve_plz: %/% liegt nicht in Deutschland, Koordinaten verworfen',
      new.lat, new.lon;
    new.lat := null;
    new.lon := null;
  else
    gap_km := public.haversine_km(new.lat, new.lon, anchor_lat, anchor_lon);
    if gap_km is null then
      raise notice 'area_request_resolve_plz: Anker % ohne Koordinaten, Client-Position ungeprueft uebernommen',
        new.market_id;
    elsif gap_km > 60.0 then
      raise notice 'area_request_resolve_plz: %/% liegt % km vom Anker % — Koordinaten verworfen',
        new.lat, new.lon, round(gap_km::numeric, 1), new.market_id;
      new.lat := null;
      new.lon := null;
    end if;
  end if;

  -- Erst jetzt, mit geprüfter Lage — und über die eine Definition.
  new.area_key := coalesce(
    public.area_key(new.lat, new.lon),
    case when new.plz is not null then 'plz:' || new.plz end
  );

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
-- 4) Der Dispatch gibt den Schlüssel mit
-- ============================================================
--
-- Wort für Wort v22, bis auf die eine zusätzliche Eingabe. Damit rechnet der
-- Lauf die Zelle nicht mehr nach — er bekommt sie. `branches.yml` muss
-- `area_key` deklariert haben, bevor diese Migration läuft (sonst 422).
create or replace function public.trigger_area_scrape()
returns trigger
language plpgsql
security definer
set search_path = public, extensions, net, vault
as $$
declare
  pat            text;
  last_dispatch  timestamptz;
  dispatch_key   text;
begin
  if new.last_synced is not null then
    return new;
  end if;

  dispatch_key := new.area_key;

  if dispatch_key is null then
    raise warning 'trigger_area_scrape: Filiale % hat weder PLZ noch Koordinaten, kein Gebiets-Lauf',
      new.market_id;
    return new;
  end if;

  select last_dispatch_at into last_dispatch
  from public.area_dispatch_log
  where area_key = dispatch_key;

  if last_dispatch is not null
     and last_dispatch > now() - interval '30 minutes' then
    raise notice 'trigger_area_scrape: Gebiet % vor % dispatcht (< 30 min Cooldown), uebersprungen',
      dispatch_key, now() - last_dispatch;
    return new;
  end if;

  select decrypted_secret into pat
  from vault.decrypted_secrets
  where name = 'github_pat';

  if pat is null then
    raise warning 'trigger_area_scrape: Vault secret github_pat missing, no run dispatched for area %',
      dispatch_key;
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
      'inputs', jsonb_build_object(
        'area',          coalesce(new.plz, ''),
        'skip_national', 'true',
        'lat',           coalesce(new.lat::text, ''),
        'lon',           coalesce(new.lon::text, ''),
        'anchor',        new.market_id,
        'area_key',      dispatch_key
      )
    )
  );

  insert into public.area_dispatch_log (area_key, last_dispatch_at)
  values (dispatch_key, now())
  on conflict (area_key) do update set last_dispatch_at = excluded.last_dispatch_at;

  return new;
exception when others then
  raise warning 'trigger_area_scrape: dispatch failed for area %: %', dispatch_key, sqlerrm;
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
-- Was die APP jetzt anders machen kann
-- ============================================================
--
-- `AreaRequestStore.areaKey(lat:lon:)` baut die Regel bis heute selbst nach —
-- mit `(v * 10).rounded() / 10`, also der Binärrechnung, die hier gerade
-- abgeschafft wird. Sie ist damit die letzte Stelle, an der ein Schlüssel
-- entstehen kann, den der Server nicht kennt. Zwei Wege, beide im App-Repo:
--
--   a) fragen statt rechnen:
--      POST /rest/v1/rpc/area_key  {"lat":53.944,"lon":14.183}  -> "cell:53.9,14.2"
--   b) rechnen, aber auf der Dezimaldarstellung — `"\(v)"` ist in Swift
--      dieselbe kürzeste Darstellung wie `v::text` in Postgres.
--
-- Bis dahin ist die App nur für f64-Werte unmittelbar neben einer x,x5-Grenze
-- falsch, und die liefert CLGeocoder nicht.
--
-- ============================================================
-- Gegenprobe nach dem Ausführen
-- ============================================================
-- Es gibt KEINE Testinfrastruktur für Migrationen — kein pgTAP, kein lokales
-- Postgres, keine Supabase-CLI. Diese Liste ist das einzige Tor. Punkte 1-4 im
-- SQL-Editor, 5-6 mit curl und dem PUBLISHABLE Key.
--
-- 1) Die Regel selbst — die drei Zeilen, um die es geht:
--    select public.area_key(53.9440, 14.1830) as ahlbeck,
--           public.area_key(53.7383, 14.0511) as ueckermuende,
--           public.area_key(53.85,   14.15)   as glatte_grenze,
--           public.area_key(53.849999999999994, 14.149999999999999) as knapp_darunter;
--    -- erwartet: cell:53.9,14.2 | cell:53.7,14.1 | cell:53.9,14.2 | cell:53.8,14.1
--    -- Die letzte Spalte ist der Fall: Vor v24 rechnete der Lauf dort
--    -- cell:53.9 und die Datenbank cell:53.8.
--
-- 2) Nichts an den Bestandszeilen hat sich verschoben (erwartet: 0 Zeilen):
--    select id, market_id, area_key, public.area_key(lat, lon) as neu
--      from public.area_requests
--     where lat is not null and area_key is distinct from public.area_key(lat, lon);
--
-- 3) Der Trigger benutzt die Funktion:
--    select prosrc like '%public.area_key(new.lat, new.lon)%'
--      from pg_proc where proname = 'area_request_resolve_plz';
--    -- erwartet: t
--
-- 4) Der Dispatch schickt den Schlüssel mit:
--    select prosrc like '%''area_key'',      dispatch_key%'
--      from pg_proc where proname = 'trigger_area_scrape';
--    -- erwartet: t
--
-- 5) Die App darf fragen (PUBLISHABLE Key, nicht der Service-Key):
--    curl -s -X POST "$SUPABASE_URL/rest/v1/rpc/area_key" \
--      -H "apikey: $ANON_KEY" -H "Authorization: Bearer $ANON_KEY" \
--      -H "Content-Type: application/json" \
--      -d '{"lat":53.9440,"lon":14.1830}'
--    -- erwartet: "cell:53.9,14.2"
--
-- 6) Ein echter Insert läuft weiter durch und löst weiter aus:
--    curl -i -X POST "$SUPABASE_URL/rest/v1/area_requests" \
--      -H "apikey: $ANON_KEY" -H "Authorization: Bearer $ANON_KEY" \
--      -H "Content-Type: application/json" \
--      -d '{"market_id":"4030191","lat":53.7383,"lon":14.0511}'
--    -- erwartet: 201, Zeile mit area_key cell:53.7,14.1
--    select id, status_code, left(content,200) from net._http_response
--     order by created desc limit 1;
--    -- erwartet: 204 — NICHT 422. Ein 422 heißt: branches.yml auf master
--    -- kennt die Eingabe area_key noch nicht, der PR ist nicht gemergt.
--    -- Aufräumen (Service-Rechte):
--    delete from public.area_requests where market_id='4030191' and last_synced is null;
--    delete from public.area_dispatch_log;
