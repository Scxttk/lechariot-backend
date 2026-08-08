//! Messgeschirr für die Lücke zwischen Aktionstagen und Produkt-Snapshot bei
//! ALDI Nord (siehe [`lechariot::scrapers::aldi_nord::MissingProduct`]).
//!
//! Holt die laufende Seite und die Vorschau über denselben Client wie der
//! Scraper und zählt, welche `productIds` aus `res.categories` in
//! `res.algoliaDataMap` fehlen. Nur GET, kein Schreibzugriff.
//!
//! `cargo run --example aldi_nord_luecke`
//!
//! Lauf vom 06.08.2026:
//!
//! ```text
//! == angebote: 213 Angebote, 2 fehlend
//!    1032980   Osteuropa-Aktion       ab 2026-08-03   NotInDataMap
//!    4369      Haltbare Produkte      ab 2026-08-06   NotInDataMap
//! == vorschau: 240 Angebote, 0 fehlend
//! ```
//!
//! `1032980` war „OSTEUROPA Original polnische Pierogi", 2,49 €, im gedruckten
//! Prospekt derselben Woche. Weder die Produktseite (`/produkt/…-1032980.html`,
//! HTTP 404) noch die gerenderte Angebotsseite kannten das Produkt — die Lücke
//! sitzt in der Quelle, nicht im Parser.

use anyhow::Result;
use lechariot::scrapers::{aldi_nord, util};

const PAGES: [(&str, &str); 2] = [
    ("angebote", "https://www.aldi-nord.de/angebote.html"),
    ("vorschau", "https://www.aldi-nord.de/angebote-vorschau.html"),
];

fn main() -> Result<()> {
    for (name, url) in PAGES {
        util::polite_pause(url);
        let html = util::blocking_client()?
            .get(url)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "de-DE,de;q=0.9")
            .send()?
            .error_for_status()?
            .text()?;

        let (offers, missing) = aldi_nord::parse_offers_reporting(&html, "ALDI_NORD_DE")?;
        println!("== {name}: {} Angebote, {} fehlend", offers.len(), missing.len());
        for m in &missing {
            println!(
                "   {:<9} {:<22} ab {:<12} {:?}",
                m.product_id,
                m.section.as_deref().unwrap_or("?"),
                m.aktion_start.as_deref().unwrap_or("?"),
                m.reason,
            );
        }
    }
    Ok(())
}
