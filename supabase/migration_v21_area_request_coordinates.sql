-- Le Chariot — Migration v21
-- Das Gebiet kommt aus den Koordinaten, nicht aus der Ankerfiliale
--
-- ============================================================
-- Das Problem
-- ============================================================
--
-- Gemeldet am 2026-07-30 aus Ahlbeck (17419), dem ersten Tester außerhalb des
-- Hauses. `area_requests` trägt nur die Ankerfiliale; der v19-BEFORE-Trigger
-- holt die PLZ aus deren `branches`-Zeile. In Ahlbeck ist die nächste bekannte
-- Filiale Penny Am Haff in Ueckermünde (17373) — **24,5 km entfernt**
-- (53,9440/14,1830 gegen 53,7383/14,0511, Haversine). Der ganze
-- Verzeichnislauf lief damit für Ueckermünde.
--
-- Nichts schlug fehl. Der Lauf war grün (53 s), 13 Filialen wurden
-- geschrieben, die Anforderung wurde als erledigt gestempelt — nur eben für
-- einen Ort, in dem niemand gefragt hatte. Gegenprobe: null Zeilen mit
-- `plz = 17419`. Dass ausgerechnet REWE, Netto und EDEKA fehlten, ist kein
-- Zufall: Das sind die drei Textsuchen je PLZ, sie bekamen „17373" zu sehen.
--
-- Dieselbe Klasse wie v20, eine Ebene höher: ein Weg, der sich selbst für
-- erfolgreich hält. Nur ist hier nicht die Policy schuld, sondern die Annahme,
-- die nächste bekannte Filiale liege im selben Ort. In einem Gebiet, das noch
-- nie geholt wurde, stehen im Verzeichnis ausschließlich Kaufland und Penny —
-- genau dort ist diese Annahme am schwächsten.
--
-- Ein Anker ist ein **Existenzbeweis**, kein Ortsbeweis: Er sagt „diese
-- Filiale gibt es", nicht „hier steht der Nutzer".
--
-- ============================================================
-- Die Korrektur
-- ============================================================
--
-- Die App schickt die MITTE DER REGION, um die ihr Picker gesucht hat — nicht
-- die Geräteposition. Die gibt es ohne Ortungsfreigabe nicht, und „jede Region
-- fordert für sich an" ist die Entscheidung vom 30.07. Das Backend
-- geocodiert daraus rückwärts (Nominatim /reverse) die echte PLZ.
--
-- Die Ankerfiliale bleibt, aus zwei Gründen. Erstens ist sie weiterhin das
-- Einzige, was sich hart prüfen lässt (die Lehre aus der Phantom-PLZ 94108,
-- siehe v19) — an ihr wird die mitgeschickte Lage gemessen. Zweitens bleibt
-- ihre PLZ der RÜCKFALL: Fällt irgendetwas am Koordinatenweg aus, ist das
-- Verhalten exakt das von heute, und heute funktioniert für alle, die nicht
-- 24 km neben ihrem Anker wohnen.
--
-- ============================================================
-- Warum lat/lon NICHT in der Policy stehen
-- ============================================================
--
-- Die Lehre aus v20, wörtlich: Eine WITH-CHECK-Bedingung sieht nicht den
-- Request, sondern das Ergebnis aller BEFORE-Trigger. Der Trigger hier NULLT
-- lat/lon, wenn sie unplausibel sind — eine Policy-Bedingung über lat/lon
-- verböte also ihr eigenes Trigger-Ergebnis, und wie in v20 käme kein Insert
-- durch, ohne dass irgendwo etwas fehlschlägt.
--
-- Gegen den Client sichern die SPALTENRECHTE, die Plausibilität sichert der
-- TRIGGER. Die Policy aus v20 bleibt deshalb unangetastet.
--
-- Idempotent — kann gefahrlos erneut laufen.

-- ============================================================
-- 1) Spalten und Rechte
-- ============================================================

alter table public.area_requests
    add column if not exists lat double precision,
    add column if not exists lon double precision;

comment on column public.area_requests.lat is
    'Mitte der Region, um die die App gesucht hat (nicht die Geraeteposition). '
    'Vom BEFORE-Trigger genullt, wenn unplausibel oder > 60 km vom Anker. '
    'Null = der reine PLZ-Weg aus v19.';
comment on column public.area_requests.lon is
    'Siehe lat. Nur beide zusammen sind etwas wert.';

-- `revoke insert on <table>` räumt auch die Spaltenrechte ab, der Zweischritt
-- ist damit idempotent (derselbe wie in v19).
--
-- `plz` bleibt bewusst DRAUSSEN: abgeleitet, nie vom Client benannt. Genau
-- daran scheitert die Phantom-PLZ.
revoke insert on public.area_requests from anon, authenticated;
grant  insert (market_id, lat, lon) on public.area_requests to anon, authenticated;

-- ============================================================
-- 2) Haversine von Hand
-- ============================================================
--
-- Kein PostGIS, kein earthdistance — v12 hat PostGIS für das Verzeichnis
-- bewusst abgelehnt („auf dem Free-Tier nur Ballast"), und für EINEN Vergleich
-- pro Insert gilt das erst recht. Dieselbe Formel wie
-- `store_finder::distance_km` im Rust-Backend, damit Prüfung und Umkreisfilter
-- nicht auseinanderlaufen.
--
-- `strict`: Ist ein Argument null, liefert die Funktion null, ohne zu rechnen.
-- Genau das braucht der Trigger — „der Anker hat keine Koordinaten" ist nicht
-- dasselbe wie „die Entfernung ist 0". `branches.lat/lon` sind nullable, und
-- das ist Absicht (der Rust-Import behält Zeilen ohne Koordinaten).
--
-- `least(1.0, ...)`: asin(>1) ist in Postgres ein FEHLER, kein NaN, und würde
-- den gesamten Insert scheitern lassen. Für echte Koordinaten unerreichbar,
-- für Müll vom Client (lat = 1e9) nicht.
create or replace function public.haversine_km(
    lat1 double precision, lon1 double precision,
    lat2 double precision, lon2 double precision
) returns double precision
language sql
immutable
strict
as $$
  select 2 * 6371.0 * asin(least(1.0, sqrt(
        power(sin(radians(lat2 - lat1) / 2), 2)
      + cos(radians(lat1)) * cos(radians(lat2))
        * power(sin(radians(lon2 - lon1) / 2), 2)
  )));
$$;

revoke execute on function public.haversine_km(
    double precision, double precision, double precision, double precision
) from public, anon, authenticated;

-- ============================================================
-- 3) BEFORE INSERT: PLZ nachschlagen UND Koordinaten prüfen
-- ============================================================
--
-- Reihenfolge und Grenzwerte, mit den Messungen dahinter:
--
--  a) PLZ aus dem Anker holen — unverändert aus v19. Sie ist ab jetzt der
--     RÜCKFALL, nicht die Wahrheit. Schlägt das Reverse-Geocoding im Lauf
--     fehl, arbeitet der Lauf mit ihr weiter und verhält sich wie heute.
--
--  b) Nur eine Hälfte der Koordinaten ist nichts wert -> beide null.
--
--  c) Deutschland-Kasten 47,0-56,0 N / 5,0-16,0 O. Fängt groben Müll ab, bevor
--     gerechnet wird — auch NaN, denn `'NaN' between 47 and 56` ist false,
--     `not between` also true. Luft an jeder Kante: List 55,0 N, Oberstdorf
--     47,4 N, Selfkant 5,9 O, Görlitz 15,0 O.
--
--  d) Abstand zum Anker: **60 km, nicht 30.** Der Grenzwert ist keine freie
--     Wahl, der Client gibt ihn vor. `MarketPickerView.maxRadiusKm` = 40, und
--     `nearbyWideningIfSparse` verdoppelt den Radius, bis sechs Filialen im
--     Umkreis stehen. In einem NIE GEHOLTEN Gebiet stehen dort nur Kaufland
--     und Penny — der Weg auf 40 km ist dort der Normalfall, nicht die
--     Ausnahme, und `LiveBranchRepository.nearby` schneidet die Bounding-Box
--     in Swift auf einen echten Kreis zurück. Ein legitimer Anker darf also
--     konstruktionsbedingt 40 km entfernt liegen.
--
--     Ein 30-km-Tor verwürfe die Koordinaten genau in dem Fall, für den v21
--     gebaut ist, und fiele still auf den Fehler von heute zurück. 60 km lässt
--     jeden legitimen Anker durch und fängt weiter „Koordinaten aus einem
--     anderen Bundesland". Zur Einordnung: Ahlbeck -> Ueckermünde 24,5 km,
--     Ahlbeck -> Karlshagen 26,7 km.
--
--  e) Anker OHNE Koordinaten: die Client-Koordinaten BLEIBEN, mit Notiz. Eine
--     `branches`-Zeile ohne lat/lon hat auch eine unzuverlässige PLZ; dort auf
--     die PLZ zurückzufallen hieße, von zwei ungeprüften Angaben die
--     schlechtere zu nehmen. Der Deutschland-Kasten bleibt die Grenze, und
--     neue Fähigkeiten entstehen dadurch nicht: Wer den anon-Key hat, wählt
--     das Zielgebiet ohnehin über den Anker.
--
--     Aus DIESER App kommt der Fall nicht vor — `PickerDirectory` nimmt als
--     Anker die NÄCHSTE Filiale, und „nächste" setzt Koordinaten voraus. Wer
--     es anders will, ersetzt das `raise notice` durch die beiden
--     Null-Zuweisungen. Eine Zeile.
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
    return new;
  end if;

  if new.lat not between 47.0 and 56.0
     or new.lon not between 5.0 and 16.0 then
    raise notice 'area_request_resolve_plz: %/% liegt nicht in Deutschland, Koordinaten verworfen',
      new.lat, new.lon;
    new.lat := null;
    new.lon := null;
    return new;
  end if;

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
-- 4) Cooldown: Rasterzelle statt PLZ
-- ============================================================
--
-- `area_dispatch_log` hing an der PLZ, und der Trigger prüft sie beim INSERT —
-- also bevor die echte PLZ bekannt ist. Auf die ANKER-PLZ zu schlüsseln hieße:
-- Eine Anforderung aus Ueckermünde unterdrückt 30 Minuten lang die aus
-- Ahlbeck. Das ist exakt die stille Fehlklasse, die v21 abschafft.
--
-- Liegen Koordinaten vor, ist der Schlüssel deshalb die RASTERZELLE (auf 0,1°
-- gerundet), sonst weiter die PLZ. Die Präfixe halten beides auseinander und
-- machen die Zeile im SQL-Editor lesbar.
--
-- Warum 0,1° — gerechnet, nicht geraten:
--
--   Zellengröße:  0,1° Breite = 11,1 km überall.
--                 0,1° Länge  =  6,5 km bei 54,0 N (Usedom),
--                                7,6 km bei 47,3 N (Bodensee).
--   Diagonale:    höchstens sqrt(11,1² + 7,6²) = 13,5 km.
--
--   1) Trennt sie den gemeldeten Fall?
--      Ahlbeck  53,9440/14,1830 -> Zelle 53,9/14,2
--      Ueckerm. 53,7383/14,0511 -> Zelle 53,7/14,1
--      In BEIDEN Achsen verschieden. Ja.
--
--   2) Was macht sie mit zwei echten Nachbarn?
--      Zwei Anforderungen in derselben Zelle liegen höchstens 13,5 km
--      auseinander. Der Lauf holt `branches::AREA_RADIUS_KM` = 25 km um den
--      ERSTEN Mittelpunkt — der zweite Nutzer liegt also mit 11,5 km Luft im
--      schon geholten Kreis. Eine Unterdrückung innerhalb einer Zelle kostet
--      ihn keine Filiale, die er erreichen könnte.
--
--   3) Und andersherum?
--      Zwei Nachbarn 500 m auseinander können auf eine Zellengrenze fallen und
--      bekommen zwei Läufe. Kostet drei Minuten Actions-Zeit und niemandem ein
--      Ergebnis — die harmlose Richtung des Fehlers, und deshalb die gewählte.
--
-- Dieselbe Regel steht als `branches::area_cell` im Rust-Backend und wird dort
-- von `the_cooldown_cell_separates_ahlbeck_from_ueckermuende` geprüft. Laufen
-- die beiden Seiten auseinander, holt ein Lauf ein Gebiet doppelt oder gar
-- nicht.
--
-- Die Altzeilen tragen blanke PLZ ('04639'), die neuen Schlüssel ein Präfix
-- ('plz:04639'). Nichts davon trifft sich, also gibt es einmalig höchstens
-- einen Extra-Dispatch je Gebiet, das in den letzten 30 Minuten lief. Nicht
-- aufgeräumt, weil es sich von selbst erledigt.
do $$
begin
  if exists (
    select 1 from information_schema.columns
     where table_schema = 'public'
       and table_name   = 'area_dispatch_log'
       and column_name  = 'plz'
  ) then
    alter table public.area_dispatch_log rename column plz to area_key;
  end if;
end
$$;

-- ============================================================
-- 5) Dispatch: Koordinaten und Anker mitgeben
-- ============================================================
--
-- `branches.yml` MUSS die Eingaben `lat`, `lon` und `anchor` schon deklariert
-- haben, sonst antwortet GitHub mit 422 („Unexpected inputs provided") — und
-- der `exception when others`-Block unten schluckt das in eine Warnung, die
-- niemand liest. Ein stummer Totalausfall des Features, dieselbe Bühne wie
-- v20. Deshalb steht der Workflow-Merge VOR dieser Migration, und deshalb
-- steht die Antwortprüfung als Punkt 7 in der Gegenprobe.
--
-- Alles als String: workflow_dispatch-Eingaben ohne `type` sind Text, und
-- `skip_national` fährt seit v19 genau so.
--
-- `anchor` ist neu und trägt den Rückweg: Die echte PLZ steht erst fest, wenn
-- der Lauf sie geholt hat. Er stempelt sie dann auf DIESE Zeile (`market_id`
-- ist der Primärschlüssel), womit die Zeile in sich stimmig wird — und damit
-- funktioniert die bestehende Fertigmeldung über die PLZ unverändert weiter
-- UND das Sonntagsnetz (`--from-area-requests`) liest kein falsches Gebiet
-- mehr.
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
  -- Der Backend-Lauf stempelt am Ende `last_synced`. Das ist ein UPDATE und
  -- kommt hier nicht an (Trigger nur auf INSERT), aber die Bedingung bleibt
  -- als Gürtel: Wer eine fertige Zeile neu einspielt, löst nichts aus.
  if new.last_synced is not null then
    return new;
  end if;

  -- Ohne Gebiet UND ohne Koordinaten gibt es nichts anzustoßen. Anders als in
  -- v19 reicht jetzt EINES von beidem: Ein Anker ohne Verzeichnis-PLZ, aber
  -- mit geprüfter Client-Position, ist ab hier ein gültiger Auftrag.
  if new.plz is null and (new.lat is null or new.lon is null) then
    raise warning 'trigger_area_scrape: Filiale % hat weder PLZ noch Koordinaten, kein Gebiets-Lauf',
      new.market_id;
    return new;
  end if;

  if new.lat is not null and new.lon is not null then
    dispatch_key := 'cell:' || round(new.lat::numeric, 1) || ',' || round(new.lon::numeric, 1);
  else
    dispatch_key := 'plz:' || new.plz;
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
        -- Die Anker-PLZ, weiterhin. Ab jetzt nur noch der Rückfall für den
        -- Fall, dass das Reverse-Geocoding im Lauf scheitert.
        'area',          coalesce(new.plz, ''),
        -- skip_national: Kaufland und Penny stehen schon bundesweit drin, die
        -- zwei Requests wären reine Wiederholung.
        'skip_national', 'true',
        'lat',           coalesce(new.lat::text, ''),
        'lon',           coalesce(new.lon::text, ''),
        'anchor',        new.market_id
      )
    )
  );

  insert into public.area_dispatch_log (area_key, last_dispatch_at)
  values (dispatch_key, now())
  on conflict (area_key) do update set last_dispatch_at = excluded.last_dispatch_at;

  return new;
exception when others then
  -- Die Anforderung darf nie daran scheitern, dass der Dispatch scheitert.
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
-- Verbleibendes Risiko, bewusst offen gelassen
-- ============================================================
--
-- `market_id` ist der Primärschlüssel von `area_requests`. Zwei Nutzer aus
-- VERSCHIEDENEN Orten können denselben nächsten Anker haben — im gemeldeten
-- Fall ist genau das so: Penny Am Haff ist die nächste Filiale sowohl für
-- Ueckermünde als auch für Ahlbeck. Wer zweiter kommt, bekommt 409 oder
-- übernimmt die fremde Zeile, und seine Koordinaten landen nie.
--
-- Die App weicht deshalb ab v21 auf den ZWEITNÄCHSTEN Anker aus, wenn die
-- vorhandene Zeile Koordinaten trägt, die mehr als eine Rasterzelle (13,5 km)
-- von den eigenen entfernt sind. Das deckt den gemeldeten Fall. Es deckt NICHT
-- den Fall, dass alle Anker einer Region schon fremd belegt sind — dann geht
-- die Anforderung still nicht raus.
--
-- Der saubere Schritt wäre v22: Surrogatschlüssel statt `market_id` als PK,
-- mit einem Unique über die Rasterzelle. Bewusst nicht in derselben Nacht wie
-- alles andere.

-- ============================================================
-- Gegenprobe nach dem Ausführen
-- ============================================================
-- Es gibt KEINE Testinfrastruktur für Migrationen — kein pgTAP, kein lokales
-- Postgres, keine Supabase-CLI. Diese Liste ist das einzige Tor, und v19 ist
-- vier Tage lang an einer fehlenden Liste vorbeigekommen.
--
-- Punkte 1-5 im SQL-Editor, 6-11 mit curl und dem PUBLISHABLE Key (nicht dem
-- Service-Key — mit dem greift keine einzige Policy und die Probe wäre wertlos).
--
-- 1) Struktur:
--    select column_name, data_type from information_schema.columns
--     where table_schema='public' and table_name='area_requests'
--     order by ordinal_position;
--    -- erwartet: lat, lon als double precision
--    select column_name from information_schema.columns
--     where table_schema='public' and table_name='area_dispatch_log';
--    -- erwartet: area_key, last_dispatch_at (KEIN plz)
--
-- 2) Haversine gegen die Messung, auf der der 60-km-Grenzwert steht:
--    select round(public.haversine_km(53.9440,14.1830,53.7383,14.0511)::numeric,1);
--    -- erwartet: 24.5
--    select public.haversine_km(53.9440,14.1830,null,null);
--    -- erwartet: NULL   (strict: Anker ohne Koordinaten rechnet nicht)
--    select public.haversine_km(53.9440,14.1830,1e9,1e9);
--    -- erwartet: eine Zahl, KEIN Fehler (least(1.0,...) greift)
--
-- 3) Spaltenrechte — genau drei Zeilen, plz nicht dabei:
--    select column_name, privilege_type from information_schema.column_privileges
--     where table_schema='public' and table_name='area_requests'
--       and grantee='anon' and privilege_type='INSERT';
--    -- erwartet: market_id, lat, lon
--
-- 4) Rasterzelle trennt den gemeldeten Fall — und stimmt mit `area_cell` überein:
--    select 'cell:'||round(53.9440::numeric,1)||','||round(14.1830::numeric,1) as ahlbeck,
--           'cell:'||round(53.7383::numeric,1)||','||round(14.0511::numeric,1) as ueckermuende;
--    -- erwartet: cell:53.9,14.2  und  cell:53.7,14.1  (verschieden)
--
-- 5) Anker heraussuchen (im Editor, Service-Rechte):
--    select market_id, plz, city, lat, lon from public.branches
--     where city ilike 'Ueckermuende%' or city ilike 'Ueckermünde%' limit 5;
--    -- eine market_id davon ist im Folgenden <ANKER>
--
-- 6) DER KERNFALL — Anker Ueckermünde, Koordinaten Ahlbeck (24,5 km):
--    curl -i -X POST "$SUPABASE_URL/rest/v1/area_requests" \
--      -H "apikey: $ANON_KEY" -H "Authorization: Bearer $ANON_KEY" \
--      -H "Content-Type: application/json" \
--      -d '{"market_id":"<ANKER>","lat":53.9440,"lon":14.1830}'
--    -- erwartet: 201
--    select market_id, plz, lat, lon from public.area_requests where market_id='<ANKER>';
--    -- erwartet: plz = '17373' (der Rueckfall, hier noch bewusst „falsch" —
--    --           der Lauf stempelt gleich die echte drauf),
--    --           lat = 53.944, lon = 14.183  <- BEHALTEN, das ist der Test
--    select area_key, last_dispatch_at from public.area_dispatch_log
--     order by last_dispatch_at desc limit 3;
--    -- erwartet: 'cell:53.9,14.2', NICHT 'plz:17373'
--
-- 7) Der Dispatch wurde ANGENOMMEN, nicht mit 422 geschluckt. Das ist die
--    Prüfung, die den v20-Fehler am ersten Tag gefunden hätte:
--    select id, status_code, left(content,200) as content, created
--      from net._http_response order by created desc limit 3;
--    -- erwartet: 204. Bei 422 nennt `content` die unbekannte Eingabe — dann
--    --           ist branches.yml auf master noch nicht gemerged.
--    -- und in GitHub Actions laeuft „Filialverzeichnis auffrischen"
--
-- 8) Steuerspalte mitgeschickt — erwartet „permission denied for column":
--    ... -d '{"market_id":"<ANKER>","plz":"17419"}'
--
-- 9) Muell-Koordinaten werden genullt, der Insert geht trotzdem durch.
--    Vorher aufraeumen (Service-Rechte):
--      delete from public.area_requests where market_id='<ANKER>';
--      delete from public.area_dispatch_log;
--    ... -d '{"market_id":"<ANKER>","lat":0,"lon":0}'
--    -- erwartet: 201, lat/lon in der Zeile sind NULL, plz gesetzt
--
-- 10) Zu weit weg (Dresdner Anker, Ahlbecker Koordinaten, ~450 km):
--    ... -d '{"market_id":"<DRESDNER ANKER>","lat":53.9440,"lon":14.1830}'
--    -- erwartet: 201, lat/lon NULL, plz = die Dresdner
--
-- 11) Nur eine Haelfte:
--    ... -d '{"market_id":"<ANKER 2>","lat":53.9440}'
--    -- erwartet: 201, lat und lon beide NULL
--
-- 12) Aufraeumen (Service-Rechte):
--    delete from public.area_requests
--     where market_id in ('<ANKER>','<ANKER 2>','<DRESDNER ANKER>');
--    delete from public.area_dispatch_log;
