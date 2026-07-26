use std::process::Command;

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

fn run(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_lechariot"))
        .args(args)
        .output()
        .expect("Binary ausführen");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

fn fixture_db(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("lechariot_cli_test_{name}_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    build_fixture(&path);
    path
}

#[test]
fn watch_add_list_remove_roundtrip() {
    let dbf = fixture_db("watch_crud");
    let db = dbf.to_str().unwrap();
    let (stdout, _, ok) = run(&["watch", "add", "Butter", "--max-price", "1.50", "--db", db]);
    assert!(ok);
    assert!(stdout.contains("Watch #1 angelegt"), "stdout: {stdout}");
    let (stdout, _, ok) = run(&["watch", "list", "--db", db]);
    assert!(ok);
    assert!(stdout.contains("Butter") && stdout.contains("1.50 €"), "stdout: {stdout}");
    let (stdout, _, ok) = run(&["watch", "remove", "1", "--db", db]);
    assert!(ok);
    assert!(stdout.contains("entfernt"), "stdout: {stdout}");
    let (stdout, _, ok) = run(&["watch", "remove", "1", "--db", db]);
    assert!(ok);
    assert!(stdout.contains("Kein Watch"), "stdout: {stdout}");
}

#[test]
fn watch_check_exits_1_on_hits() {
    let dbf = fixture_db("watch_hit");
    let db = dbf.to_str().unwrap();
    run(&["watch", "add", "Butter", "--max-price", "1.50", "--db", db]);
    let (stdout, _, ok) = run(&["watch", "check", "--db", db]);
    assert!(!ok, "Exit-Code 1 erwartet, stdout: {stdout}");
    // Nur das Angebot unter 1.50 € trifft
    assert!(stdout.contains("1 Treffer"), "stdout: {stdout}");
    assert!(stdout.contains("1.39 €"), "stdout: {stdout}");
    assert!(!stdout.contains("1.59 €"), "stdout: {stdout}");
}

#[test]
fn watch_check_exits_0_without_hits() {
    let dbf = fixture_db("watch_miss");
    let db = dbf.to_str().unwrap();
    run(&["watch", "add", "Gibtesnicht", "--db", db]);
    let (stdout, _, ok) = run(&["watch", "check", "--db", db]);
    assert!(ok, "Exit-Code 0 erwartet, stdout: {stdout}");
    assert!(stdout.contains("keine Treffer"), "stdout: {stdout}");
}

#[test]
fn list_suggest_shows_cheapest_market() {
    let dbf = fixture_db("suggest");
    let db = dbf.to_str().unwrap();
    run(&["list", "add", "Butter", "--db", db]);
    run(&["list", "add", "Raumschiff", "--db", db]);
    let (stdout, _, ok) = run(&["list", "suggest", "--db", db]);
    assert!(ok);
    // Günstigster Markt (Zwei, 1.39) gewinnt, inkl. Grundpreis und Ersparnis
    assert!(stdout.contains("1.39 € bei Testmarkt Zwei"), "stdout: {stdout}");
    assert!(stdout.contains("5.56 €/kg"), "stdout: {stdout}");
    assert!(stdout.contains("spart 0.20 € gegenüber Testmarkt Eins"), "stdout: {stdout}");
    assert!(stdout.contains("Raumschiff") && stdout.contains("keine Angebote"), "stdout: {stdout}");
}

#[test]
fn deals_lists_price_drops_biggest_first() {
    let dbf = fixture_db("deals");
    let db = dbf.to_str().unwrap();
    // Synthetischer Verlauf: o1 war 1.00 € teurer, o4 0.50 € teurer
    let conn = lechariot::db::open(db).unwrap();
    conn.execute_batch(
        "INSERT INTO price_history (offer_id, market_id, title, price, seen_at)
         VALUES ('o1', 'M1', 'MEGGLE Feine Butter', 2.59, date('now', '-7 days')),
                ('o4', 'M2', 'Käse \"Extra\", gereift', 3.49, date('now', '-7 days'));",
    )
    .unwrap();
    drop(conn);
    let (stdout, _, ok) = run(&["deals", "--db", db]);
    assert!(ok);
    assert!(stdout.contains("Preissenkungen (2)"), "stdout: {stdout}");
    // Größter Preissturz zuerst
    let pos_big = stdout.find("-1.00 €").expect("größter Drop fehlt");
    let pos_small = stdout.find("-0.50 €").expect("kleiner Drop fehlt");
    assert!(pos_big < pos_small, "stdout: {stdout}");
    assert!(stdout.contains("1.59 € statt 2.59 €"), "stdout: {stdout}");
}

#[test]
fn deals_since_filters_old_drops() {
    let dbf = fixture_db("deals_since");
    let db = dbf.to_str().unwrap();
    // Drop, dessen letzter Stand 10 Tage alt ist (altes Angebot verschwunden)
    let conn = lechariot::db::open(db).unwrap();
    conn.execute_batch(
        "INSERT INTO price_history (offer_id, market_id, title, price, seen_at)
         VALUES ('alt', 'M1', 'Altes Produkt', 5.00, date('now', '-20 days')),
                ('alt', 'M1', 'Altes Produkt', 3.00, date('now', '-10 days'));",
    )
    .unwrap();
    drop(conn);
    let (stdout, _, ok) = run(&["deals", "--since", "3", "--db", db]);
    assert!(ok);
    assert!(stdout.contains("Keine Preissenkungen"), "stdout: {stdout}");
    let (stdout, _, ok) = run(&["deals", "--since", "14", "--db", db]);
    assert!(ok);
    assert!(stdout.contains("Altes Produkt"), "stdout: {stdout}");
}

#[test]
fn search_finds_offers_by_title() {
    let dbf = fixture_db("search");
    let (stdout, _, ok) = run(&["search", "Butter", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    assert!(stdout.contains("MEGGLE Feine Butter"), "stdout: {stdout}");
    assert!(stdout.contains("2 Treffer"), "stdout: {stdout}");
    // Billigstes zuerst (ORDER BY price)
    let pos139 = stdout.find("1.39 €").unwrap();
    let pos159 = stdout.find("1.59 €").unwrap();
    assert!(pos139 < pos159);
}

#[test]
fn compare_groups_and_sorts_cheapest_first() {
    let dbf = fixture_db("compare");
    let (stdout, _, ok) = run(&["compare", "Butter", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    // Eine Gruppe, günstigster Markt (Zwei) vor Markt Eins
    let pos2 = stdout.find("Testmarkt Zwei").expect("Markt Zwei fehlt");
    let pos1 = stdout.find("Testmarkt Eins").expect("Markt Eins fehlt");
    assert!(pos2 < pos1, "stdout: {stdout}");
    // Grundpreis aus expliziter Angabe
    assert!(stdout.contains("5.56 €/kg"), "stdout: {stdout}");
}

#[test]
fn compare_matches_subtitle_for_kaufland_style_offers() {
    let dbf = fixture_db("compare_sub");
    let (stdout, _, ok) = run(&["compare", "Eistee", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    assert!(stdout.contains("PFANNER Eistee"), "stdout: {stdout}");
    assert!(stdout.contains("0.56 €/l"), "stdout: {stdout}");
}

#[test]
fn export_json_roundtrips() {
    let dbf = fixture_db("json");
    let (stdout, _, ok) = run(&["export", "--format", "json", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    let offers: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("gültiges JSON");
    assert_eq!(offers.len(), 4);
    assert!(offers.iter().any(|o| o["title"] == "PFANNER"));
}

#[test]
fn export_csv_escapes_special_characters() {
    let dbf = fixture_db("csv");
    let (stdout, _, ok) = run(&["export", "--format", "csv", "--query", "Käse", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    let mut lines = stdout.lines();
    assert!(lines.next().unwrap().starts_with("id,market_id,title"));
    let row = lines.next().expect("Datenzeile");
    // Anführungszeichen verdoppelt, Feld mit Komma in Quotes
    assert!(row.contains("\"Käse \"\"Extra\"\", gereift\""), "row: {row}");
    assert_eq!(stdout.lines().filter(|l| !l.is_empty()).count(), 2);
}

#[test]
fn export_query_filters() {
    let dbf = fixture_db("filter");
    let (stdout, _, ok) = run(&["export", "--format", "json", "--query", "Butter", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    let offers: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(offers.len(), 2);
}
