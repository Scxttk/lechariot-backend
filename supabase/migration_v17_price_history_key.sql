-- ============================================================
-- Le Chariot – Migration v17: der Schlüssel der Preis-Historie
--
-- Nachtrag zu v16. Dort ist `region` aus `price_history` gefallen — und mit
-- der Spalte hat Postgres den Unique-Constraint mitgenommen, der sie
-- enthielt (`price_history_branch_week_key` auf (market_id, product,
-- valid_from, region)). Danach hatte die Tabelle **gar keinen** passenden
-- Constraint mehr, und jeder Push brach ab:
--
--   42P10: there is no unique or exclusion constraint matching the
--          ON CONFLICT specification
--
-- Live gesehen am 2026-07-26: Der Angebots-Push lief durch (346 Zeilen), die
-- Historie danach nicht. Nicht schlimm — sie wird best-effort geschrieben und
-- der Fehler warnt nur —, aber ohne sie wächst der Preisverlauf nicht mehr.
--
-- Wer v16 aus dieser Datei-Version frisch fährt, braucht v17 nicht: der Teil
-- steht dort inzwischen mit drin. v17 ist der Nachtrag für die Datenbank, die
-- v16 in der ersten Fassung bekommen hat.
--
-- Idempotent – kann gefahrlos erneut laufen.
-- ============================================================

-- Ohne `region` können Zeilen doppelt sein, die sich vorher nur darin
-- unterschieden. Erst zusammenführen, dann den engeren Schlüssel anlegen.
delete from public.price_history a
using public.price_history b
where a.market_id = b.market_id
  and a.product = b.product
  and a.valid_from is not distinct from b.valid_from
  and (
        coalesce(a.recorded_at, '-infinity'::timestamptz)
          < coalesce(b.recorded_at, '-infinity'::timestamptz)
     or (coalesce(a.recorded_at, '-infinity'::timestamptz)
          = coalesce(b.recorded_at, '-infinity'::timestamptz)
         and a.id < b.id)
      );

alter table public.price_history drop constraint if exists price_history_branch_week_key;
alter table public.price_history drop constraint if exists price_history_week_key;

do $$
begin
    alter table public.price_history
        add constraint price_history_branch_week_key2
        unique nulls not distinct (market_id, product, valid_from);
exception
    when duplicate_object then null;
end $$;
