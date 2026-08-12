//! Messgeschirr für Issue #82: Warum liefert ALDI Nord im Nightly 0 Angebote,
//! während sieben andere Ketten im selben Lauf durchkommen?
//!
//! Die erste Messung (Lauf 31636710816) hat die naheliegende Erklärung
//! **widerlegt**: Es ist nicht schlicht „reqwest wird geblockt, curl kommt
//! durch". Beide Clients bekamen 403, nur zu verschiedenen Zeitpunkten —
//! reqwest in zwei von drei Runden, System-curl in der dritten, dort in allen
//! drei Versuchen hintereinander. Ein Client-Wechsel wäre also geraten, nicht
//! gemessen.
//!
//! Deshalb variiert diese Fassung **einen** Faktor pro Arm, gegen dieselbe URL,
//! im selben Prozess:
//!
//! * `scraper-weg` — was `aldi_nord::fetch_offers` gerade tut. Bis zur
//!   Reparatur war das der einzelne reqwest-Versuch, seither `util::curl_get`.
//! * `reqwest-alt` — der Weg vor der Reparatur, nachgebaut: ein Versuch,
//!   zwei Header, kein Sec-Fetch. Nur so bleibt der Vergleich messbar,
//!   nachdem der Scraper selbst umgezogen ist.
//! * `reqwest-header` — derselbe Client, aber mit dem vollen Browser-Header-Satz
//!   (Sec-Fetch-Quartett). Trennt „TLS-Fingerprint" von „Header".
//! * `curl-scraper` — `util::curl_get`, der Weg von Netto/ALDI SÜD/EDEKA.
//! * `curl-cookie` — curl, aber erst die Startseite, dann die Angebotsseite mit
//!   demselben Cookie-Jar. Ein Browser navigiert nie ohne die Akamai-Cookies
//!   (`bm_sz`, `_abck`), unsere Clients schon.
//!
//! Jeder Arm läuft mehrere Runden; ausgegeben wird der beobachtete Status, nicht
//! die Erwartung. Bei 403 wird der Antwortkörper mitgeschrieben — die
//! Akamai-Referenz darin sagt, ob geblockt oder gedrosselt wurde.
//!
//! Nur GET, kein Schreibzugriff. Mit `AN_PROBE_OUT=<dir>` legt der Lauf das
//! geholte HTML ab — daraus wird die Fixture für den Offline-Test.
//!
//! `cargo run --example aldi_nord_probe -- [runden]`

use anyhow::Result;
use lechariot::scrapers::{aldi_nord, util};

const HOME_URL: &str = "https://www.aldi-nord.de/";
const OFFERS_URL: &str = "https://www.aldi-nord.de/angebote.html";
const NEXT_WEEK_URL: &str = "https://www.aldi-nord.de/angebote-vorschau.html";

/// Der Header-Satz einer Dokument-Navigation, wörtlich wie ihn `netto.rs` für
/// dieselbe Akamai-Installation setzt. Das Sec-Fetch-Quartett trägt dort
/// nachweislich; einer der Header allein reicht nicht (Backlog 31.07.).
const DOCUMENT_HEADERS: &[(&str, &str)] = &[
    ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
    ("Sec-Fetch-Site", "none"),
    ("Sec-Fetch-Mode", "navigate"),
    ("Sec-Fetch-Dest", "document"),
    ("Sec-Fetch-User", "?1"),
];

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg == "markets" {
        return zwei_markets();
    }
    let runden: usize = arg.parse().ok().unwrap_or(4);
    let markt = aldi_nord::national();

    for runde in 1..=runden {
        println!("=== Runde {runde}/{runden} ===");

        // Arm A: was der Scraper heute tut.
        match aldi_nord::fetch_offers(&markt) {
            Ok(offers) => println!("  scraper-weg     OK, {} Angebote", offers.len()),
            Err(e) => println!("  scraper-weg     FEHLER {}", kurz(&format!("{e:#}"))),
        }

        // Arm B: der Weg, der im Nightly vom 11.08. jedes Mal 403 bekam.
        match reqwest_alt(OFFERS_URL) {
            Ok((status, len)) => println!("  reqwest-alt     HTTP {status}, {len} B"),
            Err(e) => println!("  reqwest-alt     FEHLER {}", kurz(&format!("{e:#}"))),
        }

        // Arm C: derselbe TLS-Stack, aber die Header eines Browsers.
        match reqwest_mit_headern(OFFERS_URL) {
            Ok((status, len)) => println!("  reqwest-header  HTTP {status}, {len} B"),
            Err(e) => println!("  reqwest-header  FEHLER {}", kurz(&format!("{e:#}"))),
        }

        // Arm D: der Weg von Netto/ALDI SÜD/EDEKA, samt Wiederholungen.
        match util::curl_get(OFFERS_URL, DOCUMENT_HEADERS) {
            Ok(html) => {
                println!("  curl-scraper    OK, {} B, Angebote={}", html.len(), zaehle(&html, &markt.id));
                schreibe("angebote.html", &html);
            }
            Err(e) => println!("  curl-scraper    FEHLER {}", kurz(&format!("{e:#}"))),
        }

        // Arm E: curl, aber mit den Akamai-Cookies der Startseite im Gepäck.
        match curl_mit_cookie_jar(runde) {
            Ok((home, offers, body)) => {
                println!("  curl-cookie     Start HTTP {home} -> Angebote HTTP {}, {} B, Angebote={}",
                    offers, body.len(), zaehle(&body, &markt.id));
                if offers == 200 {
                    schreibe("angebote_cookie.html", &body);
                } else {
                    schreibe(&format!("403_runde{runde}.html"), &body);
                }
            }
            Err(e) => println!("  curl-cookie     FEHLER {}", kurz(&format!("{e:#}"))),
        }

        // Die Vorschau nur einmal, als Gegenprobe auf eine zweite URL desselben Hosts.
        if runde == runden {
            match util::curl_get(NEXT_WEEK_URL, DOCUMENT_HEADERS) {
                Ok(html) => {
                    println!("  curl-vorschau   OK, {} B, Angebote={}", html.len(), zaehle(&html, &markt.id));
                    schreibe("angebote-vorschau.html", &html);
                }
                Err(e) => println!("  curl-vorschau   FEHLER {}", kurz(&format!("{e:#}"))),
            }
        }
    }
    Ok(())
}

/// Der Nachweis für Issue #82: Holt der **reparierte** Scraper-Weg für zwei
/// echte Filialen Angebote — und wie viele? Nur Lesen, kein Upload; die Zahl
/// ist dieselbe, die der Nightly hochladen würde.
fn zwei_markets() -> Result<()> {
    for zip in ["10115", "01067"] {
        match aldi_nord::find_market(zip)? {
            Some(markt) => {
                let offers = aldi_nord::fetch_offers(&markt)?;
                println!("PLZ {zip}: {} ({}) -> {} Angebote", markt.name, markt.id, offers.len());
                assert!(!offers.is_empty(), "PLZ {zip} lieferte 0 Angebote");
            }
            None => println!("PLZ {zip}: keine Filiale gefunden"),
        }
    }
    Ok(())
}

/// Der Abruf, wie ihn `aldi_nord::load` vor der Reparatur machte: ein
/// reqwest-Versuch mit zwei Headern, ohne Wiederholung. Wörtlich aus dem Stand
/// vor diesem PR übernommen, damit der Vergleich messbar bleibt.
fn reqwest_alt(url: &str) -> Result<(u16, usize)> {
    util::polite_pause(url);
    let res = util::blocking_client()?
        .get(url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "de-DE,de;q=0.9")
        .send()?;
    let status = res.status().as_u16();
    Ok((status, res.text()?.len()))
}

/// reqwest mit dem vollen Header-Satz einer Dokument-Navigation. Liefert
/// (Status, Body-Länge), ohne über den Status zu urteilen.
fn reqwest_mit_headern(url: &str) -> Result<(u16, usize)> {
    util::polite_pause(url);
    let mut req = util::blocking_client()?.get(url).header("Accept-Language", "de-DE,de;q=0.9");
    for (k, v) in DOCUMENT_HEADERS {
        req = req.header(*k, *v);
    }
    let res = req.send()?;
    let status = res.status().as_u16();
    Ok((status, res.text()?.len()))
}

/// Zwei curl-Aufrufe mit gemeinsamem Cookie-Jar: erst die Startseite (dort
/// setzt Akamai `bm_sz`/`_abck`), dann die Angebotsseite mit passendem Referer
/// — die Reihenfolge, die ein Browser erzeugt. Liefert (Status Start, Status
/// Angebote, Body der Angebotsseite).
fn curl_mit_cookie_jar(runde: usize) -> Result<(u16, u16, String)> {
    let jar = std::env::temp_dir().join(format!("aldi_nord_jar_{runde}.txt"));
    let home = curl_roh(HOME_URL, &jar, None)?;
    let (status, body) = curl_roh_body(OFFERS_URL, &jar, Some(HOME_URL))?;
    Ok((home.0, status, body))
}

fn curl_roh(url: &str, jar: &std::path::Path, referer: Option<&str>) -> Result<(u16, String)> {
    curl_roh_body(url, jar, referer).map(|(s, b)| (s, b))
}

fn curl_roh_body(url: &str, jar: &std::path::Path, referer: Option<&str>) -> Result<(u16, String)> {
    util::polite_pause(url);
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s")
        .arg("-L")
        .arg("--compressed")
        .arg("--max-time")
        .arg("30")
        .arg("-w")
        .arg("\n%{http_code}")
        .args(["-c", &jar.to_string_lossy()])
        .args(["-b", &jar.to_string_lossy()])
        .args(["-H", &format!("User-Agent: {}", util::USER_AGENT)])
        .args(["-H", "Accept-Language: de-DE,de;q=0.9,en;q=0.8"])
        .args(["-H", "Accept-Encoding: gzip, deflate, br"]);
    for (k, v) in DOCUMENT_HEADERS {
        cmd.args(["-H", &format!("{k}: {v}")]);
    }
    if let Some(r) = referer {
        cmd.args(["-H", &format!("Referer: {r}")]);
        // Eine Navigation von der eigenen Startseite aus ist same-origin.
        cmd.args(["-H", "Sec-Fetch-Site: same-origin"]);
    }
    let out = cmd.arg(url).output()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, status) = text.rsplit_once('\n').unwrap_or((text.as_str(), "0"));
    Ok((status.trim().parse().unwrap_or(0), body.to_string()))
}

fn zaehle(html: &str, market_id: &str) -> String {
    match aldi_nord::parse_offers(html, market_id) {
        Ok(o) => o.len().to_string(),
        Err(e) => format!("Parser-FEHLER {}", kurz(&format!("{e:#}"))),
    }
}

fn schreibe(name: &str, inhalt: &str) {
    if let Ok(dir) = std::env::var("AN_PROBE_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(format!("{dir}/{name}"), inhalt);
    }
}

/// Die Fehlerkette der Scraper ist für das Protokoll gebaut, nicht für eine
/// Messtabelle — hier zählt der Status, nicht die dreifache URL.
fn kurz(msg: &str) -> String {
    let eine_zeile = msg.replace('\n', " ");
    match eine_zeile.find("HTTP") {
        Some(i) => eine_zeile[i..].chars().take(90).collect(),
        None => eine_zeile.chars().take(90).collect(),
    }
}
