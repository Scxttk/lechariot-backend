-- Migration v20 — Gebiets-Anforderung reparieren: die v19-Policy lehnt
-- jeden gültigen Anker ab
--
-- ============================================================
-- Das Problem
-- ============================================================
--
-- Gefunden am 2026-07-30 beim End-zu-Ende-Nachweis aus dem Laufplan
-- („in einem Gebiet ohne Abdeckung onboarden"): Der Picker in Hamburg
-- zeigte korrekt nur Kaufland und Penny, die App schickte ihre
-- Gebiets-Anforderung — und Supabase antwortete 42501. Von Hand mit dem
-- publishable Key nachgestellt: JEDER Insert mit einem echten Anker
-- scheitert. Phase 13 war damit live wirkungslos; die App fängt den Fehler
-- bewusst ab („ein gescheiterter Request kostet die Zusatzketten, nicht die
-- App"), also sah niemand etwas.
--
-- Die Ursache ist ein Zusammenspiel zweier v19-Bausteine, die einzeln beide
-- richtig sind:
--
--   1. Die Insert-Policy verlangt `plz is null` — die PLZ ist abgeleitet,
--      der Client darf sie nicht mitbringen.
--   2. Der BEFORE-INSERT-Trigger `on_area_request_resolve` schlägt die PLZ
--      im Verzeichnis nach und setzt sie in NEW.
--
-- Postgres prüft die WITH-CHECK-Bedingung aber gegen die Zeile, WIE SIE
-- GESPEICHERT WIRD — also NACH den BEFORE-Triggern. Für jeden Anker, dessen
-- Filiale eine PLZ im Verzeichnis hat (das ist jeder echte Anker), ist
-- `plz` zum Prüfzeitpunkt gesetzt, und die Policy lehnt ab. Bestehen würde
-- die Prüfung nur ein Anker OHNE Verzeichnis-PLZ — genau der, bei dem der
-- Dispatch anschließend mit Warnung unterbleibt. Kein Insert konnte je
-- gleichzeitig durchkommen UND einen Lauf auslösen.
--
-- ============================================================
-- Die Korrektur
-- ============================================================
--
-- Die Bedingung `plz is null` fliegt aus der Policy. Was sie absichern
-- sollte, sichert bereits die Privilegienebene aus v19: `grant insert
-- (market_id)` — ein Client, der `plz` auch nur benennt, scheitert vorher
-- an „permission denied for column". Die Policy muss die Spalte deshalb
-- nicht noch einmal prüfen, und sie DARF es nicht, weil sie sonst das
-- Ergebnis des eigenen Triggers verbietet.
--
-- Lehre fürs nächste Mal: Eine WITH-CHECK-Bedingung sieht nicht den
-- Request, sondern das Ergebnis aller BEFORE-Trigger. Spalten, die ein
-- Trigger setzt, gehören nicht in die Policy — gegen den Client sichern
-- sie die Spaltenrechte.
--
-- Idempotent – kann gefahrlos erneut laufen.

drop policy if exists "Anon insert" on public.area_requests;
create policy "Anon insert" on public.area_requests
    for insert
    to anon, authenticated
    with check (
        active is true
        and last_synced is null
        and requested_at is not null
        and exists (
            select 1 from public.branches b
             where b.market_id = area_requests.market_id
        )
    );

-- ============================================================
-- Nachweis (mit dem PUBLISHABLE Key, nicht dem Service-Key)
-- ============================================================
--
-- 1. Echter Anker (steht in `branches`) — erwartet: 201, und die Zeile
--    trägt die vom Trigger nachgeschlagene PLZ:
--
--    curl -i -X POST "$SUPABASE_URL/rest/v1/area_requests" \
--      -H "apikey: $ANON_KEY" -H "Authorization: Bearer $ANON_KEY" \
--      -H "Content-Type: application/json" \
--      -d '{"market_id": "<market_id aus branches>"}'
--
-- 2. Erfundener Anker — erwartet: 42501, wie in v19:
--
--    ... -d '{"market_id": "GIBTSNICHT"}'
--
-- 3. Mitgeschickte Steuerspalte — erwartet: „permission denied for column":
--
--    ... -d '{"market_id": "<echt>", "plz": "99999"}'
