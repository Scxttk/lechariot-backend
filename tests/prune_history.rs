//! Tests für `prune-history`: Stichtag, Dry-Run-Schutz und die beiden
//! Löschläufe. Mock-Server statt Live-Netzwerk (wie tests/push.rs).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use lechariot::prune::{HistoryPruneOptions, run_history};
use lechariot::push::PushConfig;

#[derive(Debug, Clone)]
struct Req {
    method: String,
    target: String,
}

/// Antwortet auf GET mit `content-range: 0-0/<count>` (der Zähler-Pfad) und
/// auf DELETE mit 204. Alle Requests werden protokolliert.
fn spawn_mock(count: usize) -> (String, Arc<Mutex<Vec<Req>>>) {
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
                if content_length > 0 {
                    let mut body = vec![0u8; content_length];
                    reader.read_exact(&mut body).unwrap();
                }
                log2.lock().unwrap().push(Req { method: method.clone(), target });
                let resp = if method == "DELETE" {
                    "HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n".to_string()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-range: 0-0/{count}\r\ncontent-length: 2\r\n\r\n[]"
                    )
                };
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

fn decode(target: &str) -> String {
    target
        .replace("%3A", ":")
        .replace("%2B", "+")
        .replace("%2C", ",")
        .replace("%3F", "?")
}

#[test]
fn dry_run_counts_but_never_deletes() {
    let (base_url, log) = spawn_mock(62793);
    let opts = HistoryPruneOptions { execute: false, weeks: 26 };

    run_history(&opts, Some(&cfg(&base_url))).unwrap();

    let reqs = log.lock().unwrap().clone();
    assert!(reqs.iter().all(|r| r.method == "GET"), "Dry-Run darf nichts löschen: {reqs:#?}");
    // Drei Zählungen: gesamt, datiert-und-alt, undatiert-und-alt.
    assert_eq!(reqs.len(), 3, "{reqs:#?}");
    assert!(reqs.iter().all(|r| r.target.contains("select=id")), "{reqs:#?}");
}

// Zwei Löschläufe, und der zweite ist der, den man vergisst: Altbestand ohne
// `valid_from` ließe sich über den Datums-Filter nie fassen und bliebe für
// immer liegen.
#[test]
fn execute_deletes_dated_and_undated_rows() {
    let (base_url, log) = spawn_mock(1000);
    let opts = HistoryPruneOptions { execute: true, weeks: 26 };

    run_history(&opts, Some(&cfg(&base_url))).unwrap();

    let reqs = log.lock().unwrap().clone();
    let deletes: Vec<String> =
        reqs.iter().filter(|r| r.method == "DELETE").map(|r| decode(&r.target)).collect();
    assert_eq!(deletes.len(), 2, "{reqs:#?}");

    // Erster Lauf: alles vor dem Stichtag.
    assert!(deletes[0].contains("valid_from=lt.20"), "{}", deletes[0]);
    assert!(!deletes[0].contains("recorded_at"), "{}", deletes[0]);

    // Zweiter Lauf: ohne valid_from, datiert über recorded_at.
    assert!(deletes[1].contains("valid_from=is.null"), "{}", deletes[1]);
    assert!(deletes[1].contains("recorded_at=lt.20"), "{}", deletes[1]);
}

// Der Stichtag muss aus der Aufbewahrung folgen, sonst löscht ein Tippfehler
// im Aufruf stillschweigend zu viel.
#[test]
fn cutoff_follows_the_retention_window() {
    let (base_url, log) = spawn_mock(10);

    for weeks in [1i64, 26, 104] {
        log.lock().unwrap().clear();
        run_history(&HistoryPruneOptions { execute: false, weeks }, Some(&cfg(&base_url))).unwrap();
        let reqs = log.lock().unwrap().clone();
        let dated = reqs.iter().map(|r| decode(&r.target)).find(|t| t.contains("valid_from=lt.")).unwrap();
        let cutoff = dated
            .split("valid_from=lt.")
            .nth(1)
            .unwrap()
            .chars()
            .take(10)
            .collect::<String>();
        let expected = (chrono::Utc::now() - chrono::Duration::weeks(weeks))
            .format("%Y-%m-%d")
            .to_string();
        assert_eq!(cutoff, expected, "Stichtag für {weeks} Wochen falsch");
    }
}

// Ohne diese Bremse macht `--weeks 0` aus dem Aufräumen ein Leeren.
#[test]
fn zero_weeks_is_refused() {
    let (base_url, _log) = spawn_mock(10);
    let err = run_history(&HistoryPruneOptions { execute: true, weeks: 0 }, Some(&cfg(&base_url)))
        .unwrap_err()
        .to_string();
    assert!(err.contains("mindestens 1"), "{err}");
}
