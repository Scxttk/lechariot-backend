//! Tests für den Multi-Region-Sync: Regionen laden, Cap, markets-Upsert,
//! Fehlerisolierung und Fallback-relevante Fehlerfälle. Mock-Server statt
//! Live-Netzwerk (wie tests/push.rs).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use lechariot::models::{Market, Offer};
use lechariot::push::PushConfig;
use lechariot::sync::{self, SyncOptions};

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
/// GET-Antworten pro Ziel-Substring konfigurierbar
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
    let dir = std::env::temp_dir().join(format!("lechariot-sync-{name}-{}", std::process::id()));
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
        max_branches: 25,
        market_id: None,
    }
}

/// Scraper-Stub für den bundesweiten Lauf: liefert nichts, also pusht
/// `sync_national` auch nichts.
fn no_national(
    _store: lechariot::stores::Store,
    _market: &Market,
) -> anyhow::Result<Vec<Offer>> {
    Ok(Vec::new())
}

/// Filial-Scraper-Stub, der Alarm schlägt, wenn er unerwartet gerufen wird.
fn no_branch(_branch: &lechariot::models::Branch) -> anyhow::Result<Vec<Offer>> {
    panic!("Filial-Scraper unerwartet gerufen")
}

/// Angebot mit ausdrücklicher Filial-ID — der Push gruppiert über sie.
fn offer_for(market_id: &str, title: &str, price: f64) -> Offer {
    let mut o = offer(title, Some(price));
    o.id = Offer::build_id(market_id, title, Some("2026-07-13"));
    o.market_id = market_id.to_string();
    o
}

/// Zwei REWE-Filialen in derselben PLZ, wie sie in 01067 wirklich stehen.
const BRANCH_POSTPLATZ: &str = r#"[{
    "market_id": "1766063", "chain": "REWE",
    "name": "REWE Ketzscher oHG am Postplatz",
    "street": "Wallstr. 2b", "plz": "01067", "city": "Dresden",
    "lat": 51.0504, "lon": 13.7317, "source": "rewe-marketsearch"
}]"#;

// ---------------------------------------------------------------- Tests

#[test]
fn unreachable_or_failing_supabase_returns_error() {
    let base_url = spawn_failing_mock(500, "boom");
    let db_path = temp_db("fehler");

    let err = sync::run(&opts(&db_path), Some(&cfg(&base_url)), &no_branch, &no_national)
        .unwrap_err()
        .to_string();
    // Wichtig ist die AUSSAGE: Bei kaputtem Supabase sind die gewählten
    // Filialen unbekannt, nicht leer. Ein Lauf, der das als „niemand hat etwas
    // gewählt" verbucht, meldete Erfolg und täte nichts.
    assert!(err.contains("unbekannt"), "{err}");
}

// ------------------------------------------------- Der Voll-Lauf (Phase 12)

/// Der Lauf besteht seit Migration v16 aus zwei Teilen: die bundesweiten
/// Ketten einmal, danach jede Filiale, die jemand benutzt. Der Regionsweg,
/// der hier bis dahin stand, ist weg — er stammte aus der Zeit, als ein
/// Angebot einer PLZ gehörte.
#[test]
fn the_full_run_syncs_the_branches_people_chose() {
    static ROUTES: [(&str, &str); 3] = [
        ("branch_requests", r#"[{"market_id":"1763556"}]"#),
        ("user_profiles", r#"[{"branch_ids":["1766160","1763556"]}]"#),
        ("branches", BRANCH_POSTPLATZ),
    ];
    let (base_url, log) = spawn_routed_mock(&ROUTES);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let scraper = move |branch: &lechariot::models::Branch| {
        seen2.lock().unwrap().push(branch.market_id.clone());
        let mut o = offer("Coca-Cola", Some(0.75));
        o.market_id = branch.market_id.clone();
        Ok(vec![o])
    };
    let db_path = temp_db("voll-lauf");

    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &scraper, &no_national).unwrap();

    // Beide Quellen zusammengeführt, jede Filiale genau einmal: 1763556 steht
    // in `branch_requests` UND in einem Profil.
    assert_eq!(seen.lock().unwrap().len(), 2, "je Filiale genau ein Scrape");

    let reqs = log.lock().unwrap().clone();
    let looked_up: Vec<&str> = reqs
        .iter()
        .filter(|r| r.method == "GET" && r.target.starts_with("/rest/v1/branches"))
        .map(|r| r.target.as_str())
        .collect();
    assert_eq!(looked_up.len(), 2, "{looked_up:#?}");
    assert!(looked_up.iter().any(|t| t.contains("market_id=eq.1763556")), "{looked_up:#?}");
    assert!(looked_up.iter().any(|t| t.contains("market_id=eq.1766160")), "{looked_up:#?}");

    // Und keine Spur mehr vom Regionsweg.
    assert!(
        !reqs.iter().any(|r| r.target.starts_with("/rest/v1/regions")),
        "die Regions-Tabelle wird noch angefasst: {reqs:#?}"
    );
}

/// Ohne gewählte Filialen bleibt nur der bundesweite Teil — und das ist kein
/// Fehler, sondern der Zustand einer frischen Datenbank.
#[test]
fn without_chosen_branches_only_the_nationwide_part_runs() {
    static ROUTES: [(&str, &str); 2] =
        [("branch_requests", "[]"), ("user_profiles", "[]")];
    let (base_url, log) = spawn_routed_mock(&ROUTES);
    let db_path = temp_db("keine-filialen");

    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &no_branch, &no_national).unwrap();

    let reqs = log.lock().unwrap().clone();
    assert!(
        !reqs.iter().any(|r| r.method == "GET" && r.target.starts_with("/rest/v1/branches")),
        "{reqs:#?}"
    );
}

/// Das Limit schützt manuelle Läufe. Was es überspringt, wird genannt —
/// stillschweigend gedeckelt sähe aus wie „alles erledigt".
#[test]
fn the_branch_cap_limits_one_run() {
    static ROUTES: [(&str, &str); 3] = [
        ("branch_requests", r#"[{"market_id":"a"},{"market_id":"b"},{"market_id":"c"}]"#),
        ("user_profiles", "[]"),
        ("branches", BRANCH_POSTPLATZ),
    ];
    let (base_url, _log) = spawn_routed_mock(&ROUTES);
    let calls = Arc::new(Mutex::new(0usize));
    let calls2 = calls.clone();
    let scraper = move |branch: &lechariot::models::Branch| {
        *calls2.lock().unwrap() += 1;
        let mut o = offer("Coca-Cola", Some(0.75));
        o.market_id = branch.market_id.clone();
        Ok(vec![o])
    };
    let db_path = temp_db("filial-limit");
    let mut opts = opts(&db_path);
    opts.max_branches = 2;

    sync::run(&opts, Some(&cfg(&base_url)), &scraper, &no_national).unwrap();

    assert_eq!(*calls.lock().unwrap(), 2);
}

/// Der Fall vom 31.07.2026, mit den echten IDs und Zeitstempeln aus
/// `branch_requests`.
///
/// Lauf 30612619727 war grün und meldete „32 Filialen angefordert, Limit 25 —
/// übersprungen: DE3413, DE4600, DE6330, DE7380, LIDL_1988, LIDL_3636,
/// LIDL_5745". Übersprungen wurden damit genau die sieben Filialen, die am
/// längsten nicht dran gewesen waren: Alle anderen trugen `last_synced` vom
/// 31.07., diese sieben vom 30.07. Das Limit hat also nicht irgendwen
/// abgeschnitten, sondern zielsicher die Bedürftigsten — `LIDL_*` sortiert
/// hinter `DE*`, und geschnitten wurde alphabetisch.
///
/// Der Test hält fest, dass die Warteschlange sie jetzt **vorn** führt.
#[test]
fn the_longest_unsynced_branches_come_first() {
    static ROUTES: [(&str, &str); 2] = [
        (
            "branch_requests",
            r#"[
              {"market_id":"021868","requested_at":"2026-07-27T06:07:34+00:00","last_synced":"2026-07-31T07:23:52+00:00"},
              {"market_id":"DE3260","requested_at":"2026-07-26T08:17:34+00:00","last_synced":"2026-07-31T07:27:19+00:00"},
              {"market_id":"DE3413","requested_at":"2026-07-27T06:07:53+00:00","last_synced":"2026-07-30T07:57:42+00:00"},
              {"market_id":"LIDL_1988","requested_at":"2026-07-25T20:08:28+00:00","last_synced":"2026-07-30T07:58:45+00:00"},
              {"market_id":"LIDL_3636","requested_at":"2026-07-30T19:45:43+00:00","last_synced":"2026-07-30T19:46:49+00:00"},
              {"market_id":"LIDL_5745","requested_at":"2026-07-26T09:43:51+00:00","last_synced":"2026-07-30T07:59:10+00:00"}
            ]"#,
        ),
        ("user_profiles", "[]"),
    ];
    let (base_url, _log) = spawn_routed_mock(&ROUTES);

    let branches = sync::fetch_chosen_branches(&cfg(&base_url)).unwrap();

    assert_eq!(
        branches,
        vec!["DE3413", "LIDL_1988", "LIDL_5745", "LIDL_3636", "021868", "DE3260"],
        "nach Stand des letzten Syncs, nicht nach Alphabet"
    );
    // Die Probe aufs Exempel: Bei einem Limit von 3 überlebt jetzt Lidl.
    assert_eq!(&branches[..3], &["DE3413", "LIDL_1988", "LIDL_5745"]);
}

/// Wer noch nie gesynct wurde, steht vorn — auf den wartet die App gerade.
///
/// `branch_requests.last_synced` ist die Spalte, die die App pollt (Migration
/// v14); solange sie null ist, zeigt der Picker „wird geholt". Eine nie
/// gesyncte Filiale hinten anzustellen hieße, genau den einen Nutzer warten
/// zu lassen, der gerade zusieht.
#[test]
fn never_synced_branches_outrank_stale_ones() {
    static ROUTES: [(&str, &str); 2] = [
        (
            "branch_requests",
            r#"[
              {"market_id":"laengst-fertig","requested_at":"2026-07-01T00:00:00+00:00","last_synced":"2026-07-30T07:00:00+00:00"},
              {"market_id":"neu-spaeter","requested_at":"2026-07-31T09:00:00+00:00","last_synced":null},
              {"market_id":"neu-frueher","requested_at":"2026-07-31T08:00:00+00:00","last_synced":null}
            ]"#,
        ),
        ("user_profiles", "[]"),
    ];
    let (base_url, _log) = spawn_routed_mock(&ROUTES);

    let branches = sync::fetch_chosen_branches(&cfg(&base_url)).unwrap();

    assert_eq!(branches, vec!["neu-frueher", "neu-spaeter", "laengst-fertig"]);
}

/// Eine Filiale, die nur in einem Profil steht, hat noch keine
/// `branch_requests`-Zeile und damit keinen Zeitstempel. Sie gilt als nie
/// gesynct — aber hinter den ausdrücklichen Anforderungen: Hinter einer
/// Anforderung wartet jemand, hinter einem Profileintrag niemand.
#[test]
fn profile_only_branches_queue_behind_explicit_requests() {
    static ROUTES: [(&str, &str); 2] = [
        (
            "branch_requests",
            r#"[
              {"market_id":"angefordert","requested_at":"2026-07-31T08:00:00+00:00","last_synced":null},
              {"market_id":"gesynct","requested_at":"2026-07-01T00:00:00+00:00","last_synced":"2026-07-30T07:00:00+00:00"}
            ]"#,
        ),
        ("user_profiles", r#"[{"branch_ids":["nur-im-profil","gesynct"]}]"#),
    ];
    let (base_url, _log) = spawn_routed_mock(&ROUTES);

    let branches = sync::fetch_chosen_branches(&cfg(&base_url)).unwrap();

    // „gesynct" steht in beiden Quellen und darf trotzdem nur einmal
    // vorkommen — mit dem Zeitstempel aus der Anforderung, nicht als Neuling.
    assert_eq!(branches, vec!["angefordert", "nur-im-profil", "gesynct"]);
}

/// Ein Zeitstempel, den niemand parsen kann, darf nicht wie „nie gesynct"
/// aussehen.
///
/// Sonst stünde diese Filiale in **jedem** Lauf vorn — unlesbar bleibt sie ja
/// auch nach dem Sync — und schnitte die Übrigen ab. Das wäre derselbe stille
/// Datenverlust wie der, den dieser Commit behebt. Sie kommt deshalb hinter
/// die echten Neuzugänge und vor alles Gesyncte.
#[test]
fn an_unreadable_timestamp_does_not_win_the_queue_forever() {
    static ROUTES: [(&str, &str); 2] = [
        (
            "branch_requests",
            r#"[
              {"market_id":"kaputter-stempel","requested_at":"2026-07-01T00:00:00+00:00","last_synced":"gestern abend"},
              {"market_id":"nie-gesynct","requested_at":"2026-07-31T08:00:00+00:00","last_synced":null},
              {"market_id":"frisch","requested_at":"2026-07-01T00:00:00+00:00","last_synced":"2026-07-31T07:00:00+00:00"}
            ]"#,
        ),
        ("user_profiles", "[]"),
    ];
    let (base_url, _log) = spawn_routed_mock(&ROUTES);

    let branches = sync::fetch_chosen_branches(&cfg(&base_url)).unwrap();

    assert_eq!(branches, vec!["nie-gesynct", "kaputter-stempel", "frisch"]);
}

/// Zwei Läufe über denselben Daten müssen dieselbe Liste ergeben. Ohne
/// letzte Stufe im Vergleich hinge die Reihenfolge gleich alter Filialen an
/// der Antwortreihenfolge der Datenbank — dieselbe Flake-Klasse wie in #21
/// und #22, nur eine Ebene höher.
///
/// Ehrlich benannt: Dieser Test schlägt vor dem Fix **nicht** fehl. Bei
/// gleichem `last_synced` ist die ID die einzige Stufe, die bleibt, und dann
/// stimmt alphabetisch mit richtig überein. Er ist ein Wächter gegen künftige
/// Änderungen an [`sync::queue_order`], kein Beleg für die jetzige.
#[test]
fn the_queue_is_the_same_on_every_read() {
    static ROUTES: [(&str, &str); 2] = [
        (
            "branch_requests",
            r#"[
              {"market_id":"c","requested_at":"2026-07-01T00:00:00+00:00","last_synced":"2026-07-30T07:00:00+00:00"},
              {"market_id":"a","requested_at":"2026-07-01T00:00:00+00:00","last_synced":"2026-07-30T07:00:00+00:00"},
              {"market_id":"b","requested_at":"2026-07-01T00:00:00+00:00","last_synced":"2026-07-30T07:00:00+00:00"}
            ]"#,
        ),
        ("user_profiles", "[]"),
    ];
    let (base_url, _log) = spawn_routed_mock(&ROUTES);
    let cfg = cfg(&base_url);

    let first = sync::fetch_chosen_branches(&cfg).unwrap();
    for _ in 0..5 {
        assert_eq!(sync::fetch_chosen_branches(&cfg).unwrap(), first);
    }
    assert_eq!(first, vec!["a", "b", "c"]);
}

/// Eine kaputte Filiale darf den Lauf nicht abbrechen — die übrigen haben mit
/// ihr nichts zu tun.
#[test]
fn a_failing_branch_does_not_abort_the_run() {
    static ROUTES: [(&str, &str); 3] = [
        ("branch_requests", r#"[{"market_id":"kaputt"},{"market_id":"heil"}]"#),
        ("user_profiles", "[]"),
        ("branches", BRANCH_POSTPLATZ),
    ];
    let (base_url, _log) = spawn_routed_mock(&ROUTES);
    let calls = Arc::new(Mutex::new(0usize));
    let calls2 = calls.clone();
    let scraper = move |branch: &lechariot::models::Branch| {
        *calls2.lock().unwrap() += 1;
        if *calls2.lock().unwrap() == 1 {
            anyhow::bail!("Scraper kaputt");
        }
        let mut o = offer("Coca-Cola", Some(0.75));
        o.market_id = branch.market_id.clone();
        Ok(vec![o])
    };
    let db_path = temp_db("filiale-kaputt");

    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &scraper, &no_national).unwrap();

    assert_eq!(*calls.lock().unwrap(), 2, "die zweite Filiale muss trotzdem drankommen");
}

// ------------------------------------------------------- Filial-Sync (v13)

#[test]
fn branch_mode_scrapes_exactly_the_requested_branch() {
    static ROUTES: [(&str, &str); 1] = [("branches", BRANCH_POSTPLATZ)];
    let (base_url, log) = spawn_routed_mock(&ROUTES);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let scraper = move |branch: &lechariot::models::Branch| {
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

    // Die Angebote gehen unter der Filiale hoch, nicht unter der Kette — und
    // seit v16 ohne jede Region.
    let upsert = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .expect("offers-Upsert fehlt");
    let rows: serde_json::Value = serde_json::from_str(&upsert.body).unwrap();
    assert_eq!(rows[0]["market_id"], "1766063");
    assert_eq!(rows[0]["market"], "REWE");
    assert_eq!(rows[0]["nationwide"], false);
    assert!(rows[0].get("region").is_none(), "{}", upsert.body);

    // Aufgeräumt wird nur diese Filiale — die Nachbarfiliale derselben PLZ
    // bleibt unangetastet.
    let deletes: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "DELETE" && r.target.starts_with("/rest/v1/offers"))
        .collect();
    assert!(deletes.iter().all(|d| d.target.contains("market_id=eq.1766063")), "{deletes:#?}");
    assert!(!deletes.iter().any(|d| d.target.contains("market=eq.REWE")), "{deletes:#?}");

    // Zuletzt die Fertigmeldung, sonst pollt die App bis zum Timeout weiter.
    let done_pos = reqs
        .iter()
        .position(|r| r.method == "POST" && r.target.starts_with("/rest/v1/branch_requests"))
        .expect("Fertigmeldung an branch_requests fehlt");
    let upsert_pos = reqs
        .iter()
        .position(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .unwrap();
    assert!(upsert_pos < done_pos, "Fertigmeldung kam vor den Angeboten: {reqs:#?}");
}

#[test]
fn branch_mode_fails_loudly_on_unknown_market_id() {
    static ROUTES: [(&str, &str); 1] = [("branches", "[]")];
    let (base_url, _log) = spawn_routed_mock(&ROUTES);
    let scraper = |_: &lechariot::models::Branch| Ok(Vec::new());
    let db_path = temp_db("filiale-unbekannt");
    let opts = opts(&db_path);

    let err = sync::run_branch(&opts, Some(&cfg(&base_url)), &scraper, &no_national, "4711")
        .unwrap_err()
        .to_string();
    assert!(err.contains("4711"), "{err}");
    assert!(err.contains("Verzeichnis"), "{err}");
}

// -------------------------------------------- Bundesweite Ketten (Phase 12)

/// Der Kern von Schritt 5: ALDI wird **einmal** gespeichert, ohne Filialbezug.
/// Vorher bekam jede der 16 Regionen ihre eigene Kopie desselben Katalogs
/// (2.965 Zeilen für rund 320 Angebote).
#[test]
fn national_chains_are_pushed_once_and_marked_nationwide() {
    static ROUTES: [(&str, &str); 2] =
        [("branch_requests", "[]"), ("user_profiles", "[]")];
    let (base_url, log) = spawn_routed_mock(&ROUTES);
    let national = |store: lechariot::stores::Store, market: &Market| {
        assert!(store.stores_nationally(), "{} ist nicht bundesweit", store.chain());
        Ok(vec![
            offer_for(&market.id, "Ofenkäse", 2.22),
            offer_for(&market.id, "Rispentomaten", 1.11),
        ])
    };
    let db_path = temp_db("national");

    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &no_branch, &national).unwrap();

    let reqs = log.lock().unwrap().clone();
    let rows: Vec<serde_json::Value> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .filter_map(|r| serde_json::from_str::<serde_json::Value>(&r.body).ok())
        .flat_map(|v| v.as_array().cloned().unwrap_or_default())
        .collect();

    assert_eq!(rows.len(), 4, "je 2 Angebote für Nord und SÜD: {rows:#?}");
    for row in &rows {
        assert_eq!(row["nationwide"], true, "{row}");
    }
    let ids: std::collections::BTreeSet<&str> =
        rows.iter().filter_map(|r| r["market_id"].as_str()).collect();
    assert_eq!(
        ids,
        ["ALDI_NORD_DE", "ALDI_SUED_DE"].into_iter().collect(),
        "unter der National-Filiale gespeichert, nicht unter einer echten"
    );
}

/// Pflicht-Testfall am Aldi-Äquator: 96515 Sonneberg hat ALDI SÜD im Ort und
/// ALDI Nord 15 km nördlich. Beide Kataloge müssen nebeneinander stehen —
/// zwei Ketten, zwei National-Filialen, keine verdrängt die andere.
#[test]
fn at_the_aldi_equator_both_catalogues_stand_side_by_side() {
    static ROUTES: [(&str, &str); 2] =
        [("branch_requests", "[]"), ("user_profiles", "[]")];
    let (base_url, log) = spawn_routed_mock(&ROUTES);
    let national = |_: lechariot::stores::Store, market: &Market| {
        Ok(vec![offer_for(&market.id, &format!("Katalog {}", market.id), 1.0)])
    };
    let db_path = temp_db("aldi-aequator");

    sync::run(&opts(&db_path), Some(&cfg(&base_url)), &no_branch, &national).unwrap();

    let reqs = log.lock().unwrap().clone();
    let rows: Vec<serde_json::Value> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .filter_map(|r| serde_json::from_str::<serde_json::Value>(&r.body).ok())
        .flat_map(|v| v.as_array().cloned().unwrap_or_default())
        .collect();
    assert!(rows.iter().any(|r| r["product"] == "Katalog ALDI_NORD_DE"), "{rows:#?}");
    assert!(rows.iter().any(|r| r["product"] == "Katalog ALDI_SUED_DE"), "{rows:#?}");

    // Und keiner räumt dem anderen die Zeilen ab: Gelöscht wird je Filiale,
    // nie über die Kette. Ein Ketten-Filter hätte hier genau den Fehler
    // gemacht, den der Äquator sichtbar macht.
    let deletes: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.method == "DELETE" && r.target.starts_with("/rest/v1/offers"))
        .collect();
    assert!(deletes.iter().any(|d| d.target.contains("market_id=eq.ALDI_NORD_DE")), "{deletes:#?}");
    assert!(deletes.iter().any(|d| d.target.contains("market_id=eq.ALDI_SUED_DE")), "{deletes:#?}");
    for d in &deletes {
        assert!(!d.target.contains("market=eq."), "Löschen über die Kette: {}", d.target);
    }
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
    let national =
        |_: lechariot::stores::Store, market: &Market| Ok(vec![offer_for(&market.id, "Ofenkäse", 2.22)]);
    let db_path = temp_db("aldi-anforderung");
    let mut opts = opts(&db_path);
    opts.market_id = Some("ALDI_NORD_4711".to_string());

    sync::run_branch(&opts, Some(&cfg(&base_url)), &no_branch, &national, "ALDI_NORD_4711")
        .unwrap();

    let reqs = log.lock().unwrap().clone();
    let upsert = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .expect("offers-Upsert fehlt");
    let rows: serde_json::Value = serde_json::from_str(&upsert.body).unwrap();
    assert_eq!(rows[0]["market_id"], "ALDI_NORD_DE");
    assert_eq!(rows[0]["nationwide"], true);

    // Die Anforderung wird trotzdem beantwortet, sonst pollt die App bis zum
    // Timeout auf eine Filiale, die längst Angebote zeigt.
    let done = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/branch_requests"))
        .expect("Fertigmeldung an branch_requests fehlt");
    let body: serde_json::Value = serde_json::from_str(&done.body).unwrap();
    assert_eq!(body[0]["market_id"], "ALDI_NORD_4711");
}
