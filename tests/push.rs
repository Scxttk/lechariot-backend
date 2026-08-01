//! Tests für den Supabase-Push: Mapping (Offer -> SupabaseRow) und die
//! HTTP-Schicht gegen einen handgerollten Mock-Server (kein Live-Netzwerk).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use lechariot::models::{Market, Offer};
use lechariot::push::{self, PushConfig, PushOptions, SupabaseRow, chain_for, dedupe_rows, map_offer};
use lechariot::{db, storage, units};

fn offer(title: &str, price: Option<f64>) -> Offer {
    Offer {
        id: Offer::build_id("m1", title, Some("2026-07-13")),
        market_id: "m1".to_string(),
        title: title.to_string(),
        subtitle: None,
        overline: None,
        price,
        regular_price: None,
        category: Some("Molkerei".to_string()),
        nutri_score: None,
        valid_from: Some("2026-07-13".to_string()),
        valid_until: Some("2026-07-19".to_string()),
        images: vec![],
        biozid: false,
        flyer_page: None,
    }
}

// ---------------------------------------------------------------- Mapping

#[test]
fn map_basic_fields() {
    let mut o = offer("Gouda", Some(1.99));
    o.regular_price = Some(2.79);
    let row = map_offer(&o, "REWE", false).unwrap();
    assert_eq!(row.market, "REWE");
    assert_eq!(row.product, "Gouda");
    assert_eq!(row.price, 1.99);
    assert_eq!(row.regular_price, Some(2.79));
    assert_eq!(row.unit, "Stück");
    // Rohkategorie "Molkerei" wird normalisiert, Emoji aus der Keyword-Tabelle
    assert_eq!(row.category.as_deref(), Some("Molkerei & Eier"));
    assert_eq!(row.emoji.as_deref(), Some("🧀"));
    assert_eq!(row.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(row.valid_until.as_deref(), Some("2026-07-19"));
    assert_eq!(row.brand, None);
    assert_eq!(row.ean, None);
    assert_eq!(row.source, "lechariot-rust");
    assert!(!row.nationwide);
}

#[test]
fn map_takes_first_image_and_keeps_emoji_fallback() {
    // Mit Bild: erste nicht-leere URL landet in image_url, Emoji bleibt gesetzt.
    let mut o = offer("Gouda", Some(1.99));
    o.images = vec![
        "".to_string(),
        "https://cdn.example/gouda-450.jpg".to_string(),
        "https://cdn.example/gouda-900.jpg".to_string(),
    ];
    let row = map_offer(&o, "REWE", false).unwrap();
    assert_eq!(row.image_url.as_deref(), Some("https://cdn.example/gouda-450.jpg"));
    assert_eq!(row.emoji.as_deref(), Some("🧀"));

    // Ohne Bild: image_url None, Emoji trägt weiterhin die Anzeige.
    let row = map_offer(&offer("Gouda", Some(1.99)), "REWE", false).unwrap();
    assert_eq!(row.image_url, None);
    assert_eq!(row.emoji.as_deref(), Some("🧀"));
}

#[test]
fn map_skips_offers_without_price() {
    assert!(map_offer(&offer("Gouda", None), "REWE", false).is_none());
}

/// Ein Streichpreis, der keine Ersparnis beschreiben *kann*, fliegt raus —
/// das Angebot bleibt. Die Zahlen unten sind gemessen: alle 3732
/// Produktionszeilen mit Preis und Streichpreis am 2026-07-31.
#[test]
fn map_drops_a_strike_price_that_is_no_saving() {
    let with_regular = |price: f64, regular: f64| {
        let mut o = offer("Gouda", Some(price));
        o.regular_price = Some(regular);
        map_offer(&o, "REWE", false).unwrap()
    };

    // Die beiden NORMA-Lesefehler, die den Ausschlag gaben: 182× und 125×
    // über dem Angebotspreis.
    assert_eq!(with_regular(4.99, 909.0).regular_price, None);
    assert_eq!(with_regular(0.79, 99.0).regular_price, None);
    // Unter dem Angebotspreis (NORMA las "z.B. 1,1 kg" als 1,10 €).
    assert_eq!(with_regular(5.99, 1.10).regular_price, None);
    // Gleich dem Angebotspreis — 0 % Rabatt ist kein Rabatt. Stand so als
    // "59,99 statt 59,99" bei Kaufland in der Datenbank.
    assert_eq!(with_regular(59.99, 59.99).regular_price, None);

    // …und die echten steilen Rabatte überleben. Das sind die beiden
    // steilsten der gesamten Produktion, beide aus den APIs der Ketten
    // selbst: Penny 6,0× und Kaufland 5,0×.
    assert_eq!(with_regular(4.00, 24.00).regular_price, Some(24.00));
    assert_eq!(with_regular(9.99, 49.99).regular_price, Some(49.99));
    // Der Preis bleibt in jedem Fall stehen, auch wenn der Streichpreis fällt.
    assert_eq!(with_regular(4.99, 909.0).price, 4.99);
}

#[test]
fn map_appends_informative_subtitle() {
    // Kaufland-Stil: Marke im Titel, Produkt im Untertitel
    let mut o = offer("K-Classic", Some(0.99));
    o.subtitle = Some("H-Milch 3,5%".to_string());
    let row = map_offer(&o, "Kaufland", false).unwrap();
    assert_eq!(row.product, "K-Classic H-Milch 3,5%");
}

#[test]
fn map_drops_pure_quantity_subtitle() {
    let mut o = offer("Gouda", Some(1.99));
    o.subtitle = Some("je 250-g-Packg.".to_string());
    let row = map_offer(&o, "REWE", false).unwrap();
    assert_eq!(row.product, "Gouda");
    // …aber die Menge landet im unit-Feld statt "Stück"
    assert_eq!(row.unit, "je 250-g-Packg.");
}

#[test]
fn map_puts_multipack_quantity_into_unit() {
    let mut o = offer("MILPRIMA Haltbare fettarme Milch", Some(7.8));
    o.subtitle = Some("je 12 x 1 l".to_string());
    let row = map_offer(&o, "Penny", false).unwrap();
    assert_eq!(row.unit, "je 12 x 1 l");
    assert_eq!(row.base_unit.as_deref(), Some("l"));
    assert_eq!(row.base_price, Some(0.65));
}

#[test]
fn map_keeps_subtitle_with_product_name_and_quantity() {
    // Kaufland-Stil: Untertitel trägt Produktname UND Menge — muss erhalten
    // bleiben, sonst kollabieren alle Angebote einer Marke beim Dedupe
    let mut o = offer("K-Classic", Some(0.99));
    o.subtitle = Some("Rispentomaten, 500-g-Schale".to_string());
    let row = map_offer(&o, "Kaufland", false).unwrap();
    assert_eq!(row.product, "K-Classic Rispentomaten, 500-g-Schale");
}

#[test]
fn map_computes_base_price_from_quantity() {
    let mut o = offer("Wein", Some(3.29));
    o.subtitle = Some("0.75 l".to_string());
    let row = map_offer(&o, "REWE", false).unwrap();
    assert_eq!(row.base_unit.as_deref(), Some("l"));
    // 3.29 / 0.75 = 4.386..., auf Cent gerundet
    assert_eq!(row.base_price, Some(4.39));
}

#[test]
fn map_prefers_explicit_base_price() {
    let mut o = offer("Butterkäse", Some(1.79));
    o.overline = Some("je 650-g-Packg. (1 kg = 2.76)".to_string());
    let row = map_offer(&o, "Penny", false).unwrap();
    assert_eq!(row.base_price, Some(2.76));
    assert_eq!(row.base_unit.as_deref(), Some("kg"));
}

#[test]
fn map_serializes_to_expected_json() {
    let mut o = offer("Gouda", Some(1.99));
    o.images = vec!["https://cdn.example/gouda-450.jpg".to_string()];
    let row = map_offer(&o, "REWE", false).unwrap();
    let v = serde_json::to_value(&row).unwrap();
    assert_eq!(v["market"], "REWE");
    assert_eq!(v["price"], 1.99);
    assert_eq!(v["emoji"], "🧀");
    assert_eq!(v["image_url"], "https://cdn.example/gouda-450.jpg");
    assert_eq!(v["source"], "lechariot-rust");
    assert_eq!(v["nationwide"], false);
}

#[test]
fn dedupe_on_conflict_key() {
    let a = map_offer(&offer("Gouda", Some(1.99)), "REWE", false).unwrap();
    let b = map_offer(&offer("Gouda", Some(2.49)), "REWE", false).unwrap();
    let mut c = a.clone();
    c.nationwide = true;
    let rows = dedupe_rows(vec![a.clone(), b, c]);
    // Gleicher Schlüssel (market_id, product, valid_from): der erste gewinnt.
    // `nationwide` gehört seit Migration v16 NICHT zum Schlüssel — dieselbe
    // Filiale kann ein Produkt nicht gleichzeitig lokal und bundesweit führen,
    // und der Unique-Index in der Datenbank ließe das auch nicht zu.
    assert_eq!(rows, vec![a]);
}

/// Fenster mit Start/Ende auf dem Standard-Angebot.
fn windowed(product: &str, price: f64, from: &str, until: &str) -> SupabaseRow {
    let mut row = map_offer(&offer(product, Some(price)), "Kaufland", false).unwrap();
    row.valid_from = Some(from.to_string());
    row.valid_until = Some(until.to_string());
    row
}

/// **Der Kaufland-Fall** (gemessen 2026-07-31, 523 Kombinationen, z. B.
/// „ACTIVE O2 Erfrischungsgetränk": Woche 23.–29.07. plus Aktionstag 25.07.,
/// beide 0,99 €): Ein verschobener Starttag zum selben Preis ist dasselbe
/// Angebot — EINE Zeile, mit frühestem Start und spätestem Ende.
///
/// Vorher legte der Upsert-Schlüssel (market_id, product, valid_from) dafür
/// zwei Zeilen an, und die App zeigte das Produkt doppelt.
#[test]
fn a_shifted_start_at_the_same_price_is_the_same_offer() {
    let week = windowed("ACTIVE O2 Erfrischungsgetränk", 0.99, "2026-07-23", "2026-07-29");
    let day = windowed("ACTIVE O2 Erfrischungsgetränk", 0.99, "2026-07-25", "2026-07-25");
    let rows = dedupe_rows(vec![week.clone(), day]);
    assert_eq!(rows.len(), 1, "verschobener Start zum selben Preis: eine Zeile");
    assert_eq!(rows[0].valid_from.as_deref(), Some("2026-07-23"));
    assert_eq!(rows[0].valid_until.as_deref(), Some("2026-07-29"));

    // Auch wenn das zweite Fenster über das erste hinausreicht, entsteht das
    // Gesamtfenster — nicht das eine oder das andere.
    let a = windowed("Gouda", 1.99, "2026-07-27", "2026-08-01");
    let b = windowed("Gouda", 1.99, "2026-07-31", "2026-08-02");
    let rows = dedupe_rows(vec![a, b]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].valid_from.as_deref(), Some("2026-07-27"));
    assert_eq!(rows[0].valid_until.as_deref(), Some("2026-08-02"));
}

/// **Disjunkte Fenster bleiben zwei Zeilen** — Penny und Lidl pushen die
/// Folgewoche mit (479 Kombinationen am 2026-07-31, z. B. „CHOCO'LA
/// Schokolade" 27.07.–02.08. und 03.08.–09.08.). `valid_from` einfach aus
/// dem Schlüssel zu nehmen hätte genau diese echten Wochen verschmolzen —
/// der Grund, warum es im Schlüssel steht und bleibt.
#[test]
fn disjoint_weeks_stay_two_rows() {
    let this_week = windowed("CHOCO'LA Schokolade", 0.69, "2026-07-27", "2026-08-02");
    let next_week = windowed("CHOCO'LA Schokolade", 0.69, "2026-08-03", "2026-08-09");
    let rows = dedupe_rows(vec![this_week.clone(), next_week.clone()]);
    assert_eq!(rows, vec![this_week, next_week]);
}

/// **Und zwei künftige Wochen bleiben ebenfalls getrennt.** Der Fall, den erst
/// die Vorschau erzeugt: Lidls Übersicht führte am 01.08. *zwei* künftige
/// Prospekte (03.–08.08. und 10.–15.08.), und ein Produkt kann in beiden zum
/// selben Preis stehen. Verschmölzen sie, verlöre die Vorschau eine ganze
/// Woche — dieselbe Regel wie oben, nur ohne laufende Zeile als Anker.
///
/// Drei disjunkte Fenster desselben Produkts zum selben Preis müssen **drei**
/// Zeilen bleiben.
#[test]
fn two_future_weeks_stay_separate_rows() {
    let laufend = windowed("MILBONA Butter", 1.99, "2026-07-27", "2026-08-01");
    let naechste = windowed("MILBONA Butter", 1.99, "2026-08-03", "2026-08-08");
    let uebernaechste = windowed("MILBONA Butter", 1.99, "2026-08-10", "2026-08-15");

    let rows = dedupe_rows(vec![laufend.clone(), naechste.clone(), uebernaechste.clone()]);

    assert_eq!(rows, vec![laufend, naechste, uebernaechste]);

    // Die Gegenprobe im selben Test: Berühren sich zwei Zukunftsfenster, sind
    // sie **ein** Angebot — sonst hätte die Regel gar keine Kante.
    let a = windowed("MILBONA Butter", 1.99, "2026-08-03", "2026-08-08");
    let b = windowed("MILBONA Butter", 1.99, "2026-08-06", "2026-08-12");
    let merged = dedupe_rows(vec![a, b]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].valid_from.as_deref(), Some("2026-08-03"));
    assert_eq!(merged[0].valid_until.as_deref(), Some("2026-08-12"));
}

/// **Überlappende Fenster mit verschiedenem Preis bleiben zwei Zeilen** —
/// das sind echte verschiedene Angebote: Aktionstage mit eigenem Preis und
/// Lidl-Sammeltitel wie „Bio-Käse", hinter denen verschiedene Artikel
/// stecken (77 Kombinationen am 2026-07-31, z. B. „Bio-Käse" 1,59 € ab
/// 03.08. neben 3,94 € ab 06.08.).
#[test]
fn overlapping_windows_with_different_prices_stay_apart() {
    let a = windowed("Bio-Käse", 1.59, "2026-08-03", "2026-08-08");
    let b = windowed("Bio-Käse", 3.94, "2026-08-06", "2026-08-08");
    let rows = dedupe_rows(vec![a.clone(), b.clone()]);
    assert_eq!(rows, vec![a, b]);
}

// Kern von Phase 11: Zwei Filialen derselben Kette in derselben PLZ sind
// zwei Angebotssätze. Unter dem alten Schlüssel (market, product, valid_from,
// waren sie ununterscheidbar — die zweite Filiale hätte die erste
// beim Dedupe verschluckt und in Supabase überschrieben.
#[test]
fn two_branches_of_one_chain_in_one_plz_stay_apart() {
    let mut strehlen = offer("Gouda", Some(1.99));
    strehlen.market_id = "565005".to_string();
    let mut postplatz = offer("Gouda", Some(2.49));
    postplatz.market_id = "1766160".to_string();

    let a = map_offer(&strehlen, "REWE", false).unwrap();
    let b = map_offer(&postplatz, "REWE", false).unwrap();
    assert_eq!(a.market_id, "565005");
    assert_eq!(b.market_id, "1766160");
    // Gleiche Kette, gleiche PLZ, gleiches Produkt, gleiche Woche — und
    // trotzdem zwei Zeilen mit zwei Preisen.
    assert_eq!(dedupe_rows(vec![a.clone(), b.clone()]), vec![a, b]);
}

// Die vom Scraper gestempelte Kette (stores::scrape_store) schlägt jede
// Namensdeutung — sonst entscheidet Fremdtext über offers.market.
#[test]
fn chain_from_market_field_beats_branch_name() {
    let koeln = Market::new("ALDI_SUED_B330", "ALDI SÜD Köln-Altstadt-Nord");
    assert_eq!(chain_for(&koeln.clone().with_chain("ALDI SÜD")), Some("ALDI SÜD"));
    // Unbekannter Wert wird verworfen statt ungeprüft durchgereicht.
    assert_eq!(chain_for(&Market::new("42", "Feinkost Meier").with_chain("Tante Emma")), None);
}

// Regression: der Stadtteil im Filialnamen hat die Kette gekippt — der ganze
// nationale ALDI-SÜD-Katalog stand in Köln (50667) unter market='ALDI Nord'.
#[test]
fn chain_detection_ignores_district_in_branch_name() {
    let m = |id: &str, name: &str| Market::new(id, name);
    assert_eq!(chain_for(&m("ALDI_SUED_B330", "ALDI SÜD Köln-Altstadt-Nord")), Some("ALDI SÜD"));
    assert_eq!(chain_for(&m("ALDI_NORD_DE036002", "ALDI Nord Dresden-Süd")), Some("ALDI Nord"));
    // Filialname ohne erkennbare Gesellschaft: überspringen statt raten.
    assert_eq!(chain_for(&m("4711", "ALDI Filiale")), None);
}

#[test]
fn chain_detection_from_market() {
    let m = |id: &str, name: &str| Market::new(id, name);
    assert_eq!(chain_for(&m("LIDL_DE", "Lidl Deutschland")), Some("Lidl"));
    // EDEKA-Vertriebsmarken ohne "edeka" im Namen (ID ist nur numerisch)
    assert_eq!(chain_for(&m("022745", "E center Peltzer")), Some("EDEKA"));
    assert_eq!(chain_for(&m("4711", "Marktkauf Dresden")), Some("EDEKA"));
    assert_eq!(chain_for(&m("ALDI_NORD_DE", "ALDI Nord Deutschland")), Some("ALDI Nord"));
    assert_eq!(chain_for(&m("ALDI_SUED_DE", "ALDI Süd Deutschland")), Some("ALDI SÜD"));
    assert_eq!(chain_for(&m("831971", "REWE Christian Koehler oHG")), Some("REWE"));
    assert_eq!(chain_for(&m("1234", "Kaufland Dresden")), Some("Kaufland"));
    assert_eq!(chain_for(&m("42", "Feinkost Meier")), None);
}

#[test]
fn chain_for_matches_store_chain_for_every_store() {
    // markets.chain (sync: Store::chain()) und offers.market (push: chain_for)
    // müssen exakt denselben String tragen — die App filtert mit market=in.(…)
    // aus der markets-Tabelle, jede Abweichung macht Angebote unsichtbar.
    use lechariot::stores::Store;
    for store in Store::ALL {
        let canonical = store.chain();
        let market = Market::new(canonical, &format!("{canonical} Testfiliale"));
        assert_eq!(
            chain_for(&market),
            Some(canonical),
            "chain_for weicht für {canonical} von Store::chain() ab"
        );
        // … und erst recht, wenn der Scraper die Kette gestempelt hat.
        assert_eq!(
            chain_for(&market.with_chain(canonical)),
            Some(canonical),
            "chain_for verwirft die gestempelte Kette {canonical}"
        );
    }
}

#[test]
fn base_price_units_module_roundtrip() {
    // Absicherung, dass push dieselbe Ableitung nutzt wie compare/suggest
    let up = units::derive_unit_price(Some(0.99), &[Some("je 500-g-Packg.")]).unwrap();
    assert!((up.eur - 1.98).abs() < 1e-9);
}

// ---------------------------------------------------------- HTTP-Schicht

#[derive(Debug, Clone)]
struct Req {
    method: String,
    target: String, // Pfad + Query
    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Minimaler HTTP/1.1-Mock: nimmt Requests an, protokolliert sie und
/// antwortet immer mit 200 `[]`.
///
/// Jede Verbindung bekommt einen eigenen Thread. Vorher liefen alle in einer
/// Schleife, und weil `keep-alive` die Verbindung offen hält, wurde die
/// zweite nie angenommen: Der Push benutzt zwei Clients (einen für die
/// REST-Aufrufe, einen ohne Redirects fürs Hochladen), und sobald beide
/// gleichzeitig etwas wollen, stand der Mock. Kein Produktionsfehler — aber
/// er hätte den Test unmöglich gemacht, der den Upload im Lauf prüft.
fn spawn_mock() -> (String, Arc<Mutex<Vec<Req>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let log2 = log2.clone();
            std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let mut parts = line.split_whitespace();
                let (Some(method), Some(target)) = (parts.next(), parts.next()) else { break };
                let (method, target) = (method.to_string(), target.to_string());
                let mut headers = Vec::new();
                let mut content_length = 0usize;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 {
                        break;
                    }
                    let h = h.trim_end().to_string();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        let (k, v) = (k.trim().to_string(), v.trim().to_string());
                        if k.eq_ignore_ascii_case("content-length") {
                            content_length = v.parse().unwrap_or(0);
                        }
                        headers.push((k, v));
                    }
                }
                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut body).unwrap();
                }
                log2.lock().unwrap().push(Req {
                    method,
                    target,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let resp = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n[]";
                if reader.get_mut().write_all(resp.as_bytes()).is_err() {
                    break;
                }
            }
            });
        }
    });
    (format!("http://{addr}"), log)
}

/// Wie spawn_mock, antwortet aber immer mit dem angegebenen Fehlerstatus.
fn spawn_failing_mock(status: u16, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 65536];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {status} ERR\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}")
}

fn temp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("lechariot-push-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("test.db").to_string_lossy().into_owned()
}

/// DB mit einem REWE-Markt und `n` bepreisten Angeboten (valid_from 2026-07-13)
/// plus einem Altwochen-Angebot ohne Preis anlegen.
fn seed_db(path: &str, n: usize) {
    let _ = std::fs::remove_file(path);
    let conn = db::open(path).unwrap();
    db::upsert_market(&conn, &Market::new("m1", "REWE Christian Koehler oHG"))
        .unwrap();
    for i in 0..n {
        db::upsert_offer(&conn, &offer(&format!("Produkt {i:03}"), Some(1.0 + i as f64 / 100.0)))
            .unwrap();
    }
    db::upsert_offer(&conn, &offer("Ohne Preis", None)).unwrap();
}

fn run_push(db_path: &str, base_url: &str) -> anyhow::Result<()> {
    let opts = PushOptions {
        db_path: db_path.to_string(),
        chain: None, branch_id: None,
        nationwide: false,
        dry_run: false,
        mirror_images: false,
        defer_mirror: false,
    };
    let cfg = PushConfig { base_url: base_url.to_string(), api_key: "test-key".to_string() };
    push::run(&opts, Some(&cfg))
}

/// Ein Kachelbild aus dem Prospekt liegt als Datei auf dem Runner. Es MUSS
/// hochgeladen werden, auch wenn das Spiegeln der Händler-Bilder aus ist.
///
/// Beides hing bis #26 an einem Schalter. Als das Spiegeln am 27.07.
/// abgeschaltet wurde (die App lädt Händler-CDN-URLs direkt), schaltete das
/// unbemerkt auch den Upload der Kachelbilder ab — und `file:///tmp/...`
/// stand danach als Bild-URL in Produktion. Am 31.07. mit drei Lidl-Läufen
/// nachgemessen: 441 solcher Zeilen.
fn seed_crop_offer(db_path: &str, image_url: &str) {
    let _ = std::fs::remove_file(db_path);
    let conn = db::open(db_path).unwrap();
    db::upsert_market(&conn, &Market::new("LIDL_TEST", "Lidl Teststadt")).unwrap();
    let mut o = offer("GRILLMEISTER Bratwurst", Some(4.39));
    o.market_id = "LIDL_TEST".to_string();
    o.id = Offer::build_id("LIDL_TEST", "GRILLMEISTER Bratwurst", Some("2026-07-13"));
    o.images = vec![image_url.to_string()];
    db::upsert_offer(&conn, &o).unwrap();
}

#[test]
fn local_crops_are_uploaded_even_when_mirroring_is_off() {
    let dir = lechariot::scrapers::lidl_prospekt::crop_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("LIDL_TEST_hochladen_{}.png", std::process::id()));
    std::fs::write(&file, b"Bytes reichen: downscale gibt None und laedt das Original hoch").unwrap();

    let db_path = temp_db("kachel-upload");
    seed_crop_offer(&db_path, &format!("file://{}", file.display()));
    let (base_url, log) = spawn_mock();

    // run_push setzt mirror_images: false — genau der Fall, um den es geht.
    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let bodies: String = reqs.iter().map(|r| r.body.as_str()).collect();
    assert!(
        reqs.iter().any(|r| r.target.contains("/storage/v1/object/offer-images/")),
        "kein Upload versucht: {:#?}",
        reqs.iter().map(|r| &r.target).collect::<Vec<_>>()
    );
    assert!(
        !bodies.contains("file://"),
        "file:// darf niemals in die Datenbank — im Telefon ist das ein toter Link: {bodies}"
    );
    assert!(
        bodies.contains("/storage/v1/object/public/offer-images/"),
        "Zeile trägt keine Bucket-URL: {bodies}"
    );
    let _ = std::fs::remove_file(&file);
}

/// Lässt sich das Kachelbild nicht hochladen, ist die Antwort „kein Bild" —
/// nicht „der lokale Pfad". Die App bildet `.empty` und `.failure` auf
/// dasselbe Kategorie-Symbol ab, ein toter Link sähe also aus wie kein Bild
/// und meldete sich nie.
#[test]
fn a_crop_that_cannot_be_uploaded_becomes_no_image() {
    let dir = lechariot::scrapers::lidl_prospekt::crop_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join(format!("LIDL_TEST_gibt_es_nicht_{}.png", std::process::id()));
    let _ = std::fs::remove_file(&missing);

    let db_path = temp_db("kachel-fehlt");
    seed_crop_offer(&db_path, &format!("file://{}", missing.display()));
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let bodies: String = log.lock().unwrap().iter().map(|r| r.body.clone()).collect();
    assert!(!bodies.contains("file://"), "toter Pfad in der Datenbank: {bodies}");
    assert!(bodies.contains("\"image_url\":null"), "Bild nicht auf null gesetzt: {bodies}");
}

/// Netto-Bilder werden IMMER gespiegelt, auch wenn das Spiegeln der übrigen
/// Händler-Bilder aus ist — und ohne dass sich für andere Ketten etwas ändert.
///
/// Grund (gemessen 2026-07-31): Das Netto-CDN weist jeden schlichten Client
/// mit 403 ab, auch mit Referer und iPhone-UA — kein Hotlink-Schutz, Akamai.
/// Die Händler-URL ist für die App ein toter Link; 1.722 gültige Angebote
/// standen deshalb ohne Bild da. Der Cache ist hier vorbelegt (stellt einen
/// früheren Spiegel-Lauf dar), damit der Test ohne Live-Netzwerk auskommt —
/// der Cache-Treffer greift nur, wenn die Scope-Zuordnung die Netto-URL
/// überhaupt in einen Durchgang aufnimmt. Wer die Zuordnung kaputtmacht,
/// sieht diesen Test fallen.
#[test]
fn netto_images_are_mirrored_even_when_mirroring_is_off() {
    let db_path = temp_db("netto-spiegeln");
    let _ = std::fs::remove_file(&db_path);
    let netto_src = "https://www.netto-online.de/media_nfs/images/2026-31/42-450x450-Blumenkohl-n.webp";
    let rewe_src = "https://img.rewe-static.de/gouda.jpg";
    {
        let conn = db::open(&db_path).unwrap();
        db::upsert_market(&conn, &Market::new("9110", "Netto Marken-Discount Dresden").with_chain("Netto")).unwrap();
        db::upsert_market(&conn, &Market::new("565005", "REWE Supermarkt").with_chain("REWE")).unwrap();
        let mut n = offer("Blumenkohl", Some(0.99));
        n.market_id = "9110".to_string();
        n.id = Offer::build_id("9110", "Blumenkohl", Some("2026-07-13"));
        n.images = vec![netto_src.to_string()];
        db::upsert_offer(&conn, &n).unwrap();
        let mut r = offer("Gouda", Some(1.99));
        r.market_id = "565005".to_string();
        r.id = Offer::build_id("565005", "Gouda", Some("2026-07-13"));
        r.images = vec![rewe_src.to_string()];
        db::upsert_offer(&conn, &r).unwrap();
    }
    let (base_url, log) = spawn_mock();
    // Cache für BEIDE URLs vorbelegen: Käme die REWE-URL in einen Durchgang,
    // würde auch sie zur Bucket-URL — dass sie es nicht wird, ist die zweite
    // Hälfte der Behauptung (kein Verhaltenswechsel für andere Ketten).
    let netto_bucket = storage::public_url(&base_url, &storage::object_path(netto_src));
    let rewe_bucket = storage::public_url(&base_url, &storage::object_path(rewe_src));
    {
        let conn = db::open(&db_path).unwrap();
        db::cache_image_url(&conn, netto_src, &netto_bucket).unwrap();
        db::cache_image_url(&conn, rewe_src, &rewe_bucket).unwrap();
    }

    // run_push setzt mirror_images: false — genau der Fall der Nightly.
    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let offer_posts: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers"))
        .collect();
    // Phase 1: Der erste Upsert trägt noch die Händler-URL — die Angebote
    // warten nicht auf Bilder.
    assert!(
        offer_posts.first().unwrap().body.contains(netto_src),
        "Phase 1 muss die Händler-URL upserten: {}",
        offer_posts.first().unwrap().body
    );
    // Phase 2: Der Bild-Nachtrag trägt die Bucket-URL für Netto …
    let bodies: String = offer_posts.iter().map(|r| r.body.as_str()).collect();
    assert!(
        bodies.contains(&netto_bucket),
        "Netto-Zeile wurde nie auf die Bucket-URL umgeschrieben: {bodies}"
    );
    // … und NUR für Netto: Die REWE-Zeile behält ihre Händler-URL, obwohl der
    // Cache eine Bucket-URL für sie hätte.
    assert!(
        !bodies.contains(&rewe_bucket),
        "REWE darf ohne mirror_images nicht gespiegelt werden: {bodies}"
    );
    // Der Nachtrag enthält genau die eine Netto-Zeile, nicht den ganzen Satz.
    let patch = offer_posts.iter().find(|r| r.body.contains(&netto_bucket)).unwrap();
    assert!(
        !patch.body.contains("Gouda"),
        "Bild-Nachtrag fasst fremde Zeilen an: {}",
        patch.body
    );
    // Der Nachtrag muss NACH dem ersten Upsert liegen (Angebote zuerst,
    // Bilder später — die App soll nicht auf Bilder warten).
    let first = reqs
        .iter()
        .position(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers"))
        .unwrap();
    let patch_pos = reqs.iter().position(|r| r.body.contains(&netto_bucket)).unwrap();
    assert!(first < patch_pos);
}

#[test]
fn push_batches_deletes_and_upserts() {
    let db_path = temp_db("batch");
    seed_db(&db_path, 150);
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    // 1x DELETE (stale), 2x POST offers (100 + 50), 2x POST price_history.
    // Der Regions-Upsert, der hier bis Migration v16 als sechster stand, ist
    // weg — die Tabelle gibt es nicht mehr.
    assert_eq!(reqs.len(), 5, "Requests: {reqs:#?}");

    let del = &reqs[0];
    assert_eq!(del.method, "DELETE");
    assert!(del.target.starts_with("/rest/v1/offers?"), "{}", del.target);
    // Aufgeräumt wird pro FILIALE, nicht pro Kette (migration_v13) — sonst
    // räumte der Push der einen REWE-Filiale die Wochen der Nachbarfiliale ab.
    assert!(del.target.contains("market_id=eq.m1"), "{}", del.target);
    assert!(!del.target.contains("market=eq.REWE"), "{}", del.target);
    // Kein Region-Filter mehr: Seit v16 ist die Filiale der ganze Schlüssel.
    assert!(!del.target.contains("region"), "{}", del.target);
    // Löscht alte Wochen UND Legacy-Zeilen ohne valid_from (URL-encodiert:
    // or=(valid_from.lt.2026-07-13,valid_from.is.null))
    let decoded = del.target.replace("%28", "(").replace("%29", ")").replace("%2C", ",");
    assert!(
        decoded.contains("or=(valid_from.lt.2026-07-13,valid_from.is.null)"),
        "{}",
        del.target
    );
    assert_eq!(del.header("apikey"), Some("test-key"));
    assert_eq!(del.header("authorization"), Some("Bearer test-key"));

    let (b1, b2) = (&reqs[1], &reqs[2]);
    for b in [b1, b2] {
        assert_eq!(b.method, "POST");
        assert!(b.target.starts_with("/rest/v1/offers?"), "{}", b.target);
        assert!(b.target.contains("on_conflict=market_id%2Cproduct%2Cvalid_from")
                || b.target.contains("on_conflict=market_id,product,valid_from"),
                "{}", b.target);
        assert_eq!(b.header("prefer"), Some("resolution=merge-duplicates"));
    }
    let rows1: Vec<SupabaseRow> = parse_rows(&b1.body);
    let rows2: Vec<SupabaseRow> = parse_rows(&b2.body);
    assert_eq!(rows1.len(), 100);
    assert_eq!(rows2.len(), 50);
    assert!(rows1.iter().all(|r| r.market == "REWE" && !r.nationwide));
}

// Ende-zu-Ende gegen den Fall aus Köln (50667): der Filialname der ALDI-SÜD-
// Filiale trägt "Nord", die Angebote müssen trotzdem als ALDI SÜD hochgehen.
#[test]
fn push_uses_stored_chain_not_branch_name() {
    let db_path = temp_db("chain");
    let _ = std::fs::remove_file(&db_path);
    {
        let conn = db::open(&db_path).unwrap();
        let koeln = Market::new("ALDI_SUED_B330", "ALDI SÜD Köln-Altstadt-Nord")
            .with_chain("ALDI SÜD");
        db::upsert_market(&conn, &koeln).unwrap();
        let mut o = offer("Aperitivo Italiano 0,7 l", Some(4.99));
        o.market_id = "ALDI_SUED_B330".to_string();
        db::upsert_offer(&conn, &o).unwrap();
    }
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let post = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers"))
        .expect("offers-POST fehlt");
    let rows = parse_rows(&post.body);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].market, "ALDI SÜD");

    // … und die Aufräum-DELETEs treffen dieselbe Kette, nicht ALDI Nord.
    let del = reqs
        .iter()
        .find(|r| r.method == "DELETE" && r.target.starts_with("/rest/v1/offers"))
        .expect("offers-DELETE fehlt");
    assert!(!del.target.contains("Nord"), "{}", del.target);
}

// Dasselbe Ende zu Ende gegen die HTTP-Schicht: Zwei REWE-Filialen in 01219
// werden als zwei Angebotssätze hochgeladen, und das Aufräumen veralteter
// Wochen trifft jede Filiale einzeln. Mit dem alten Ketten-Filter hätte der
// eine DELETE die frisch gepushten Zeilen der anderen Filiale erwischt.
#[test]
fn stale_cleanup_is_per_branch_not_per_chain() {
    let db_path = temp_db("zwei-filialen");
    let _ = std::fs::remove_file(&db_path);
    {
        let conn = db::open(&db_path).unwrap();
        for (id, name) in [("565005", "REWE Supermarkt"), ("1766160", "REWE Friedrichstadt")] {
            db::upsert_market(&conn, &Market::new(id, name).with_chain("REWE")).unwrap();
            let mut o = offer("Gouda", Some(1.99));
            o.market_id = id.to_string();
            o.id = Offer::build_id(id, "Gouda", Some("2026-07-13"));
            db::upsert_offer(&conn, &o).unwrap();
        }
    }
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let deletes: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "DELETE" && r.target.starts_with("/rest/v1/offers?"))
        .collect();
    assert_eq!(deletes.len(), 2, "je Filiale ein DELETE: {deletes:#?}");
    // Reihenfolge: nach market_id sortiert (BTreeMap), also "1766160" zuerst.
    for (del, market_id) in deletes.iter().zip(["1766160", "565005"]) {
        assert!(del.target.contains(&format!("market_id=eq.{market_id}")), "{}", del.target);
        assert!(!del.target.contains("region"), "{}", del.target);
    }

    let rows: Vec<SupabaseRow> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .flat_map(|r| parse_rows(&r.body))
        .collect();
    assert_eq!(rows.len(), 2, "beide Filialen kommen durch: {rows:#?}");
    let mut ids: Vec<&str> = rows.iter().map(|r| r.market_id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["1766160", "565005"]);
    assert!(rows.iter().all(|r| r.market == "REWE" && r.product == "Gouda"));
}

fn parse_rows(body: &str) -> Vec<SupabaseRow> {
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| SupabaseRow {
            market_id: r["market_id"].as_str().unwrap().to_string(),
            market: r["market"].as_str().unwrap().to_string(),
            product: r["product"].as_str().unwrap().to_string(),
            price: r["price"].as_f64().unwrap(),
            regular_price: r["regular_price"].as_f64(),
            unit: r["unit"].as_str().unwrap().to_string(),
            category: r["category"].as_str().map(String::from),
            emoji: None,
            image_url: r["image_url"].as_str().map(String::from),
            valid_from: r["valid_from"].as_str().map(String::from),
            valid_until: r["valid_until"].as_str().map(String::from),
            base_price: r["base_price"].as_f64(),
            base_unit: r["base_unit"].as_str().map(String::from),
            brand: None,
            ean: None,
            source: r["source"].as_str().unwrap().to_string(),
            nationwide: r["nationwide"].as_bool().unwrap_or(false),
            match_key: r["match_key"]
                .as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str()).map(String::from).collect())
                .unwrap_or_default(),
        })
        .collect()
}

// Regression (Lidl, Juli 2026): Angebote ohne valid_from dürfen nie nach
// Supabase — die App filtert sie serverseitig weg und der Upsert-Schlüssel
// (market, product, valid_from, region) ist nicht NULL-sicher, jeder Lauf
// würde sie duplizieren.
#[test]
fn push_skips_offers_without_valid_from() {
    let db_path = temp_db("nodate");
    seed_db(&db_path, 3);
    {
        let conn = db::open(&db_path).unwrap();
        let mut dateless = offer("Ohne Datum", Some(2.49));
        dateless.valid_from = None;
        db::upsert_offer(&conn, &dateless).unwrap();
    }
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let upserted: Vec<SupabaseRow> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .flat_map(|r| parse_rows(&r.body))
        .collect();
    assert_eq!(upserted.len(), 3, "nur die 3 datierten Angebote");
    assert!(upserted.iter().all(|r| r.valid_from.is_some()));
    assert!(!upserted.iter().any(|r| r.product == "Ohne Datum"));
}

#[test]
fn push_fails_with_german_error_on_http_error() {
    let db_path = temp_db("err");
    seed_db(&db_path, 2);
    let base_url = spawn_failing_mock(401, "{\"message\":\"Invalid API key\"}");

    let err = run_push(&db_path, &base_url).unwrap_err().to_string();
    assert!(err.contains("fehlgeschlagen"), "{err}");
    assert!(err.contains("401"), "{err}");
    assert!(err.contains("Invalid API key"), "{err}");
}

/// Ohne Region ist der Push **bundesweit** (Phase 12, ALDI): Die Zeilen gehen
/// mit `region: null` hoch, das Aufräumen alter Wochen filtert `region=is.null`
/// statt `eq.` — `eq.` trifft NULL nie und ließe jede alte Woche stehen — und
/// `regions` wird nicht angefasst, denn dieser Push gehört zu keiner PLZ.
#[test]
fn push_nationwide_stores_without_a_branch_filter() {
    let db_path = temp_db("national");
    seed_db(&db_path, 2);
    let (base_url, log) = spawn_mock();

    let opts = PushOptions {
        db_path,
        chain: None,
        branch_id: None,
        nationwide: true,
        dry_run: false,
        mirror_images: false,
        defer_mirror: false,
    };
    let cfg = PushConfig { base_url, api_key: "test-key".to_string() };
    push::run(&opts, Some(&cfg)).unwrap();

    let log = log.lock().unwrap();
    let deletes: Vec<&Req> = log.iter().filter(|r| r.method == "DELETE").collect();
    assert!(!deletes.is_empty(), "kein Aufräumen alter Wochen");
    for req in &deletes {
        assert!(req.target.contains("market_id=eq."), "{}", req.target);
        assert!(!req.target.contains("region"), "{}", req.target);
    }

    let posts: Vec<&Req> = log.iter().filter(|r| r.method == "POST").collect();
    let offers: Vec<&&Req> = posts.iter().filter(|r| r.target.starts_with("/rest/v1/offers")).collect();
    assert!(!offers.is_empty(), "keine Angebote hochgeladen");
    for req in &offers {
        assert!(req.body.contains("\"nationwide\":true"), "{}", req.body);
    }
}

#[test]
fn dry_run_makes_no_requests() {
    let db_path = temp_db("dry");
    seed_db(&db_path, 3);
    let (_base_url, log) = spawn_mock();
    let opts = PushOptions { db_path, chain: None, branch_id: None, nationwide: false, dry_run: true, mirror_images: true, defer_mirror: false };
    // cfg: None — Dry-Run braucht weder Env noch Netzwerk (auch nicht fürs Spiegeln)
    push::run(&opts, None).unwrap();
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn push_skips_unsafe_image_url_but_uses_cached_bucket_url() {
    let db_path = temp_db("mirror");
    let (base_url, log) = spawn_mock();

    // REWE-Markt + ein Angebot MIT Bild. Die Bild-URL zeigt auf den lokalen
    // Mock (http, Loopback) — also genau das, was der SSRF-Schutz ablehnen
    // muss: kein https, IP im Loopback-Bereich, Host nicht auf der CDN-Liste.
    let _ = std::fs::remove_file(&db_path);
    let conn = db::open(&db_path).unwrap();
    db::upsert_market(&conn, &Market::new("m1", "REWE Christian Koehler oHG"))
        .unwrap();
    let mut o = offer("Gouda", Some(1.99));
    let src = format!("{base_url}/img/gouda.jpg");
    o.images = vec![src.clone()];
    db::upsert_offer(&conn, &o).unwrap();
    drop(conn);

    let cfg = PushConfig { base_url: base_url.clone(), api_key: "k".to_string() };
    let opts = PushOptions {
        db_path: db_path.clone(),
        chain: None, branch_id: None,
        nationwide: false,
        dry_run: false,
        mirror_images: true,
        defer_mirror: false,
    };

    // Erster Lauf: die unsichere URL wird übersprungen — kein Download, kein
    // Storage-Upload, aber der Push läuft durch und die Zeile behält die
    // Händler-URL (Requirement: kein Abbruch, graceful skip).
    push::run(&opts, Some(&cfg)).unwrap();
    let reqs = log.lock().unwrap().clone();
    assert!(
        !reqs.iter().any(|r| r.target == "/img/gouda.jpg"),
        "unsichere Bild-URL wurde geladen: {reqs:#?}"
    );
    assert!(
        !reqs.iter().any(|r| r.target.starts_with("/storage/v1/object/offer-images/")),
        "unsicheres Bild wurde hochgeladen: {reqs:#?}"
    );
    let offers_post = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers"))
        .expect("Offers-Upsert fehlt");
    assert!(
        offers_post.body.contains("/img/gouda.jpg")
            && !offers_post.body.contains("/storage/v1/object/public/"),
        "Zeile muss die Händler-URL behalten: {}",
        offers_post.body
    );

    // Cache vorbelegen (stellt einen früheren, erfolgreichen Spiegel-Lauf dar):
    // der Cache-Treffer greift VOR jedem Netzwerkzugriff, unabhängig vom
    // SSRF-Schutz.
    let bucket_url = storage::public_url(&base_url, &storage::object_path(&src));
    let conn = db::open(&db_path).unwrap();
    db::cache_image_url(&conn, &src, &bucket_url).unwrap();
    drop(conn);

    // Zweiter Lauf: Cache-Treffer -> weder Download noch Upload, Zeile trägt die
    // Bucket-URL.
    log.lock().unwrap().clear();
    push::run(&opts, Some(&cfg)).unwrap();
    let reqs2 = log.lock().unwrap().clone();
    assert!(
        !reqs2.iter().any(|r| r.target.starts_with("/storage/v1/object/offer-images/")),
        "Bild wurde trotz Cache hochgeladen: {reqs2:#?}"
    );
    assert!(
        !reqs2.iter().any(|r| r.target == "/img/gouda.jpg"),
        "Bild wurde trotz Cache geladen"
    );
    // Die Zeile trägt jetzt die Bucket-URL aus dem Cache.
    let offers_post2 = reqs2
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers"))
        .unwrap();
    assert!(offers_post2.body.contains("/storage/v1/object/public/offer-images/"));
}

#[test]
fn chain_filter_limits_push() {
    let db_path = temp_db("filter");
    seed_db(&db_path, 2);
    let (base_url, log) = spawn_mock();
    let opts = PushOptions {
        db_path,
        chain: Some("Lidl".to_string()), // DB enthält nur REWE
        branch_id: None,
        nationwide: false,
        dry_run: false,
        mirror_images: false,
        defer_mirror: false,
    };
    let cfg = PushConfig { base_url, api_key: "k".to_string() };
    push::run(&opts, Some(&cfg)).unwrap();
    assert!(log.lock().unwrap().is_empty());
}

// ------------------------------------------------------- Preis-Historie

/// Wie spawn_mock, antwortet aber auf Targets mit dem angegebenen Präfix mit
/// `fail_status` statt 200 — für Tests, die einen Teil-Ausfall simulieren.
fn spawn_selective_mock(
    fail_prefix: &'static str,
    fail_status: u16,
) -> (String, Arc<Mutex<Vec<Req>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let mut parts = line.split_whitespace();
                let (Some(method), Some(target)) = (parts.next(), parts.next()) else { break };
                let (method, target) = (method.to_string(), target.to_string());
                let mut headers = Vec::new();
                let mut content_length = 0usize;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 {
                        break;
                    }
                    let h = h.trim_end().to_string();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        let (k, v) = (k.trim().to_string(), v.trim().to_string());
                        if k.eq_ignore_ascii_case("content-length") {
                            content_length = v.parse().unwrap_or(0);
                        }
                        headers.push((k, v));
                    }
                }
                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut body).unwrap();
                }
                let fail = target.starts_with(fail_prefix);
                log2.lock().unwrap().push(Req {
                    method,
                    target,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let resp = if fail {
                    format!("HTTP/1.1 {fail_status} ERR\r\ncontent-length: 2\r\n\r\n{{}}")
                } else {
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n[]"
                        .to_string()
                };
                if reader.get_mut().write_all(resp.as_bytes()).is_err() {
                    break;
                }
            }
        }
    });
    (format!("http://{addr}"), log)
}

fn history_posts(reqs: &[Req]) -> Vec<&Req> {
    reqs.iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/price_history"))
        .collect()
}

#[test]
fn push_sends_history_rows() {
    let db_path = temp_db("hist");
    seed_db(&db_path, 150);
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let hist = history_posts(&reqs);
    // 150 Zeilen in Batches à 100
    assert_eq!(hist.len(), 2, "History-Requests: {reqs:#?}");
    let rows: Vec<serde_json::Value> = hist
        .iter()
        .flat_map(|r| {
            serde_json::from_str::<serde_json::Value>(&r.body)
                .unwrap()
                .as_array()
                .unwrap()
                .clone()
        })
        .collect();
    assert_eq!(rows.len(), 150);
    let first = &rows[0];
    assert_eq!(first["market"], "REWE");
    assert_eq!(first["nationwide"], false);
    assert_eq!(first["valid_from"], "2026-07-13");
    assert!(first["price"].is_number());
    // Nur die Historien-Spalten — keine Anzeige-Felder wie image_url/emoji
    assert!(first.get("image_url").is_none(), "{first}");
    assert!(first.get("emoji").is_none(), "{first}");
    assert!(first.get("source").is_none(), "{first}");
}

#[test]
fn history_upsert_headers() {
    let db_path = temp_db("hist-headers");
    seed_db(&db_path, 2);
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let hist = history_posts(&reqs);
    assert_eq!(hist.len(), 1);
    let h = hist[0];
    assert!(
        h.target.contains("on_conflict=market_id%2Cproduct%2Cvalid_from")
            || h.target.contains("on_conflict=market_id,product,valid_from"),
        "{}",
        h.target
    );
    assert_eq!(h.header("prefer"), Some("resolution=merge-duplicates"));
    assert_eq!(h.header("apikey"), Some("test-key"));
    assert_eq!(h.header("authorization"), Some("Bearer test-key"));
}

#[test]
fn history_skips_null_price() {
    let db_path = temp_db("hist-null");
    // seed_db legt zusätzlich ein Angebot "Ohne Preis" (price: None) an
    seed_db(&db_path, 3);
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let hist = history_posts(&reqs);
    assert_eq!(hist.len(), 1);
    let rows: serde_json::Value = serde_json::from_str(&hist[0].body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 3);
    assert!(!hist[0].body.contains("Ohne Preis"), "{}", hist[0].body);
}

#[test]
fn history_failure_does_not_fail_push() {
    let db_path = temp_db("hist-fail");
    seed_db(&db_path, 2);
    let (base_url, log) = spawn_selective_mock("/rest/v1/price_history", 500);

    // Offers-Push muss trotz kaputter Historie erfolgreich durchlaufen …
    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    // … der Historie-Request wurde versucht …
    assert_eq!(history_posts(&reqs).len(), 1, "{reqs:#?}");
    // … und die Angebote gingen trotzdem hoch.
    assert!(
        reqs.iter().any(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers")),
        "{reqs:#?}"
    );
}

// ------------------------------------------------- Zwei-Phasen-Push (defer)

// On-Demand-Pfad: Phase 1 upsertet sofort mit der Händler-URL, erst danach
// werden Bilder gespiegelt und die Zeilen mit Bucket-URLs nachgetragen.
#[test]
fn deferred_mirror_upserts_offers_before_mirroring() {
    let db_path = temp_db("defer");
    let (base_url, log) = spawn_mock();

    let _ = std::fs::remove_file(&db_path);
    let conn = db::open(&db_path).unwrap();
    db::upsert_market(&conn, &Market::new("m1", "REWE Christian Koehler oHG")).unwrap();
    let mut o = offer("Gouda", Some(1.99));
    let src = format!("{base_url}/img/gouda.jpg");
    o.images = vec![src.clone()];
    db::upsert_offer(&conn, &o).unwrap();
    // Cache vorbelegen: der Bild-Nachtrag (Phase 2) nutzt den Cache-Treffer und
    // schreibt die Bucket-URL ohne Netzwerkzugriff nach — der SSRF-Schutz würde
    // die Loopback-URL sonst (korrekt) ablehnen.
    let bucket_url = storage::public_url(&base_url, &storage::object_path(&src));
    db::cache_image_url(&conn, &src, &bucket_url).unwrap();
    drop(conn);

    let cfg = PushConfig { base_url: base_url.clone(), api_key: "k".to_string() };
    let opts = PushOptions {
        db_path,
        chain: None,
        // Filial-Sync: Die Fertigmeldung muss VOR dem Spiegeln stehen.
        branch_id: Some("1763556".to_string()),
        nationwide: false,
        dry_run: false,
        mirror_images: true,
        defer_mirror: true,
    };
    push::run(&opts, Some(&cfg)).unwrap();

    let reqs = log.lock().unwrap().clone();

    // Phase 1: erster Offers-Upsert trägt die Händler-URL (kein Bucket).
    let first_upsert = reqs
        .iter()
        .position(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers"))
        .expect("Offers-Upsert fehlt");
    assert!(
        reqs[first_upsert].body.contains("/img/gouda.jpg")
            && !reqs[first_upsert].body.contains("/storage/v1/object/public/"),
        "Phase 1 muss die Händler-URL upserten: {}",
        reqs[first_upsert].body
    );

    // Phase 2: Nachtrag-Upsert mit Bucket-URL (aus dem Cache) NACH Phase 1 und
    // mit regulärem Konfliktschlüssel.
    let patch_pos = reqs
        .iter()
        .enumerate()
        .skip(first_upsert + 1)
        .find(|(_, r)| {
            r.method == "POST"
                && r.target.starts_with("/rest/v1/offers")
                && r.body.contains("/storage/v1/object/public/offer-images/")
        })
        .map(|(i, _)| i)
        .expect("Bild-Nachtrag-Upsert mit Bucket-URL fehlt");
    assert!(first_upsert < patch_pos, "Nachtrag muss nach Phase 1 kommen: {reqs:#?}");
    let patch = &reqs[patch_pos];
    assert!(
        patch.target.contains("on_conflict=market_id%2Cproduct%2Cvalid_from")
            || patch.target.contains("on_conflict=market_id,product,valid_from"),
        "{}",
        patch.target
    );

    // Die Fertigmeldung liegt DAZWISCHEN: nach den Angeboten, vor dem
    // Bild-Nachtrag. Genau dafür gibt es defer_mirror — die App soll die
    // Angebote sehen, sobald sie da sind, und nicht auf Bilder warten, für
    // die sie ohnehin einen Emoji-Fallback hat. Live gemessen: Scrape und
    // Push waren nach 43 s durch, das Spiegeln danach dauerte Minuten.
    let done_pos = reqs
        .iter()
        .position(|r| r.method == "POST" && r.target.starts_with("/rest/v1/branch_requests"))
        .expect("Fertigmeldung fehlt");
    assert!(first_upsert < done_pos, "Fertigmeldung vor den Angeboten: {reqs:#?}");
    assert!(done_pos < patch_pos, "Fertigmeldung erst nach dem Bild-Nachtrag: {reqs:#?}");
}

/// **Der Weg, den kein Live-Lauf hier prüfen kann.** Die Kachelbilder des
/// Lidl-Prospekts entstehen als Datei und nicht hinter einer URL; ob sie
/// tatsächlich im Bucket landen, hängt an einem Zweig in `storage::mirror`,
/// den man ohne Service-Key nicht gegen Supabase ausprobieren kann.
///
/// Genau so sah der Fehler aus, gegen den dieses Projekt seine Lehren
/// geschrieben hat: Ein Lauf meldet Erfolg, und niemand hat je nachgesehen, ob
/// am Ende ein Bild ankommt. Also gegen den Mock-Server nachgesehen.
#[test]
fn a_leaflet_crop_is_uploaded_to_the_bucket() {
    let dir = lechariot::scrapers::lidl_prospekt::crop_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("LIDL_TEST_upload_probe.png");
    // Ein echtes PNG, damit der Weg über `downscale` derselbe ist wie im Lauf.
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(64, 64));
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    std::fs::write(&file, bytes.into_inner()).unwrap();

    let (base_url, log) = spawn_mock();
    let cfg = PushConfig { base_url: base_url.clone(), api_key: "test-key".to_string() };
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let source = format!("file://{}", file.display());
    let (public, uploaded) =
        storage::mirror(&client, &cfg, &source).expect("Kachelbild nicht gespiegelt");
    let _ = std::fs::remove_file(&file);

    // Leerer Bucket: Das Bild muss wirklich hochgeladen worden sein.
    assert!(uploaded, "Kachelbild als unverändert abgetan, obwohl es fehlte");
    // Die zurückgegebene URL zeigt in den öffentlichen Bucket, nicht auf die
    // lokale Datei — sonst stünde ein `file://`-Pfad in der Datenbank.
    assert!(public.starts_with(&base_url), "keine Bucket-URL: {public}");
    assert!(public.contains("/storage/v1/object/public/offer-images/"));
    assert!(!public.contains("file://"));

    let log = log.lock().unwrap();
    let upload = log
        .iter()
        .find(|r| r.method == "POST" && r.target.contains("/storage/v1/object/offer-images/"))
        .expect("kein Upload angekommen");
    assert!(!upload.body.is_empty(), "Upload ohne Bilddaten");
    assert!(
        upload
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.starts_with("image/")),
        "Upload ohne Bild-Content-Type: {:?}",
        upload.headers
    );
    // Idempotent — ein zweiter Lauf darf dasselbe Objekt überschreiben.
    assert!(
        upload
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-upsert") && v == "true"),
        "x-upsert fehlt"
    );
}
