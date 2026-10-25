-- Le Chariot — Migration v26
-- Kontingent für die Edge Function `gerichte`. Idempotent.
--
-- NOCH NICHT GEGEN DIE PRODUKTION AUSGEFÜHRT. Von Hand im SQL-Editor laufen
-- lassen, VOR dem ersten Deploy der Funktion: Ohne die Funktion unten
-- antwortet `gerichte` mit 503 („kontingent_unbekannt") und fragt das Modell
-- gar nicht erst — sie läuft nie ungezählt.
--
-- ============================================================
-- Wozu
-- ============================================================
--
-- `gerichte` ruft für die App Claude Haiku auf. Jeder Aufruf kostet Geld auf
-- einem Konto mit Monatslimit, und die Funktion ist ohne Anmeldung
-- erreichbar (die App hat keine Konten). Also wird gezählt:
--
--   - je Installation und Berliner Kalendertag (Vorgabe 20), und
--   - für alle zusammen (Vorgabe 100), weil eine Installations-ID eine
--     Zufalls-UUID ist, die sich jeder neu ausdenken kann.
--
-- Die Grenzen stehen in der Funktion (Secrets `GERICHTE_GRENZE`,
-- `GERICHTE_GRENZE_GESAMT`), nicht hier — eine geänderte Grenze soll kein
-- SQL brauchen.
--
-- ============================================================
-- Was gespeichert wird
-- ============================================================
--
-- `schluessel` ist SHA-256 über `gerichte:<install_id>`, gerechnet in der
-- Funktion, nie die Installations-ID selbst. Die Zeile sagt „irgendwer hat
-- heute n-mal gefragt", nicht wer und nicht was — der Text der Anfrage wird
-- nirgends gespeichert. Zeilen älter als zwei Tage räumt die Zählfunktion
-- selbst weg. Deshalb taucht die Tabelle in `delete_installation` /
-- `export_installation` (v25) nicht auf: Es gibt darin nichts, das einer
-- Installation zuzuordnen wäre.
--
-- Kein Zugriff für anon oder authenticated: RLS an, keine Policy, und die
-- Funktion ist nur für `service_role` ausführbar.

create table if not exists public.gerichte_kontingent (
    schluessel text    not null,
    tag        date    not null,
    anzahl     integer not null default 0,
    primary key (schluessel, tag)
);

alter table public.gerichte_kontingent enable row level security;

-- Zählt eine Anfrage und sagt, ob sie laufen darf.
--
-- Erst die eigene Zeile, dann die Gesamtzeile: Wer sein Tageslimit schon
-- erreicht hat, soll den gemeinsamen Topf nicht weiter leeren. Gezählt wird
-- auch, was abgelehnt wird — ein Zähler, der bei der Grenze stehen bleibt,
-- wäre dieselbe Auskunft mit mehr Code.
create or replace function public.gerichte_anfrage_zaehlen(
    p_schluessel text,
    p_grenze integer,
    p_grenze_gesamt integer
)
returns json
language plpgsql
security definer
set search_path = public
as $$
declare
    v_tag    date := (now() at time zone 'Europe/Berlin')::date;
    v_eigene integer;
    v_gesamt integer;
begin
    if p_schluessel is null or length(p_schluessel) <> 64 then
        raise exception 'schluessel ungültig';
    end if;

    delete from public.gerichte_kontingent where tag < v_tag - 2;

    insert into public.gerichte_kontingent as k (schluessel, tag, anzahl)
    values (p_schluessel, v_tag, 1)
    on conflict (schluessel, tag) do update set anzahl = k.anzahl + 1
    returning anzahl into v_eigene;

    if v_eigene > p_grenze then
        return json_build_object('erlaubt', false, 'grund', 'tag', 'rest', 0);
    end if;

    insert into public.gerichte_kontingent as k (schluessel, tag, anzahl)
    values ('gesamt', v_tag, 1)
    on conflict (schluessel, tag) do update set anzahl = k.anzahl + 1
    returning anzahl into v_gesamt;

    if v_gesamt > p_grenze_gesamt then
        return json_build_object('erlaubt', false, 'grund', 'gesamt', 'rest', p_grenze - v_eigene);
    end if;

    return json_build_object('erlaubt', true, 'rest', p_grenze - v_eigene);
end;
$$;

-- `length(...) <> 64` oben lässt 'gesamt' als Schlüssel einer Installation
-- nicht zu; die Gesamtzeile kann also nur diese Funktion schreiben.

revoke all on function public.gerichte_anfrage_zaehlen(text, integer, integer) from public, anon, authenticated;
grant execute on function public.gerichte_anfrage_zaehlen(text, integer, integer) to service_role;
