use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};

use crate::models::{Market, Offer};
use crate::scrapers::{store_finder, util};

// ALDI Nord über aldi-nord.de (Next.js/Magnolia, server-seitig gerendert).
//
// Die Angebotsseite https://www.aldi-nord.de/angebote.html enthält im
// <script id="__NEXT_DATA__">-Block das Feld props.pageProps.apiData —
// ein JSON-String mit einem "OFFER_GET"-Eintrag, dessen res.algoliaDataMap
// alle Angebote der Woche strukturiert enthält (Preis, Streichpreis,
// Marke, Verkaufseinheit, Gültigkeit, Bilder, sogar isBiocidalProduct).
// res.categories liefert die Aktionstage mit Sektionstiteln und productIds
// -> Kategorie + Gültigkeitsdaten pro Produkt.
//
// ALDI-Nord-Angebote gelten bundesweit; find_market liefert deshalb einen
// synthetischen National-Markt (wie lidl.rs).

const OFFERS_URL: &str = "https://www.aldi-nord.de/angebote.html";

/// Echte Filiale über den Store-Finder; None ohne Filiale im Umkreis der PLZ.
/// Angebote bleiben der nationale Katalog (siehe store_finder.rs).
pub fn find_market(zip: &str) -> Result<Option<Market>> {
    Ok(store_finder::resolve("ALDI Nord", store_finder::aldi_nord_branch(zip), national()))
}

/// Der bundesweite Katalog-Markt. Seit Phase 12 nicht mehr nur Rückfall des
/// Store-Finders, sondern die Filiale, unter der ALDI Nord gespeichert wird.
pub fn national() -> Market {
    Market::new("ALDI_NORD_DE", "ALDI Nord Deutschland")
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    util::polite_pause(OFFERS_URL);
    let html = util::blocking_client()?
        .get(OFFERS_URL)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "de-DE,de;q=0.9")
        .send()
        .with_context(|| util::ctx("ALDI Nord", "Angebote laden", OFFERS_URL))?
        .error_for_status()
        .with_context(|| util::ctx("ALDI Nord", "Angebote laden (HTTP-Status)", OFFERS_URL))?
        .text()
        .with_context(|| util::ctx("ALDI Nord", "Angebote lesen", OFFERS_URL))?;

    let offers = parse_offers(&html, &market.id)?;
    if offers.is_empty() {
        bail!("Keine ALDI-Nord-Angebote gefunden — Seitenstruktur hat sich möglicherweise geändert");
    }
    Ok(offers)
}

pub fn parse_offers(html: &str, market_id: &str) -> Result<Vec<Offer>> {
    let next_data = extract_next_data(html)
        .context("__NEXT_DATA__-Block nicht gefunden — Seitenstruktur geändert?")?;
    let root: serde_json::Value =
        serde_json::from_str(next_data).context("__NEXT_DATA__ JSON parse fehlgeschlagen")?;

    let api_data_str = root
        .pointer("/props/pageProps/apiData")
        .and_then(|v| v.as_str())
        .context("apiData fehlt in __NEXT_DATA__")?;
    let api_data: serde_json::Value =
        serde_json::from_str(api_data_str).context("apiData JSON parse fehlgeschlagen")?;

    let res = api_data
        .as_array()
        .and_then(|entries| {
            entries.iter().find_map(|e| {
                let arr = e.as_array()?;
                if arr.first()?.as_str()? == "OFFER_GET" {
                    arr.get(1)?.get("res")
                } else {
                    None
                }
            })
        })
        .context("OFFER_GET fehlt in apiData")?;

    // productId -> (Sektionstitel, Aktions-Start, Aktions-Ende)
    let mut meta: HashMap<String, (Option<String>, Option<String>, Option<String>)> =
        HashMap::new();
    if let Some(categories) = res.get("categories").and_then(|v| v.as_array()) {
        for aktion in categories {
            let start = aktion.get("startDate").and_then(|v| v.as_str()).map(String::from);
            let end = aktion.get("endDate").and_then(|v| v.as_str()).map(String::from);
            let Some(content) = aktion.get("content").and_then(|v| v.as_array()) else { continue };
            for section in content {
                let title = section.get("title").and_then(|v| v.as_str()).map(String::from);
                let Some(ids) = section.get("productIds").and_then(|v| v.as_array()) else {
                    continue;
                };
                for id in ids.iter().filter_map(|v| v.as_str()) {
                    meta.entry(id.to_string())
                        .or_insert_with(|| (title.clone(), start.clone(), end.clone()));
                }
            }
        }
    }

    let data_map = res
        .get("algoliaDataMap")
        .and_then(|v| v.as_object())
        .context("algoliaDataMap fehlt in OFFER_GET")?;

    let mut offers = Vec::new();
    let mut seen = HashSet::new();

    for (object_id, entry) in data_map {
        let Some(title) = entry.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
        else {
            continue;
        };
        let title = title.to_string();

        let (category, aktion_from, aktion_until) = meta
            .get(object_id)
            .cloned()
            .unwrap_or((None, None, None));

        let price_obj = entry.get("currentPrice");
        let price = price_obj.and_then(|p| p.get("priceValue")).and_then(|v| v.as_f64());
        let regular_price = price_obj
            .and_then(|p| p.pointer("/strikePrice/strikePriceValue"))
            .and_then(|v| v.as_f64());

        let subtitle = entry
            .get("salesUnit")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                entry
                    .get("shortDescription")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            });
        let overline = entry
            .get("brandName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);

        // Gültigkeit: Aktionstag aus categories, sonst promotionPrices-LocalDates.
        let promo = entry.pointer("/promotionPrices/0");
        let valid_from = aktion_from.or_else(|| {
            promo?
                .get("validFromLocalDate")
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        let valid_until = aktion_until.or_else(|| {
            promo?
                .get("validUntilLocalDate")
                .and_then(|v| v.as_str())
                .map(String::from)
        });

        let category = category.or_else(|| {
            entry
                .get("mainCategoryID")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
        });

        let images: Vec<String> = entry
            .get("assets")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.get("url").and_then(|v| v.as_str()))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let biozid = entry
            .get("isBiocidalProduct")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let id = Offer::build_id(market_id, &title, valid_from.as_deref());
        if !seen.insert(id.clone()) {
            continue;
        }

        offers.push(Offer {
            id,
            market_id: market_id.to_string(),
            title,
            subtitle,
            overline,
            price,
            regular_price,
            category,
            nutri_score: None,
            valid_from,
            valid_until,
            images,
            biozid,
            flyer_page: None,
        });
    }

    Ok(offers)
}

fn extract_next_data(html: &str) -> Option<&str> {
    let marker = "<script id=\"__NEXT_DATA__\" type=\"application/json\"";
    let start = html.find(marker)?;
    let json_start = html[start..].find('>')? + start + 1;
    let json_end = html[json_start..].find("</script>")? + json_start;
    Some(&html[json_start..json_end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_data_extraction() {
        let html = r#"<html><script id="__NEXT_DATA__" type="application/json" nonce="x">{"a":1}</script></html>"#;
        assert_eq!(extract_next_data(html), Some(r#"{"a":1}"#));
        assert_eq!(extract_next_data("<html></html>"), None);
    }

    /// Live-Test gegen aldi-nord.de: cargo test aldi_nord -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen aldi-nord.de"]
    fn live_fetch_offers() {
        let market = find_market("10115").expect("Markt").expect("Filiale");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(5) {
            println!(
                "- {} | {:?} | {:?} € (statt {:?}) | {:?} | {:?} bis {:?}",
                o.title, o.subtitle, o.price, o.regular_price, o.category, o.valid_from, o.valid_until
            );
        }
        assert!(offers.len() >= 80, "Erwartet >= 80 Angebote, war {}", offers.len());
    }
}
