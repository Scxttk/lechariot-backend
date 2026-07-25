use anyhow::{Context, Result, bail};
use std::collections::HashSet;

use crate::models::{Market, Offer};
use crate::scrapers::store_finder;
use crate::scrapers::util::{self, curl_get};

// ALDI Süd über die öffentliche Produktsuche-API der Website (kein Login):
//
//   GET https://api.aldi-sued.de/v3/product-search
//       ?currency=EUR&serviceType=walk-in
//       &categoryKey=1588161426582123   (Kategoriebaum "Wochenangebote")
//       &limit=60&offset=...
//
// Der Kategoriebaum "Wochenangebote" enthält die aktuellen Filial-Angebote
// (Frische-, Marken-, Eigenmarken-Angebote) mit Preis in Cent, Streichpreis
// ("wasPriceDisplay"), Marke, Verkaufseinheit und Bildern. Gültigkeitsdaten
// liefert die API nicht — valid_from/valid_until bleiben None.
// Die tagesbezogenen Aktionsartikel (/angebote/<datum>) hängen an einem
// kuratierten Promotion-Baum im Frontend und sind hier bewusst nicht
// enthalten. Die API steckt hinter Akamai (TLS-Fingerprinting), deshalb
// util::curl_get statt reqwest.
//
// ALDI-Süd-Angebote gelten im gesamten Süd-Gebiet einheitlich; find_market
// liefert deshalb einen synthetischen National-Markt (wie lidl.rs).

const SEARCH_URL: &str = "https://api.aldi-sued.de/v3/product-search";
const WEEKLY_OFFERS_CATEGORY: &str = "1588161426582123";
const PAGE_SIZE: usize = 60; // API erlaubt nur [12,16,24,30,32,48,60]
const MAX_OFFERS: usize = 1000;

/// Echte Filiale über den Store-Finder; None ohne Filiale im Umkreis der PLZ.
/// Angebote bleiben der nationale Katalog (siehe store_finder.rs).
pub fn find_market(zip: &str) -> Result<Option<Market>> {
    Ok(store_finder::resolve("ALDI SÜD", store_finder::aldi_sued_branch(zip), national()))
}

/// Der bundesweite Katalog-Markt. Seit Phase 12 nicht mehr nur Rückfall des
/// Store-Finders, sondern die Filiale, unter der ALDI SÜD gespeichert wird.
pub fn national() -> Market {
    Market::new("ALDI_SUED_DE", "ALDI Süd Deutschland")
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    let mut offers = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = 0usize;

    loop {
        let url = format!(
            "{SEARCH_URL}?currency=EUR&serviceType=walk-in&categoryKey={WEEKLY_OFFERS_CATEGORY}&limit={PAGE_SIZE}&offset={offset}"
        );
        let body = curl_get(
            &url,
            &[
                ("Accept", "application/json, text/plain, */*"),
                ("Origin", "https://www.aldi-sued.de"),
                ("Referer", "https://www.aldi-sued.de/"),
                ("Sec-Fetch-Site", "same-site"),
                ("Sec-Fetch-Mode", "cors"),
                ("Sec-Fetch-Dest", "empty"),
            ],
        )
        .with_context(|| util::ctx("ALDI Süd", "Angebote laden", &url))?;

        let raw: serde_json::Value = serde_json::from_str(&body)
            .with_context(|| util::ctx("ALDI Süd", "Angebote JSON parsen", &url))?;

        let total = raw
            .pointer("/meta/pagination/totalCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let items = raw.get("data").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        if items.is_empty() {
            break;
        }

        for item in &items {
            if let Some(offer) = parse_product(item, &market.id) {
                if seen.insert(offer.id.clone()) {
                    offers.push(offer);
                }
            }
        }

        offset += items.len();
        if offset >= total.min(MAX_OFFERS) {
            break;
        }
    }

    if offers.is_empty() {
        bail!("[ALDI Süd] Keine Angebote gefunden ({SEARCH_URL}) — API-Struktur hat sich möglicherweise geändert");
    }
    Ok(offers)
}

// Ein Produkt der Suche defensiv in ein Offer übersetzen.
pub fn parse_product(item: &serde_json::Value, market_id: &str) -> Option<Offer> {
    let title = item
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();

    let price_obj = item.get("price");
    // Preise kommen in Cent ("amount": 189 -> 1,89 €).
    let price = price_obj
        .and_then(|p| p.get("amountRelevant").or_else(|| p.get("amount")))
        .and_then(|v| v.as_f64())
        .map(|cents| cents / 100.0);
    // Streichpreis nur als Anzeigetext: "3,49 €"
    let regular_price = price_obj
        .and_then(|p| p.get("wasPriceDisplay"))
        .and_then(|v| v.as_str())
        .and_then(parse_price_display);

    let subtitle = item
        .get("sellingSize")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let overline = item
        .get("brandName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    // Kategorien: ["Wochenangebote", "Frischeprodukte im Angebot"] -> spezifischste
    let category = item
        .get("categories")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter_map(|c| c.get("name").and_then(|v| v.as_str()))
                .filter(|n| !n.is_empty())
                .last()
        })
        .map(String::from);

    // Bild-URLs enthalten {width}/{slug}-Platzhalter.
    let slug = item.get("urlSlugText").and_then(|v| v.as_str()).unwrap_or("product");
    let images: Vec<String> = item
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("url").and_then(|v| v.as_str()))
                .map(|u| u.replace("{width}", "450").replace("{slug}", slug))
                .collect()
        })
        .unwrap_or_default();

    let id = Offer::build_id(market_id, &title, None);

    Some(Offer {
        id,
        market_id: market_id.to_string(),
        title,
        subtitle,
        overline,
        price,
        regular_price,
        category,
        nutri_score: None,
        // Die API liefert keine Gültigkeitsdaten; die Wochenangebote gelten
        // Mo–Sa der laufenden Woche. Ohne valid_from würde der Push die
        // Angebote verwerfen (Upsert-Schlüssel ist nicht NULL-sicher).
        valid_from: Some(current_week_monday()),
        valid_until: Some(current_week_saturday()),
        images,
        biozid: false,
        flyer_page: None,
    })
}

/// Montag der laufenden Woche als "YYYY-MM-DD" (wie rewe.rs).
fn current_week_monday() -> String {
    use chrono::Datelike;
    let today = chrono::Local::now().date_naive();
    let monday = today - chrono::Days::new(u64::from(today.weekday().num_days_from_monday()));
    monday.format("%Y-%m-%d").to_string()
}

/// Samstag der laufenden Woche als "YYYY-MM-DD".
fn current_week_saturday() -> String {
    use chrono::Datelike;
    let today = chrono::Local::now().date_naive();
    let monday = today - chrono::Days::new(u64::from(today.weekday().num_days_from_monday()));
    (monday + chrono::Days::new(5)).format("%Y-%m-%d").to_string()
}

// "3,49 €" -> 3.49
fn parse_price_display(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',')
        .collect::<String>()
        .replace(',', ".");
    cleaned.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_display_parsing() {
        assert_eq!(parse_price_display("3,49 €"), Some(3.49));
        assert_eq!(parse_price_display("0,79 €"), Some(0.79));
        assert_eq!(parse_price_display(""), None);
    }

    /// Live-Test gegen api.aldi-sued.de: cargo test aldi_sued -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen api.aldi-sued.de"]
    fn live_fetch_offers() {
        let market = find_market("86150").expect("Markt").expect("Filiale");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(5) {
            println!(
                "- {} | {:?} | {:?} € (statt {:?}) | {:?} | {:?}",
                o.title, o.subtitle, o.price, o.regular_price, o.category, o.overline
            );
        }
        // Wochenangebote sind typischerweise 60-100 Artikel.
        assert!(offers.len() >= 50, "Erwartet >= 50 Angebote, war {}", offers.len());
    }
}
