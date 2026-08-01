//! Die Vorschau auf die Folgewoche — offline gegen eingefrorene Fixtures.
//!
//! Alle drei Fixtures sind wörtliche Auszüge vom 2026-08-01. Was gemessen
//! wurde und was nicht, steht in `src/preview.rs::sources` und ausführlich im
//! [[Le Chariot Backlog]].

use chrono::NaiveDate;
use lechariot::preview;
use lechariot::scrapers;

// ============================ Kaufland ============================

/// **Der Befund:** Die Folgewoche liegt in derselben Antwort, aber nicht im
/// gerenderten HTML — sie steht im SSR-Zustand. Der bestehende Kachel-Parser
/// kann sie deshalb strukturell nicht sehen.
#[test]
fn kaufland_next_week_rides_in_the_same_response() {
    let html = include_str!("fixtures/kaufland/uebersicht_vorschau.html");

    let next = scrapers::kaufland::parse_next_week_offers(html, "DE7380", "2026-08-01");

    assert_eq!(next.len(), 35, "Am 01.08. standen 35 künftige Angebote drin");
    assert!(
        next.iter().all(|o| o.valid_from.as_deref().unwrap() > "2026-08-01"),
        "Eine Zeile ohne Zukunftsdatum in der Vorschau"
    );
    let lachsschinken = next
        .iter()
        .find(|o| o.title == "ORIGINAL RADEBERGER")
        .expect("Der Lachsschinken aus der Messung fehlt");
    assert_eq!(lachsschinken.price, Some(2.79));
    assert_eq!(lachsschinken.valid_from.as_deref(), Some("2026-08-06"));
    assert_eq!(lachsschinken.valid_until.as_deref(), Some("2026-08-12"));
}

/// **Die Gegenprobe, und sie ist der eigentliche Test:** Der alte Weg über die
/// gerenderten Kacheln findet in derselben Datei **nichts**. Fiele dieser Test,
/// hieße das, der neue Parser hätte gar nichts Neues erschlossen.
#[test]
fn the_old_tile_parser_sees_none_of_them() {
    let html = include_str!("fixtures/kaufland/uebersicht_vorschau.html");

    let tiles = scrapers::kaufland::parse_offers(html, "DE7380").unwrap();

    assert!(tiles.is_empty(), "Der Kachel-Weg sollte hier nichts finden");
}

/// Ein Fenster, das heute schon läuft, ist keine Vorschau. Ohne diese Grenze
/// stünde die laufende Woche doppelt in der Datenbank.
#[test]
fn kaufland_preview_never_returns_the_running_week() {
    let html = include_str!("fixtures/kaufland/uebersicht_vorschau.html");

    // Stichtag hinter allen Fenstern: Es darf nichts mehr übrig bleiben.
    let none = scrapers::kaufland::parse_next_week_offers(html, "DE7380", "2026-12-31");
    assert!(none.is_empty());

    // Und einer davor: Dann sind es dieselben 35, nicht mehr.
    let all = scrapers::kaufland::parse_next_week_offers(html, "DE7380", "2026-07-01");
    assert_eq!(all.len(), 37, "Vor allen Fenstern zählen auch die zwei laufenden mit");
}

/// Die IDs tragen `valid_from` — sonst kollidiert die Zeile der Folgewoche mit
/// der laufenden desselben Produkts und eine der beiden verschwindet still.
#[test]
fn kaufland_preview_ids_carry_the_window() {
    let html = include_str!("fixtures/kaufland/uebersicht_vorschau.html");

    let next = scrapers::kaufland::parse_next_week_offers(html, "DE7380", "2026-08-01");

    assert!(next.iter().all(|o| o.id.contains("2026-08-06")), "ID ohne Startdatum");
}

// ============================ ALDI Nord ============================

/// Die Vorschauseite trägt denselben `__NEXT_DATA__`-Aufbau wie die laufende —
/// deshalb liest sie derselbe Parser. Der Test ist der Beleg dafür.
#[test]
fn aldi_nord_preview_page_parses_with_the_normal_parser() {
    let html = include_str!("fixtures/aldi_nord/angebote_vorschau.html");

    let offers = scrapers::aldi_nord::parse_offers(html, "ALDI_NORD_DE").unwrap();

    assert!(!offers.is_empty(), "Die Vorschauseite liefert keine Angebote");
    assert!(
        offers.iter().any(|o| o.valid_from.as_deref() == Some("2026-08-03")),
        "Keine Zeile mit dem gemessenen Aktionstag Mo 3.8."
    );
}

// ============================ Lidl ============================

fn slugs() -> Vec<String> {
    // Die drei Prospekte, die die Übersicht am 2026-08-01 führte.
    ["aktionsprospekt-27-07-2026-01-08-2026-aaa111",
     "aktionsprospekt-27-07-2026-01-08-2026-bbb222",
     "aktionsprospekt-03-08-2026-08-08-2026-ccc333",
     "aktionsprospekt-10-08-2026-15-08-2026-7a1f3e"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// **Die Regel aus #32, eine Ebene höher:** Zwei disjunkte Zukunftswochen sind
/// zwei Prospekte. Kämen sie als eine Gruppe zurück, verlöre die Vorschau eine
/// ganze Woche — dieselbe Klasse Fehler wie ein Dedupe, der getrennte Fenster
/// verschmilzt.
#[test]
fn lidl_disjoint_future_weeks_stay_separate_groups() {
    let groups = scrapers::lidl_prospekt::next_week_slugs(&slugs(), day("2026-08-01"));

    assert_eq!(groups.len(), 2, "Zwei künftige Prospekte, zwei Gruppen");
    assert!(groups[0][0].starts_with("aktionsprospekt-03-08-2026"), "nach Startdatum geordnet");
    assert!(groups[1][0].starts_with("aktionsprospekt-10-08-2026"));
}

/// Der laufende Prospekt gehört nicht in die Vorschau — er steht schon in
/// `week_slugs`. Und mehrere Varianten derselben Woche sind **eine** Gruppe:
/// aus ihnen wählt die Absatzregion, sie sind keine zwei Wochen.
#[test]
fn lidl_preview_excludes_the_running_week_and_groups_variants() {
    let with_variants = {
        let mut s = slugs();
        s.push("aktionsprospekt-03-08-2026-08-08-2026-ddd444".to_string());
        s
    };

    let groups = scrapers::lidl_prospekt::next_week_slugs(&with_variants, day("2026-08-01"));

    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].len(), 2, "Zwei Varianten derselben Woche sind eine Gruppe");
    assert!(
        groups.iter().flatten().all(|s| !s.starts_with("aktionsprospekt-27-07")),
        "Der laufende Prospekt steht in der Vorschau"
    );
}

/// Am letzten Tag der laufenden Woche zählt der Prospekt, der heute endet,
/// weiter zur laufenden — nicht zur Vorschau.
#[test]
fn lidl_the_last_day_of_the_running_week_is_not_a_preview() {
    let groups = scrapers::lidl_prospekt::next_week_slugs(&slugs(), day("2026-08-08"));

    assert_eq!(groups.len(), 1, "Nur noch der 10.08.-Prospekt ist Zukunft");
}

// ============================ Der Schalter ============================

/// **Voreinstellung AUS.** Ohne diese Zusage darf der Zweig nicht gemergt
/// werden: Er würde die Nightly beim nächsten Lauf verändern.
///
/// Ein Test, kein Kommentar — die Umgebungsvariable wird hier gesetzt und
/// wieder entfernt, deshalb steht alles in einem Testfall (Rust fährt Tests
/// parallel, und `set_var` gilt für den ganzen Prozess).
#[test]
fn the_preview_flag_is_off_unless_someone_switches_it_on() {
    // SAFETY: einziger Test, der die Variable anfasst; parallel läuft nichts,
    // das sie liest.
    unsafe {
        std::env::remove_var("LECHARIOT_PREVIEW");
        assert!(!preview::enabled(), "unbesetzt muss AUS sein");

        for off in ["", "0", "false", "nein", "off", " 0 "] {
            std::env::set_var("LECHARIOT_PREVIEW", off);
            assert!(!preview::enabled(), "{off:?} sollte AUS sein");
        }
        for on in ["1", "true", "ja", "yes", "vielleicht"] {
            std::env::set_var("LECHARIOT_PREVIEW", on);
            assert!(preview::enabled(), "{on:?} sollte AN sein");
        }
        std::env::remove_var("LECHARIOT_PREVIEW");
    }
}

// ============================== REWE ==============================

const REWE_RAW: &str = include_str!("fixtures/rewe/discounts_raw_vorschau.json");

fn rewe_raw() -> serde_json::Value {
    serde_json::from_str(REWE_RAW).unwrap()
}

/// **Der Befund, und er ist derselbe wie bei Kaufland:** Die Folgewoche liegt
/// in derselben Antwort. `stationary-offers/<markt>` liefert `current` **und**
/// `next`; `rewerse discounts` gibt nur eine der beiden heraus, je nachdem, was
/// `defaultWeek` sagt. Kein zweiter Endpunkt, kein zweites Zertifikat.
///
/// Am 2026-08-01 auf dem Runner gemessen (Markt 565005): current 18 Kategorien
/// / 352 Angebote, **next 19 Kategorien / 333 Angebote** (03.08.–08.08.).
/// Der Fixture ist der gekürzte Auszug davon.
#[test]
fn rewe_next_week_rides_in_the_same_response() {
    let next = scrapers::rewe::parse_next_week_offers(&rewe_raw(), "565005", "2026-08-01");

    assert_eq!(next.len(), 12, "12 DEFAULT-Kacheln in den drei Kategorien");
    assert!(
        next.iter().all(|o| o.valid_from.as_deref() == Some("2026-08-03")),
        "Eine Zeile ohne das gemessene Startdatum"
    );
    assert!(next.iter().all(|o| o.valid_until.as_deref() == Some("2026-08-08")));

    let butter = next
        .iter()
        .find(|o| o.title == "Weihenstephan Butter oder Die Streichzarte")
        .expect("Die Butter aus der Messung fehlt");
    assert_eq!(butter.price, Some(1.29));
    assert_eq!(butter.category.as_deref(), Some("Top-Angebote in deinem Markt"));
    // Die Marke steht im Overline und muss mitkommen — sonst fehlt sie dem
    // Matcher, genau der Fehler aus [EXTRAKT-1].
    assert_eq!(butter.overline.as_deref(), Some("Mo. 3.8. bis Sa. 8.8."));
    // `regularPrice` trägt hier „Knaller", kein Preis. Wer das als Zahl liest,
    // erfindet einen Streichpreis.
    assert_eq!(butter.regular_price, None);
}

/// **Die Gegenprobe:** Der bestehende Weg (`parse_offers` auf der gekürzten
/// `discounts -json`-Form) findet in derselben Datei **nichts** — er kennt
/// weder `data.offers` noch `priceData`. Fiele dieser Test, hätte der neue
/// Parser nichts Neues erschlossen.
#[test]
fn the_old_rewe_parser_sees_none_of_them() {
    let old = scrapers::rewe::parse_offers(rewe_raw(), "565005").unwrap();

    assert!(old.is_empty(), "Der alte Weg sollte in der Rohform nichts finden");
}

/// Nicht jede Kachel ist ein Angebot: `HERO` und `MOOD` sind Bühnen und Banner.
/// Der Fixture trägt je eine davon — ohne den `cellType`-Filter stünden sie als
/// Produkte in der Datenbank.
#[test]
fn rewe_preview_skips_stage_and_mood_tiles() {
    let raw = rewe_raw();
    let alle: usize = raw
        .pointer("/data/offers/next/categories")
        .and_then(|v| v.as_array())
        .map(|cats| {
            cats.iter()
                .filter_map(|c| c.get("offers")?.as_array())
                .map(|o| o.len())
                .sum()
        })
        .unwrap();

    let next = scrapers::rewe::parse_next_week_offers(&raw, "565005", "2026-08-01");

    assert_eq!(alle, 14, "Im Fixture stehen 14 Kacheln");
    assert_eq!(next.len(), 12, "Zwei davon sind HERO/MOOD und dürfen nicht mit");
}

/// **Die Grenze, und sie ist bei REWE schärfer als anderswo.**
/// `nextWeekAvailableFrom` steht auf `saturday` — am Samstag zeigt `next` also
/// die Woche, die am Montag *anfängt*, und `defaultWeek` kann auf `next`
/// springen. Ohne die Prüfung `fromDate > heute` stünde ein Preis der
/// laufenden Woche in der Vorschau.
#[test]
fn rewe_preview_never_returns_the_running_week() {
    let raw = rewe_raw();

    // Stichtag am Starttag selbst und danach: nichts mehr.
    for today in ["2026-08-03", "2026-08-05", "2026-12-31"] {
        let none = scrapers::rewe::parse_next_week_offers(&raw, "565005", today);
        assert!(none.is_empty(), "{today}: die Vorschau lieferte die laufende Woche");
    }
}

/// `available: false` heißt „REWE hat die Folgewoche noch nicht freigegeben" —
/// dann ist die Vorschau leer, nicht kaputt.
#[test]
fn rewe_preview_is_empty_while_next_week_is_not_released() {
    let mut raw = rewe_raw();
    raw.pointer_mut("/data/offers/next/available")
        .map(|v| *v = serde_json::Value::Bool(false));

    let next = scrapers::rewe::parse_next_week_offers(&raw, "565005", "2026-08-01");

    assert!(next.is_empty());
}

/// Die Quellen-Tabelle beschreibt neun Ketten — keine doppelt, keine
/// vergessen. Sie ist Dokumentation, und Dokumentation altert; dieser Test
/// ist der Wecker.
#[test]
fn preview_sources_cover_every_chain_exactly_once() {
    let mut named: Vec<&str> = preview::sources::ALREADY_LIVE
        .iter()
        .chain(preview::sources::BUILT)
        .chain(preview::sources::NO_PREVIEW)
        .copied()
        .chain(preview::sources::MEASURED_NOT_BUILT.iter().map(|(c, _)| *c))
        .collect();
    named.sort_unstable();
    let count = named.len();
    named.dedup();

    assert_eq!(count, named.len(), "Eine Kette steht doppelt in der Tabelle");
    let mut chains: Vec<&str> = lechariot::stores::Store::ALL.iter().map(|s| s.chain()).collect();
    chains.sort_unstable();
    assert_eq!(named, chains, "Die Tabelle deckt nicht genau die neun Ketten ab");
}
