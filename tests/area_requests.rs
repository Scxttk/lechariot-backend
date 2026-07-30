//! Gebiets-Anforderungen (Migration v19).
//!
//! Geprüft wird der HTTP-Vertrag mit PostgREST, nicht PostgREST selbst: welche
//! Filter gesetzt werden und was aus der Antwort gelesen wird. Genau daran
//! hing der Fehler vom 2026-07-25 — eine Anforderung, die niemand je wieder
//! anfasste, weil die Abfrage die falsche Menge lieferte.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use lechariot::branches::{
    mark_anchors_synced, mark_area_resolved, mark_areas_synced, targets_from_open_requests,
};
use lechariot::push::PushConfig;

struct Req {
    method: String,
    target: String,
    body: String,
}

/// Minimaler HTTP/1.1-Mock, der jede Anfrage protokolliert und immer mit
/// demselben JSON-Rumpf antwortet.
fn spawn_mock(body: &'static str) -> (String, Arc<Mutex<Vec<Req>>>) {
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
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut buf = vec![0u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut buf).unwrap();
                }
                log2.lock().unwrap().push(Req {
                    method,
                    target,
                    body: String::from_utf8_lossy(&buf).into_owned(),
                });
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                );
                if reader.get_mut().write_all(resp.as_bytes()).is_err() {
                    break;
                }
            }
        }
    });
    (format!("http://{addr}"), log)
}

fn cfg(base_url: &str) -> PushConfig {
    PushConfig { base_url: base_url.to_string(), api_key: "test-key".to_string() }
}

/// Die Fertigmeldung geht über die **PLZ**, nicht über die Ankerfiliale: Zwei
/// Nutzer desselben Ortes wählen verschiedene Anker und meinen dasselbe
/// Gebiet, ein Lauf bedient beide.
#[test]
fn the_completion_is_reported_per_area_not_per_anchor_branch() {
    let (base_url, log) = spawn_mock(r#"[{"market_id":"531032"},{"market_id":"4711"}]"#);

    let n = mark_areas_synced(&cfg(&base_url), &["04639".into(), "01219".into()]).unwrap();

    assert_eq!(n, 2, "beide Zeilen des Gebiets müssen gemeldet werden");
    let log = log.lock().unwrap();
    let req = log.iter().find(|r| r.method == "PATCH").expect("kein PATCH abgesetzt");
    assert!(req.target.contains("area_requests"), "falsche Tabelle: {}", req.target);
    assert!(
        req.target.contains("plz=in.") && req.target.contains("04639") && req.target.contains("01219"),
        "Filter trifft nicht beide Gebiete: {}",
        req.target
    );
    assert!(req.body.contains("last_synced"), "Rumpf setzt kein last_synced: {}", req.body);
}

/// Ohne `return=representation` antwortet PostgREST mit 204, und „0 Gebiete
/// gemeldet" wäre von „alle Gebiete gemeldet" nicht zu unterscheiden.
#[test]
fn the_completion_asks_for_the_affected_rows() {
    let (base_url, log) = spawn_mock("[]");

    let n = mark_areas_synced(&cfg(&base_url), &["04639".into()]).unwrap();

    assert_eq!(n, 0, "eine leere Antwort heißt: keine Zeile getroffen");
    let log = log.lock().unwrap();
    let req = log.iter().find(|r| r.method == "PATCH").unwrap();
    assert!(
        req.target.contains("area_requests"),
        "falsche Tabelle: {}",
        req.target
    );
}

/// Ein Fehlerstatus darf nicht als „nichts zu tun" durchgehen.
#[test]
fn a_failing_completion_is_an_error_not_a_zero() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let body = r#"{"message":"permission denied"}"#;
            let resp = format!(
                "HTTP/1.1 403 ERR\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let err = mark_areas_synced(&cfg(&format!("http://{addr}")), &["04639".into()]).unwrap_err();
    assert!(format!("{err:#}").contains("403"), "Status fehlt in der Meldung: {err:#}");
}

/// Das Sicherheitsnetz fragt genau die Zeilen ab, auf die noch jemand wartet —
/// offen und aktiv. Der Filter `plz=not.is.null` ist mit v21 weggefallen: Eine
/// Zeile mit Koordinaten und ohne PLZ ist ein gültiger Auftrag.
#[test]
fn open_requests_are_the_ones_still_waiting() {
    let (base_url, log) = spawn_mock(
        r#"[{"market_id":"a","plz":"04639"},{"market_id":"b","plz":"01219"},{"market_id":"c","plz":"04639"},{"market_id":"d"}]"#,
    );

    let targets = targets_from_open_requests(&cfg(&base_url)).unwrap();

    let areas: Vec<_> = targets.iter().filter_map(|t| t.plz.clone()).collect();
    assert_eq!(areas, vec!["04639".to_string(), "01219".to_string()], "ohne Dubletten");
    assert!(
        targets.iter().all(|t| t.anchor_market_id.is_some()),
        "jede Anforderung muss ihre Ankerzeile kennen"
    );
    let log = log.lock().unwrap();
    let req = log.iter().find(|r| r.method == "GET").expect("kein GET abgesetzt");
    for expected in ["last_synced=is.null", "active=eq.true"] {
        assert!(req.target.contains(expected), "Filter {expected} fehlt: {}", req.target);
    }
    assert!(
        !req.target.contains("plz=not.is.null"),
        "Zeilen mit Koordinaten und ohne PLZ dürfen nicht wegfiltert werden: {}",
        req.target
    );
}

/// **Die Regression zum Vorfall vom 2026-07-30.** Die Zeile trägt die PLZ der
/// Ankerfiliale (Ueckermünde) und die Koordinaten der Region (Ahlbeck). Läse
/// das Sicherheitsnetz wie vor v21 nur `plz`, holte der Sonntagslauf ein
/// zweites Mal die falsche Stadt — grün, und die Anforderung wäre danach zu.
#[test]
fn open_requests_carry_coordinates_not_just_a_postcode() {
    let (base_url, log) = spawn_mock(
        r#"[{"market_id":"4030191","plz":"17373","lat":53.9440,"lon":14.1830}]"#,
    );

    let targets = targets_from_open_requests(&cfg(&base_url)).unwrap();

    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets[0].coords,
        Some((53.9440, 14.1830)),
        "die Lage der Region muss mitkommen, sonst gewinnt die Anker-PLZ wieder"
    );
    assert_eq!(targets[0].plz.as_deref(), Some("17373"), "die Anker-PLZ bleibt als Rückfall");
    assert_eq!(targets[0].anchor_market_id.as_deref(), Some("4030191"));
    let log = log.lock().unwrap();
    let req = log.iter().find(|r| r.method == "GET").unwrap();
    assert!(
        req.target.contains("lat") && req.target.contains("lon"),
        "ohne lat/lon im select kommen die Koordinaten gar nicht erst an: {}",
        req.target
    );
}

/// Die aufgelöste PLZ landet auf der Ankerzeile — und **ohne** `last_synced`:
/// Das Gebiet nachzutragen passiert vor den Ketten, fertig ist der Lauf da
/// noch lange nicht.
#[test]
fn the_resolved_postcode_is_written_back_to_the_anchor_row() {
    let (base_url, log) = spawn_mock(r#"[{"market_id":"4030191"}]"#);

    let changed = mark_area_resolved(&cfg(&base_url), "4030191", "17419").unwrap();

    assert!(changed);
    let log = log.lock().unwrap();
    let req = log.iter().find(|r| r.method == "PATCH").expect("kein PATCH abgesetzt");
    assert!(
        req.target.contains("market_id=eq.4030191"),
        "die Zeile wird über den Anker adressiert: {}",
        req.target
    );
    assert!(req.body.contains("17419"), "die echte PLZ fehlt im Rumpf: {}", req.body);
    assert!(
        !req.body.contains("last_synced"),
        "der Lauf ist an dieser Stelle nicht fertig: {}",
        req.body
    );
}

/// Zweite Fertigmeldung über den Anker. Sie fängt den Fall, dass die PLZ auf
/// der Zeile nicht die des gelaufenen Gebiets ist — dann trifft der PLZ-Filter
/// sie nicht und die Anforderung bliebe für immer offen.
#[test]
fn the_completion_also_reports_the_anchor_row() {
    let (base_url, log) = spawn_mock(r#"[{"market_id":"4030191"}]"#);

    let n = mark_anchors_synced(&cfg(&base_url), &["4030191".into()]).unwrap();

    assert_eq!(n, 1);
    let log = log.lock().unwrap();
    let req = log.iter().find(|r| r.method == "PATCH").unwrap();
    assert!(
        req.target.contains("market_id=in.") && req.target.contains("4030191"),
        "Filter trifft die Ankerzeile nicht: {}",
        req.target
    );
    assert!(req.body.contains("last_synced"), "Rumpf setzt kein last_synced: {}", req.body);
}
