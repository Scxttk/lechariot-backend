use std::net::SocketAddr;

use lechariot::db;
use lechariot::models::{Market, Offer};

fn offer(
    id: &str,
    market_id: &str,
    title: &str,
    subtitle: Option<&str>,
    overline: Option<&str>,
    price: f64,
    regular: Option<f64>,
) -> Offer {
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

// Fixture-DB wie in tests/api.rs: zwei Märkte, vergleichbare Angebote,
// ein Titel mit HTML-relevanten Sonderzeichen.
fn build_fixture(path: &std::path::Path) {
    let conn = db::open(path.to_str().unwrap()).expect("DB öffnen");
    db::upsert_market(&conn, &Market::new("M1", "Testmarkt Eins")).unwrap();
    db::upsert_market(&conn, &Market::new("M2", "Testmarkt Zwei")).unwrap();

    db::upsert_offer(&conn, &offer("o1", "M1", "MEGGLE Feine Butter", Some("je 250-g-Packg. (1 kg = 6.36)"), None, 1.59, Some(2.59))).unwrap();
    db::upsert_offer(&conn, &offer("o2", "M2", "MEGGLE Feine Butter", Some("je 250-g-Packg. (1 kg = 5.56)"), None, 1.39, Some(2.59))).unwrap();
    db::upsert_offer(&conn, &offer("o3", "M1", "PFANNER", Some("Eistee"), Some("je 2-l-Packg. (1 l = 0.56)"), 1.11, Some(1.99))).unwrap();
    db::upsert_offer(&conn, &offer("o4", "M2", "Käse \"Extra\", gereift <b>", Some("0.2 kg"), None, 2.99, None)).unwrap();
}

fn fixture_db(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir()
        .join(format!("lechariot_web_test_{name}_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    build_fixture(&path);
    path
}

// Web-Router auf einem ephemeren Port in einem Hintergrund-Thread starten
// (Muster aus tests/api.rs).
fn spawn_server(db_path: &std::path::Path) -> SocketAddr {
    let db_path = db_path.to_str().unwrap().to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let app = lechariot::web::router(db_path.clone())
                .nest("/api", lechariot::api::router(db_path));
            axum::serve(listener, app).await.unwrap();
        });
    });
    rx.recv().expect("Server-Adresse")
}

fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let resp = reqwest::blocking::get(format!("http://{addr}{path}")).expect("HTTP-Request");
    let status = resp.status().as_u16();
    let body = resp.text().expect("Body");
    (status, body)
}

#[test]
fn overview_shows_market_stats_and_top_discounts() {
    let dbf = fixture_db("overview");
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/");
    assert_eq!(status, 200);
    assert!(body.contains("Übersicht"), "body: {body}");
    assert!(body.contains("Testmarkt Eins"), "body: {body}");
    assert!(body.contains("Testmarkt Zwei"), "body: {body}");
    assert!(body.contains("Angebote pro Markt"), "body: {body}");
    // Top-Rabatte: PFANNER hat den größten Rabatt (1.11 statt 1.99)
    assert!(body.contains("Top-Rabatte"), "body: {body}");
    assert!(body.contains("1.11 € statt 1.99 €"), "body: {body}");
}

#[test]
fn json_api_still_reachable_under_api_prefix() {
    let dbf = fixture_db("api_prefix");
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/api/offers?q=Butter");
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(json.as_array().expect("Array").len(), 2);
}

#[test]
fn search_lists_offers_cheapest_first_and_escapes_html() {
    let dbf = fixture_db("search");
    let addr = spawn_server(&dbf);

    // Formular ohne q
    let (status, body) = get(addr, "/search");
    assert_eq!(status, 200);
    assert!(body.contains("name=\"q\""), "body: {body}");

    // Treffer, billigster zuerst
    let (status, body) = get(addr, "/search?q=Butter");
    assert_eq!(status, 200);
    assert!(body.contains("2 Treffer"), "body: {body}");
    let pos_cheap = body.find("1.39 €").expect("1.39 fehlt");
    let pos_exp = body.find("1.59 €").expect("1.59 fehlt");
    assert!(pos_cheap < pos_exp, "body: {body}");
    assert!(body.contains("6.36 €/kg") || body.contains("5.56 €/kg"), "body: {body}");

    // Kein Treffer
    let (status, body) = get(addr, "/search?q=Raumschiff");
    assert_eq!(status, 200);
    assert!(body.contains("Keine Angebote für 'Raumschiff' gefunden."), "body: {body}");

    // DB-Inhalt mit Sonderzeichen wird escaped
    let (status, body) = get(addr, "/search?q=K%C3%A4se");
    assert_eq!(status, 200);
    assert!(body.contains("Käse &quot;Extra&quot;, gereift &lt;b&gt;"), "body: {body}");
    assert!(!body.contains("gereift <b>"), "body: {body}");

    // Reflektierte Nutzereingabe wird escaped
    let (status, body) = get(addr, "/search?q=%3Cscript%3E");
    assert_eq!(status, 200);
    assert!(!body.contains("<script>"), "body: {body}");
    assert!(body.contains("&lt;script&gt;"), "body: {body}");
}

#[test]
fn compare_groups_products_cheapest_first() {
    let dbf = fixture_db("compare");
    let addr = spawn_server(&dbf);
    let (status, body) = get(addr, "/compare?q=Butter");
    assert_eq!(status, 200);
    // Gruppenname genau einmal als Überschrift
    assert_eq!(body.matches("<h2>").count(), 1, "body: {body}");
    assert!(body.contains("MEGGLE Feine Butter"), "body: {body}");
    // Billigerer Markt (M2, 1.39) vor teurerem (M1, 1.59)
    let pos_m2 = body.find("Testmarkt Zwei").expect("M2 fehlt");
    let pos_m1 = body.find("Testmarkt Eins").expect("M1 fehlt");
    assert!(pos_m2 < pos_m1, "body: {body}");
    assert!(body.contains("1.39 €") && body.contains("1.59 €"), "body: {body}");
}

#[test]
fn watchlist_add_and_remove_roundtrip() {
    let dbf = fixture_db("watchlist");
    let addr = spawn_server(&dbf);
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Leere Watchlist
    let (status, body) = get(addr, "/watchlist");
    assert_eq!(status, 200);
    assert!(body.contains("Keine Beobachtungen angelegt."), "body: {body}");

    // Anlegen -> Redirect -> taucht auf
    let resp = client
        .post(format!("http://{addr}/watchlist/add"))
        .form(&[("query", "Kaffee"), ("max_price", "5.00")])
        .send()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 303);
    assert_eq!(resp.headers()["location"], "/watchlist");
    let (status, body) = get(addr, "/watchlist");
    assert_eq!(status, 200);
    assert!(body.contains("Kaffee"), "body: {body}");
    assert!(body.contains("bis 5.00 €"), "body: {body}");

    // Entfernen -> Redirect -> verschwunden
    let resp = client
        .post(format!("http://{addr}/watchlist/remove"))
        .form(&[("id", "1")])
        .send()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 303);
    let (status, body) = get(addr, "/watchlist");
    assert_eq!(status, 200);
    assert!(!body.contains("Kaffee"), "body: {body}");

    // Leere Query -> 400 mit deutscher Fehlermeldung
    let resp = client
        .post(format!("http://{addr}/watchlist/add"))
        .form(&[("query", "  ")])
        .send()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    assert!(resp.text().unwrap().contains("fehlt oder ist leer"), "Fehlermeldung fehlt");
}

#[test]
fn watchlist_add_rejects_cross_origin_post() {
    let dbf = fixture_db("csrf");
    let addr = spawn_server(&dbf);
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Cross-origin Formular-POST (fremder Origin) wird abgewiesen.
    let resp = client
        .post(format!("http://{addr}/watchlist/add"))
        .header("Origin", "http://evil.example")
        .form(&[("query", "CSRF"), ("max_price", "1.00")])
        .send()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // Auch ein cross-site Sec-Fetch-Site wird abgewiesen.
    let resp = client
        .post(format!("http://{addr}/watchlist/add"))
        .header("Sec-Fetch-Site", "cross-site")
        .form(&[("query", "CSRF"), ("max_price", "1.00")])
        .send()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // Nichts davon darf in die DB gelangt sein.
    let (status, body) = get(addr, "/watchlist");
    assert_eq!(status, 200);
    assert!(body.contains("Keine Beobachtungen angelegt."), "body: {body}");

    // Gleich-originiger POST (passender Origin) funktioniert weiterhin.
    let resp = client
        .post(format!("http://{addr}/watchlist/add"))
        .header("Origin", format!("http://{addr}"))
        .header("Sec-Fetch-Site", "same-origin")
        .form(&[("query", "Kaffee"), ("max_price", "5.00")])
        .send()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 303);
    let (status, body) = get(addr, "/watchlist");
    assert_eq!(status, 200);
    assert!(body.contains("Kaffee"), "body: {body}");
}

#[test]
fn history_renders_svg_sparkline() {
    let dbf = fixture_db("history");
    // Synthetischer Verlauf: Butter war vor einer Woche teurer (Muster aus tests/cli.rs)
    let conn = db::open(dbf.to_str().unwrap()).unwrap();
    conn.execute_batch(
        "INSERT INTO price_history (offer_id, market_id, title, price, seen_at)
         VALUES ('o1', 'M1', 'MEGGLE Feine Butter', 2.59, date('now', '-7 days'));",
    )
    .unwrap();
    drop(conn);
    let addr = spawn_server(&dbf);

    let (status, body) = get(addr, "/history?offer=Butter");
    assert_eq!(status, 200);
    assert!(body.contains("<svg"), "body: {body}");
    assert!(body.contains("polyline"), "body: {body}");
    assert!(body.contains("MEGGLE Feine Butter"), "body: {body}");
    assert!(body.contains("2.59 €") && body.contains("1.59 €"), "body: {body}");

    // Unbekanntes Produkt
    let (status, body) = get(addr, "/history?offer=Raumschiff");
    assert_eq!(status, 200);
    assert!(body.contains("Keine Preisdaten für 'Raumschiff'."), "body: {body}");
}
