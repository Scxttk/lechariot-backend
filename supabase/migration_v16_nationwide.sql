-- ============================================================
-- Smart Shop – Migration v16: `region` wird zu `nationwide`
--
-- Der Abschluss von Phase 12. Bis hierher trug jedes Angebot neben seiner
-- Filiale noch eine PLZ — die Doppelbuchhaltung, die der ganze Umbau
-- loswerden wollte:
--
--   * Vier Filialen standen unter MEHREREN PLZ, weil der Store-Finder sie
--     für jede Nachbar-PLZ erneut als nächste meldete (Penny 531053 gleich
--     in 01067/01069/01099/01139). Zusammen 2.889 Zeilen, die dasselbe
--     Angebot derselben Filiale mehrfach führten.
--   * Seit Migration v13 gehört ein Angebot der Filiale. Die PLZ sagt
--     seitdem nichts mehr darüber, wo es gilt — nur noch, über welche Suche
--     der Scraper die Filiale gefunden hat.
--
-- ------------------------------------------------------------
-- Warum die Spalte nicht ersatzlos fällt
-- ------------------------------------------------------------
-- Seit Phase 12 Schritt 5 heißt `region IS NULL` in Wahrheit **bundesweit
-- gültig**: ALDI Nord und ALDI SÜD werden genau einmal unter ALDI_NORD_DE /
-- ALDI_SUED_DE gespeichert, ohne Filiale und ohne PLZ. Genau daran erkennt
-- die App sie — eine Zeile ohne Region gehört jeder Filiale ihrer Kette.
--
-- Fiele `region` ersatzlos weg, wären beide ALDI-Kataloge unsichtbar. Die
-- Spalte wird deshalb nicht gelöscht, sondern umbenannt in das, was sie
-- längst beantwortet: `nationwide boolean`.
--
-- Begründung und die beiden verworfenen Alternativen stehen in
-- `04-projekte/lechariot/Le Chariot Entscheidungen.md`, Abschnitt
-- „`region` wird zu `nationwide`".
--
-- ------------------------------------------------------------
-- Was dabei verschmilzt
-- ------------------------------------------------------------
-- Der Unique-Key schrumpft von (market_id, product, valid_from, region) auf
-- (market_id, product, valid_from). Die mehrfach geführten Filialen fallen
-- damit auf je eine Zeile zusammen — das ist der Zweck, nicht ein
-- Nebeneffekt. Teil 1 räumt die Duplikate deshalb VOR dem Index-Wechsel weg,
-- sonst scheitert das Anlegen des Index.
--
-- ACHTUNG, REIHENFOLGE: Diese Migration und der neue Binary gehören
-- zusammen. Der alte Binary upsertet mit `on_conflict=…,region` — dieser
-- Index existiert danach nicht mehr, PostgREST antwortet mit 42P10. Also:
-- PR mergen und die Migration im selben Zeitfenster ausführen, am besten
-- nicht kurz vor 06:30 (Nightly).
--
-- Idempotent – kann gefahrlos erneut laufen.
-- ============================================================

-- ------------------------------------------------------------
-- Teil 1: Duplikate zusammenführen, bevor der Index enger wird
-- ------------------------------------------------------------
-- Von jeder Gruppe (market_id, product, valid_from) bleibt die zuletzt
-- geschriebene Zeile stehen. `id` als Tiebreaker, damit die Auswahl auch bei
-- gleichem created_at eindeutig ist.
--
-- ACHTUNG, Spaltennamen: `offers` heißt die Spalte `created_at`,
-- `price_history` dagegen `recorded_at` — die beiden Tabellen sind hier
-- nicht symmetrisch (nachgesehen am 2026-07-26, nachdem ein erster Anlauf
-- mit `recorded_at` an genau dieser Stelle mit 42703 abbrach).
delete from public.offers a
using public.offers b
where a.market_id = b.market_id
  and a.product = b.product
  and a.valid_from is not distinct from b.valid_from
  and (
        coalesce(a.created_at, '-infinity'::timestamptz)
          < coalesce(b.created_at, '-infinity'::timestamptz)
     or (coalesce(a.created_at, '-infinity'::timestamptz)
          = coalesce(b.created_at, '-infinity'::timestamptz)
         and a.id < b.id)
      );

-- ------------------------------------------------------------
-- Teil 2: die Spalte umdeuten
-- ------------------------------------------------------------
alter table public.offers add column if not exists nationwide boolean;

-- Bestandszeilen: ohne PLZ = bundesweit, mit PLZ = an einer Filiale.
update public.offers
   set nationwide = (region is null)
 where nationwide is null;

alter table public.offers alter column nationwide set default false;
alter table public.offers alter column nationwide set not null;

-- Dasselbe für die Historie, damit der Preisverlauf einer bundesweiten
-- Kette weiter auffindbar bleibt.
alter table public.price_history add column if not exists nationwide boolean;
update public.price_history
   set nationwide = (region is null)
 where nationwide is null;
alter table public.price_history alter column nationwide set default false;
alter table public.price_history alter column nationwide set not null;

-- ------------------------------------------------------------
-- Teil 3: der Schlüssel schrumpft
-- ------------------------------------------------------------
create unique index if not exists offers_branch_product_valid_key2
    on public.offers (market_id, product, valid_from);

drop index if exists offers_branch_product_valid_key;

-- Dasselbe für die Historie. Beim ersten Anlauf am 2026-07-26 vergessen —
-- der Angebots-Push lief danach durch, die Historie brach mit 42P10 ab
-- („no unique or exclusion constraint matching the ON CONFLICT
-- specification"), weil ihr Schlüssel `region` noch enthielt.
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

-- ------------------------------------------------------------
-- Teil 4: `region` fällt weg — zuletzt und als eigener Block
-- ------------------------------------------------------------
-- Bis hierher ist alles umkehrbar. Wer in zwei Etappen fahren will, lässt
-- diesen Teil beim ersten Mal weg und führt ihn aus, sobald die App
-- nachweislich auf `nationwide` läuft.
alter table public.offers drop column if exists region;
alter table public.price_history drop column if exists region;

-- ------------------------------------------------------------
-- Teil 5: die Regions-Verwaltung
-- ------------------------------------------------------------
-- `regions` war die Warteschlange des PLZ-Wegs: Die App trug eine PLZ ein,
-- ein Trigger stieß den Sync an. Seit Phase 12 fordert sie stattdessen
-- Filialen über `branch_requests` an, und die Nightly frischt die selbst
-- gewählten Filialen auf (Schritt 7a). Damit ist hier nichts mehr zu tun.
drop trigger if exists on_region_insert on public.regions;
drop function if exists public.trigger_region_scrape() cascade;
drop table if exists public.region_dispatch_log;
drop table if exists public.regions;
