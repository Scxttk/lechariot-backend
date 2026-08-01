//! Messgeschirr für Schritt 4 der Vorschau: Kommt der **vorhandene**
//! Scraper-Weg an die Folgewoche von Netto und ALDI SÜD heran?
//!
//! Nur GET, kein Schreibzugriff. Gibt aus, was wirklich geantwortet hat —
//! nicht, was zu erwarten wäre. Wird nach der Messung nicht gebraucht, steht
//! aber im Repo, damit die Zahl nachvollziehbar bleibt.
//!
//! `cargo run --example vorschau_probe -- netto|aldisued`

use anyhow::Result;
use lechariot::scrapers::{netto, util};

fn main() -> Result<()> {
    match std::env::args().nth(1).unwrap_or_default().as_str() {
        "netto" => netto_probe(),
        "aldisued" => aldi_sued_probe(),
        other => {
            eprintln!("unbekannt: {other:?} — netto oder aldisued");
            Ok(())
        }
    }
}

/// Netto: Der Vorschau-Prospekt hängt laut Messung vom 01.08. an der
/// Filialwahl (Cookie) — genau der Weg, den `netto::fetch_offers` schon geht.
/// Hier wird gefragt, was mit gesetztem Cookie überhaupt erreichbar ist.
fn netto_probe() -> Result<()> {
    let market = netto::find_market("01219")?;
    println!("Filiale: {} ({})", market.name, market.id);
    let cookie = format!("netto_user_stores_id={}", market.id);
    let headers: &[(&str, &str)] = &[
        ("Accept", "text/html,application/xhtml+xml"),
        ("Cookie", &cookie),
        ("Referer", "https://www.netto-online.de/"),
    ];

    // Die Seiten-IDs, die der Scraper heute holt, plus die Nachbarn: Wenn eine
    // eigene ID die Folgewoche trägt, ist die Vorschau ein Listeneintrag.
    for page in [1u32, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
        let url = format!("https://www.netto-online.de/filialangebote/{page}");
        match util::curl_get(&url, headers) {
            Ok(html) => println!(
                "  /filialangebote/{page:<2} {:>7} B  {}",
                html.len(),
                datumszeilen(&html)
            ),
            Err(e) => println!("  /filialangebote/{page:<2} FEHLER: {e}"),
        }
    }

    // Und die Prospektübersicht, auf der am 01.08. „ab Montag, 03.08.26" stand.
    for url in [
        "https://www.netto-online.de/prospekte",
        "https://www.netto-online.de/filialangebote/",
    ] {
        match util::curl_get(url, headers) {
            Ok(html) => {
                println!("\n{url}  {} B", html.len());
                println!("  Datumszeilen: {}", datumszeilen(&html));
                for l in prospekt_links(&html) {
                    println!("  Link: {l}");
                }
            }
            Err(e) => println!("\n{url} FEHLER: {e}"),
        }
    }
    Ok(())
}

/// ALDI SÜD: Antworten die datierten Seiten auf dem vorhandenen Weg? Am 01.08.
/// standen sie im Browser live, der nackte API-Abruf bekam „Access Denied".
fn aldi_sued_probe() -> Result<()> {
    let headers: &[(&str, &str)] = &[
        ("Accept", "text/html,application/xhtml+xml"),
        ("Referer", "https://www.aldi-sued.de/de/angebote.html"),
    ];
    for tag in ["2026-08-03", "2026-08-06", "2026-08-07"] {
        for url in [
            format!("https://www.aldi-sued.de/de/angebote/{tag}.html"),
            format!("https://www.aldi-sued.de/de/angebote/{tag}"),
        ] {
            match util::curl_get(&url, headers) {
                Ok(html) => println!(
                    "{url}\n   {} B, Preis-Treffer: {}, __NEXT_DATA__: {}",
                    html.len(),
                    html.matches("€").count(),
                    html.contains("__NEXT_DATA__")
                ),
                Err(e) => println!("{url}\n   FEHLER: {e}"),
            }
        }
    }
    Ok(())
}

/// Alle `TT.MM.` bzw. `TT.MM.JJ`-Vorkommen, damit sichtbar wird, welche Woche
/// eine Seite beschreibt.
fn datumszeilen(html: &str) -> String {
    let re = regex::Regex::new(r"\b\d{2}\.\d{2}\.(?:\d{2,4})?").unwrap();
    let mut seen: Vec<String> = Vec::new();
    for m in re.find_iter(html) {
        let s = m.as_str().to_string();
        if !seen.contains(&s) {
            seen.push(s);
        }
    }
    seen.truncate(12);
    seen.join(" ")
}

fn prospekt_links(html: &str) -> Vec<String> {
    let re = regex::Regex::new(r#"href="([^"]*(?:prospekt|angebote)[^"]*)""#).unwrap();
    let mut out: Vec<String> = Vec::new();
    for c in re.captures_iter(html) {
        let l = c[1].to_string();
        if !out.contains(&l) {
            out.push(l);
        }
    }
    out.truncate(20);
    out
}
