-- ============================================================
-- Le Chariot – Migration v8: Filial-Koordinaten
-- Erweitert `markets` um lat/lon: der Region-Sync trägt jetzt echte
-- Filialen (Store-Finder von Lidl/ALDI, Koordinaten aus den
-- Marktsuchen von Penny/Kaufland) statt reiner Platzhalter ein.
-- NULL, wo der jeweilige Finder keine Koordinaten liefert.
-- Idempotent – kann gefahrlos erneut laufen.
-- ============================================================

alter table public.markets add column if not exists lat double precision;
alter table public.markets add column if not exists lon double precision;
