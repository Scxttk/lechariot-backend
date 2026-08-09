//! Messgeschirr für den Streichpreis: Welche Kette druckt heute einen alten
//! Preis — und wie viel davon kommt in `offers.regular_price` an?
//!
//! Die Übersicht in `docs/scrapers/README.md` steht seit dem 31.07. und war
//! eine Momentaufnahme; EDEKA hat sein Preis-Markup am 04.08. gewechselt
//! (#67), REWEs CLI hat mit v1.2.0 eine flachere Sicht bekommen. Diese Probe
//! beantwortet die Frage neu, statt die alte Antwort weiterzureichen — je
//! Kette mit zwei Zahlen: **was die Quelle hergibt** und **was ankommt**.
//!
//! Nur GET, kein Schreibzugriff, keine Supabase-Secrets.
//!
//! ```sh
//! cargo run --release --example streichpreis_probe             # alle drei
//! cargo run --release --example streichpreis_probe -- lidl     # nur eine
//! ```
//!
//! REWE braucht das Clientzertifikat (`REWE_CERT`/`REWE_KEY`, sonst
//! `cert.pem`/`private.key`) und läuft deshalb in CI; Lidl und EDEKA laufen
//! auch von einem Entwicklungsrechner.
//!
//! Die Zeilen mit `STREICHPREIS` sind für die Zusammenfassung des Workflows
//! gedacht: `Kette · Markt · Angebote · Quelle · übernommen · Urteil`.

use anyhow::Result;
use lechariot::scrapers::{edeka, lidl_prospekt, rewe};
use regex::Regex;
use scraper::{Html, Selector};
use std::sync::LazyLock;

/// Die Wörter, mit denen deutsche Prospekte einen alten Preis ankündigen —
/// als ganze Wörter gesucht.
static WORT_FORMEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(statt|uvp|normalpreis|vorher)\b").expect("Regex"));

/// Dresden — dieselbe PLZ, gegen die die Scraper-Referenz gemessen ist.
const LIDL_ZIP: &str = "01219";
/// Der Markt, an dem die Vorschau-Fixture entstanden ist (REWE Dresden).
const REWE_MARKET: &str = "565005";
/// Die drei Märkte der Nightly, wie in `edeka_preis_probe`.
const EDEKA_MARKETS: &[&str] = &["021868", "421696", "421347"];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let want = |chain: &str| args.is_empty() || args.iter().any(|a| a == chain);

    if want("lidl") {
        lidl();
    }
    if want("rewe") {
        rewe_chain();
    }
    if want("edeka") {
        edeka_chain();
    }
    Ok(())
}

/// Eine Messzeile, damit der Workflow sie ohne Rust auswerten kann.
fn line(chain: &str, market: &str, offers: usize, source: &str, taken: usize, verdict: &str) {
    println!("STREICHPREIS\t{chain}\t{market}\t{offers}\t{source}\t{taken}\t{verdict}");
}

/// Lidl: der Prospekt druckt den alten Preis, seit #40 wird er zugeteilt.
///
/// Gemessen wird gegen die Textebene desselben Prospekts, den der Lauf liest
/// — `printed` ist die Obergrenze dessen, was überhaupt dastünde, `assigned`
/// das, was ein Angebot bekommt. Die Lücke dazwischen ist Absicht und kein
/// Verlust: `assign_regular_prices` verwirft jede Plakette, die auf zwei
/// Preise passt.
fn lidl() {
    println!("\n=== Lidl (Prospekt, PLZ {LIDL_ZIP}) ===");
    let (flyer, xml) = match lidl_prospekt::fetch_bbox_layout(LIDL_ZIP) {
        Ok(v) => v,
        Err(e) => {
            println!("FEHLER: {e:#}");
            line("Lidl", LIDL_ZIP, 0, "?", 0, "FEHLER");
            return;
        }
    };
    println!(
        "Prospekt gültig {} bis {}, {} Seiten",
        flyer.offer_start_date.as_deref().unwrap_or("?"),
        flyer.offer_end_date.as_deref().unwrap_or("?"),
        flyer.pages.len()
    );
    let audit = lidl_prospekt::regular_price_audit(
        &xml,
        LIDL_ZIP,
        flyer.offer_start_date.as_deref(),
        flyer.offer_end_date.as_deref(),
    );
    println!(
        "{} Angebote — {} Streichpreis-Angaben im PDF, {} davon an einer verwertbaren Kachel, \
         {} zugeteilt",
        audit.offers, audit.printed, audit.owned, audit.assigned
    );
    if std::env::var("STREICHPREIS_DUMP").is_ok() {
        for text in &audit.samples {
            println!("  | {}", text.chars().take(140).collect::<String>());
        }
    }
    let verdict = if audit.assigned > 0 { "gebaut" } else { "OHNE STREICHPREIS" };
    line(
        "Lidl",
        LIDL_ZIP,
        audit.offers,
        &audit.printed.to_string(),
        audit.assigned,
        verdict,
    );
}

/// REWE: `-json` kennt kein Feld für den alten Preis, `-raw` schon.
///
/// Deshalb wird hier die **Rohantwort** befragt und nicht die Sicht, die der
/// Lauf für die laufende Woche benutzt: `priceData.regularPrice` gibt es in
/// beiden Wochen-Blöcken, trägt aber auch Etiketten („Knaller", „Aktion").
/// Gezählt wird nur, was sich als Betrag lesen lässt **und** über dem
/// Angebotspreis liegt — alles andere ist kein Streichpreis.
fn rewe_chain() {
    println!("\n=== REWE (rewerse -raw, Markt {REWE_MARKET}) ===");
    let cert = std::env::var("REWE_CERT").unwrap_or_else(|_| "cert.pem".into());
    let key = std::env::var("REWE_KEY").unwrap_or_else(|_| "private.key".into());
    let raw = match rewe::fetch_raw(REWE_MARKET, &cert, &key) {
        Ok(v) => v,
        Err(e) => {
            println!("FEHLER: {e:#}");
            line("REWE", REWE_MARKET, 0, "?", 0, "FEHLER");
            return;
        }
    };

    let mut gesamt = 0usize;
    let mut betraege = 0usize;
    let mut echte = 0usize;
    let mut etiketten: std::collections::BTreeMap<String, usize> = Default::default();

    for week in ["current", "next"] {
        let Some(block) = raw.pointer(&format!("/data/offers/{week}")) else {
            println!("  {week}: fehlt in der Antwort");
            continue;
        };
        let (mut w_offers, mut w_amounts, mut w_real) = (0usize, 0usize, 0usize);
        for cat in block.get("categories").and_then(|v| v.as_array()).into_iter().flatten() {
            for offer in cat.get("offers").and_then(|v| v.as_array()).into_iter().flatten() {
                if offer.get("cellType").and_then(|v| v.as_str()) != Some("DEFAULT") {
                    continue;
                }
                w_offers += 1;
                let price = amount(offer.pointer("/priceData/price"));
                let regular_raw = offer
                    .pointer("/priceData/regularPrice")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                match amount(offer.pointer("/priceData/regularPrice")) {
                    Some(r) => {
                        w_amounts += 1;
                        // Ein „alter Preis", der unter dem Angebotspreis
                        // liegt, ist keiner — dann steht dort etwas anderes.
                        if price.is_some_and(|p| r > p) {
                            w_real += 1;
                            if w_real <= 3 {
                                println!(
                                    "  Beispiel [{week}] {} — {:.2} statt {r:.2} €",
                                    offer.get("title").and_then(|v| v.as_str()).unwrap_or("?"),
                                    price.unwrap_or_default()
                                );
                            }
                        }
                    }
                    None if !regular_raw.is_empty() => {
                        *etiketten.entry(regular_raw).or_default() += 1;
                    }
                    None => {}
                }
            }
        }
        println!(
            "  {week}: {w_offers} Angebote, {w_amounts} mit Betrag in regularPrice, {w_real} davon über dem Preis"
        );
        gesamt += w_offers;
        betraege += w_amounts;
        echte += w_real;
    }

    if !etiketten.is_empty() {
        let mut top: Vec<_> = etiketten.iter().collect();
        top.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let top: Vec<String> =
            top.iter().take(6).map(|(t, n)| format!("{t:?}×{n}")).collect();
        println!("  regularPrice ohne Betrag: {}", top.join(", "));
    }

    println!("Summe: {gesamt} Angebote, {betraege} mit Betrag, {echte} brauchbar");
    // Was ankommt: Die laufende Woche liest `parse_offers` aus `-json` und
    // setzt `regular_price: None`. Solange das so ist, sind es null.
    let verdict = if echte > 0 { "QUELLE HAT MEHR" } else { "Quelle gibt nichts her" };
    line("REWE", REWE_MARKET, gesamt, &echte.to_string(), 0, verdict);
}

/// EDEKA: nach dem Markup-Wechsel vom 04.08. neu nachgesehen.
///
/// Gesucht wird nicht ein Feld, sondern jede Form, in der eine Seite einen
/// alten Preis drucken könnte: das Wort („statt", „UVP", „Normalpreis"), die
/// Darstellung (`line-through`, `<del>`, `<s>`) und die maschinenlesbare
/// Vorlesehilfe. Der Rabattsatz zählt ausdrücklich **nicht** als Quelle — aus
/// „-38 %" folgt ein Bereich, kein Preis (siehe #43).
fn edeka_chain() {
    for market_id in EDEKA_MARKETS {
        println!("\n=== EDEKA (Markt {market_id}) ===");
        let html = match edeka::fetch_offers_html(market_id) {
            Ok(h) => h,
            Err(e) => {
                println!("FEHLER: {e:#}");
                line("EDEKA", market_id, 0, "?", 0, "FEHLER");
                continue;
            }
        };
        let offers = edeka::parse_offers(&html, market_id).unwrap_or_default();
        let taken = offers.iter().filter(|o| o.regular_price.is_some()).count();

        let lower = html.to_lowercase();
        // Als ganze Wörter, nicht als Teilstück: „vorheriger erfolgreicher
        // Registrierung" steht dreimal im Kleingedruckten der App-Angebote
        // und hat den ersten Lauf dieser Probe „3 Funde" melden lassen. Eine
        // Messung, die den Rechtstext mitzählt, misst den Rechtstext.
        let woerter = WORT_FORMEN.find_iter(&lower).count();
        let strich: usize = ["line-through", "<del", "<s>", "durchgestrichen"]
            .iter()
            .map(|w| lower.matches(w).count())
            .sum();

        // Die Vorlesehilfe ist die eine Stelle, an der EDEKA Preise
        // maschinenlesbar führt — jede Form einmal, mit Anzahl.
        let doc = Html::parse_document(&html);
        let sel = Selector::parse("span.sr-only, div.sr-only").expect("Selektor");
        let mut formen: std::collections::BTreeMap<String, usize> = Default::default();
        for el in doc.select(&sel) {
            let text = el.text().collect::<String>().trim().to_string();
            if !text.to_lowercase().contains("preis") {
                continue;
            }
            let form: String = text
                .chars()
                .map(|c| if c.is_ascii_digit() { 'N' } else { c })
                .collect();
            *formen.entry(form).or_default() += 1;
        }
        println!(
            "{} Angebote, „statt/UVP/Normalpreis/vorher\" {woerter}×, Durchstreichung {strich}×",
            offers.len()
        );
        for (form, n) in &formen {
            println!("  {n:4}× {form}");
        }

        let quelle = woerter + strich;
        let verdict = if quelle > 0 { "QUELLE HAT MEHR" } else { "Quelle gibt nichts her" };
        line("EDEKA", market_id, offers.len(), &quelle.to_string(), taken, verdict);
    }
}

/// „1,29 €" → 1.29. Alles, was kein Betrag ist („Knaller"), fällt durch.
fn amount(v: Option<&serde_json::Value>) -> Option<f64> {
    let s = v?.as_str()?.trim();
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.replace(',', ".").parse().ok().filter(|p: &f64| *p > 0.0)
}
