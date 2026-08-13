// Offline-Parser-Tests gegen eingefrorene Fixtures (tests/fixtures/<chain>/).
// Die Fixtures sind auf wenige repräsentative Angebote gekürzte Live-Antworten
// vom 2026-07-17 (KW 29, PLZ 01219 Dresden). Laufen ohne Netz bei jedem
// `cargo test`.

use lechariot::scrapers;

// ---------------------------------------------------------------- REWE

#[test]
fn rewe_fixture_parses_discounts() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/rewe/discounts.json")).unwrap();
    let offers = scrapers::rewe::parse_offers(raw, "565005").unwrap();
    assert_eq!(offers.len(), 4);

    // validUntil (top-level, ISO-Zeitstempel) wird auf das Datum gekürzt und
    // auf jedes Angebot gesetzt; valid_from liefert rewerse v1.2.0 nicht mehr
    // und wird als Montag derselben Woche abgeleitet.
    let tomaten = &offers[0];
    assert_eq!(tomaten.title, "Mini Rispentomaten");
    assert_eq!(tomaten.price, Some(1.69));
    assert_eq!(tomaten.valid_until.as_deref(), Some("2026-07-18"));
    assert_eq!(tomaten.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(tomaten.category.as_deref(), Some("Obst und Gemüse"));

    // price bereits als Float, Nutri-Score gesetzt
    let actimel = offers.iter().find(|o| o.title == "Danone Actimel Drink").unwrap();
    assert_eq!(actimel.price, Some(2.22));
    assert_eq!(actimel.nutri_score.as_deref(), Some("B"));
    assert_eq!(actimel.category.as_deref(), Some("Kühlung"));

    // priceParseFail (price == 0) -> Fallback auf priceRaw "1.299,00 €"
    let fallback = offers.iter().find(|o| o.title == "Test Preisfallback").unwrap();
    assert_eq!(fallback.price, Some(1299.00));
    assert_eq!(fallback.nutri_score, None);
}

// ---------------------------------------------------------------- Netto

#[test]
fn netto_fixture_parses_tiles_with_period() {
    let mut offers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    lechariot::scrapers::netto::parse_page(
        include_str!("fixtures/netto/filialangebote_1.html"),
        "4816",
        &mut offers,
        &mut seen,
    );
    assert_eq!(offers.len(), 4);

    let eis = &offers[0];
    assert_eq!(eis.title, "Langnese Cremissimo Eis");
    assert_eq!(eis.price, Some(1.79));
    assert_eq!(eis.regular_price, Some(3.99));
    assert_eq!(eis.category.as_deref(), Some("Wochenangebote"));
    assert_eq!(eis.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(eis.valid_until.as_deref(), Some("2026-07-18"));

    let kirschen = offers.iter().find(|o| o.title == "Kirschen").unwrap();
    assert_eq!(kirschen.price, Some(0.39));
    assert_eq!(kirschen.subtitle.as_deref(), Some("100 g"));
}

// Nettos Bilder MÜSSEN gespiegelt werden — die App kann sie nie selbst laden
// (403 für jeden schlichten Client, gemessen 2026-07-31). Ob gespiegelt wird,
// entscheidet allein der Host: `push::scope_of` schickt eine URL nur dann in
// den Pflicht-Durchgang (`UnfetchableRemotes`), wenn
// `storage::is_client_blocked_url` sie erkennt. Alles andere landet im
// Kür-Durchgang, und der ist seit dem 27.07. abgeschaltet.
//
// Das ist die stille Bruchstelle dieser Kette: Zieht Netto seine Bilder auf
// einen anderen Host um, fällt die URL lautlos aus der Pflicht in die Kür, der
// Spiegel hört auf, und in der Datenbank sieht weiter alles gesund aus. Genau
// so sah der Fehler vom 31.07. aus. Der Test hält den Host deshalb an der
// Quelle fest — beim Aktualisieren der Fixture wird er rot und sagt, warum.
#[test]
fn netto_image_urls_stay_on_a_host_the_mirror_treats_as_mandatory() {
    let mut offers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    lechariot::scrapers::netto::parse_page(
        include_str!("fixtures/netto/filialangebote_1.html"),
        "4816",
        &mut offers,
        &mut seen,
    );

    let bilder: Vec<&String> = offers.iter().flat_map(|o| &o.images).collect();
    assert_eq!(bilder.len(), 4, "die Fixture trägt vier Produktbilder");
    for url in bilder {
        assert!(
            lechariot::storage::is_client_blocked_url(url),
            "Netto-Bild-URL wird nicht mehr als spiegelpflichtig erkannt — \
             CLIENT_BLOCKED_HOST_SUFFIXES in storage.rs ergänzen, sonst hört \
             der Spiegel still auf und die App zeigt wieder Emojis: {url}"
        );
    }

    // Gegenprobe, sonst prüft die Schleife oben nur, dass die Funktion „ja"
    // sagt: Ein Umzug auf einen anderen Host wäre wirklich unerkannt.
    assert!(
        !lechariot::storage::is_client_blocked_url(
            "https://cdn.netto.example/media/1-450x450-Eis.webp"
        ),
        "der Wächter muss den Umzug bemerken können, sonst bewacht er nichts"
    );
}

// ---------------------------------------------------------------- ALDI Nord

#[test]
fn aldi_nord_fixture_parses_next_data() {
    let offers = lechariot::scrapers::aldi_nord::parse_offers(
        include_str!("fixtures/aldi_nord/angebote.html"),
        "ALDI_NORD_DE",
    )
    .unwrap();
    assert_eq!(offers.len(), 3);

    let avocado = offers.iter().find(|o| o.title == "Avocado").unwrap();
    assert_eq!(avocado.price, Some(0.66));
    assert_eq!(avocado.regular_price, Some(0.79));
    assert_eq!(avocado.subtitle.as_deref(), Some("Stück"));
    assert_eq!(avocado.category.as_deref(), Some("Frische-Aktion: Obst & Gemüse"));
    assert_eq!(avocado.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(avocado.valid_until.as_deref(), Some("2026-07-18"));
}

/// Die Lücke zwischen Aktionstagen und Produkt-Snapshot fällt auf.
///
/// Nachgebaut aus der echten Seite vom 06.08.2026: Die „Osteuropa-Aktion" der
/// KW 32 nannte `1032980` — „OSTEUROPA Original polnische Pierogi", 2,49 €, im
/// Prospekt derselben Woche —, `algoliaDataMap` kannte die ID nicht, und das
/// Angebot verschwand spurlos. Dazu ein Eintrag mit leerem `name`, der ebenso
/// unbrauchbar ist.
#[test]
fn aldi_nord_reports_products_the_datamap_does_not_have() {
    use lechariot::scrapers::aldi_nord::MissingReason;

    let (offers, missing) = lechariot::scrapers::aldi_nord::parse_offers_reporting(
        include_str!("fixtures/aldi_nord/angebote_luecke.html"),
        "ALDI_NORD_DE",
    )
    .unwrap();

    // Nur das Produkt mit Namen und Preis wird ein Angebot.
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].title, "Würstchen");

    assert_eq!(missing.len(), 2);

    let pierogi = missing.iter().find(|m| m.product_id == "1032980").unwrap();
    assert_eq!(pierogi.reason, MissingReason::NotInDataMap);
    assert_eq!(pierogi.section.as_deref(), Some("Osteuropa-Aktion"));
    assert_eq!(pierogi.aktion_start.as_deref(), Some("2026-08-03"));

    let namenlos = missing.iter().find(|m| m.product_id == "9999999").unwrap();
    assert_eq!(namenlos.reason, MissingReason::EmptyName);
    assert_eq!(namenlos.section.as_deref(), Some("Osteuropa-Aktion"));
}

/// Eine vollständige Seite meldet keine Lücke — sonst wäre die Warnung Lärm.
#[test]
fn aldi_nord_complete_fixture_reports_nothing_missing() {
    let (offers, missing) = lechariot::scrapers::aldi_nord::parse_offers_reporting(
        include_str!("fixtures/aldi_nord/angebote.html"),
        "ALDI_NORD_DE",
    )
    .unwrap();
    assert_eq!(offers.len(), 3);
    assert!(missing.is_empty(), "unerwartet gemeldet: {missing:?}");
}

/// Die echte Seite, unverändert geparst — die Gegenprobe zu Issue #82.
///
/// Der Verdacht dort war eine geänderte Quelle (wie bei EDEKA am 08.08., #67).
/// Diese Fixture ist ein wörtlicher Ausschnitt der Seite, die der Runner am
/// 12.08.2026 geholt hat (Lauf 31637056816): dasselbe `__NEXT_DATA__`, der
/// Aktionstag „Sa. 15.8." mit allen acht Produkten seiner `productIds`, nichts
/// umgeschrieben. Der ganze Abzug ergab 246 Angebote — das Markup steht also,
/// die Kette fiel am Abruf aus, nicht am Parser.
#[test]
fn aldi_nord_live_seite_vom_12_08_parst_unveraendert() {
    let (offers, missing) = lechariot::scrapers::aldi_nord::parse_offers_reporting(
        include_str!("fixtures/aldi_nord/angebote_live_20260812.html"),
        "ALDI_NORD_DE",
    )
    .unwrap();
    // Acht `productIds`, sieben Angebote: „Pesto" von BARILLA steht in der
    // Quelle zweimal (1013067 und 10130820001, gleicher Preis, einmal
    // „190-g-Glas", einmal nur „Glas"). Beide tragen dieselbe Angebots-ID aus
    // Titel und Datum und fallen zu einer Zeile zusammen — gewollt, sonst
    // stünde dasselbe Pesto zweimal in der App.
    assert_eq!(offers.len(), 7);
    assert!(missing.is_empty(), "unerwartet gemeldet: {missing:?}");

    // Ein Angebot mit allem, worauf die App angewiesen ist.
    let cola = offers.iter().find(|o| o.title.contains("Cola")).unwrap();
    assert_eq!(cola.price, Some(0.65));
    assert_eq!(cola.regular_price, Some(1.09));
    assert_eq!(cola.subtitle.as_deref(), Some("0,33-L-Dose"));
    assert_eq!(cola.valid_from.as_deref(), Some("2026-08-15"));
    assert_eq!(cola.valid_until.as_deref(), Some("2026-08-15"));

    // Und eins ohne Streichpreis, damit die Fixture nicht nur den Sonderfall trägt.
    let broetchen = offers.iter().find(|o| o.title.contains("Chia-Brötchen")).unwrap();
    assert_eq!(broetchen.price, Some(0.25));
    assert_eq!(broetchen.regular_price, None);
}

// ---------------------------------------------------------------- ALDI Süd

#[test]
fn aldi_sued_fixture_parses_products_with_cent_prices() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/aldi_sued/product_search.json")).unwrap();
    let items = raw["data"].as_array().unwrap();
    assert_eq!(items.len(), 4);

    let offers: Vec<_> = items
        .iter()
        .filter_map(|it| lechariot::scrapers::aldi_sued::parse_product(it, "ALDI_SUED_DE"))
        .collect();
    assert_eq!(offers.len(), 4);

    // Preise kommen in Cent: 189 -> 1.89 €
    let aepfel = &offers[0];
    assert_eq!(aepfel.title, "Äpfel Krumme Dinger 2 kg");
    assert_eq!(aepfel.price, Some(1.89));
    assert_eq!(aepfel.subtitle.as_deref(), Some("2 kg"));
}

// ---------------------------------------------------------------- Kaufland

#[test]
fn kaufland_fixture_parses_sections_and_tiles() {
    let offers =
        scrapers::kaufland::parse_offers(include_str!("fixtures/kaufland/uebersicht.html"), "DE7380")
            .unwrap();
    assert_eq!(offers.len(), 9);

    let lachs = &offers[0];
    assert_eq!(lachs.title, "K-BLUE BAY");
    assert_eq!(lachs.subtitle.as_deref(), Some("Lachsforellenfilet"));
    assert_eq!(lachs.price, Some(3.19));
    assert_eq!(lachs.regular_price, Some(3.79));
    assert_eq!(lachs.category.as_deref(), Some("Fisch"));
    assert_eq!(lachs.valid_from.as_deref(), Some("2026-07-16"));
    assert_eq!(lachs.valid_until.as_deref(), Some("2026-07-22"));
}

// Regression: Kaufland-Titel sind Marken, das Produkt steht im Untertitel —
// die Offer-ID muss den Untertitel enthalten, sonst kollidieren alle
// Angebote einer Marke (Fix vom 2026-07, Commit 7b695e2).
#[test]
fn kaufland_offer_ids_include_subtitle() {
    let offers =
        scrapers::kaufland::parse_offers(include_str!("fixtures/kaufland/uebersicht.html"), "DE7380")
            .unwrap();

    let bay: Vec<_> = offers.iter().filter(|o| o.title == "K-BLUE BAY").collect();
    assert_eq!(bay.len(), 2);
    assert_ne!(bay[0].id, bay[1].id, "gleiche Marke, anderes Produkt -> andere ID");
}

// Quirk: dasselbe Angebot erscheint in der Warengruppe UND in "Unsere
// Knüller". Beide Vorkommen teilen dieselbe ID (Dedup passiert beim
// DB-Upsert, nicht im Parser).
#[test]
fn kaufland_duplicate_listing_across_categories_shares_id() {
    let offers =
        scrapers::kaufland::parse_offers(include_str!("fixtures/kaufland/uebersicht.html"), "DE7380")
            .unwrap();

    let bauer: Vec<_> = offers.iter().filter(|o| o.title == "BAUER").collect();
    assert_eq!(bauer.len(), 2, "Duplikat aus zwei Kategorien erwartet");
    assert_eq!(bauer[0].id, bauer[1].id);
    assert_ne!(bauer[0].category, bauer[1].category);
}

// ---------------------------------------------------------------- EDEKA

#[test]
fn edeka_fixture_parses_offers_and_dates() {
    let offers =
        scrapers::edeka::parse_offers(include_str!("fixtures/edeka/angebote.html"), "022745")
            .unwrap();
    assert_eq!(offers.len(), 6);

    let blaubeeren = &offers[0];
    assert_eq!(blaubeeren.title, "Kulturheidelbeeren");
    assert_eq!(blaubeeren.price, Some(3.99));
    assert_eq!(blaubeeren.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(blaubeeren.valid_until.as_deref(), Some("2026-07-18"));

    // App-Preis wird wie ein Festpreis geparst
    let butter = offers.iter().find(|o| o.title.starts_with("Meggle")).unwrap();
    assert_eq!(butter.price, Some(0.99));
}

// Das Markup vom August: Preis in einem `<span class="sr-only">` statt in
// einem `<div>`, Euro-Zeichen an der Zahl, dazu die neue Zeile „Rabattierter
// Preis von … (Insgesamt -45% Rabatt)". Vom 05.08. bis 09.08. fand der Parser
// darin **keinen einzigen** Preis, und der Ketten-Wächter hat jede Nacht
// zurecht nichts hochgeladen. Der alte Fixture-Test oben bleibt daneben
// stehen: Beide Fassungen müssen tragen.
#[test]
fn edeka_neues_preis_markup_parst_alle_preisarten() {
    let offers = scrapers::edeka::parse_offers(
        include_str!("fixtures/edeka/angebote_2026-08.html"),
        "021868",
    )
    .unwrap();
    assert_eq!(offers.len(), 5);

    let festpreis = offers.iter().find(|o| o.title == "Himbeeren").unwrap();
    assert_eq!(festpreis.price, Some(1.49));
    assert_eq!(festpreis.valid_from.as_deref(), Some("2026-08-03"));
    assert_eq!(festpreis.valid_until.as_deref(), Some("2026-08-08"));

    // "Rabattierter Preis von 1.49€ (Insgesamt -45% Rabatt)"
    let rabattiert = offers.iter().find(|o| o.title == "Grünländer Scheiben").unwrap();
    assert_eq!(rabattiert.price, Some(1.49));

    // Zwei Preise in einer Kachel: der App-Preis steht zuerst und gewinnt.
    let app = offers.iter().find(|o| o.title.starts_with("Jacobs Krönung oder")).unwrap();
    assert_eq!(app.price, Some(5.99));

    // EDEKA druckt keinen Streichpreis (#43) — der Rabatt-Prozentsatz ist
    // keiner, und aus ihm einen zu rechnen wäre erfunden.
    assert!(offers.iter().all(|o| o.regular_price.is_none()));

    // Die zwei preislosen Kacheln bleiben preislos, aber es sind eben nur zwei.
    assert_eq!(offers.iter().filter(|o| o.price.is_some()).count(), 3);
}

// EDEKA-NULL-Preise sind echt: "Tagespreis"-Kacheln und reine
// PAYBACK-Punkte-Kacheln haben im HTML (Kachel + Dialog) keinen Preis.
// Sie kommen bewusst mit price = None an — kein Parser-Bug.
#[test]
fn edeka_priceless_promo_tiles_stay_price_none() {
    let offers =
        scrapers::edeka::parse_offers(include_str!("fixtures/edeka/angebote.html"), "022745")
            .unwrap();

    let tagespreis = offers.iter().find(|o| o.title.contains("Grillkäse")).unwrap();
    assert_eq!(tagespreis.price, None);

    let payback = offers.iter().find(|o| o.title == "Arla Kærgården").unwrap();
    assert_eq!(payback.price, None);

    assert_eq!(offers.iter().filter(|o| o.price.is_none()).count(), 2);
}

// ---------------------------------------------------------------- Penny

#[test]
fn penny_fixture_parses_all_tiles() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/penny/offers_kuehlregal.json")).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut offers = Vec::new();
    scrapers::penny::parse_offer_tiles(
        &raw, "4030829", "kuehlregal", "2026-07-13", "2026-07-19", &mut seen, &mut offers,
    );
    assert_eq!(offers.len(), 6);

    let joghurt = &offers[0];
    assert_eq!(joghurt.title, "MÖVENPICK Feinjoghurt");
    assert_eq!(joghurt.price, Some(0.77));
    assert_eq!(joghurt.regular_price, Some(0.99));
    assert_eq!(joghurt.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(joghurt.category.as_deref(), Some("kuehlregal"));
}

// Regression: Penny-Aktionspreise kommen als String mit Fußnoten-Sternchen
// ("0.49*") — json_price muss */€ abstreifen (Fix vom 2026-07, Commit 52d87ca).
#[test]
fn penny_star_prices_are_parsed() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/penny/offers_kuehlregal.json")).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut offers = Vec::new();
    scrapers::penny::parse_offer_tiles(
        &raw, "4030829", "kuehlregal", "2026-07-13", "2026-07-19", &mut seen, &mut offers,
    );

    let buttermilch = offers.iter().find(|o| o.title == "MILPRIMA Reine Buttermilch").unwrap();
    assert_eq!(buttermilch.price, Some(0.49), "String-Preis \"0.49*\" muss geparst werden");
    assert_eq!(buttermilch.regular_price, Some(0.59));
    for o in &offers {
        assert!(o.price.is_some(), "Preis fehlt bei {}", o.title);
    }
}

// Kategorien-übergreifende Dedup: derselbe Titel darf nur einmal ankommen.
#[test]
fn penny_duplicate_titles_across_categories_are_deduped() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/penny/offers_kuehlregal.json")).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut offers = Vec::new();
    scrapers::penny::parse_offer_tiles(
        &raw, "4030829", "top-angebote", "2026-07-13", "2026-07-19", &mut seen, &mut offers,
    );
    scrapers::penny::parse_offer_tiles(
        &raw, "4030829", "kuehlregal", "2026-07-13", "2026-07-19", &mut seen, &mut offers,
    );
    assert_eq!(offers.len(), 6, "zweite Kategorie mit denselben Titeln darf nichts hinzufügen");
}

// ---------------------------------------------------------------- Lidl

// ---------------------------------------------------------------- Store-Finder
// Offline-Fixtures der Filialfinder (Lidl: Bing SDS, ALDI: Uberall) und des
// Nominatim-Geocoders, gekürzte Live-Antworten vom 2026-07-17 (PLZ 01219).

#[test]
fn store_finder_virtualearth_hit_yields_lidl_branch_with_geo() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/virtualearth_hit.json")).unwrap();
    let market = scrapers::store_finder::parse_virtualearth(&raw).unwrap().unwrap();
    assert_eq!(market.id, "LIDL_1988");
    assert_eq!(market.name, "Lidl Strehlen");
    assert_eq!(market.lat, Some(51.0338));
    assert_eq!(market.lon, Some(13.74984));
}

// Regression: leerer ShownStoreName ergab den Filialnamen "Lidl " — jetzt
// fällt der Name auf die Stadt (Locality) zurück.
#[test]
fn store_finder_virtualearth_empty_store_name_falls_back_to_city() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/virtualearth_hit_no_name.json"))
            .unwrap();
    let market = scrapers::store_finder::parse_virtualearth(&raw).unwrap().unwrap();
    assert_eq!(market.id, "LIDL_5745");
    assert_eq!(market.name, "Lidl Dresden");
}

#[test]
fn store_finder_virtualearth_empty_means_no_branch() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/virtualearth_empty.json")).unwrap();
    assert!(scrapers::store_finder::parse_virtualearth(&raw).unwrap().is_none());
}

#[test]
fn store_finder_virtualearth_malformed_is_error_not_absence() {
    // Formatänderung darf die Kette nicht stumm abmelden — Fehler löst den
    // Platzhalter-Fallback aus.
    let raw = serde_json::json!({"unexpected": true});
    assert!(scrapers::store_finder::parse_virtualearth(&raw).is_err());
}

/// Koordinaten der PLZ 01219 (wie in fixtures/store_finder/nominatim_plz.json),
/// Ausgangspunkt aller Uberall-Fixtures hier.
const PLZ_01219: (f64, f64) = (51.0231864, 13.7659125);

#[test]
fn store_finder_uberall_hit_yields_branch_with_geo() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/uberall_hit.json")).unwrap();
    let market = scrapers::store_finder::parse_uberall(&raw, PLZ_01219, "ALDI Nord", "ALDI_NORD")
        .unwrap()
        .unwrap();
    assert_eq!(market.id, "ALDI_NORD_DE036039");
    assert_eq!(market.name, "ALDI Nord Dresden");
    assert_eq!(market.lat, Some(51.019498));
    assert_eq!(market.lon, Some(13.735563));
}

#[test]
fn store_finder_uberall_beyond_cutoff_means_no_branch() {
    // Nächste Filiale 48 km entfernt -> Kette in der Region nicht vertreten.
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/uberall_far.json")).unwrap();
    assert!(scrapers::store_finder::parse_uberall(&raw, PLZ_01219, "ALDI Nord", "ALDI_NORD")
        .unwrap()
        .is_none());
}

// Ohne `distance` filtert die Abfrage gar nicht mehr nach Umkreis (max=1, kein
// Radiusparameter) — die Entfernung muss dann aus den Filialkoordinaten kommen,
// sonst gilt jede Kette überall als vertreten.
#[test]
fn store_finder_uberall_without_distance_uses_coordinates() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/uberall_hit_no_distance.json"))
            .unwrap();
    // Chemnitz, ~60 km von 01219 entfernt
    assert!(
        scrapers::store_finder::parse_uberall(&raw, PLZ_01219, "ALDI Nord", "ALDI_NORD")
            .unwrap()
            .is_none(),
        "Filiale außerhalb des Cutoffs darf ohne distance-Feld nicht durchrutschen"
    );

    // Dieselbe Antwort, Filiale direkt vor der Haustür -> Treffer.
    let mut near = raw.clone();
    let store = near.pointer_mut("/response/locations/0").unwrap();
    store["lat"] = serde_json::json!(PLZ_01219.0);
    store["lng"] = serde_json::json!(PLZ_01219.1);
    let market = scrapers::store_finder::parse_uberall(&near, PLZ_01219, "ALDI Nord", "ALDI_NORD")
        .unwrap()
        .unwrap();
    assert_eq!(market.id, "ALDI_NORD_DE099999");
}

#[test]
fn store_finder_uberall_without_distance_and_coordinates_is_error() {
    // Weder distance noch lat/lng: Entfernung nicht prüfbar -> Fehler, damit
    // resolve den nationalen Platzhalter setzt statt still zu raten.
    let raw = serde_json::json!({
        "status": "SUCCESS",
        "response": {"locations": [{"identifier": "DE099999", "city": "Chemnitz"}]}
    });
    assert!(
        scrapers::store_finder::parse_uberall(&raw, PLZ_01219, "ALDI Nord", "ALDI_NORD").is_err()
    );
}

#[test]
fn store_finder_uberall_empty_means_no_branch() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/uberall_empty.json")).unwrap();
    assert!(scrapers::store_finder::parse_uberall(&raw, PLZ_01219, "ALDI SÜD", "ALDI_SUED")
        .unwrap()
        .is_none());
}

#[test]
fn store_finder_uberall_error_status_is_error() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/uberall_error.json")).unwrap();
    assert!(
        scrapers::store_finder::parse_uberall(&raw, PLZ_01219, "ALDI Nord", "ALDI_NORD").is_err()
    );
}

#[test]
fn store_finder_nominatim_parses_plz_coordinates() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/store_finder/nominatim_plz.json")).unwrap();
    let (lat, lon) = scrapers::store_finder::parse_nominatim(&raw).unwrap();
    assert!((lat - 51.0231864).abs() < 1e-9);
    assert!((lon - 13.7659125).abs() < 1e-9);
    assert!(scrapers::store_finder::parse_nominatim(&serde_json::json!([])).is_none());
}

/// Reverse-Geocoding ist ab v21 der Weg, auf dem ein Gebiets-Lauf seine PLZ
/// bekommt: Die App schickt die Mitte der Region, hier wird die Postleitzahl
/// daraus. Die Fixture ist die Seestraße in Seebad Ahlbeck — die Ecke, an der
/// der Fehler vom 2026-07-30 aufgefallen ist.
#[test]
fn store_finder_nominatim_reverse_yields_postcode_and_city() {
    let raw: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/store_finder/nominatim_reverse_ahlbeck.json"
    ))
    .unwrap();

    let (plz, city) = scrapers::store_finder::parse_nominatim_reverse(&raw);

    assert_eq!(plz.as_deref(), Some("17419"), "genau die PLZ, die dem Tester gefehlt hat");
    assert_eq!(city.as_deref(), Some("Seebad Ahlbeck"), "auf dem Land steht dort `village`");

    // Und der Grund, warum es einen eigenen Parser gibt: `/reverse` antwortet
    // mit einem Objekt, `/search` mit einem Array. Die Array-Parser geben für
    // diese Antwort still None zurück — das sähe aus wie „Gegend unbekannt".
    assert!(
        scrapers::store_finder::parse_nominatim(&raw).is_none(),
        "der Vorwärts-Parser darf hier gar nichts verstehen"
    );
}

/// Über See, im Wald oder bei einer Lücke in OSM antwortet Nominatim mit
/// `{"error": …}`. Das ist kein Fehler des Laufs: Dann trägt die PLZ der
/// Ankerfiliale die Textsuchen weiter, also das Verhalten von vor v21.
#[test]
fn store_finder_nominatim_reverse_survives_an_ungeocodable_point() {
    let raw = serde_json::json!({"error": "Unable to geocode"});

    let (plz, city) = scrapers::store_finder::parse_nominatim_reverse(&raw);

    assert!(plz.is_none() && city.is_none());
}

#[test]
fn store_finder_resolve_falls_back_to_national_on_error() {
    use lechariot::models::Market;
    use lechariot::scrapers::store_finder::resolve;

    let national = || Market::new("LIDL_DE", "Lidl Deutschland");
    // Treffer gewinnt
    let hit = Market::new("LIDL_1988", "Lidl Strehlen");
    assert_eq!(resolve("Lidl", Ok(Some(hit)), national()).unwrap().id, "LIDL_1988");
    // Sauberes "keine Filiale" -> Kette nicht registrieren
    assert!(resolve("Lidl", Ok(None), national()).is_none());
    // Finder-Fehler -> WARN + nationaler Platzhalter (graceful degradation)
    let fallback = resolve("Lidl", Err(anyhow::anyhow!("Netz weg")), national()).unwrap();
    assert_eq!(fallback.id, "LIDL_DE");
}

// ------------------------------------------------- Lidl-Prospekt (eigene Quelle)

// Der Weg für Lidl, seit marktguru am 2026-07-31 entfernt wurde: Lidls eigener
// Wochenprospekt als PDF mit Textebene. Die Fixture ist die gekürzte
// `pdftotext -bbox-layout`-Ausgabe der oberen Hälfte von Seite 15 des
// Prospekts vom 2026-07-20 (Absatzregion 20, Dresden) — echte
// Wortkoordinaten, damit die Kachelbildung getestet wird und nicht ein
// nachgebautes Idealbild.

#[test]
fn lidl_prospekt_builds_tiles_from_word_coordinates() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_bbox_layout.xml"),
        "LIDL_1988",
        Some("2026-07-20"),
        Some("2026-07-25"),
    );
    assert!(offers.len() >= 5, "nur {} Kacheln erkannt", offers.len());

    // Preis und Produktname stehen nicht in derselben Textzeile — genau das
    // muss die Geometrie zusammenbringen.
    let leerdammer = offers.iter().find(|o| o.title.contains("LEERDAMMER")).unwrap();
    assert_eq!(leerdammer.price, Some(2.99));
    assert_eq!(leerdammer.regular_price, Some(4.99));
    assert!(leerdammer.subtitle.as_deref().unwrap().contains("1 kg = 13.29"));

    // Rabatt-Reste einer Nachbarkachel gehören nicht in den Namen.
    let milka = offers.iter().find(|o| o.title.starts_with("MILKA")).unwrap();
    assert_eq!(milka.price, Some(1.99));
    assert!(!milka.title.contains('%'), "Rabatt im Titel: {:?}", milka.title);

    // Weicher Trennstrich im Satz: "CELE\u{ad}BRATIONS" ist ein Wort.
    assert!(
        offers.iter().any(|o| o.title.contains("CELEBRATIONS")),
        "weicher Trennstrich nicht zusammengezogen: {:?}",
        offers.iter().map(|o| &o.title).collect::<Vec<_>>()
    );

    for o in &offers {
        assert!(o.price.is_some(), "Preis fehlt bei {}", o.title);
        assert_eq!(o.valid_from.as_deref(), Some("2026-07-20"));
        assert_eq!(o.valid_until.as_deref(), Some("2026-07-25"));
        assert!(o.flyer_page.is_some(), "Seitenzahl fehlt bei {}", o.title);
        // Streichpreis muss über dem Angebotspreis liegen, sonst hat die
        // Kachel den Grundpreis eingesammelt.
        if let (Some(p), Some(r)) = (o.price, o.regular_price) {
            assert!(r > p, "Streichpreis {r} <= Preis {p} bei {}", o.title);
        }
    }
}

// Die Fixture ist echte `pdftotext -bbox-layout`-Ausgabe des Prospekts vom
// 27.07. (Seiten 3 und 7, Absatzregion von 01219): zwei Kacheln, denen ein
// mehrzeiliges Banner vor dem Titel klebt — eine mit echtem Produkt dahinter,
// eine ohne.
#[test]
fn lidl_prospekt_rescues_a_title_behind_a_leading_banner() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_banner_titel.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );

    // Der Lavendel hat „Erhältlich ab Do. 30.7. Für draußen" vor dem Namen
    // und wurde bis 2026-07-31 deshalb komplett verworfen. Das Banner wird
    // abgeschnitten, das Produkt bleibt.
    let lavendel = offers
        .iter()
        .find(|o| o.title == "Lavendel angustifolia")
        .expect("Lavendel trotz Banner nicht gerettet");
    assert_eq!(lavendel.price, Some(1.69));

    // Gegenprobe: Die zweite Kachel trägt dasselbe Banner und dahinter NUR
    // den Preis (2.49*) — sie ist wirklich namenlos und bleibt verworfen.
    // Ein Banner als Produktname wäre schlimmer als ein fehlendes Angebot.
    assert_eq!(
        offers.len(),
        1,
        "die namenlose Banner-Kachel wurde gerettet: {:?}",
        offers.iter().map(|o| &o.title).collect::<Vec<_>>()
    );
}

// Dieselbe Machart wie die Banner-Fixture: echte `pdftotext -bbox-layout`-
// Ausgabe des Prospekts vom 27.07., Seite 12 (Fleischtheke) und Seite 17
// (TEMPO). Drei Preiskacheln, denen eine Plakette vor dem Namen klebt —
// „Tiefpreis Garantie", „Frischluftstall", „42er- Pack".
#[test]
fn lidl_prospekt_rescues_a_price_tile_behind_a_plaque() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_plakette.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );
    let titles: Vec<&str> = offers.iter().map(|o| o.title.as_str()).collect();

    // Diese drei Kacheln tragen ihren Preis und ihren Namen selbst; bis
    // 2026-07-31 fielen sie an der Auswahl der selbsttragenden Kacheln durch,
    // weil dort der Titel VOR dem Plakettenschnitt geprüft wurde.
    for erwartet in [
        "METZGERFRISCH Frisches Rinder-Hackfleisch",
        "METZGERFRISCH Rinder-Minuten-steaks",
        "TEMPO Taschen tücher",
    ] {
        assert!(
            titles.contains(&erwartet),
            "{erwartet:?} nicht gerettet, gefunden: {titles:?}"
        );
    }

    // Und der Preis gehört zum Produkt, nicht zur Plakette.
    let hack = offers
        .iter()
        .find(|o| o.title.starts_with("METZGERFRISCH Frisches"))
        .unwrap();
    assert_eq!(hack.price, Some(9.99));
    let tempo = offers.iter().find(|o| o.title.starts_with("TEMPO")).unwrap();
    assert_eq!(tempo.price, Some(3.99));

    // Keine Plakette ist selbst zum Angebot geworden.
    for t in &titles {
        let lower = t.to_lowercase();
        assert!(
            !lower.contains("tiefpreis")
                && !lower.contains("frischluftstall")
                && !lower.starts_with("42er"),
            "Plakette als Produktname: {t:?}"
        );
    }
}

// Streichpreise: die Fixture ist die ungekürzte `pdftotext -bbox-layout`-
// Ausgabe der Seiten 15 und 61 des Prospekts vom 27.07.2026 (Absatzregion von
// 01219). **Ungekürzt mit Absicht** — schneidet man Flows heraus, verschiebt
// sich die Kachelbildung, und dann prüft der Test eine Paarung, die es im
// Prospekt so nie gab (beim Kürzen auf die halbe Seite 61 bekam DIAMANT Zucker
// den Streichpreis von FOL EPI).
//
// Die zwei Seiten decken alle vier Schreibweisen ab, in denen der Prospekt den
// alten Preis druckt, dazu die Kachel mit mehreren Preisen.
#[test]
fn lidl_prospekt_reads_the_crossed_out_price() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_streichpreis.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );
    let find = |name: &str| {
        offers
            .iter()
            .find(|o| o.title.starts_with(name))
            .unwrap_or_else(|| panic!("{name} fehlt: {:?}", offers.iter().map(|o| &o.title).collect::<Vec<_>>()))
    };

    // Plakette mit Stichwort, ohne Rabattzahl: „d) UVP 3.49" neben 2.95*.
    assert_eq!((find("WRIGLEY").price, find("WRIGLEY").regular_price), (Some(2.95), Some(3.49)));
    // Stichwort mitten in der Beschreibungszeile: „… 1 kg = 11.10
    // Normalpreis: 1.99 1 kg = 19.90".
    assert_eq!((find("RITTER SPORT").price, find("RITTER SPORT").regular_price), (Some(1.11), Some(1.99)));
    // Plakette ganz ohne Stichwort: „-44%" über „1.99" über „1.11*".
    assert_eq!((find("APPEL").price, find("APPEL").regular_price), (Some(1.11), Some(1.99)));
    // Stichwort und Rabattzahl in einer Plakette: „-19% UVP 4.98".
    assert_eq!((find("WAGNER").price, find("WAGNER").regular_price), (Some(3.99), Some(4.98)));

    // Und nie unter dem Angebotspreis — dann hätte die Kachel einen
    // Grundpreis eingesammelt.
    for o in &offers {
        if let (Some(p), Some(r)) = (o.price, o.regular_price) {
            assert!(r > p, "Streichpreis {r} <= Preis {p} bei {}", o.title);
        }
    }
}

/// Eine Kachel, drei Preise, ein Streichpreis.
///
/// WAGNER Steinofen Pizza steht auf Seite 61 mit 3.99*, 3.79* und 0.99* in
/// einer Kachel und trägt genau eine Plakette („-19% UVP 4.98"). Bis
/// 2026-07-31 bekamen **alle drei** den Streichpreis 4.98 — für den 0.99er
/// wären das 80 % Rabatt, die der Prospekt nirgends behauptet. Der gedruckte
/// Rabatt geht nur für 3.99 auf (19,88 %, abgeschnitten 19).
#[test]
fn a_crossed_out_price_belongs_to_exactly_one_product_of_its_tile() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_streichpreis.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );
    let wagner: Vec<(Option<f64>, Option<f64>)> = offers
        .iter()
        .filter(|o| o.title.starts_with("WAGNER"))
        .map(|o| (o.price, o.regular_price))
        .collect();
    assert_eq!(wagner.len(), 3, "Kachel hat nicht mehr drei Preise: {wagner:?}");
    assert_eq!(
        wagner.iter().filter(|(_, r)| r.is_some()).count(),
        1,
        "der Streichpreis hängt an mehr als einem Preis: {wagner:?}"
    );
    assert!(wagner.contains(&(Some(3.99), Some(4.98))), "am falschen Preis: {wagner:?}");

    // Gegenstück ohne Rabattzahl: JOHNNIE WALKER trägt „Normalpreis: 15.99"
    // in seiner Beschreibung, teilt sich die Kachel aber mit dem Wodka
    // daneben. Welchem der beiden Preise die Zahl gehört, sagt der Prospekt
    // nicht — also bekommt sie keiner.
    let jw = offers.iter().find(|o| o.title.starts_with("JOHNNIE WALKER")).unwrap();
    assert_eq!(jw.regular_price, None, "mehrdeutiger Streichpreis übernommen");
}

/// Layout-Reste, die es bis in die Produktion geschafft haben.
///
/// Am 2026-07-31 gegen die Produktion gezählt: Von 371 verschiedenen
/// Lidl-Produkten des Laufs vom 31.07. benennen dreizehn keine Ware, sondern
/// beschriften das Foto oder stehen im Kleingedruckten. „0 Fragmente" vom
/// 2026-07-30 beschrieb einen Lauf, nicht den Dauerzustand.
///
/// Fixture ist die ungekürzte Seite 39 des Prospekts vom 27.07. — die
/// SILVERCREST-Küchengeräteseite, auf der „Zubereitung von 190 g Crushed
/// Ice" zweimal als eigener Artikel in der Datenbank landete (9.99 € und
/// 17.99 €).
#[test]
fn a_caption_on_the_photo_is_not_a_product() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_layoutrest.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );
    let titles: Vec<&str> = offers.iter().map(|o| o.title.as_str()).collect();
    assert!(
        !titles.iter().any(|t| t.starts_with("Zubereitung")),
        "Fotobeschriftung als Artikel: {titles:?}"
    );

    // **Die Gegenprobe ist hier die eigentliche Prüfung.** Zu streng gefiltert
    // kostet echte Angebote, und diese Seite trägt genau die Namen, an denen
    // eine zu breite Regel scheitern würde: einer beginnt mit „Mit" mitten im
    // Namen, einer trägt seine Typenbezeichnung statt einer Gattung.
    //
    // **„3 Kochtöpfe mit Deckel (ca. Ø 16/20/24 …)" stand bis 2026-08-01 in
    // dieser Liste und steht jetzt bewusst nicht mehr darin.** Die Zeile ist
    // die Inhaltsangabe des GSW-Kochtopf-Sets, gesetzt in 8,4 pt — und der
    // Preis, den sie trug, war nie ihrer: 2.99 € gehört der SILVERCREST
    // Schubladenmatte, die daneben steht und ihn seit der Schriftgrad-Schranke
    // (`MIN_NAME_PT`) selbst bekommt. Das Set selbst bleibt mit seinem eigenen
    // Preis in der Liste, eine Zeile darüber.
    for erwartet in [
        "GSW Edelstahl-Kochtopf-Set",
        "SILVERCREST Tritan-Trinkflasche Mit einsetzbarem Frucht",
        "SILVERCREST Schubladenmatte",
    ] {
        assert!(
            titles.iter().any(|t| t.starts_with(erwartet)),
            "{erwartet:?} verschluckt, übrig: {titles:?}"
        );
    }
}

/// Der Schriftgrad entscheidet, wer den Preis bekommt — nicht der Wortlaut.
///
/// Fixture ist die ungekürzte Seite 47 des Prospekts vom 27.07.2026, die
/// PARKSIDE-Sanitärseite. Sie ist der Fall, an dem jede Wortliste scheitert:
/// „Rohrschneider" ist die Beschriftung eines Fotos, „PARKSIDE Rohr-Werkzeug"
/// und „PARKSIDE Bolzenschneider" sind Artikel, und alle drei sehen im Text
/// gleich aus. Im Druck nicht: Die Beschriftung steht in 8,01 pt, die Namen in
/// 11,96 pt.
///
/// Bis 2026-08-01 nahm „Rohrschneider" dem Bolzenschneider seine 9.99 € weg —
/// der Artikel fiel ersatzlos aus der Liste, die Beschriftung stand drin.
#[test]
fn the_type_size_says_which_line_is_a_product_name() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_schriftgrad_s47.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );
    let paare: Vec<(&str, Option<f64>)> =
        offers.iter().map(|o| (o.title.as_str(), o.price)).collect();

    for beschriftung in ["Rohrschneider", "Farbdisplay Auflösung Speicher"] {
        assert!(
            !paare.iter().any(|(t, _)| *t == beschriftung),
            "Fotobeschriftung als Artikel: {paare:?}"
        );
    }
    // Der Preis ist nicht weg, er hängt jetzt am richtigen Namen.
    assert!(
        paare.contains(&("PARKSIDE Bolzenschneider", Some(9.99))),
        "der freigewordene Preis fand seinen Artikel nicht: {paare:?}"
    );

    // **Gegenprobe, und sie ist der eigentliche Zweck des Tests.** Auf
    // derselben Seite stehen zwei Artikel, deren Namen genauso kurz und
    // genauso werkzeughaft klingen wie die Beschriftungen oben. Sie stehen im
    // Namensgrad und müssen bleiben — sonst hat die Regel nur die Richtung
    // gewechselt, in die sie irrt.
    for artikel in [
        "PARKSIDE RohrWerkzeug",
        "PARKSIDE Santiär-Universal-Montageschlüssel",
        "PARKSIDE Rohrreinigungsspirale Zur Beseitigung von Rückständen",
    ] {
        assert!(
            paare.iter().any(|(t, _)| *t == artikel),
            "{artikel:?} verschluckt: {paare:?}"
        );
    }
}

/// Eine Preiskachel, die eine andere Kachel umschließt, ist noch kein Beleg,
/// dass ihr Preis dorthin gehört.
///
/// Fixture ist die ungekürzte Seite 60 des Prospekts vom 27.07.2026. Dort
/// umschließt die breite Kachel „CHEF SELECT Burger Scheiben" (11.49*) das
/// Rechteck von „EXQUISA Frischkäse" vollständig; dessen eigener Preis (1.11*)
/// steht 17 pt daneben. `Island::gap` ist bei Überschneidung 0, nach reinem
/// Abstand gewänne also die falsche Kachel — und der Frischkäse kostete
/// 11.49 €.
///
/// Entschieden wird über die Rechnung im **eigenen** Text: „Je 200/175 g,
/// 1 kg = 5.55/6.34" ergibt 1.11 €.
#[test]
fn an_enclosing_price_tile_does_not_outrank_the_products_own_price() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_umschlossen_s60.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );
    let paare: Vec<(&str, Option<f64>)> =
        offers.iter().map(|o| (o.title.as_str(), o.price)).collect();

    assert!(
        paare.contains(&("EXQUISA Frischkäse", Some(1.11))),
        "der Frischkäse hat nicht seinen eigenen Preis: {paare:?}"
    );
    assert!(
        paare.contains(&("CHEF SELECT Burger Scheiben", Some(11.49))),
        "die umschließende Kachel bekam ihren Preis nicht unter eigenem Namen: {paare:?}"
    );

    // Gegenprobe: Auf derselben Seite steht eine Kachel, die zwei Sternpreise
    // trägt und deren Rechnung aufgeht — sie darf die Regel nicht anfassen.
    assert_eq!(
        paare
            .iter()
            .filter(|(t, _)| t.starts_with("PAMPERS"))
            .count(),
        2,
        "eine richtig zusammengesetzte Mehrpreis-Kachel wurde mitgerissen: {paare:?}"
    );
}

#[test]
fn lidl_prospekt_marks_loyalty_prices_as_such() {
    let offers = scrapers::lidl_prospekt::extract_offers(
        include_str!("fixtures/lidl/prospekt_bbox_layout.xml"),
        "LIDL_1988",
        Some("2026-07-20"),
        Some("2026-07-25"),
    );
    // "Mit Lidl Plus"-Preise gibt es nur mit Kundenkarte. Sie wandern mit,
    // müssen aber gekennzeichnet sein, damit die Einkaufsplan-Karte nicht mit
    // einem Preis rechnet, den man an der Kasse ohne App nicht bekommt.
    let plus: Vec<_> =
        offers.iter().filter(|o| o.subtitle.as_deref().is_some_and(|s| s.contains("Lidl Plus"))).collect();
    assert!(!plus.is_empty(), "kein einziger Lidl-Plus-Preis gekennzeichnet");
}

#[test]
fn lidl_prospekt_reads_validity_and_regions_from_the_flyer_json() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/lidl/prospekt_flyer.json")).unwrap();
    let flyer = scrapers::lidl_prospekt::parse_flyer(&raw).unwrap();
    // Die Laufzeit steht im JSON, nicht im Bild.
    assert_eq!(flyer.offer_start_date.as_deref(), Some("2026-07-20"));
    assert_eq!(flyer.offer_end_date.as_deref(), Some("2026-07-25"));
    // Absatzregion: der Schlüssel, über den die Variante zur PLZ passt.
    assert_eq!(flyer.regions.iter().map(|r| r.code.as_str()).collect::<Vec<_>>(), vec!["440"]);
    assert!(flyer.pdf_url.as_deref().unwrap().ends_with(".pdf"));
}

#[test]
fn store_finder_reads_the_absatzregion_as_number_or_string() {
    let number = serde_json::json!({"d": {"results": [{"EntityID": "1988", "AR": 20}]}});
    assert_eq!(scrapers::store_finder::parse_region_code(&number).as_deref(), Some("20"));
    let text = serde_json::json!({"d": {"results": [{"EntityID": "1988", "AR": "446"}]}});
    assert_eq!(scrapers::store_finder::parse_region_code(&text).as_deref(), Some("446"));
    // Ohne AR bleibt es None — der Prospekt fällt dann auf eine
    // überregionale Variante zurück statt zu scheitern.
    let missing = serde_json::json!({"d": {"results": [{"EntityID": "1988"}]}});
    assert_eq!(scrapers::store_finder::parse_region_code(&missing), None);
    let empty = serde_json::json!({"d": {"results": []}});
    assert_eq!(scrapers::store_finder::parse_region_code(&empty), None);
}

// Die zehn Angebote des damaligen marktguru-Abgleichs, deren Preis nirgends
// im Prospekttext steht, waren am 2026-07-25 ausnahmslos Onlineshop-Möbel. Genau die stehen sauber
// strukturiert im `products`-Feld des Prospekt-JSON — inklusive Bild und
// Kategorie, die der PDF-Weg gar nicht liefern kann.
#[test]
fn lidl_prospekt_takes_online_shop_articles_from_the_flyer_json() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/lidl/prospekt_flyer.json")).unwrap();
    let flyer = scrapers::lidl_prospekt::parse_flyer(&raw).unwrap();
    let offers = scrapers::lidl_prospekt::products_as_offers(
        &flyer,
        "LIDL_1988",
        Some("2026-07-20"),
        Some("2026-07-25"),
    );

    let bettwaesche = offers.iter().find(|o| o.title.contains("Wendebettwäsche")).unwrap();
    // `price` kommt im JSON als String — muss trotzdem als Zahl ankommen.
    assert_eq!(bettwaesche.price, Some(9.99));
    // Vom Kategoriepfad bleibt das letzte, aussagekräftigste Glied.
    assert_eq!(bettwaesche.category.as_deref(), Some("Kinderbettwäsche"));
    assert_eq!(bettwaesche.images.len(), 1);
    assert_eq!(bettwaesche.subtitle.as_deref(), Some("Onlineshop"));

    // Beim Zusammenführen darf nichts doppelt gezählt werden.
    assert!(scrapers::lidl_prospekt::merge_products(&flyer, "LIDL_1988", &offers).is_empty());
}

// ------------------------------------------- Lidl-Prospekt: Kachelbilder

// Der Prospekt trägt seine Produktfotos nicht als benannte Objekte; die
// Textebene weiß nur, wo Wörter stehen. Das Foto ist der Platz *darüber*,
// nach oben begrenzt durch die nächste Kachel derselben Spalte. Diese
// Rechnung ist der ganze Unterschied zwischen „Lidl hat Bilder" und „Lidl
// zeigt Emojis", seit marktguru weg ist.
#[test]
fn lidl_prospekt_derives_a_photo_strip_above_each_tile() {
    let (offers, shots) = scrapers::lidl_prospekt::extract_offers_with_shots(
        include_str!("fixtures/lidl/prospekt_bbox_layout.xml"),
        "LIDL_1988",
        Some("2026-07-20"),
        Some("2026-07-25"),
    );
    assert!(!shots.is_empty(), "kein einziger Bildstreifen berechnet");

    // Jeder Streifen gehört zu einem Angebot, das es noch gibt — das Dedup
    // wirft Kacheln weg, und verwaiste Streifen würden Bilder für Angebote
    // rastern, die nie in der Datenbank landen.
    let ids: std::collections::HashSet<&str> = offers.iter().map(|o| o.id.as_str()).collect();
    for shot in &shots {
        assert!(
            ids.contains(shot.offer_id.as_str()),
            "Streifen ohne Angebot: {}",
            shot.offer_id
        );
    }

    // Höchstens einer je Angebot.
    let unique: std::collections::HashSet<&str> =
        shots.iter().map(|s| s.offer_id.as_str()).collect();
    assert_eq!(unique.len(), shots.len(), "doppelte Streifen je Angebot");

    // Nichts Flaches: Unter 25 pt ist es der Zeilenabstand zur Kachel darüber,
    // kein Foto. Gemessen an dieser Seite liegen die echten bei 85-111 pt.
    for shot in &shots {
        assert!(shot.h >= 25.0, "zu flacher Streifen: {:?}", shot);
        assert!(shot.w > 0.0 && shot.page >= 1);
        assert!(shot.y >= 0.0, "Streifen über dem Seitenrand: {:?}", shot);
    }

    // Und die Gegenprobe zur Messung: Für den Leerdammer, dessen Foto oben in
    // der Kachel steht, muss ein hoher Streifen herauskommen.
    let leerdammer = offers
        .iter()
        .find(|o| o.title.contains("LEERDAMMER"))
        .expect("Leerdammer fehlt");
    let shot = shots
        .iter()
        .find(|s| s.offer_id == leerdammer.id)
        .expect("kein Streifen für den Leerdammer");
    assert!(
        shot.h > 60.0,
        "Leerdammer-Streifen nur {} pt hoch — Foto vermutlich abgeschnitten",
        shot.h
    );
}

/// **Der Wackeltest, als Test.**
///
/// `lidl_prospekt_builds_tiles_from_word_coordinates` fiel in der Nacht vom
/// 2026-07-31 mal um und mal nicht — bei byte-gleichem Commit — mit „weicher
/// Trennstrich nicht zusammengezogen". Die Ursache lag nicht beim Trennstrich:
/// `cluster` gab seine Kacheln aus einer `HashMap` heraus, und die iteriert in
/// zufälliger Reihenfolge. Da `pairs.sort_by` stabil ist, entschied damit der
/// Zufall, welches Produkt zu welchem Preis gepaart wurde.
///
/// Ein Extraktor, der aus derselben Seite zweimal etwas anderes liest, ist
/// keine Quelle, auf die man einen Preisvergleich stellt.
#[test]
fn extraction_is_deterministic_across_runs() {
    let xml = include_str!("fixtures/lidl/prospekt_bbox_layout.xml");
    let first = scrapers::lidl_prospekt::extract_offers(xml, "LIDL_1988", Some("2026-07-20"), Some("2026-07-25"));
    assert!(!first.is_empty());

    // Jeder Durchlauf legt neue HashMaps an, und jede bekommt einen eigenen
    // Hash-Seed — zwanzig Läufe treffen die alte Reihenfolge zuverlässig.
    for run in 1..=20 {
        let again = scrapers::lidl_prospekt::extract_offers(xml, "LIDL_1988", Some("2026-07-20"), Some("2026-07-25"));
        assert_eq!(
            first.iter().map(|o| (&o.title, o.price)).collect::<Vec<_>>(),
            again.iter().map(|o| (&o.title, o.price)).collect::<Vec<_>>(),
            "Lauf {run} las etwas anderes aus derselben Seite"
        );
    }
}

/// Auch die Onlineshop-Artikel kommen aus einer `HashMap` — dieselbe Falle wie
/// bei den Prospektkacheln, nur eine Ebene weiter. Gemessen an zwei echten
/// Läufen hintereinander am 2026-07-31: dieselben 393 Angebote, andere
/// Reihenfolge, und jede Abweichung war eine Onlineshop-Zeile.
#[test]
fn online_shop_products_come_out_in_a_stable_order() {
    let raw: serde_json::Value = serde_json::from_str(
        r#"{"flyer":{"id":"f","name":"n","title":"t",
             "offerStartDate":"2026-07-27","offerEndDate":"2026-08-01",
             "pdfUrl":"https://example.invalid/x.pdf","fileSize":1,
             "regions":[],"pages":[{"id":"p1","number":1,"width":1415,"height":2400}],
             "products":{
               "c":{"title":"Gamma Artikel","price":"3.00","categoryPrimary":"K/Drei"},
               "a":{"title":"Alpha Artikel","price":"1.00","categoryPrimary":"K/Eins"},
               "b":{"title":"Beta Artikel","price":"2.00","categoryPrimary":"K/Zwei"}}}}"#,
    )
    .unwrap();
    // **Je Durchlauf neu geparst.** Dieselbe `HashMap` zweimal auszulesen
    // liefert immer dieselbe Reihenfolge — der Zufall steckt im Seed, den jede
    // neue Instanz bekommt. Ein Test, der das Flyer-Objekt wiederverwendet,
    // besteht auch ohne die Sortierung und prüft damit nichts.
    let parse = || scrapers::lidl_prospekt::parse_flyer(&raw).unwrap();

    let first = scrapers::lidl_prospekt::products_as_offers(
        &parse(), "LIDL_1988", Some("2026-07-27"), Some("2026-08-01"),
    );
    assert_eq!(first.len(), 3, "Fixture liefert drei Artikel");

    for run in 1..=20 {
        let again = scrapers::lidl_prospekt::products_as_offers(
            &parse(), "LIDL_1988", Some("2026-07-27"), Some("2026-08-01"),
        );
        assert_eq!(
            first.iter().map(|o| &o.title).collect::<Vec<_>>(),
            again.iter().map(|o| &o.title).collect::<Vec<_>>(),
            "Lauf {run} lieferte die Onlineshop-Artikel in anderer Reihenfolge"
        );
    }
}

// ------------------------------------- EDEKA: Markt-Redirect, blockiert vs. weg
//
// Der stille Verlustfall vom 2026-07-30: Der Markt-Redirect
// (/eh/<region>/<slug>/index.jsp -> /maerkte/<id>/) kam auf dem GitHub-Runner
// zweimal leer zurück, während dieselbe URL von einem Mac sofort mit 308
// antwortet — Runner-IP an Akamai, nicht der Umlaut im Slug. EDEKA Böse in
// Ahlbeck fehlte danach im Verzeichnis.
//
// Diese Tests laufen gegen einen lokalen Server, den echtes System-curl
// abfragt — dieselbe Kommandozeile wie in der Produktion, nur ein anderer
// Host. Geprüft wird das, woran der Fehler hing: dass „geblockt" und „gibt es
// nicht" auseinandergehalten werden.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Antwortet auf jede Anfrage mit `response` und zählt die Anfragen.
fn spawn_http(response: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits2 = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            hits2.fetch_add(1, Ordering::SeqCst);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}/eh/mv/edeka-boese/index.jsp"), hits)
}

/// Der Normalfall: 308 mit Location, ein Versuch, Ziel zurück.
#[test]
fn edeka_market_redirect_resolves_a_308() {
    let (url, hits) = spawn_http(
        "HTTP/1.1 308 Permanent Redirect\r\nLocation: https://www.edeka.de/maerkte/4030423/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );

    let target = lechariot::scrapers::util::curl_redirect_url_with_backoff(&url, &[], &[0, 0, 0])
        .expect("308 muss aufgelöst werden");

    assert_eq!(target, "https://www.edeka.de/maerkte/4030423/");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "ein Erfolg braucht keinen zweiten Versuch");
}

/// **Der gemeldete Fall.** Antwort ohne Location — hier als 403, der Runner sah
/// eine Akamai-Challenge. Der Lauf muss es erneut versuchen, und zwar begrenzt.
#[test]
fn a_blocked_market_redirect_is_retried_and_then_named() {
    let (url, hits) = spawn_http("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");

    let err = lechariot::scrapers::util::curl_redirect_url_with_backoff(&url, &[], &[0, 0, 0])
        .expect_err("eine Blockade darf nicht als Erfolg durchgehen");

    assert_eq!(hits.load(Ordering::SeqCst), 4, "einmal plus drei Wiederholungen");
    let msg = format!("{err:#}");
    assert!(msg.contains("403"), "der Status gehört in die Meldung: {msg}");
    assert!(msg.contains("Blockade"), "die Meldung muss die Blockade benennen: {msg}");
}

/// Akamai antwortet auf eine geblockte Anfrage auch mit **200** und einer
/// Challenge-Seite. Ohne Location ist das kein aufgelöster Redirect — sonst
/// stünde eine Filiale mit geratener ID im Verzeichnis, und eine falsche ID
/// ist schlimmer als keine.
#[test]
fn a_two_hundred_without_location_counts_as_blocked() {
    let (url, hits) =
        spawn_http("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi");

    let err = lechariot::scrapers::util::curl_redirect_url_with_backoff(&url, &[], &[0])
        .expect_err("200 ohne Location ist kein Redirect-Ziel");

    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(format!("{err:#}").contains("200"), "{err:#}");
}

/// **Und die Gegenprobe, damit der Wiederholungsversuch keinen echten Fehler
/// zudeckt:** Eine Marktseite, die es nicht gibt, wird genau EINMAL gefragt.
/// Vor dieser Runde kostete jeder tote Markt drei Anfragen und sechs Sekunden
/// und meldete danach dasselbe wie eine Blockade.
#[test]
fn a_genuine_404_is_not_retried_and_says_so() {
    let (url, hits) = spawn_http("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");

    let err = lechariot::scrapers::util::curl_redirect_url_with_backoff(&url, &[], &[0, 0, 0])
        .expect_err("404 ist ein Fehler, nur kein wiederholbarer");

    assert_eq!(hits.load(Ordering::SeqCst), 1, "ein 404 wird nicht wiederholt");
    let msg = format!("{err:#}");
    assert!(msg.contains("404"), "{msg}");
    assert!(
        msg.contains("gibt es nicht"),
        "die Meldung muss den echten Fehler beim Namen nennen: {msg}"
    );
    assert!(!msg.contains("Blockade"), "ein 404 ist keine Blockade: {msg}");
}

/// Die Einteilung ohne Netz, Fall für Fall — das ist die Entscheidung, an der
/// alles hängt.
#[test]
fn the_redirect_verdict_separates_blocked_from_gone() {
    use lechariot::scrapers::util::{RedirectOutcome, redirect_outcome};

    assert_eq!(
        redirect_outcome("308", "https://www.edeka.de/maerkte/4030423/"),
        RedirectOutcome::Target("https://www.edeka.de/maerkte/4030423/".into())
    );
    // Ein Ziel gilt, egal mit welchem Status es kam.
    assert!(matches!(redirect_outcome("301", "https://x/"), RedirectOutcome::Target(_)));

    for status in ["404", "410"] {
        assert!(
            matches!(redirect_outcome(status, ""), RedirectOutcome::Gone(_)),
            "{status} muss als „gibt es nicht“ gelten"
        );
    }
    // 000 = curl kam nicht durch, 200 = Challenge-Seite, 429/5xx = zu viel.
    for status in ["000", "200", "403", "429", "500", "503"] {
        assert!(
            matches!(redirect_outcome(status, ""), RedirectOutcome::Blocked(_)),
            "{status} muss wiederholt werden"
        );
    }
}

/// Die echten Pausen bleiben begrenzt: Der Redirect fällt je Filiale an.
#[test]
fn the_backoff_stays_bounded() {
    let total: u64 = lechariot::scrapers::util::REDIRECT_BACKOFF_MS.iter().sum();
    assert!(total <= 13_000, "{total} ms Wartezeit je Filiale ist zu viel");
    assert!(
        lechariot::scrapers::util::REDIRECT_BACKOFF_MS.windows(2).all(|w| w[1] > w[0]),
        "die Pausen müssen wachsen"
    );
}

// ---------------------------------------------------------------- NORMA

#[test]
fn norma_index_lists_every_theme_page_once() {
    let themes =
        scrapers::norma::parse_index(include_str!("fixtures/norma/angebote_index.html"));

    assert_eq!(
        themes,
        vec![
            "/de/angebote/ab-freitag,-31.07.26/wochenend-spezial-t-378267/",
            "/de/angebote/ab-montag,-03.08.26/mehr-fuers-geld-t-379179/",
            "/de/angebote/ab-montag,-03.08.26/unser-obst-und-gemuese-t-379181/",
        ],
        "Kampagnen-Query, Artikelseite, reiner Termin und /at/ gehören nicht dazu"
    );
}

#[test]
fn norma_theme_parses_tiles_with_prices_and_strike_prices() {
    let mut offers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    scrapers::norma::parse_theme(
        include_str!("fixtures/norma/thema_mehr_fuers_geld.html"),
        "/de/angebote/ab-montag,-03.08.26/mehr-fuers-geld-t-379179/",
        "NORMA_DE",
        &mut offers,
        &mut seen,
    );
    assert_eq!(offers.len(), 6);

    // Termin und Thema stehen nur im Pfad — beides muss an jedem Angebot hängen.
    for o in &offers {
        assert_eq!(o.valid_from.as_deref(), Some("2026-08-03"));
        assert_eq!(o.category.as_deref(), Some("Mehr fürs Geld."));
        // Laufzeit abgeleitet (7 Tage, siehe norma::TERM_DAYS) — die App liest
        // valid_until als nicht-optionales Date, null bricht das Decoding.
        assert_eq!(o.valid_until.as_deref(), Some("2026-08-09"));
    }

    // Marke vor den Produktnamen; Preis aus dem aria-label, nicht aus dem
    // sichtbaren "–,99"; "statt 1,59" ist der Streichpreis; das Pfand fliegt
    // aus der Mengenangabe, damit sie als reine Menge durchgeht.
    let cola = &offers[0];
    assert_eq!(cola.title, "Coca-Cola/ Fanta/ Sprite/ MezzoMix Erfrischungsgetränk");
    assert_eq!(cola.price, Some(0.99));
    assert_eq!(cola.regular_price, Some(1.59));
    assert_eq!(cola.subtitle.as_deref(), Some("je 1,25 l"));
    assert_eq!(cola.overline.as_deref(), Some("Aus unserem Sortiment (1 l = –,79)"));
    assert_eq!(
        cola.images,
        vec![
            "https://www.norma-online.de/ext/img/product/angebote/26_08_03/900_erfrischungsgetraenk_wo.png"
        ]
    );

    // Leere <strong class="supplier"> -> kein Markenpräfix; "UVP 2,49" zählt
    // genauso als Streichpreis wie "statt".
    let waffeln = offers.iter().find(|o| o.title == "Kakaowaffeln").unwrap();
    assert_eq!(waffeln.price, Some(1.99));
    assert_eq!(waffeln.regular_price, Some(2.49));
    assert_eq!(waffeln.subtitle.as_deref(), Some("je 225 g"));

    // …-uvp-spacer ist eine andere Klasse als …-uvp: kein Streichpreis.
    let kekse = offers.iter().find(|o| o.title == "Butterkekse").unwrap();
    assert_eq!(kekse.price, Some(4.99));
    assert_eq!(kekse.regular_price, None);

    // …-uvp-info dagegen trägt dieselbe Klasse *zusätzlich*. Dort steht das
    // Rabattband, kein früherer Preis — sonst wären aus "25% billiger"
    // 25,00 € Streichpreis geworden.
    let maggi = offers.iter().find(|o| o.title == "Maggi-Produkte").unwrap();
    assert_eq!(maggi.price, None);
    assert_eq!(maggi.regular_price, None);
}

/// Der Fall aus der Produktion: „NORMA 4,99 statt 909 — Gut Bartenhof Dicke
/// Rippe". Über dem Preis stand kein Streichpreis, sondern das Beispielgewicht
/// „z.B. 909 g" in einer `…-uvp-info`-Zeile. Ohne Streichpreis fehlt der App
/// nichts; mit einem falschen malt sie ein Rabatt-Abzeichen über Schweinefleisch.
#[test]
fn norma_hint_line_above_the_price_is_no_strike_price() {
    let mut offers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    scrapers::norma::parse_theme(
        include_str!("fixtures/norma/thema_wochenend_spezial.html"),
        "/de/angebote/ab-freitag,-07.08.26/wochenend-spezial-t-379197/",
        "NORMA_DE",
        &mut offers,
        &mut seen,
    );
    assert_eq!(offers.len(), 3);

    let rippe = offers.iter().find(|o| o.title == "Gut Bartenhof Dicke Rippe").unwrap();
    assert_eq!(rippe.price, Some(4.99));
    assert_eq!(rippe.regular_price, None, "'z.B. 909 g' ist ein Gewicht, kein Preis");

    // Dieselbe Zeile ohne Ziffern — war schon vorher harmlos, muss es bleiben.
    let pfirsich = offers.iter().find(|o| o.title == "Super Nivo Plattpfirsiche").unwrap();
    assert_eq!(pfirsich.price, None);
    assert_eq!(pfirsich.regular_price, None);

    // Gegenprobe auf derselben Seite: die reine …-uvp-Zeile bleibt ein
    // Streichpreis. Von 87 NORMA-Zeilen mit Streichpreis waren 85 dieser Art.
    let hering = offers.iter().find(|o| o.title == "Appel Herings-Filets").unwrap();
    assert_eq!(hering.price, Some(1.11));
    assert_eq!(hering.regular_price, Some(1.99), "'UVP = 1,99' ist einer");
}

/// Der zweite Fall aus der Produktion: „NORMA 0,79 statt 99 — San Benedetto
/// Premiumlimonaden ZERO". Unter einem Euro druckt NORMA einen Gedankenstrich
/// statt der führenden Null („UVP –,99"), und anders als beim Preis selbst
/// steht in der UVP-Zeile kein aria-label daneben, das man stattdessen lesen
/// könnte.
#[test]
fn norma_dash_in_the_strike_price_is_a_leading_zero() {
    let mut offers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    scrapers::norma::parse_theme(
        include_str!("fixtures/norma/thema_sommergenuss.html"),
        "/de/angebote/ab-montag,-03.08.26/sommergenuss-t-379188/",
        "NORMA_DE",
        &mut offers,
        &mut seen,
    );
    assert_eq!(offers.len(), 3);

    let limo = offers.iter().find(|o| o.title.starts_with("San Benedetto")).unwrap();
    assert_eq!(limo.price, Some(0.79));
    assert_eq!(limo.regular_price, Some(0.99), "'UVP –,99' sind 99 Cent, nicht 99 Euro");

    // Gegenproben: ausgeschriebene Streichpreise beider Schreibweisen.
    let kaese = offers.iter().find(|o| o.title == "Fol Epi Käsescheiben").unwrap();
    assert_eq!(kaese.price, Some(1.39));
    assert_eq!(kaese.regular_price, Some(2.69));

    let becher = offers.iter().find(|o| o.title.starts_with("Paw Patrol")).unwrap();
    assert_eq!(becher.price, Some(3.99));
    assert_eq!(becher.regular_price, Some(4.99));
}

#[test]
fn norma_same_product_from_two_brands_keeps_both() {
    let mut offers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    scrapers::norma::parse_theme(
        include_str!("fixtures/norma/thema_mehr_fuers_geld.html"),
        "/de/angebote/ab-montag,-03.08.26/mehr-fuers-geld-t-379179/",
        "NORMA_DE",
        &mut offers,
        &mut seen,
    );

    // Der eigentliche Grund, warum die Marke im Titel steht: Sheba und
    // Whiskas verkaufen im selben Prospekt beide "Katzennassnahrung". Ohne
    // Marke fielen beide auf dieselbe Offer-ID und eines verschwände still.
    let katze: Vec<_> = offers.iter().filter(|o| o.title.contains("Katzennassnahrung")).collect();
    assert_eq!(katze.len(), 2, "beide Marken müssen überleben");
    assert_eq!(katze[0].title, "Sheba Katzennassnahrung");
    assert_eq!(katze[1].title, "Whiskas Katzennassnahrung");
    assert_ne!(katze[0].id, katze[1].id);
    assert_eq!(katze[0].price, Some(15.99));
    assert_eq!(katze[1].price, Some(11.99));
    // "UVP = 13,95" — das Gleichheitszeichen darf den Parser nicht stören.
    assert_eq!(katze[1].regular_price, Some(13.95));
}

/// Die Filialdaten stehen als URL-kodiertes JSON im showMap-Link — die
/// belastbarere Quelle als die Kachel-Verschachtelung daneben.
#[test]
fn norma_store_finder_reads_the_showmap_payload() {
    let stores = scrapers::store_finder::parse_norma(
        include_str!("fixtures/norma/filialfinder_01219.html"),
        50.0,
    );
    assert_eq!(stores.len(), 3);
    // Nach Entfernung sortiert, nicht nach der Reihenfolge im HTML.
    assert_eq!(stores[0].id, "2131");
    // "+" ist ein Leerzeichen, %5Cu00df wird zum ß der JSON-Ebene.
    assert_eq!(stores[0].street.as_deref(), Some("Paradiesstraße 40"));
    assert_eq!(stores[0].plz.as_deref(), Some("01217"));
    assert_eq!(stores[0].city.as_deref(), Some("Dresden"));
    assert_eq!(stores[0].lat, Some(51.02408));
    assert_eq!(stores[0].lon, Some(13.74388));

    let market = stores.into_iter().next().unwrap().into_market();
    assert_eq!(market.id, "NORMA_2131");
    assert_eq!(market.name, "NORMA Dresden");
}

/// Jede Zeile im Verzeichnis braucht Koordinaten und Adresse, sonst kann die
/// App sie weder auf die Karte noch in die Auswahlliste setzen.
#[test]
fn norma_store_finder_fills_the_directory_row() {
    let branch = scrapers::store_finder::parse_norma(
        include_str!("fixtures/norma/filialfinder_01219.html"),
        50.0,
    )
    .into_iter()
    .next()
    .unwrap()
    .into_branch();

    assert_eq!(branch.market_id, "NORMA_2131");
    // Dieselbe Schreibweise wie `Store::Norma.chain()` — sonst findet die App
    // die Angebote der Kette nicht zu ihrer Filiale.
    assert_eq!(branch.chain, "NORMA");
    assert_eq!(branch.name, "NORMA Dresden");
    assert_eq!(branch.street.as_deref(), Some("Paradiesstraße 40"));
    assert_eq!(branch.plz.as_deref(), Some("01217"));
    assert_eq!(branch.lat, Some(51.02408));
    assert_eq!(branch.lon, Some(13.74388));
    assert_eq!(branch.source, "norma-filialfinder");
}

#[test]
fn norma_store_finder_drops_branches_beyond_the_radius() {
    // Dieselbe Antwort, nur mit dem Umkreis der Gebietssuche: Großharthau
    // liegt 25.547 m entfernt und fällt bei 25 km heraus. Der Server hält den
    // Radius zwar ein — aber `geoDistance` steht an jeder Filiale, und der
    // Tag, an dem er es nicht mehr tut, darf keine Filiale aus dem
    // Nachbarkreis ins Verzeichnis spülen.
    let stores = scrapers::store_finder::parse_norma(
        include_str!("fixtures/norma/filialfinder_01219.html"),
        lechariot::branches::AREA_RADIUS_KM,
    );
    assert_eq!(stores.len(), 2);
    assert!(stores.iter().all(|s| s.id != "2411"));
}

// --------------------------------------- Lidl-Prospekt: Streichpreis-Bilanz

/// Die Zahl, die sonst niemand kennt: **wie viel von dem, was der Prospekt
/// druckt, überhaupt ankommt.**
///
/// `regular_price_audit` ist das Messgeschirr hinter
/// `examples/streichpreis_probe` und die Grundlage der Übersicht in
/// `docs/scrapers/README.md`. Ohne Test wäre es eine Zahl, der man glauben
/// muss — und die still auf null fällt, wenn jemand `regular_candidates`
/// umbaut.
///
/// Die Fixture-Seite trägt zehn Streichpreis-Angaben, alle an einer
/// verwertbaren Kachel, und acht davon werden zugeteilt. Die zwei
/// verbleibenden sind **kein Verlust, sondern die Regel aus #40**: Eine
/// Plakette, die rechnerisch auf zwei Preise derselben Kachel passt, bekommt
/// keiner.
#[test]
fn lidl_regular_price_audit_counts_source_against_result() {
    let xml = include_str!("fixtures/lidl/prospekt_bbox_layout.xml");
    let audit = scrapers::lidl_prospekt::regular_price_audit(
        xml,
        "LIDL_1988",
        Some("2026-07-20"),
        Some("2026-07-25"),
    );

    assert_eq!(audit.offers, 11, "Angebote der Fixture-Seite");
    assert_eq!(audit.printed, 10, "gedruckte Streichpreis-Angaben");
    assert_eq!(audit.owned, 10, "davon an einer verwertbaren Kachel");
    assert_eq!(audit.assigned, 8, "zugeteilt");
    assert_eq!(audit.samples.len(), 10, "je Fund eine Textinsel");

    // Beide Formen müssen im Nenner stehen — die Plakette mit Stichwort und
    // die nackte mit Rabatt. Zählte nur eine, sähe die Quote je nach Prospekt
    // besser aus, als sie ist.
    let mit_stichwort = audit
        .samples
        .iter()
        .filter(|t| t.contains("UVP") || t.contains("Normalpreis"))
        .count();
    assert_eq!(mit_stichwort, 8);
    assert_eq!(audit.samples.len() - mit_stichwort, 2, "Plaketten ohne Stichwort");

    // Und die Gegenprobe zur Zahl: Was zugeteilt wurde, steht auch am
    // Angebot, und immer über dem Angebotspreis.
    let offers = scrapers::lidl_prospekt::extract_offers(
        xml,
        "LIDL_1988",
        Some("2026-07-20"),
        Some("2026-07-25"),
    );
    let mit_streichpreis: Vec<_> = offers.iter().filter(|o| o.regular_price.is_some()).collect();
    assert_eq!(mit_streichpreis.len(), audit.assigned);
    for offer in mit_streichpreis {
        let (preis, alt) = (offer.price.expect("Preis"), offer.regular_price.expect("alt"));
        assert!(alt > preis, "{}: {alt} ist kein alter Preis zu {preis}", offer.title);
    }
}

/// Die Verlustrechnung verbucht jede gedruckte Angabe — und zwar genau einmal.
///
/// `printed` gegen `owned` sagt nur, **dass** die Hälfte der gedruckten
/// Streichpreise nie an einer Kachel ankommt, nicht warum. Die Aufteilung
/// nach Grund ist die Antwort, und sie taugt nur, wenn sie vollständig ist:
/// Eine Klasse, die still ein Drittel verschluckt, verschiebt die
/// Rangfolge — und danach wird entschieden, wo gebaut wird.
#[test]
fn every_printed_crossed_out_price_gets_a_reason() {
    let audit = scrapers::lidl_prospekt::regular_price_audit(
        include_str!("fixtures/lidl/prospekt_streichpreis.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );
    assert!(audit.printed > 0, "Fixture ohne Streichpreise");
    assert_eq!(
        audit.losses.iter().map(|l| l.count).sum::<usize>(),
        audit.printed,
        "die Gründe summieren sich nicht auf die gedruckten Angaben: {:?}",
        audit
            .losses
            .iter()
            .map(|l| (&l.reason, l.count))
            .collect::<Vec<_>>()
    );
    // Jede Klasse trägt ihre Belege, sonst lässt sich die Zahl nicht ansehen.
    for l in &audit.losses {
        assert!(!l.islands.is_empty(), "{} ohne Beleg", l.reason);
    }
}

/// Dieselbe Textebene, dieselbe Tabelle — dieselbe Forderung wie an den
/// Extraktor selbst (#69).
///
/// Die Verlustrechnung geht über `cluster`, und dort hing schon einmal eine
/// `HashMap` mit eigenem Hash-Seed. Eine Messung, die bei byte-gleicher
/// Eingabe mal so und mal so ausfällt, taugt nicht als Entscheidungsgrundlage.
#[test]
fn the_loss_table_is_the_same_on_every_run() {
    let xml = include_str!("fixtures/lidl/prospekt_streichpreis.xml");
    let tabelle = || {
        scrapers::lidl_prospekt::regular_price_audit(
            xml,
            "LIDL_1988",
            Some("2026-07-27"),
            Some("2026-08-01"),
        )
        .losses
        .iter()
        .map(|l| (l.reason.clone(), l.count))
        .collect::<Vec<_>>()
    };
    let first = tabelle();
    assert!(!first.is_empty());
    for run in 1..=20 {
        assert_eq!(first, tabelle(), "Lauf {run} zählte anders");
    }
}

/// Die Seiten ohne Sternpreis sind ein eigener Befund, kein misslungenes Paar.
///
/// Gemessen am Prospekt vom 10.08.2026: 85 der 101 nicht angekommenen
/// Streichpreis-Angaben stehen auf 18 Seiten, auf denen **kein einziger**
/// Sternpreis steht. Lidl setzt den Preis im Onlineshop-Teil ohne ihn
/// („39.99" statt „39.99*"), und der Stern ist der einzige Anker des
/// Extraktors. Diese Seiten liefern deshalb gar kein Angebot — sie sind kein
/// Streichpreis-Problem, sondern eine Abdeckungsfrage.
#[test]
fn a_page_without_a_star_price_is_counted_as_its_own_reason() {
    let audit = scrapers::lidl_prospekt::regular_price_audit(
        include_str!("fixtures/lidl/prospekt_streichpreis.xml"),
        "LIDL_1988",
        Some("2026-07-27"),
        Some("2026-08-01"),
    );
    // Die Fixture ist eine Sternpreis-Seite: Dort darf die Klasse nicht
    // auftauchen, sonst zählt sie etwas anderes als ihren Namen.
    assert!(
        !audit
            .losses
            .iter()
            .any(|l| l.reason == "Seite ohne jeden Sternpreis"),
        "Sternpreis-Seite als sternlos verbucht"
    );
}
