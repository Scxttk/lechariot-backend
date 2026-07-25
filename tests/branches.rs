// Offline-Parser-Tests für das Filialverzeichnis (`public.branches`).
//
// Die Fixtures sind auf zwei bis drei Einträge gekürzte Live-Antworten der
// Store-Finder vom 2026-07-25. Der gemeinsame Nachweis dieser Datei: Aus
// **jeder** der acht Ketten kommen Adresse und Koordinaten an — vorher trugen
// REWE, Netto und EDEKA in `markets` durchweg NULL, weil die Felder in der
// Antwort standen und nicht gelesen wurden.

use smartshop::models::Branch;
use smartshop::scrapers;

fn json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap()
}

fn find<'a>(branches: &'a [Branch], market_id: &str) -> &'a Branch {
    branches
        .iter()
        .find(|b| b.market_id == market_id)
        .unwrap_or_else(|| panic!("Filiale {market_id} fehlt in {branches:#?}"))
}

/// Jede Verzeichniszeile braucht Kette, Name und eine Quelle — ohne die ist
/// sie in der App nicht darstellbar bzw. nicht nachvollziehbar.
fn assert_well_formed(branches: &[Branch]) {
    for branch in branches {
        assert!(!branch.market_id.is_empty(), "leere market_id: {branch:?}");
        assert!(!branch.chain.is_empty(), "leere chain: {branch:?}");
        assert!(!branch.name.is_empty(), "leerer name: {branch:?}");
        assert!(!branch.source.is_empty(), "leere source: {branch:?}");
    }
}

// ---------------------------------------------------------------- REWE

/// Der Fall, der das ganze Vorhaben ausgelöst hat: Die Suche nach 01219
/// liefert Filialen in 01257 — mit Adresse und Koordinaten, die bisher
/// weggeworfen wurden.
#[test]
fn rewe_search_yields_addresses_and_coordinates() {
    let raw = json(include_str!("fixtures/rewe/markets_search.json"));
    let branches = scrapers::rewe::parse_branches(&raw).unwrap();

    assert_well_formed(&branches);
    assert_eq!(branches.len(), 2);

    let first = &branches[0];
    assert_eq!(first.market_id, "565005");
    assert_eq!(first.chain, "REWE");
    assert_eq!(first.name, "REWE Supermarkt");
    assert_eq!(first.street.as_deref(), Some("Pirnaer Landstr. 145"));
    // Die Pointe: gesucht wurde 01219, geliefert wird 01257.
    assert_eq!(first.plz.as_deref(), Some("01257"));
    assert_eq!(first.city.as_deref(), Some("Dresden / Leuben"));
    assert!((first.lat.unwrap() - 51.01137).abs() < 1e-9);
    assert!((first.lon.unwrap() - 13.82882).abs() < 1e-9);
}

/// `find_market` nimmt weiter den ersten Treffer — nur eben mit Koordinaten.
/// Ohne diesen Test könnte die Umstellung unbemerkt eine andere Filiale
/// wählen, und damit stillschweigend andere Angebote scrapen.
#[test]
fn rewe_first_branch_is_still_the_scraped_market() {
    let raw = json(include_str!("fixtures/rewe/markets_search.json"));
    let market = scrapers::rewe::parse_branches(&raw).unwrap()[0].as_market();

    assert_eq!(market.id, "565005");
    assert_eq!(market.name, "REWE Supermarkt");
    assert!(market.lat.is_some(), "die Koordinaten waren der Grund für den Umbau");
}

#[test]
fn rewe_rejects_a_response_without_markets() {
    assert!(scrapers::rewe::parse_branches(&json("{}")).is_err());
}

// ---------------------------------------------------------------- Netto

/// Für 01219 kennt der Finder vier Filialen, darunter die in der
/// Johannes-Paul-Thilman-Straße. Die App zeigte davon genau eine, weil der
/// Rest der Antwort verworfen wurde.
#[test]
fn netto_storefinder_yields_every_open_branch() {
    let raw = json(include_str!("fixtures/netto/storefinder.json"));
    let branches = scrapers::netto::parse_branches(&raw).unwrap();

    assert_well_formed(&branches);
    // Drei im Fixture, eine davon geschlossen.
    assert_eq!(branches.len(), 2, "geschlossene Filialen gehören nicht ins Verzeichnis");
    assert!(branches.iter().all(|b| b.market_id != "9999"));

    let jpt = find(&branches, "4695");
    assert_eq!(jpt.street.as_deref(), Some("Johannes-Paul-Thilman-Str. 3"));
    assert_eq!(jpt.plz.as_deref(), Some("01219"));
    assert_eq!(jpt.city.as_deref(), Some("Dresden-Leubnitz-Neuostra"));
    // Koordinaten kommen als Strings und müssen geparst werden.
    assert!((jpt.lat.unwrap() - 51.0127648).abs() < 1e-9);
    assert!((jpt.lon.unwrap() - 13.7764638).abs() < 1e-9);
}

/// `store_name` ist bei jeder Netto-Filiale "Netto Marken-Discount" — ohne
/// den Ort wären alle vier Zeilen in der App nicht unterscheidbar.
#[test]
fn netto_names_carry_the_city() {
    let raw = json(include_str!("fixtures/netto/storefinder.json"));
    let branches = scrapers::netto::parse_branches(&raw).unwrap();

    assert_eq!(find(&branches, "4816").name, "Netto Marken-Discount Dresden-Strehlen");
    assert_eq!(find(&branches, "4695").name, "Netto Marken-Discount Dresden-Leubnitz-Neuostra");
}

#[test]
fn netto_rejects_a_response_that_is_not_a_list() {
    assert!(scrapers::netto::parse_branches(&json("{}")).is_err());
}

// ---------------------------------------------------------------- EDEKA

/// EDEKA nennt in der Marktsuche eine andere ID als der Angebots-Pfad. Der
/// Parser liest deshalb nur die Adressdaten; die Scrape-ID kommt aus der URL
/// bzw. deren Redirect.
#[test]
fn edeka_marketsearch_yields_address_drafts() {
    let raw = json(include_str!("fixtures/edeka/marketsearch.json"));
    let drafts = scrapers::edeka::parse_branch_drafts(&raw);

    assert_eq!(drafts.len(), 2);
    let first = &drafts[0];
    assert_eq!(first.name, "E center Peltzer");
    assert_eq!(first.street.as_deref(), Some("Dohnaer Straße 168"));
    assert_eq!(first.plz.as_deref(), Some("01239"));
    assert_eq!(first.city.as_deref(), Some("Dresden"));
    assert!((first.lat.unwrap() - 51.00879).abs() < 1e-9);
    assert!((first.lon.unwrap() - 13.78459).abs() < 1e-9);
}

/// Neue URLs tragen die Scrape-ID schon — dieser Weg kommt ohne Netz aus und
/// ist der einzige, der offline prüfbar ist.
#[test]
fn edeka_resolves_ids_that_are_already_in_the_url() {
    let raw = json(include_str!("fixtures/edeka/marketsearch.json"));
    let drafts = scrapers::edeka::parse_branch_drafts(&raw);
    let prohlis = drafts.into_iter().find(|d| d.name == "EDEKA Dresden-Prohlis").unwrap();

    let branch = scrapers::edeka::resolve(prohlis).unwrap();

    assert_eq!(branch.market_id, "022745");
    assert_eq!(branch.chain, "EDEKA");
    assert_eq!(branch.street.as_deref(), Some("Jacob-Winter-Platz 13"));
    assert!(branch.lat.is_some());
}

#[test]
fn edeka_returns_nothing_for_a_response_without_markets() {
    assert!(scrapers::edeka::parse_branch_drafts(&json("{}")).is_empty());
}

// ---------------------------------------------------------------- Kaufland

#[test]
fn kaufland_storefinder_yields_full_addresses() {
    let stores: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/kaufland/storefinder.json")).unwrap();
    let branches = scrapers::kaufland::parse_branches(&stores);

    assert_well_formed(&branches);
    let strehlen = find(&branches, "DE7380");
    assert_eq!(strehlen.chain, "Kaufland");
    assert_eq!(strehlen.name, "Kaufland Dresden-Strehlen, O.D.C.");
    assert_eq!(strehlen.plz.as_deref(), Some("01219"));
    assert!(strehlen.street.is_some());
    // Die Koordinaten stehen als Strings ("51.0194994") in der Datei.
    assert!(strehlen.lat.is_some() && strehlen.lon.is_some());
}

// ---------------------------------------------------------------- Penny

#[test]
fn penny_market_list_yields_full_addresses() {
    let markets: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/penny/markets.json")).unwrap();
    let branches = scrapers::penny::parse_branches(&markets);

    assert_well_formed(&branches);
    assert_eq!(branches.len(), 2);
    let first = &branches[0];
    assert_eq!(first.chain, "Penny");
    assert!(first.street.is_some(), "streetWithHouseNumber muss ankommen");
    assert!(first.plz.is_some());
    assert!(first.lat.is_some() && first.lon.is_some());
}

// ---------------------------------------------------------------- Lidl

#[test]
fn lidl_spatial_search_yields_every_branch() {
    let raw = json(include_str!("fixtures/store_finder/virtualearth_branches.json"));
    let branches = scrapers::store_finder::parse_virtualearth_branches(&raw).unwrap();

    assert_well_formed(&branches);
    // Drei Einträge, einer ohne EntityID — der fällt raus.
    assert_eq!(branches.len(), 2);

    let strehlen = find(&branches, "LIDL_1988");
    assert_eq!(strehlen.chain, "Lidl");
    assert_eq!(strehlen.name, "Lidl Strehlen");
    assert_eq!(strehlen.street.as_deref(), Some("Strehlener Platz 1"));
    assert_eq!(strehlen.plz.as_deref(), Some("01219"));
    assert!((strehlen.lat.unwrap() - 51.0338).abs() < 1e-9);
}

/// `ShownStoreName` ist oft leer. Ohne Rückfall stünde "Lidl " im
/// Verzeichnis — dieselbe Regel, die `parse_virtualearth` schon kennt.
#[test]
fn lidl_falls_back_to_the_district_when_the_name_is_empty() {
    let raw = json(include_str!("fixtures/store_finder/virtualearth_branches.json"));
    let branches = scrapers::store_finder::parse_virtualearth_branches(&raw).unwrap();

    assert_eq!(find(&branches, "LIDL_5798").name, "Lidl Seevorstadt");
}

// ---------------------------------------------------------------- ALDI

#[test]
fn aldi_uberall_yields_branches_within_the_radius() {
    let raw = json(include_str!("fixtures/store_finder/uberall_branches.json"));
    let branches =
        scrapers::store_finder::parse_uberall_branches(&raw, CENTER, "ALDI Nord", "ALDI_NORD", 25.0)
            .unwrap();

    assert_well_formed(&branches);
    // Chemnitz liegt 48 km entfernt und gehört nicht in eine Dresden-Suche.
    assert_eq!(branches.len(), 2);
    assert!(branches.iter().all(|b| b.city.as_deref() != Some("Chemnitz")));

    let branch = find(&branches, "ALDI_NORD_DE036002");
    assert_eq!(branch.chain, "ALDI Nord");
    assert_eq!(branch.name, "ALDI Nord Dresden");
    assert_eq!(branch.street.as_deref(), Some("Uhlandstraße 5"));
    assert_eq!(branch.plz.as_deref(), Some("01069"));
    assert!(branch.lat.is_some());
}

/// Ein größerer Radius holt die weiter entfernten Filialen mit — der Cutoff
/// wirkt wirklich über die Entfernung und nicht über die Trefferzahl.
#[test]
fn aldi_radius_decides_what_comes_along() {
    let raw = json(include_str!("fixtures/store_finder/uberall_branches.json"));
    let wide = scrapers::store_finder::parse_uberall_branches(&raw, CENTER, "ALDI Nord", "ALDI_NORD", 50.0)
        .unwrap();

    assert_eq!(wide.len(), 4);
}

/// Uberall liefert `distance` nicht immer, und die Abfrage filtert
/// serverseitig nicht nach Radius. Ohne das Feld muss die Entfernung aus den
/// Koordinaten kommen — sonst rutscht eine Chemnitzer Filiale in eine
/// Dresdner Suche. Dieselbe Rechnung wie in `parse_uberall`, wo derselbe Fall
/// ALDI Nord in Köln vertreten gemacht hat.
#[test]
fn aldi_computes_the_distance_when_uberall_omits_it() {
    let raw = json(include_str!("fixtures/store_finder/uberall_branches.json"));
    let branches =
        scrapers::store_finder::parse_uberall_branches(&raw, CENTER, "ALDI Nord", "ALDI_NORD", 25.0)
            .unwrap();

    assert!(
        branches.iter().all(|b| b.market_id != "ALDI_NORD_DE099998"),
        "eine Filiale ohne distance darf nicht ungeprüft durchrutschen"
    );
}

// ---------------------------------------------------------------- Gebiet

/// Dresden-Strehlen als Mittelpunkt (Nominatim für 01219).
const CENTER: (f64, f64) = (51.0231864, 13.7659125);

#[test]
fn the_radius_keeps_the_neighbourhood_and_drops_the_next_city() {
    let near = Branch::new("A", "Lidl", "Lidl Strehlen", "test").with_geo(Some(51.0338), Some(13.7498));
    let far = Branch::new("B", "Lidl", "Lidl Chemnitz", "test").with_geo(Some(50.83), Some(12.92));

    assert!(smartshop::branches::within(&near, CENTER, 25.0));
    assert!(!smartshop::branches::within(&far, CENTER, 25.0));
}

/// Eine Filiale ohne Koordinaten wegen einer Datenlücke des Finders
/// verschwinden zu lassen wäre schlimmer als sie zu behalten — die Suche galt
/// ja diesem Gebiet.
#[test]
fn a_branch_without_coordinates_survives_the_radius() {
    let unknown = Branch::new("C", "EDEKA", "EDEKA ohne Geo", "test");

    assert!(smartshop::branches::within(&unknown, CENTER, 1.0));
}

/// Benachbarte Gebiete überlappen sich im Umkreis; dieselbe Filiale kommt
/// dann zweimal. Der erste Treffer gewinnt.
#[test]
fn the_same_branch_from_two_areas_is_written_once() {
    let first = Branch::new("565005", "REWE", "Aus 01219", "test");
    let second = Branch::new("565005", "REWE", "Aus 01257", "test");
    let other = Branch::new("565264", "REWE", "Andere Filiale", "test");

    let rows = smartshop::branches::deduplicated(vec![first, second, other]);

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "Aus 01219");
}

/// Ohne die Stadt fällt die REWE-Suche auf die PLZ zurück — und die liefert
/// nachweislich die falsche Nachbarschaft.
#[test]
fn nominatim_yields_the_city_for_the_rewe_search() {
    let raw = json(include_str!("fixtures/store_finder/nominatim_plz.json"));
    // Das alte Fixture hat keinen addressdetails-Block.
    assert_eq!(scrapers::store_finder::parse_nominatim_city(&raw), None);

    let with_details = json(
        r#"[{"lat":"51.02","lon":"13.76","address":{"suburb":"Strehlen","city":"Dresden"}}]"#,
    );
    assert_eq!(
        scrapers::store_finder::parse_nominatim_city(&with_details),
        Some("Dresden".to_string())
    );

    // Auf dem Land heißt das Feld town oder village.
    let village = json(r#"[{"lat":"51.0","lon":"13.0","address":{"village":"Bannewitz"}}]"#);
    assert_eq!(
        scrapers::store_finder::parse_nominatim_city(&village),
        Some("Bannewitz".to_string())
    );
}

#[test]
fn aldi_rejects_a_failed_uberall_response() {
    let raw = json(include_str!("fixtures/store_finder/uberall_error.json"));
    assert!(
        scrapers::store_finder::parse_uberall_branches(&raw, CENTER, "ALDI Nord", "ALDI_NORD", 25.0)
            .is_err()
    );
}
