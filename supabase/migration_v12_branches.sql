-- ============================================================
-- Le Chariot – Migration v12: Filialverzeichnis `branches`
--
-- Bisher gab es nur `markets`: EINE Filiale je (Kette, PLZ) — die, die der
-- Store-Finder als nächste zur PLZ meldete. Das erzeugt Zeilen wie
-- „REWE in 01219", obwohl die gefundene Filiale in 01257 steht (gemessen
-- 2026-07-25: die REWE-Suche nach 01219 liefert fünf Filialen, keine davon
-- in 01219).
--
-- `branches` ist das Verzeichnis darunter: alle Filialen, die wir von den
-- Findern kennen, mit Adresse und Koordinaten. Die App zeigt daraus die
-- Märkte in der Nähe an, BEVOR irgendetwas gescrapt ist — und ab Phase 11
-- hängen die Angebote an `market_id` statt an (Kette, Region).
--
-- `markets` bleibt vorerst unangetastet: Die App läuft heute darauf, und
-- der Umbau des Angebots-Schlüssels ist ein eigener Schritt.
--
-- Idempotent – kann gefahrlos erneut laufen.
-- ============================================================

create table if not exists public.branches (
    -- Filial-ID der Kette, wie sie auch der Angebots-Scraper benutzt
    -- (REWE wwIdent, Kaufland "n", Lidl LIDL_<EntityID>, ALDI <prefix>_<id>).
    -- Über sie wird ab Phase 11 gescrapt, deshalb ist sie der Primärschlüssel
    -- und nicht (Kette, PLZ): eine PLZ kann mehrere Filialen derselben Kette
    -- haben, genau das war der Fehler im alten Modell.
    market_id   text primary key,
    -- Anzeigename der Kette, identisch mit `offers.market` ("REWE", "Kaufland").
    chain       text not null,
    name        text not null,
    street      text,
    plz         text,
    city        text,
    lat         double precision,
    lon         double precision,
    -- Welcher Finder die Zeile geliefert hat ("kaufland-storefinder",
    -- "uberall", "bing-sds", …) — bei Abweichungen zwischen zwei Quellen
    -- muss nachvollziehbar sein, wem die Zeile gehört.
    source      text,
    updated_at  timestamptz not null default now()
);

-- Umkreissuche der App: "alle Filialen in diesem Kasten". Ohne PostGIS, weil
-- ein Bounding-Box-Filter über zwei btree-Spalten für ein paar zehntausend
-- Zeilen genügt und PostGIS auf dem Free-Tier nur Ballast wäre.
create index if not exists branches_lat_lon_idx on public.branches (lat, lon);
create index if not exists branches_plz_idx on public.branches (plz);

alter table public.branches enable row level security;

-- Lesen darf jeder (die App fragt mit dem anon-Key nach Filialen in der Nähe).
drop policy if exists "Public read" on public.branches;
create policy "Public read" on public.branches
    for select using (true);

-- Geschrieben wird ausschließlich aus dem Sync mit dem service_role-Key.
-- Anders als bei `regions` gibt es hier bewusst KEINEN anon-Insert-Pfad:
-- Das Verzeichnis ist Backend-Datenbestand, keine Warteschlange. Der
-- Anforderungsweg für neue Filialen kommt in Phase 11 als eigene Tabelle
-- mit eigener, spaltenweise beschränkter Policy.
drop policy if exists "Service write" on public.branches;
create policy "Service write" on public.branches
    for all using (auth.role() = 'service_role');

revoke insert, update, delete on public.branches from anon, authenticated;
