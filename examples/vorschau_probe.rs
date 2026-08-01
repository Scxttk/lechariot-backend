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
use sha2::Digest;

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
    // **Wörtlich der Header-Satz aus `netto::fetch_offers`.** Der erste Anlauf
    // dieser Probe ließ das Sec-Fetch-Quartett weg und setzte stattdessen einen
    // Referer — und bekam von Akamai für jede Seite 403. Das war kein Befund
    // über Netto, sondern einer über die Probe: Der Backlog hält seit dem
    // 31.07. fest, dass jeder dieser Header trägt und einer allein nicht reicht.
    let headers: &[(&str, &str)] = &[
        ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
        ("Cookie", &cookie),
        ("Sec-Fetch-Site", "none"),
        ("Sec-Fetch-Mode", "navigate"),
        ("Sec-Fetch-Dest", "document"),
        ("Sec-Fetch-User", "?1"),
    ];

    // **Die entscheidende Frage**, und sie wird mit dem Parser des Scrapers
    // gestellt, nicht mit einer Datumssuche über das rohe HTML: Erzeugt
    // `netto::parse_page` aus diesen Seiten Zeilen der Folgewoche? Rohe Daten
    // im Quelltext beweisen gar nichts — sie können aus einem Prospekt-Teaser
    // stammen, den kein Angebot trägt.
    let mut nach_start: std::collections::BTreeMap<String, usize> = Default::default();
    for page in [1u32, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
        let url = format!("https://www.netto-online.de/filialangebote/{page}");
        match util::curl_get(&url, headers) {
            Ok(html) => {
                let mut offers = Vec::new();
                let mut seen = std::collections::HashSet::new();
                netto::parse_page(&html, &market.id, &mut offers, &mut seen);
                println!(
                    "  /filialangebote/{page:<2} {:>7} B  angebote={:<4} {}",
                    html.len(),
                    offers.len(),
                    datumszeilen(&html)
                );
                for o in &offers {
                    *nach_start
                        .entry(o.valid_from.clone().unwrap_or_else(|| "ohne".into()))
                        .or_default() += 1;
                }
            }
            Err(e) => println!("  /filialangebote/{page:<2} FEHLER: {e}"),
        }
    }
    println!("\n  Angebote je valid_from (Parser des Scrapers, alle Seiten):");
    for (from, n) in &nach_start {
        println!("    {from}  {n}");
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
    // Derselbe Satz wie in `aldi_sued::fetch_offers`, nur `Accept` auf HTML —
    // dort geht es gegen die JSON-API, hier gegen die datierte Seite.
    let headers: &[(&str, &str)] = &[
        ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
        ("Origin", "https://www.aldi-sued.de"),
        ("Referer", "https://www.aldi-sued.de/"),
        ("Sec-Fetch-Site", "same-origin"),
        ("Sec-Fetch-Mode", "navigate"),
        ("Sec-Fetch-Dest", "document"),
        ("Sec-Fetch-User", "?1"),
    ];
    // Alle datierten Seiten kamen im ersten Lauf mit **exakt gleicher Größe**
    // zurück. Das ist der Verdacht: Der Server ignoriert das Datum und liefert
    // immer dieselbe Seite. Ein Hash entscheidet das, eine Byte-Zahl nicht.
    let mut hashes: Vec<(String, String)> = Vec::new();
    for tag in ["2026-08-03", "2026-08-06", "2026-08-07"] {
        let url = format!("https://www.aldi-sued.de/de/angebote/{tag}.html");
        match util::curl_get(&url, headers) {
            Ok(html) => {
                let h: String = sha2::Sha256::digest(html.as_bytes())
                    .iter()
                    .take(6)
                    .map(|b| format!("{b:02x}"))
                    .collect();
                println!(
                    "{url}\n   {} B  sha={h}  €={}  __NEXT_DATA__={}  nennt das Datum: {}",
                    html.len(),
                    html.matches('€').count(),
                    html.contains("__NEXT_DATA__"),
                    html.contains(tag)
                );
                hashes.push((tag.to_string(), h));
            }
            Err(e) => println!("{url}\n   FEHLER: {e}"),
        }
    }
    let verschieden = hashes.iter().map(|(_, h)| h).collect::<std::collections::HashSet<_>>().len();
    println!(
        "\n  {} verschiedene Seiten bei {} Datumsangaben — {}",
        verschieden,
        hashes.len(),
        if verschieden <= 1 {
            "der Datumsteil im Pfad ist Dekoration, die Seite ist immer dieselbe"
        } else {
            "das Datum wirkt sich aus"
        }
    );
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
