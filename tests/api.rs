use std::net::SocketAddr;

use lechariot::db;
use lechariot::models::{Market, Offer};

fn offer(id: &str, market_id: &str, title: &str, subtitle: Option<&str>, overline: Option<&str>, price: f64, regular: Option<f64>) -> Offer {
    Offer {
        id: id.to_string(),
        market_id: market_id.to_string(),
        title: title.to_string(),
        subtitle: subtitle.map(String::from),
        overline: overline.map(String::from),
        price: Some(price),
        regular_price: regular,
        category: Some("Test".to_string()),
        nutri_score: None,
        valid_from: Some("2026-07-13".to_string()),
        valid_until: Some("2026-07-19".to_string()),
        images: vec![],
        biozid: false,
        flyer_page: None,
    }
}

// Fixture-DB mit zwei Märkten und vergleichbaren Angeboten anlegen
// (kopiert aus tests/cli.rs)
fn build_fixture(path: &std::path::Path) {
    let conn = db::open(path.to_str().unwrap()).expect("DB öffnen");
    db::upsert_market(&conn, &Market::new("M1", "Testmarkt Eins")).unwrap();
    db::upsert_market(&conn, &Market::new("M2", "Testmarkt Zwei")).unwrap();

    // Gleiches Produkt in beiden Märkten, M2 billiger
    db::upsert_offer(&conn, &offer("o1", "M1", "MEGGLE Feine Butter", Some("je 250-g-Packg. (1 kg = 6.36)"), None, 1.59, Some(2.59))).unwrap();
    db::upsert_offer(&conn, &offer("o2", "M2", "MEGGLE Feine Butter", Some("je 250-g-Packg. (1 kg = 5.56)"), None, 1.39, Some(2.59))).unwrap();
    // Kaufland-Stil: Marke im Titel, Produkt im Untertitel, Menge im Overline
    db::upsert_offer(&conn, &offer("o3", "M1", "PFANNER", Some("Eistee"), Some("je 2-l-Packg. (1 l = 0.56)"), 1.11, Some(1.99))).unwrap();
    // Titel mit CSV-Sonderzeichen
    db::upsert_offer(&conn, &offer("o4", "M2", "Käse \"Extra\", gereift", Some("0.2 kg"), None, 2.99, None)).unwrap();
}

fn fixture_db(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("lechariot_api_test_{name}_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    build_fixture(&path);
    path
}

// Server auf einem ephemeren Port in einem Hintergrund-Thread starten und
// die tatsächliche Adresse zurückgeben. Der Thread läuft bis Testende.
fn spawn_server(db_path: &std::path::Path) -> SocketAddr {
    let db_path = db_path.to_str().unwrap().to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, lechariot::api::router(db_path)).await.unwrap();
        });
    });
    rx.recv().expect("Server-Adresse")
}

fn get(addr: SocketAddr, path: &str) -> (u16, serde_json::Value) {
    let resp = reqwest::blocking::get(format!("http://{addr}{path}")).expect("HTTP-Request");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().expect("JSON-Body");
    (status, body)
}

#[test]
fn offers_returns_matches_cheapest_first() {
    let dbf = fixture_db("offers");
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/offers?q=Butter");
    assert_eq!(status, 200);
    let offers = body.as_array().expect("Array");
    assert_eq!(offers.len(), 2);
    assert_eq!(offers[0]["price"], 1.39);
    assert_eq!(offers[1]["price"], 1.59);
    assert_eq!(offers[0]["title"], "MEGGLE Feine Butter");

    // Filter kombiniert: max_price schließt das teurere Angebot aus
    let (status, body) = get(addr, "/offers?q=Butter&max_price=1.50");
    assert_eq!(status, 200);
    assert_eq!(body.as_array().unwrap().len(), 1);

    let (status, body) = get(addr, "/offers?q=Butter&market=M1");
    assert_eq!(status, 200);
    let offers = body.as_array().unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0]["market_id"], "M1");
}

#[test]
fn offers_without_query_is_bad_request() {
    let dbf = fixture_db("offers_400");
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/offers");
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("'q'"), "body: {body}");
}

#[test]
fn deals_with_invalid_since_is_bad_request() {
    let dbf = fixture_db("deals_400");
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/deals?since=abc");
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("since"), "body: {body}");
}

#[test]
fn compare_groups_offers_with_unit_prices() {
    let dbf = fixture_db("compare");
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/compare?q=Butter");
    assert_eq!(status, 200);
    let groups = body.as_array().expect("Array");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["name"], "MEGGLE Feine Butter");
    let offers = groups[0]["offers"].as_array().unwrap();
    assert_eq!(offers.len(), 2);
    // Günstigster Markt zuerst, Grundpreis aus expliziter Angabe
    assert_eq!(offers[0]["market"], "Testmarkt Zwei");
    assert_eq!(offers[0]["unit_price"], "5.56 €/kg");
    assert_eq!(offers[1]["market"], "Testmarkt Eins");
}

#[test]
fn markets_lists_fixture_markets() {
    let dbf = fixture_db("markets");
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/markets");
    assert_eq!(status, 200);
    let markets = body.as_array().unwrap();
    assert_eq!(markets.len(), 2);
    assert_eq!(markets[0]["name"], "Testmarkt Eins");
}

#[test]
fn watches_check_reports_hits() {
    let dbf = fixture_db("watch_hits");
    let conn = db::open(dbf.to_str().unwrap()).unwrap();
    db::add_watch(&conn, "Butter", Some(1.50)).unwrap();
    db::add_watch(&conn, "Gibtesnicht", None).unwrap();
    drop(conn);
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/watches/check");
    assert_eq!(status, 200);
    assert_eq!(body["hits"], true);
    let watches = body["watches"].as_array().unwrap();
    assert_eq!(watches.len(), 2);
    // Nur das Angebot unter 1.50 € trifft
    let butter_hits = watches[0]["hits"].as_array().unwrap();
    assert_eq!(butter_hits.len(), 1);
    assert_eq!(butter_hits[0]["price"], 1.39);
    assert!(watches[1]["hits"].as_array().unwrap().is_empty());
}

#[test]
fn watches_check_without_hits() {
    let dbf = fixture_db("watch_miss");
    let conn = db::open(dbf.to_str().unwrap()).unwrap();
    db::add_watch(&conn, "Gibtesnicht", None).unwrap();
    drop(conn);
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/watches/check");
    assert_eq!(status, 200);
    assert_eq!(body["hits"], false);
}

#[test]
fn list_suggest_returns_cheapest_offer() {
    let dbf = fixture_db("suggest");
    let conn = db::open(dbf.to_str().unwrap()).unwrap();
    db::list_add(&conn, "Butter").unwrap();
    db::list_add(&conn, "Raumschiff").unwrap();
    drop(conn);
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/list/suggest");
    assert_eq!(status, 200);
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["offer"]["market"], "Testmarkt Zwei");
    assert_eq!(entries[0]["offer"]["price"], 1.39);
    assert_eq!(entries[0]["offer"]["unit_price"], "5.56 €/kg");
    assert!(entries[1]["offer"].is_null());
}
