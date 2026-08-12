//! Messgeschirr für Issue #82: Warum liefert ALDI Nord im Nightly 0 Angebote,
//! während sieben andere Ketten im selben Lauf durchkommen?
//!
//! Die Frage, und nur diese: Antwortet `www.aldi-nord.de` dem **bestehenden**
//! Weg (reqwest, `aldi_nord::fetch_offers`) anders als dem Weg, den
//! Netto/ALDI SÜD/EDEKA längst gehen (System-curl mit vollem Browser-Header-Satz,
//! `util::curl_get`)? Beide Arme laufen hier gegen dieselbe URL, im selben
//! Prozess, unmittelbar nacheinander — der einzige Unterschied ist der Client.
//!
//! Der reqwest-Arm ruft den **echten** Scraper auf und nicht eine Nachbildung:
//! Eine Messung, die den Weg nachbaut statt ihn zu benutzen, misst sich selbst
//! (siehe `vorschau_probe.rs`, dort ist genau das einmal passiert).
//!
//! Nur GET, kein Schreibzugriff. Mit `AN_PROBE_OUT=<dir>` legt der Lauf das
//! per curl geholte HTML ab — daraus wird die Fixture für den Offline-Test.
//!
//! `cargo run --example aldi_nord_probe -- [runden]`

use anyhow::Result;
use lechariot::scrapers::{aldi_nord, util};

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
    let runden: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(3);
    let markt = aldi_nord::national();

    for runde in 1..=runden {
        println!("=== Runde {runde}/{runden} ===");

        // Arm A: der Weg, der im Nightly vom 11.08. jedes Mal 403 bekam.
        match aldi_nord::fetch_offers(&markt) {
            Ok(offers) => println!("  reqwest (Scraper heute): OK, {} Angebote", offers.len()),
            Err(e) => println!("  reqwest (Scraper heute): FEHLER {e:#}"),
        }

        // Arm B: der Weg von Netto/ALDI SÜD/EDEKA.
        for url in [OFFERS_URL, NEXT_WEEK_URL] {
            let kurz = url.rsplit('/').next().unwrap_or(url);
            match util::curl_get(url, DOCUMENT_HEADERS) {
                Ok(html) => {
                    let angebote = aldi_nord::parse_offers(&html, &markt.id)
                        .map(|o| o.len().to_string())
                        .unwrap_or_else(|e| format!("Parser-FEHLER {e:#}"));
                    println!(
                        "  curl    {kurz:<24} OK, {:>7} B, __NEXT_DATA__={}, Angebote={angebote}",
                        html.len(),
                        html.contains("__NEXT_DATA__"),
                    );
                    if let Ok(dir) = std::env::var("AN_PROBE_OUT") {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::fs::write(format!("{dir}/{kurz}"), &html);
                    }
                }
                Err(e) => println!("  curl    {kurz:<24} FEHLER {e:#}"),
            }
        }
    }
    Ok(())
}
