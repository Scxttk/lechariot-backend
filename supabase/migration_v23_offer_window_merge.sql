-- Migration v23 — Bestandsbereinigung: überlappende Gültigkeitsfenster
-- desselben Angebots verschmelzen.
--
-- NOCH NICHT AUSGEFÜHRT. Von Hand im SQL-Editor laufen lassen, und zwar
-- NACH dem Merge des Push-Fixes (dedupe_rows, Stufe 2) — sonst legt der
-- nächste Sync-Lauf dieselben Duplikate sofort wieder an.
--
-- Regel (identisch zur Push-Seite): Zeilen derselben Filiale mit demselben
-- Produkt und demselben Preis, deren Gültigkeitsfenster sich überlappen,
-- sind EIN Angebot. Die früheste Zeile bleibt stehen und bekommt das
-- Gesamtfenster (frühester Start, spätestes Ende; ein offenes Ende bleibt
-- offen), die übrigen fallen weg.
--
-- Ausdrücklich unangetastet: disjunkte Fenster (echte Folgewochen) und
-- überlappende Fenster mit verschiedenem Preis (Aktionstage,
-- Lidl-Sammeltitel wie „Bio-Käse").
--
-- Gemessen am 2026-07-31 (read-only, vor dem Schreiben dieser Migration):
-- 38.126 Zeilen, 1.079 doppelte (market_id, product)-Kombinationen mit
-- 2.158 Zeilen; diese Regel entfernt 523 Zeilen — alle bei Kaufland, alle
-- nach dem Muster „Wochenfenster plus Aktionstag zum selben Preis".
--
-- Ohne diese Bereinigung heilt der Bestand auch von selbst, nur langsamer:
-- Der Push löscht je Filiale alle Zeilen mit valid_from vor dem neuesten
-- Start des Laufs, spätestens mit der Folgewoche sind die Altzeilen weg.
-- Die Migration zieht das nur vor.

begin;

create temp table _offer_window_merge as
with ranked as (
    select id,
           market_id,
           product,
           price,
           valid_from,
           valid_until,
           coalesce(valid_until, date '9999-12-31') as until_x,
           max(coalesce(valid_until, date '9999-12-31')) over (
               partition by market_id, product, price
               order by valid_from, coalesce(valid_until, date '9999-12-31'), id
               rows between unbounded preceding and 1 preceding
           ) as prev_max
      from public.offers
     where valid_from is not null
),
clustered as (
    -- Ein neues Cluster beginnt, sobald ein Start hinter dem spätesten
    -- bisherigen Ende seiner Gruppe liegt — dieselbe Durchlauf-Logik wie in
    -- push::merge_overlapping_windows.
    select *,
           count(*) filter (where prev_max is null or valid_from > prev_max) over (
               partition by market_id, product, price
               order by valid_from, until_x, id
               rows between unbounded preceding and current row
           ) as cluster
      from ranked
)
select market_id,
       product,
       price,
       cluster,
       (array_agg(id order by valid_from, id)) as ids,
       min(valid_from) as new_from,
       case when count(*) filter (where valid_until is null) > 0
            then null
            else max(valid_until)
       end as new_until
  from clustered
 group by market_id, product, price, cluster
having count(*) > 1;

-- Erst löschen, dann das Fenster des Bleibenden weiten — umgekehrt stieße
-- das Update auf den Unique-Index (market_id, product, valid_from), weil
-- die wegfallende Zeile genau dieses valid_from noch trägt.
delete from public.offers o
 using _offer_window_merge m
 where o.id = any(m.ids)
   and o.id <> m.ids[1];

update public.offers o
   set valid_from = m.new_from,
       valid_until = m.new_until
  from _offer_window_merge m
 where o.id = m.ids[1];

drop table _offer_window_merge;

commit;
