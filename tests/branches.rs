// Offline-Parser-Tests für das Filialverzeichnis (`public.branches`).
//
// Die Fixtures sind auf zwei bis drei Einträge gekürzte Live-Antworten der
// Store-Finder vom 2026-07-25. Der gemeinsame Nachweis dieser Datei: Aus
// **jeder** der acht Ketten kommen Adresse und Koordinaten an — vorher trugen
// REWE, Netto und EDEKA in `markets` durchweg NULL, weil die Felder in der
// Antwort standen und nicht gelesen wurden.

use lechariot::models::Branch;
use lechariot::scrapers;

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

/// Der Kölner Fall: In der Marktsuche für 50667 steht auf Platz 1 ein Markt
/// **ohne `url`-Feld** (EDEKA Heyßel, Hohenstaufenring 28 — der Markt in der
/// Innenstadt). Der Parser hat solche Einträge früher per `filter_map`
/// verworfen; damit verschwand ausgerechnet die nächstgelegene Filiale
/// spurlos aus dem Verzeichnis. Für 01219 (Dresden) gibt es keinen solchen
/// Eintrag — deshalb fiel es dort nie auf.
#[test]
fn edeka_keeps_markets_without_a_market_url() {
    let raw = json(include_str!("fixtures/edeka/marketsearch_koeln.json"));
    let drafts = scrapers::edeka::parse_branch_drafts(&raw);

    assert_eq!(drafts.len(), 3, "der Markt ohne url darf nicht verschwinden");

    let heyssel = &drafts[0];
    assert_eq!(heyssel.name, "EDEKA Heyßel");
    assert!(heyssel.url.is_none(), "dieser Markt hat in der Antwort kein url-Feld");
    // Adresse und Koordinaten stehen trotzdem in der Antwort und müssen
    // ankommen — sonst wäre die Meldung über den übersprungenen Markt
    // nicht einmal zuzuordnen.
    assert_eq!(heyssel.street.as_deref(), Some("Hohenstaufenring 28"));
    assert_eq!(heyssel.plz.as_deref(), Some("50674"));
    assert_eq!(heyssel.city.as_deref(), Some("Köln"));
    assert!((heyssel.lat.unwrap() - 50.93119).abs() < 1e-9);
    assert!((heyssel.lon.unwrap() - 6.9412).abs() < 1e-9);
}

/// Ohne Markt-URL gibt es keine Scrape-ID (weder /maerkte/10008482/ noch die
/// zusammengesetzte /eh/-URL existieren, beide 404). `branches.market_id` ist
/// der Primär- und Scrape-Schlüssel — eine geratene ID wäre schlimmer als
/// keine Zeile. `resolve` muss deshalb scheitern, aber mit einer Begründung,
/// die den Markt benennt: `find_branches` macht daraus die Warnung, die den
/// stillen Verlust ersetzt.
#[test]
fn edeka_rejects_a_draft_without_a_market_url() {
    let raw = json(include_str!("fixtures/edeka/marketsearch_koeln.json"));
    let heyssel = scrapers::edeka::parse_branch_drafts(&raw).into_iter().next().unwrap();

    let err = scrapers::edeka::resolve(heyssel).unwrap_err();

    let msg = format!("{err:#}");
    assert!(msg.contains("EDEKA Heyßel"), "Meldung muss den Markt benennen: {msg}");
    assert!(msg.contains("Markt-URL"), "Meldung muss den Grund nennen: {msg}");
}

/// Die Kölner Markt-URLs tragen Umlaute und ß **unkodiert** ("dürener-straße"),
/// die Dresdner teilweise prozentkodiert. Beide Formen müssen unverändert an
/// curl durchgereicht werden; der Parser darf daran nichts drehen.
#[test]
fn edeka_keeps_market_urls_verbatim() {
    let raw = json(include_str!("fixtures/edeka/marketsearch_koeln.json"));
    let drafts = scrapers::edeka::parse_branch_drafts(&raw);

    assert_eq!(
        drafts[1].url.as_deref(),
        Some("https://www.edeka.de/eh/rhein-ruhr/edeka-zickuhr-dürener-straße-199-203/index.jsp")
    );
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

    assert!(lechariot::branches::within(&near, CENTER, 25.0));
    assert!(!lechariot::branches::within(&far, CENTER, 25.0));
}

/// Eine Filiale ohne Koordinaten wegen einer Datenlücke des Finders
/// verschwinden zu lassen wäre schlimmer als sie zu behalten — die Suche galt
/// ja diesem Gebiet.
#[test]
fn a_branch_without_coordinates_survives_the_radius() {
    let unknown = Branch::new("C", "EDEKA", "EDEKA ohne Geo", "test");

    assert!(lechariot::branches::within(&unknown, CENTER, 1.0));
}

/// Benachbarte Gebiete überlappen sich im Umkreis; dieselbe Filiale kommt
/// dann zweimal. Der erste Treffer gewinnt.
#[test]
fn the_same_branch_from_two_areas_is_written_once() {
    let first = Branch::new("565005", "REWE", "Aus 01219", "test");
    let second = Branch::new("565005", "REWE", "Aus 01257", "test");
    let other = Branch::new("565264", "REWE", "Andere Filiale", "test");

    let rows = lechariot::branches::deduplicated(vec![first, second, other]);

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

// ------------------------------------------------- Gebiets-Anforderung (v21)

/// Der Cooldown der Migration v21 sperrt 30 Minuten je **Rasterzelle**. Diese
/// Funktion muss dieselbe Zeichenkette liefern wie das SQL — für Migrationen
/// gibt es keine Testinfrastruktur, das hier ist die einzige Stelle, an der
/// die Entscheidung überhaupt automatisch geprüft wird.
///
/// Der Nachweis ist der gemeldete Fall: Ahlbeck und Ueckermünde dürfen sich
/// nicht dieselbe Zelle teilen, sonst unterdrückt die eine Anforderung die
/// andere und der zweite Ort wird nie geholt.
#[test]
fn the_cooldown_cell_separates_ahlbeck_from_ueckermuende() {
    let ahlbeck = lechariot::branches::area_cell(53.9440, 14.1830);
    let ueckermuende = lechariot::branches::area_cell(53.7383, 14.0511);

    assert_eq!(ahlbeck, "cell:53.9,14.2");
    assert_eq!(ueckermuende, "cell:53.7,14.1");
    assert_ne!(ahlbeck, ueckermuende, "in beiden Achsen verschieden");
}

/// Und die Gegenrichtung: Wer in derselben Zelle liegt, wird zu Recht
/// unterdrückt — der Lauf des Ersten holt 25 km um seinen Mittelpunkt, die
/// Zelle ist höchstens 13,5 km lang. Niemand verliert dadurch eine Filiale.
#[test]
fn two_points_in_one_cell_are_inside_the_same_run() {
    let a = (53.91, 14.16);
    let b = (53.94, 14.24);

    assert_eq!(lechariot::branches::area_cell(a.0, a.1), lechariot::branches::area_cell(b.0, b.1));
    let gap = scrapers::store_finder::distance_km(a, b);
    assert!(gap < lechariot::branches::AREA_RADIUS_KM, "{gap:.1} km müssen in den Lauf passen");
}

/// Die Zahl, auf der das 60-km-Tor der Migration steht. Ein Anker darf
/// konstruktionsbedingt bis zu `maxRadiusKm` (40 km in der App) entfernt
/// liegen; ein 30-km-Tor verwürfe die Koordinaten ausgerechnet im gemeldeten
/// Fall und fiele still auf den alten Fehler zurück.
#[test]
fn the_reported_gap_is_inside_the_sixty_kilometre_gate() {
    let gap = scrapers::store_finder::distance_km((53.9440, 14.1830), (53.7383, 14.0511));

    assert!((24.4..24.6).contains(&gap), "Ahlbeck -> Ueckermünde sind 24,5 km, gemessen {gap:.2}");
    assert!(gap > 15.0, "und mehr als der Regionsbegriff CUTOFF_KM hergibt");
}

/// Ohne Koordinaten bleibt der Schlüssel die PLZ — der Weg des Sonntagslaufs
/// und aller Anforderungen von Apps, die v21 noch nicht kennen.
#[test]
fn without_coordinates_the_key_stays_the_postcode() {
    let target = lechariot::branches::AreaTarget::from_plz("17419");

    assert_eq!(target.dedup_key(), "plz:17419");
    assert_eq!(target.coords, None);
    assert_eq!(target.anchor_market_id, None);
}

// ------------------------------------------------- Die Rundung (Migration v24)

/// **Der stille Fehler.** `round(v::numeric, 1)` in Postgres rundet eine
/// Dezimalzahl, `(v * 10.0).round() / 10.0` einen Binärwert — und die
/// Multiplikation mit 10 zieht einen Wert knapp unter einer Halben auf die
/// Halbe hoch. Für den f64 direkt unter 53,85 kam so `cell:53.9` heraus,
/// während die Datenbank `cell:53.8` schrieb; die App wartete dann auf einen
/// Schlüssel, den es nicht gibt, und die Fertigmeldung des Laufs traf keine
/// Zeile. Beides ohne ein einziges Fehlerzeichen.
///
/// Die Erwartungen hier sind die des SQL (`round(v::text::numeric, 1)`), von
/// Hand nachgerechnet — nicht das Ergebnis derselben Funktion, sonst prüfte
/// der Test sich selbst.
#[test]
fn the_cell_rounds_the_decimal_not_the_binary_value() {
    // 53.849999999999994 ist der größte f64 unterhalb von 53,85. Dezimal
    // gerundet ist das 53,8 — binär gerechnet wurde daraus 53,9.
    assert_eq!(
        lechariot::branches::area_cell(53.849999999999994, 14.149999999999999),
        "cell:53.8,14.1"
    );
    // Und die Gegenprobe: 53,85 selbst liegt als f64 ÜBER der Grenze
    // (53.850000000000001…), rundet also auf beiden Wegen auf.
    assert_eq!(lechariot::branches::area_cell(53.85, 14.15), "cell:53.9,14.2");
}

/// Von der Null weg, nicht zur geraden Ziffer. `format!("{:.1}")` rundet zur
/// geraden Ziffer, und x,25 und x,75 sind als f64 exakt darstellbar — dort
/// wäre der Unterschied sofort sichtbar und träfe ganz gewöhnliche
/// Koordinaten mit zwei Nachkommastellen.
#[test]
fn halves_round_away_from_zero_like_numeric_does() {
    for (lat, lon, expected) in [
        (53.25, 13.25, "cell:53.3,13.3"),
        (53.75, 13.75, "cell:53.8,13.8"),
        (53.35, 13.45, "cell:53.4,13.5"),
        (53.95, 13.95, "cell:54.0,14.0"),
        (53.05, 13.55, "cell:53.1,13.6"),
    ] {
        assert_eq!(lechariot::branches::area_cell(lat, lon), expected, "{lat}/{lon}");
    }
}

/// Was der Trigger schreibt, wenn die Koordinaten gerade noch durchs Tor
/// gehen, und was er bei 0 schreibt: eine Nachkommastelle, nie `-0.0`.
/// Postgres' `round(numeric, 1)` liefert die Stelle auch dann, wenn sie 0 ist
/// (`54` wird zu `54.0`), und `-0.04` rundet auf `0.0`.
#[test]
fn the_cell_always_carries_exactly_one_decimal() {
    assert_eq!(lechariot::branches::area_cell(54.0, 14.0), "cell:54.0,14.0");
    assert_eq!(lechariot::branches::area_cell(0.04, -0.04), "cell:0.0,0.0");
    assert_eq!(lechariot::branches::area_cell(-53.96, -13.96), "cell:-54.0,-14.0");
}

/// **Wie erreichbar der Fehler wirklich war** — gemessen, nicht geschätzt.
/// Über den ganzen deutschen Kasten (Breite 47,0-56,0, Länge 5,0-16,0)
/// stimmen alte und neue Rundung für JEDE Koordinate mit bis zu vier
/// Nachkommastellen überein. Ein Geokodierer liefert 53.944 — über diesen Weg
/// war der Fehler nie zu erreichen, und die Notiz „bei genau 53.85" trifft
/// ihn nicht: 53,85 liegt als f64 über der Grenze.
///
/// Der Test hält beide Aussagen zugleich fest: dass die Änderung nichts
/// verschiebt, was heute vorkommt, und dass sie überhaupt etwas ändert.
#[test]
fn for_ordinary_coordinates_both_rules_agree() {
    let old_rule = |v: f64| format!("{:.1}", (v * 10.0).round() / 10.0 + 0.0);
    let new_rule = |v: f64| {
        let cell = lechariot::branches::area_cell(v, 0.0);
        cell.trim_start_matches("cell:").split(',').next().unwrap().to_string()
    };

    let mut checked = 0u32;
    for scale in [10i64, 100, 1000, 10_000] {
        for step in (47 * scale)..=(56 * scale) {
            let lat = step as f64 / scale as f64;
            assert_eq!(old_rule(lat), new_rule(lat), "Breite {lat}");
            checked += 1;
        }
        for step in (5 * scale)..=(16 * scale) {
            let lon = step as f64 / scale as f64;
            assert_eq!(old_rule(lon), new_rule(lon), "Länge {lon}");
            checked += 1;
        }
    }
    assert!(checked > 200_000, "nur {checked} Werte geprüft — der Kasten ist größer");

    // Und der Gegenbeweis, dass die Regeln trotzdem verschieden sind: der f64
    // unmittelbar unter einer x,x5-Grenze.
    let boundary = 53.849999999999994;
    assert_eq!(old_rule(boundary), "53.9");
    assert_eq!(new_rule(boundary), "53.8");
}

/// Der eigentliche Umbau: Wo die Datenbank ihren Schlüssel mitgegeben hat,
/// wird nicht mehr gerundet, sondern genommen.
///
/// Der Fall ist echt und steht so schon im App-Code: Verwirft der
/// BEFORE-Trigger die Koordinaten als unplausibel oder als zu weit vom Anker,
/// führt er die Zeile unter `plz:…`, während der Client Koordinaten hat. Wer
/// dann die Zelle nachrechnet, sucht eine Zeile, die es unter diesem Namen
/// nicht gibt — und meldet still nichts.
#[test]
fn a_handed_over_key_is_used_verbatim() {
    let target = lechariot::branches::AreaTarget {
        coords: Some((53.9440, 14.1830)),
        plz: Some("17419".into()),
        anchor_market_id: Some("4030191".into()),
        area_key: Some("plz:17419".into()),
    };

    assert_eq!(target.dedup_key(), "plz:17419");
    assert_ne!(
        target.dedup_key(),
        lechariot::branches::area_cell(53.9440, 14.1830),
        "der Test wäre wertlos, wenn die eigene Rechnung zufällig dasselbe ergäbe"
    );
}
