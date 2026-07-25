//! Tests für den Multi-Region-Sync: Regionen laden, Cap, markets-Upsert,
//! Fehlerisolierung und Fallback-relevante Fehlerfälle. Mock-Server statt
//! Live-Netzwerk (wie tests/push.rs).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use smartshop::models::{Market, Offer};
use smartshop::push::PushConfig;
use smartshop::sync::{self, FetchResult, SyncOptions};

/// Scraper-Stub für den bundesweiten Lauf: liefert nichts, also pusht
/// `sync_national` auch nichts. Die Tests hier prüfen den Regions-Weg; der
/// bundesweite hat eigene Tests weiter unten.
fn no_national(
    _store: smartshop::stores::Store,
    _market: &Market,
) -> anyhow::Result<Vec<Offer>> {
    Ok(Vec::new())
}

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

/// Minimaler HTTP/1.1-Mock: GET auf /rest/v1/regions liefert `regions_body`,
/// alles andere 200 `[]`. Alle Requests werden protokolliert.
fn spawn_mock(regions_body: &'static str) -> (String, Arc<Mutex<Vec<Req>>>) {
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
                let is_regions_get = method == "GET" && target.starts_with("/rest/v1/regions");
                log2.lock().unwrap().push(Req {
                    method,
                    target,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let payload = if is_regions_get { regions_body } else { "[]" };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{payload}",
                    payload.len()
                );
                if reader.get_mut().write_all(resp.as_bytes()).is_err() {
                    break;
                }
            }
        }
    });
    (format!("http://{addr}"), log)
}

/// Antwortet immer mit dem angegebenen Fehlerstatus.
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
    let dir = std::env::temp_dir().join(format!("smartshop-sync-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.db").to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&path);
    path
}

fn cfg(base_url: &str) -> PushConfig {
    PushConfig { base_url: base_url.to_string(), api_key: "test-key".to_string() }
}

fn opts(db_path: &str) -> SyncOptions {
    SyncOptions {
        db_path: db_path.to_string(),
        dry_run: false,
        max_regions: 10,
        only: None,
        market_id: None,
    }
}

/// Fetcher-Stub: eine REWE-Kette mit 2 Angeboten, protokolliert die PLZs.
fn ok_fetcher(calls: Arc<Mutex<Vec<String>>>) -> impl Fn(&str) -> FetchResult {
    move |plz: &str| {
        calls.lock().unwrap().push(plz.to_string());
        vec![(
            "REWE".to_string(),
            Ok(Some((
                Market::new("m1", format!("REWE Filiale {plz}")).with_geo(Some(51.02), Some(13.75)),
                vec![offer("Gouda", Some(1.99)), offer("Butter", Some(2.49))],
            ))),
        )]
    }
}

// ---------------------------------------------------------------- Tests

#[test]
fn fetch_regions_parses_plz_list_in_order() {
    let (base_url, log) = spawn_mock(r#"[{"plz":"01219"},{"plz":"10115"},{"plz":"80331"}]"#);
    let regions = sync::fetch_regions(&cfg(&base_url)).unwrap();
    assert_eq!(
        regions.iter().map(|r| r.plz.as_str()).collect::<Vec<_>>(),
        vec!["01219", "10115", "80331"]
    );

    let reqs = log.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    let get = &reqs[0];
    assert_eq!(get.method, "GET");
    assert!(get.target.starts_with("/rest/v1/regions?"), "Target: {}", get.target);
    assert!(get.target.contains("active=eq.true"), "Target: {}", get.target);
    // Unsyncte Regionen zuerst, dann älteste Anfrage.
    assert!(
        get.target.contains("order=last_synced.asc.nullsfirst%2Crequested_at.asc")
            || get.target.contains("order=last_synced.asc.nullsfirst,requested_at.asc"),
        "Target: {}",
        get.target
    );
    assert_eq!(get.header("apikey"), Some("test-key"));
    assert_eq!(get.header("authorization"), Some("Bearer test-key"));
}

#[test]
fn cap_limits_number_of_synced_regions() {
    let regions: Vec<String> = (0..12).map(|i| format!(r#"{{"plz":"{:05}"}}"#, 10000 + i)).collect();
    let body: &'static str = Box::leak(format!("[{}]", regions.join(",")).into_boxed_str());
    let (base_url, _log) = spawn_mock(body);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let fetcher = ok_fetcher(calls.clone());
    let db_path = temp_db("cap");
    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    let called = calls.lock().unwrap().clone();
    assert_eq!(called.len(), 10, "nur max_regions Regionen syncen: {called:?}");
    assert_eq!(called[0], "10000");
    assert_eq!(called[9], "10009");
}

#[test]
fn markets_upsert_sends_expected_payload() {
    let (base_url, log) = spawn_mock(r#"[{"plz":"01219"}]"#);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fetcher = ok_fetcher(calls);
    let db_path = temp_db("markets");
    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    let reqs = log.lock().unwrap().clone();
    let markets: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/markets"))
        .collect();
    assert_eq!(markets.len(), 1, "Requests: {reqs:#?}");
    let m = markets[0];
    assert!(
        m.target.contains("on_conflict=chain%2Cplz") || m.target.contains("on_conflict=chain,plz"),
        "Target: {}",
        m.target
    );
    assert_eq!(m.header("prefer"), Some("resolution=merge-duplicates"));
    let body: serde_json::Value = serde_json::from_str(&m.body).unwrap();
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["chain"], "REWE");
    assert_eq!(rows[0]["branch_name"], "REWE Filiale 01219");
    assert_eq!(rows[0]["market_id"], "m1");
    assert_eq!(rows[0]["plz"], "01219");
    assert_eq!(rows[0]["lat"], 51.02);
    assert_eq!(rows[0]["lon"], 13.75);
    assert!(rows[0]["updated_at"].is_string());

    // Danach läuft der bestehende Push: Offers-Upsert + regions.last_synced
    assert!(reqs.iter().any(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers")));
    assert!(reqs.iter().any(|r| r.method == "POST" && r.target.starts_with("/rest/v1/regions")));
}

#[test]
fn per_region_failure_does_not_abort_run() {
    let (base_url, log) = spawn_mock(r#"[{"plz":"00001"},{"plz":"00002"}]"#);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let inner = ok_fetcher(calls.clone());
    let fetcher = |plz: &str| -> FetchResult {
        if plz == "00001" {
            calls.lock().unwrap().push(plz.to_string());
            vec![("REWE".to_string(), Err(anyhow!("Scraper kaputt")))]
        } else {
            inner(plz)
        }
    };
    let db_path = temp_db("isolation");
    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    assert_eq!(calls.lock().unwrap().len(), 2, "beide Regionen versucht");
    let reqs = log.lock().unwrap().clone();
    // markets-Upsert nur für die erfolgreiche Region 00002
    let markets: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/markets"))
        .collect();
    assert_eq!(markets.len(), 1, "Requests: {reqs:#?}");
    assert!(markets[0].body.contains("00002"));
}

#[test]
fn all_regions_failing_returns_error() {
    let (base_url, _log) = spawn_mock(r#"[{"plz":"00001"},{"plz":"00002"}]"#);
    let fetcher =
        |_plz: &str| -> FetchResult { vec![("REWE".to_string(), Err(anyhow!("Scraper kaputt")))] };
    let db_path = temp_db("allfail");
    let err = sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap_err();
    assert!(err.to_string().contains("Alle 2 Region(en) fehlgeschlagen"), "Fehler: {err:#}");
}

#[test]
fn empty_regions_table_returns_error() {
    let (base_url, _log) = spawn_mock("[]");
    let fetcher = ok_fetcher(Arc::new(Mutex::new(Vec::new())));
    let db_path = temp_db("empty");
    let err = sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap_err();
    assert!(err.to_string().contains("Keine aktiven Regionen"), "Fehler: {err:#}");
}

#[test]
fn unreachable_or_failing_supabase_returns_error() {
    let base_url = spawn_failing_mock(500, "kaputt");
    let fetcher = ok_fetcher(Arc::new(Mutex::new(Vec::new())));
    let db_path = temp_db("unreachable");
    let err = sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap_err();
    assert!(err.to_string().contains("Regionen laden fehlgeschlagen"), "Fehler: {err:#}");
}

#[test]
fn dry_run_only_reads_regions() {
    let (base_url, log) = spawn_mock(r#"[{"plz":"01219"}]"#);
    let fetcher = ok_fetcher(Arc::new(Mutex::new(Vec::new())));
    let db_path = temp_db("dryrun");
    let opts = SyncOptions { db_path, dry_run: true, max_regions: 10, only: None, market_id: None };
    sync::run(&opts, Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    let reqs = log.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1, "nur der regions-GET erwartet: {reqs:#?}");
    assert_eq!(reqs[0].method, "GET");
    assert!(reqs[0].target.starts_with("/rest/v1/regions"));
}

#[test]
fn only_mode_registers_and_syncs_single_plz_without_region_list() {
    let (base_url, log) = spawn_mock("[]");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fetcher = ok_fetcher(calls.clone());
    let db_path = temp_db("only");
    let opts = SyncOptions {
        db_path,
        dry_run: false,
        max_regions: 10,
        only: Some("04626".to_string()),
        market_id: None,
    };
    sync::run(&opts, Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    // Genau die eine PLZ wurde gescrapet …
    assert_eq!(calls.lock().unwrap().clone(), vec!["04626"]);

    let reqs = log.lock().unwrap().clone();
    // … die Regionsliste wurde NICHT geladen …
    assert!(
        !reqs.iter().any(|r| r.method == "GET" && r.target.starts_with("/rest/v1/regions")),
        "{reqs:#?}"
    );
    // … und die PLZ wurde idempotent registriert (erster regions-POST; der
    // Push schickt am Ende noch den last_synced-Upsert an dieselbe Tabelle).
    let register = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/regions"))
        .expect("regions-POST fehlt");
    assert!(register.body.contains("04626"), "Body: {}", register.body);
}

#[test]
fn chain_without_nearby_branch_is_skipped_not_failed() {
    let (base_url, log) = spawn_mock(r#"[{"plz":"01219"}]"#);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let inner = ok_fetcher(calls);
    // REWE liefert, Lidl hat laut Store-Finder keine Filiale in der Nähe.
    let fetcher = |plz: &str| -> FetchResult {
        let mut result = inner(plz);
        result.push(("Lidl".to_string(), Ok(None)));
        result
    };
    let db_path = temp_db("no-branch");
    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    let reqs = log.lock().unwrap().clone();
    let markets: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/markets"))
        .collect();
    assert_eq!(markets.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&markets[0].body).unwrap();
    let rows = body.as_array().unwrap();
    // Nur REWE registriert — Lidl taucht nicht auf
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["chain"], "REWE");
    // Region gilt trotzdem als erfolgreich: Offers-Push lief
    assert!(reqs.iter().any(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers")));

    // Definitives "keine Filiale" räumt die evtl. vorhandene Markt-Zeile ab.
    let dels: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "DELETE" && r.target.starts_with("/rest/v1/markets"))
        .collect();
    assert_eq!(dels.len(), 1, "Requests: {reqs:#?}");
    assert!(dels[0].target.contains("chain=eq.Lidl"), "{}", dels[0].target);
    assert!(dels[0].target.contains("plz=eq.01219"), "{}", dels[0].target);

    // … und die Angebote gleich mit: ohne Filiale bleiben sie sonst für immer
    // in der Region liegen, der Push räumt nur Ketten ab, die er selbst pusht.
    let offer_dels: Vec<&Req> = reqs
        .iter()
        .filter(|r| {
            r.method == "DELETE"
                && r.target.starts_with("/rest/v1/offers")
                && r.target.contains("market=eq.Lidl")
        })
        .collect();
    assert_eq!(offer_dels.len(), 1, "Requests: {reqs:#?}");
    assert!(offer_dels[0].target.contains("region=eq.01219"), "{}", offer_dels[0].target);
}

// Finder-Fehler dürfen NICHT löschen — nur ein definitives Ok(None). Fehler
// erreichen sync als Err (bzw. via Fallback als Ok(Some)) und lassen die
// bestehende Markt-Zeile stehen.
#[test]
fn chain_error_keeps_existing_market_row() {
    let (base_url, log) = spawn_mock(r#"[{"plz":"01219"}]"#);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let inner = ok_fetcher(calls);
    let fetcher = |plz: &str| -> FetchResult {
        let mut result = inner(plz);
        result.push(("Lidl".to_string(), Err(anyhow!("Finder kaputt"))));
        result
    };
    let db_path = temp_db("chain-err");
    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    let reqs = log.lock().unwrap().clone();
    assert!(
        !reqs.iter().any(|r| r.method == "DELETE" && r.target.starts_with("/rest/v1/markets")),
        "Fehler darf keine Markt-Zeile löschen: {reqs:#?}"
    );
    assert!(
        !reqs.iter().any(|r| r.method == "DELETE" && r.target.contains("market=eq.Lidl")),
        "Fehler darf keine Angebote löschen: {reqs:#?}"
    );
}

// ------------------------------------------------- Vorab-Kopie (Seeding)

/// Wie spawn_mock, aber GET-Antworten sind pro Ziel-Substring konfigurierbar
/// (erste passende Route gewinnt); alles andere 200 `[]`.
fn spawn_routed_mock(
    routes: &'static [(&'static str, &'static str)],
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
                let payload = if method == "GET" {
                    routes
                        .iter()
                        .find(|(needle, _)| target.contains(needle))
                        .map(|(_, body)| *body)
                        .unwrap_or("[]")
                } else {
                    "[]"
                };
                log2.lock().unwrap().push(Req {
                    method,
                    target,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{payload}",
                    payload.len()
                );
                if reader.get_mut().write_all(resp.as_bytes()).is_err() {
                    break;
                }
            }
        }
    });
    (format!("http://{addr}"), log)
}

/// Was der On-Demand-Sync NICHT mehr tut: fremde Angebote in die neue Region
/// kopieren. Bis Phase 12 wurde der Lidl-Katalog aus der ergiebigsten anderen
/// Region geholt und auf die Ziel-Filiale umgeschrieben — 743 Dresdner Zeilen
/// standen so als Passauer Angebote da, ohne jede Kennzeichnung.
#[test]
fn only_mode_copies_nothing_it_has_not_scraped() {
    let (base_url, log) = spawn_mock("[]");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fetcher = ok_fetcher(calls.clone());
    let db_path = temp_db("kein-seed");
    let opts = SyncOptions {
        db_path,
        dry_run: false,
        max_regions: 10,
        only: Some("10115".to_string()),
        market_id: None,
    };

    sync::run(&opts, Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    let reqs = log.lock().unwrap().clone();
    // Keine Suche nach einer Quellregion …
    assert!(
        !reqs.iter().any(|r| r.method == "GET"
            && r.target.starts_with("/rest/v1/offers")
            && r.target.contains("select=region")),
        "Quellregion-Suche der Vorab-Kopie ist noch da: {reqs:#?}"
    );
    // … und hochgeladen wird nur, was der Fetcher wirklich geliefert hat.
    for req in reqs.iter().filter(|r| {
        r.method == "POST" && r.target.starts_with("/rest/v1/offers?")
    }) {
        assert!(req.body.contains("REWE"), "fremde Zeilen im Upsert: {}", req.body);
    }
    assert_eq!(*calls.lock().unwrap(), vec!["10115".to_string()]);
}

// ------------------------------------------------------- Filial-Sync (v13)

/// Zwei REWE-Filialen in derselben PLZ, wie sie in 01067 wirklich stehen.
const BRANCH_POSTPLATZ: &str = r#"[{
    "market_id": "1766063", "chain": "REWE",
    "name": "REWE Ketzscher oHG am Postplatz",
    "street": "Wallstr. 2b", "plz": "01067", "city": "Dresden",
    "lat": 51.0504, "lon": 13.7317, "source": "rewe-marketsearch"
}]"#;

#[test]
fn branch_mode_scrapes_exactly_the_requested_branch() {
    static ROUTES: [(&str, &str); 1] = [("branches", BRANCH_POSTPLATZ)];
    let (base_url, log) = spawn_routed_mock(&ROUTES);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let scraper = move |branch: &smartshop::models::Branch| {
        seen2.lock().unwrap().push(branch.market_id.clone());
        let mut o = offer("Coca-Cola", Some(0.75));
        o.market_id = branch.market_id.clone();
        Ok(vec![o])
    };
    let db_path = temp_db("filiale");
    let mut opts = opts(&db_path);
    opts.market_id = Some("1766063".to_string());

    sync::run_branch(&opts, Some(&cfg(&base_url)), &scraper, &no_national, "1766063").unwrap();

    // Der Scraper bekommt die angeforderte Filiale — kein Store-Finder,
    // keine „nächste Filiale zur PLZ".
    assert_eq!(*seen.lock().unwrap(), vec!["1766063".to_string()]);

    let reqs = log.lock().unwrap().clone();
    let lookup = reqs
        .iter()
        .find(|r| r.method == "GET" && r.target.starts_with("/rest/v1/branches"))
        .expect("Verzeichnis-Abfrage fehlt");
    assert!(lookup.target.contains("market_id=eq.1766063"), "{}", lookup.target);

    // Die Angebote gehen unter der Filiale hoch, mit deren eigener PLZ als
    // Region — nicht unter der Kette.
    let upsert = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .expect("offers-Upsert fehlt");
    let rows: serde_json::Value = serde_json::from_str(&upsert.body).unwrap();
    assert_eq!(rows[0]["market_id"], "1766063");
    assert_eq!(rows[0]["market"], "REWE");
    assert_eq!(rows[0]["region"], "01067");

    // Aufgeräumt wird nur diese Filiale — die Nachbarfiliale derselben PLZ
    // bleibt unangetastet.
    let deletes: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "DELETE" && r.target.starts_with("/rest/v1/offers"))
        .collect();
    assert!(deletes.iter().all(|d| d.target.contains("market_id=eq.1766063")), "{deletes:#?}");
    assert!(!deletes.iter().any(|d| d.target.contains("market=eq.REWE")), "{deletes:#?}");

    // … und die Filiale wird als die des Gebiets gemeldet.
    let market = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/markets"))
        .expect("markets-Upsert fehlt");
    assert!(market.body.contains("1766063"), "{}", market.body);
    assert!(market.body.contains("01067"), "{}", market.body);

    // Zuletzt die Fertigmeldung, sonst pollt die App bis zum Timeout weiter.
    let done_pos = reqs
        .iter()
        .position(|r| r.method == "POST" && r.target.starts_with("/rest/v1/branch_requests"))
        .expect("Fertigmeldung an branch_requests fehlt");
    let done = &reqs[done_pos];
    assert!(done.target.contains("on_conflict=market_id"), "{}", done.target);
    let body: serde_json::Value = serde_json::from_str(&done.body).unwrap();
    assert_eq!(body[0]["market_id"], "1766063");
    assert!(body[0]["last_synced"].as_str().unwrap().starts_with("20"), "{}", done.body);
    // Erst die Angebote, dann „fertig" — andersherum liest die App eine
    // Fertigmeldung auf eine noch leere Region.
    let upsert_pos = reqs
        .iter()
        .position(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .unwrap();
    assert!(upsert_pos < done_pos, "Fertigmeldung kam vor den Angeboten: {reqs:#?}");
}

#[test]
fn branch_mode_fails_loudly_on_unknown_market_id() {
    let (base_url, _log) = spawn_mock("[]");
    let scraper = |_: &smartshop::models::Branch| Ok(Vec::new());
    let db_path = temp_db("filiale-unbekannt");
    let opts = opts(&db_path);

    let err = sync::run_branch(&opts, Some(&cfg(&base_url)), &scraper, &no_national, "4711")
        .unwrap_err()
        .to_string();
    assert!(err.contains("4711"), "{err}");
    assert!(err.contains("Verzeichnis"), "{err}");
}

// -------------------------------------------- Bundesweite Ketten (Phase 12)

/// Angebot mit ausdrücklicher Filial-ID — der Push gruppiert über sie.
fn offer_for(market_id: &str, title: &str, price: f64) -> Offer {
    let mut o = offer(title, Some(price));
    o.id = Offer::build_id(market_id, title, Some("2026-07-13"));
    o.market_id = market_id.to_string();
    o
}

/// Fetcher-Stub für eine Region mit ALDI: Die Kette wird gefunden, liefert
/// hier aber **keine** Angebote — genau so ruft `main` sie seit Phase 12 auf
/// (nur `find_market`, kein Angebots-Abruf).
fn aldi_fetcher(chains: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> FetchResult {
    move |plz: &str| {
        chains
            .iter()
            .map(|(chain, market_id)| {
                (
                    chain.to_string(),
                    Ok(Some((
                        Market::new(*market_id, format!("{chain} {plz}")).with_chain(*chain),
                        Vec::new(),
                    ))),
                )
            })
            .collect()
    }
}

/// Der Kern von Schritt 5: ALDI wird **einmal** gespeichert, ohne Region.
/// Vorher bekam jede der 16 Regionen ihre eigene Kopie desselben Katalogs.
#[test]
fn national_chains_are_pushed_once_without_a_region() {
    let (base_url, log) = spawn_mock(r#"[{"plz":"01219"},{"plz":"01067"}]"#);
    let fetcher = ok_fetcher(Arc::new(Mutex::new(Vec::new())));
    let national = |store: smartshop::stores::Store, market: &Market| {
        assert!(store.stores_nationally(), "{} ist nicht bundesweit", store.chain());
        Ok(vec![
            offer_for(&market.id, "Ofenkäse", 2.22),
            offer_for(&market.id, "Rispentomaten", 1.11),
        ])
    };
    let db_path = temp_db("national");
    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &national).unwrap();

    let reqs = log.lock().unwrap().clone();
    let national_rows: Vec<serde_json::Value> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .filter_map(|r| serde_json::from_str::<serde_json::Value>(&r.body).ok())
        .flat_map(|v| v.as_array().cloned().unwrap_or_default())
        .filter(|row| row["market"].as_str().is_some_and(|m| m.starts_with("ALDI")))
        .collect();

    assert_eq!(national_rows.len(), 4, "je 2 Angebote für Nord und SÜD: {national_rows:#?}");
    for row in &national_rows {
        assert!(row["region"].is_null(), "bundesweite Zeile mit Region: {row}");
    }
    let ids: std::collections::BTreeSet<&str> =
        national_rows.iter().filter_map(|r| r["market_id"].as_str()).collect();
    assert_eq!(
        ids,
        ["ALDI_NORD_DE", "ALDI_SUED_DE"].into_iter().collect(),
        "unter der National-Filiale gespeichert, nicht unter einer echten"
    );

    // Der eigentliche Punkt steckt in der Zahl oben: zwei Regionen, aber
    // trotzdem nur 2×2 Zeilen. Im alten Modell wären es 2×2×2 gewesen — und
    // in Produktion 2.965 statt rund 320.
}

/// Der Regions-Sync meldet die ALDI-Filiale weiterhin nach `markets` (die App
/// muss wissen, dass es die Kette hier gibt), speichert aber keine Angebote
/// mehr für sie — und räumt weg, was der alte, regionale Weg hinterlassen hat.
#[test]
fn region_sync_reports_the_aldi_branch_but_stores_no_offers_for_it() {
    static CHAINS: [(&str, &str); 1] = [("ALDI Nord", "ALDI_NORD_4711")];
    let (base_url, log) = spawn_mock(r#"[{"plz":"01219"}]"#);
    let fetcher = aldi_fetcher(&CHAINS);
    let db_path = temp_db("aldi-region");
    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &no_national).unwrap();

    let reqs = log.lock().unwrap().clone();
    let market = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/markets"))
        .expect("markets-Upsert fehlt");
    assert!(market.body.contains("ALDI_NORD_4711"), "{}", market.body);
    assert!(market.body.contains("01219"), "{}", market.body);

    assert!(
        !reqs.iter().any(|r| r.method == "POST"
            && r.target.starts_with("/rest/v1/offers?")
            && r.body.contains("ALDI")),
        "der Regions-Sync darf keine ALDI-Angebote mehr hochladen: {reqs:#?}"
    );

    // Nachlass aufräumen: die regionalen Kopien aus der Zeit vor Phase 12.
    assert!(
        reqs.iter().any(|r| r.method == "DELETE"
            && r.target.starts_with("/rest/v1/offers")
            && r.target.contains("market=eq.ALDI+Nord")
            && r.target.contains("region=eq.01219")),
        "regionale ALDI-Zeilen werden nicht gelöscht: {reqs:#?}"
    );
}

/// Pflicht-Testfall am Aldi-Äquator: 96515 Sonneberg hat ALDI SÜD im Ort und
/// ALDI Nord 15 km nördlich. Beide Kataloge müssen nebeneinander stehen —
/// zwei Ketten, zwei National-Filialen, keine verdrängt die andere.
#[test]
fn at_the_aldi_equator_both_catalogues_stand_side_by_side() {
    static CHAINS: [(&str, &str); 2] =
        [("ALDI Nord", "ALDI_NORD_NEUHAUS"), ("ALDI SÜD", "ALDI_SUED_SONNEBERG")];
    let (base_url, log) = spawn_mock(r#"[{"plz":"96515"}]"#);
    let fetcher = aldi_fetcher(&CHAINS);
    let national = |_: smartshop::stores::Store, market: &Market| {
        // Zwei klar unterscheidbare Kataloge.
        Ok(vec![offer_for(&market.id, &format!("Katalog {}", market.id), 1.0)])
    };
    let db_path = temp_db("aldi-aequator");
    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &fetcher, &national).unwrap();

    let reqs = log.lock().unwrap().clone();

    // Beide Filialen sind in 96515 gemeldet.
    let markets: String = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/markets"))
        .map(|r| r.body.clone())
        .collect();
    assert!(markets.contains("ALDI_NORD_NEUHAUS"), "{markets}");
    assert!(markets.contains("ALDI_SUED_SONNEBERG"), "{markets}");

    // Beide Kataloge sind hochgeladen, jeder unter seiner eigenen ID.
    let rows: Vec<serde_json::Value> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .filter_map(|r| serde_json::from_str::<serde_json::Value>(&r.body).ok())
        .flat_map(|v| v.as_array().cloned().unwrap_or_default())
        .collect();
    assert!(rows.iter().any(|r| r["product"] == "Katalog ALDI_NORD_DE"), "{rows:#?}");
    assert!(rows.iter().any(|r| r["product"] == "Katalog ALDI_SUED_DE"), "{rows:#?}");

    // Und keiner räumt dem anderen die Zeilen ab. Am bundesweiten Bestand
    // (region is null) wird ausschließlich je Filiale gelöscht — ein
    // Ketten-Filter hätte hier genau den Fehler gemacht, den der Äquator
    // sichtbar macht. Die Löschung über die Kette gibt es nur mit PLZ, das
    // ist das Aufräumen der regionalen Altlast.
    let deletes: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "DELETE" && r.target.starts_with("/rest/v1/offers"))
        .collect();
    for d in &deletes {
        if d.target.contains("region=eq.96515") {
            continue; // Altlast dieser einen Region
        }
        assert!(
            d.target.contains("market_id=eq.ALDI_NORD_DE")
                || d.target.contains("market_id=eq.ALDI_SUED_DE"),
            "Löschen über die Kette statt über die Filiale: {}",
            d.target
        );
    }
    assert!(
        deletes.iter().any(|d| d.target.contains("market_id=eq.ALDI_NORD_DE")),
        "kein Aufräumen alter Wochen für ALDI Nord: {deletes:#?}"
    );
    assert!(
        deletes.iter().any(|d| d.target.contains("market_id=eq.ALDI_SUED_DE")),
        "kein Aufräumen alter Wochen für ALDI SÜD: {deletes:#?}"
    );
}

/// Eine ALDI-Filiale kann man in der App anfordern — scrapen kann man sie
/// nicht, die Angebots-APIs kennen keinen Filialbezug. Statt so zu tun als ob,
/// wird der bundesweite Katalog aufgefrischt und die Anforderung beantwortet.
#[test]
fn a_branch_request_for_aldi_refreshes_the_national_catalogue() {
    static BRANCH: &str = r#"[{
        "market_id": "ALDI_NORD_4711", "chain": "ALDI Nord",
        "name": "ALDI Nord Dresden-Prohlis",
        "street": "Prohliser Allee 10", "plz": "01239", "city": "Dresden",
        "lat": 50.99, "lon": 13.79, "source": "uberall"
    }]"#;
    static ROUTES: [(&str, &str); 1] = [("branches", BRANCH)];
    let (base_url, log) = spawn_routed_mock(&ROUTES);
    let scraper = |_: &smartshop::models::Branch| panic!("ALDI darf nicht filialweise gescrapt werden");
    let national = |_: smartshop::stores::Store, market: &Market| {
        Ok(vec![offer_for(&market.id, "Ofenkäse", 2.22)])
    };
    let db_path = temp_db("aldi-anforderung");
    let mut opts = opts(&db_path);
    opts.market_id = Some("ALDI_NORD_4711".to_string());

    sync::run_branch(&opts, Some(&cfg(&base_url)), &scraper, &national, "ALDI_NORD_4711").unwrap();

    let reqs = log.lock().unwrap().clone();
    let upsert = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .expect("offers-Upsert fehlt");
    let rows: serde_json::Value = serde_json::from_str(&upsert.body).unwrap();
    assert_eq!(rows[0]["market_id"], "ALDI_NORD_DE");
    assert!(rows[0]["region"].is_null(), "{}", upsert.body);

    // Die Anforderung wird trotzdem beantwortet, sonst pollt die App bis zum
    // Timeout auf eine Filiale, die längst Angebote zeigt.
    let done = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/branch_requests"))
        .expect("Fertigmeldung an branch_requests fehlt");
    let body: serde_json::Value = serde_json::from_str(&done.body).unwrap();
    assert_eq!(body[0]["market_id"], "ALDI_NORD_4711");
}
