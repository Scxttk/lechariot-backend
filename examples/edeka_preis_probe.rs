//! Messgeschirr für das EDEKA-Preisfeld: Was steht heute wirklich in einer
//! Angebots-Kachel — und was davon findet der Parser?
//!
//! Seit dem 05.08. meldet die Nightly „0 Angebote hochladbar (222 ohne Preis)".
//! Der Parser sucht den Preis in einem `div.sr-only` mit „…reis von …".
//! Diese Probe behauptet nichts über das Markup, sondern schreibt auf, was die
//! Seite liefert: wie viele Kacheln es gibt, wie viele davon der Parser mit
//! Preis herausbekommt, und für die ersten Kacheln das Roh-HTML.
//!
//! Nur GET, kein Schreibzugriff. Läuft in CI, weil edeka.de hinter Akamai
//! sitzt und ein Entwicklungsrechner geblockt sein kann — dann misst er den
//! Rechner statt die Seite.
//!
//! `cargo run --example edeka_preis_probe -- [markt-id …]`

use anyhow::Result;
use lechariot::scrapers::edeka;
use scraper::{Html, Selector};

/// Die Märkte aus der Nightly: NP1 lieferte am 05.08. als einziger noch
/// Preise, die anderen drei schon nicht mehr — mit beiden Sorten im Lauf ist
/// zu sehen, ob es zwei Markup-Varianten gibt oder nur noch eine.
const DEFAULT_MARKETS: &[&str] = &["021868", "421696", "421347"];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let markets: Vec<String> = if args.is_empty() {
        DEFAULT_MARKETS.iter().map(|s| s.to_string()).collect()
    } else {
        args
    };

    let mut mit_preis_gesamt = 0usize;
    for market_id in &markets {
        println!("\n=== Markt {market_id} — {} ===", edeka::offers_url(market_id));
        let html = match edeka::fetch_offers_html(market_id) {
            Ok(html) => html,
            Err(e) => {
                println!("FEHLER: {e:#}");
                println!("PREIS-PROBE\t{market_id}\t0\t0\t0\tFEHLER");
                continue;
            }
        };
        println!("HTML: {} B, #angebot--Anker: {}", html.len(), html.matches("#angebot-").count());

        // Roh-HTML wegschreiben, wenn ein Ziel genannt ist — daraus wird das
        // Fixture, und der CI-Lauf hängt es als Artefakt an.
        if let Ok(dir) = std::env::var("EDEKA_PROBE_DUMP") {
            let pfad = std::path::Path::new(&dir).join(format!("{market_id}.html"));
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&pfad, &html)?;
            println!("Roh-HTML: {}", pfad.display());
        }

        // Was der Parser heute herausbekommt.
        match edeka::parse_offers(&html, market_id) {
            Ok(offers) => {
                // Nicht „hat einen Preis", sondern „hat einen Preis über null":
                // ein 0.00-€-Angebot wäre genauso unbrauchbar wie gar keines.
                let mit_preis =
                    offers.iter().filter(|o| o.price.is_some_and(|p| p > 0.0)).count();
                mit_preis_gesamt += mit_preis;
                println!("Parser: {} Angebote, davon {mit_preis} mit Preis > 0", offers.len());
                for o in offers.iter().take(3) {
                    println!("  - {} | {:?} €", o.title, o.price);
                }
                let urteil = if mit_preis > 0 { "ok" } else { "OHNE PREIS" };
                println!(
                    "PREIS-PROBE\t{market_id}\t{}\t{}\t{mit_preis}\t{urteil}",
                    html.matches("#angebot-").count(),
                    offers.len()
                );
            }
            Err(e) => {
                println!("Parser: FEHLER {e:#}");
                println!("PREIS-PROBE\t{market_id}\t0\t0\t0\tFEHLER");
            }
        }

        markup(&html);
    }

    // Der Lauf soll rot sein, wenn kein einziger Preis herauskommt — sonst ist
    // die Probe ein grüner Haken ohne Aussage.
    if mit_preis_gesamt == 0 {
        anyhow::bail!("Kein einziger Preis > 0 über alle geprüften Märkte");
    }
    println!("\nSumme: {mit_preis_gesamt} Angebote mit Preis > 0");
    Ok(())
}

/// Wo steht der Preis heute? Erst zählen, welche der bekannten Kandidaten
/// überhaupt vorkommen, dann eine Kachel im Rohzustand zeigen.
fn markup(html: &str) {
    let doc = Html::parse_document(html);
    let sel_article = Selector::parse("article").unwrap();
    let sel_anchor = Selector::parse(r##"a[href^="#angebot-"]"##).unwrap();
    let sel_sronly = Selector::parse("div.sr-only").unwrap();
    let sel_sronly_any = Selector::parse(".sr-only").unwrap();

    let kacheln: Vec<_> =
        doc.select(&sel_article).filter(|a| a.select(&sel_anchor).next().is_some()).collect();
    println!("Kacheln mit Anker: {}", kacheln.len());

    let mut mit_div_sronly = 0;
    let mut mit_sronly_irgendwas = 0;
    let mut mit_reis_von = 0;
    let mut mit_euro = 0;
    for k in &kacheln {
        if k.select(&sel_sronly).next().is_some() {
            mit_div_sronly += 1;
        }
        if k.select(&sel_sronly_any).next().is_some() {
            mit_sronly_irgendwas += 1;
        }
        let text: String = k.text().collect();
        if text.contains("reis von") {
            mit_reis_von += 1;
        }
        if text.contains('€') || text.contains("EUR") {
            mit_euro += 1;
        }
    }
    println!(
        "  div.sr-only: {mit_div_sronly} | .sr-only (beliebiges Tag): {mit_sronly_irgendwas} | \
         Text „reis von\": {mit_reis_von} | Text mit €/EUR: {mit_euro}"
    );

    // Die ersten beiden Kacheln im Rohzustand — daraus wird das Fixture.
    for (i, k) in kacheln.iter().take(2).enumerate() {
        println!("\n--- Kachel {i} (Roh-HTML) ---\n{}", k.html());
    }

    // Und die erste Kachel mit zwei Preisen: Welche Zahl zeigt die Seite groß?
    if let Some(k) = kacheln
        .iter()
        .find(|k| k.select(&sel_sronly_any).filter(|e| e.inner_html().contains("App")).count() > 0)
    {
        println!("\n--- Kachel mit App-Preis (Roh-HTML) ---\n{}", k.html());
    }

    // Und die sr-only-Texte der ersten zehn Kacheln, kompakt.
    println!("\n--- sr-only-Texte (erste 10 Kacheln) ---");
    for (i, k) in kacheln.iter().take(10).enumerate() {
        let texte: Vec<String> = k
            .select(&sel_sronly_any)
            .map(|e| e.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
            .collect();
        println!("  [{i}] {texte:?}");
    }
}
