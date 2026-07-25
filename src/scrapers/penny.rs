use anyhow::{Context, Result, bail};
use std::collections::HashSet;

use crate::models::{Branch, Market, Offer};
use crate::scrapers::util;

// Penny-Angebote über die öffentlichen JSON-Endpoints von penny.de.
// Kein Client-Zertifikat nötig, nur ein Browser-User-Agent.
//
// Märkte:     GET /.rest/market                     -> Array aller Märkte (PLZ-Filterung clientseitig)
// Kategorien: aus dem HTML von /angebote            -> data-category-name + data-current-week/-next-week
// Angebote:   GET /.rest/offers/by-category/{JAHR-WOCHE}/{kategorie}?region={sellingRegion}
//             -> { "offerTiles": [...] }

const BASE_URL: &str = "https://www.penny.de";

pub fn find_market(zip: &str) -> Result<Market> {
    block_on(async {
        let client = build_client()?;
        let markets = fetch_markets(&client).await?;

        // Exakte PLZ bevorzugen, sonst den Markt mit der numerisch nächsten PLZ nehmen
        let target: i64 = zip.parse().with_context(|| format!("Ungültige PLZ: {zip}"))?;
        let market = markets
            .iter()
            .filter_map(|m| {
                let mzip: i64 = m.get("zipCode")?.as_str()?.parse().ok()?;
                Some(((mzip - target).abs(), m))
            })
            .min_by_key(|(dist, _)| *dist)
            .map(|(_, m)| m)
            .with_context(|| format!("Kein Penny-Markt für PLZ {zip} gefunden"))?;

        let id = market
            .get("wwIdent")
            .and_then(|v| v.as_str())
            .context("wwIdent fehlt in der Markt-Antwort")?;
        let name = market
            .get("marketName")
            .and_then(|v| v.as_str())
            .unwrap_or("PENNY");

        // Koordinaten kommen als Strings ("53.03672"); fehlend/kaputt -> None.
        let coord = |k: &str| market.get(k).and_then(|v| v.as_str()).and_then(|s| s.parse().ok());
        Ok(Market::new(id, name).with_geo(coord("latitude"), coord("longitude")))
    })
}

/// Das komplette deutsche Penny-Verzeichnis — ein einziger Request.
///
/// `/.rest/market` liefert ohnehin alle Märkte auf einmal (2120 am
/// 2026-07-25), `find_market` filtert davon nur clientseitig nach PLZ. Für
/// das Verzeichnis nehmen wir dieselbe Liste unverkürzt.
pub fn fetch_branches() -> Result<Vec<Branch>> {
    block_on(async {
        let client = build_client()?;
        let markets = fetch_markets(&client).await?;
        Ok(parse_branches(&markets))
    })
}

/// Marktliste als Verzeichniszeilen. Koordinaten kommen als Strings
/// ("53.03672"); fehlend oder kaputt -> None.
pub fn parse_branches(markets: &[serde_json::Value]) -> Vec<Branch> {
    markets
        .iter()
        .filter_map(|market| {
            let id = market.get("wwIdent").and_then(|v| v.as_str())?;
            let text = |key: &str| {
                market
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let coord =
                |key: &str| market.get(key).and_then(|v| v.as_str()).and_then(|s| s.parse().ok());
            let name = text("marketName").unwrap_or_else(|| "PENNY".to_string());
            Some(
                Branch::new(id, "Penny", name, "penny-rest")
                    .with_address(text("streetWithHouseNumber"), text("zipCode"), text("city"))
                    .with_geo(coord("latitude"), coord("longitude")),
            )
        })
        .collect()
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    block_on(async {
        let client = build_client()?;

        // sellingRegion des Markts über die Marktliste ermitteln
        let markets = fetch_markets(&client).await?;
        let region = markets
            .iter()
            .find(|m| m.get("wwIdent").and_then(|v| v.as_str()) == Some(market.id.as_str()))
            .and_then(|m| m.get("sellingRegion").and_then(|v| v.as_str()))
            .map(String::from);

        // Kategorien und Wochen stehen im HTML der Angebotsseite
        let angebote_url = format!("{BASE_URL}/angebote");
        util::polite_pause(&angebote_url);
        let html = client
            .get(&angebote_url)
            .send()
            .await
            .with_context(|| util::ctx("Penny", "Angebotsseite laden", &angebote_url))?
            .text()
            .await
            .with_context(|| util::ctx("Penny", "Angebotsseite lesen", &angebote_url))?;

        let categories = extract_attr_values(&html, "data-category-name");
        if categories.is_empty() {
            bail!("Keine Angebots-Kategorien auf penny.de/angebote gefunden");
        }
        let current_week = extract_attr_values(&html, "data-current-week")
            .into_iter()
            .next()
            .context("data-current-week fehlt auf penny.de/angebote")?;
        let next_week = extract_attr_values(&html, "data-next-week").into_iter().next();

        let mut offers = Vec::new();
        let mut weeks = vec![current_week];
        weeks.extend(next_week);

        for week in &weeks {
            let (valid_from, valid_until) = week_dates(week)?;
            let mut seen = HashSet::new();

            for category in &categories {
                let mut url = format!("{BASE_URL}/.rest/offers/by-category/{week}/{category}");
                if let Some(r) = &region {
                    url.push_str(&format!("?region={r}"));
                }

                util::polite_pause(&url);
                let resp = client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| util::ctx("Penny", "Angebote laden", &url))?;

                // Nicht jede Kategorie existiert in jeder Region/Woche
                if !resp.status().is_success() {
                    continue;
                }

                let raw: serde_json::Value = match resp.json().await {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                parse_offer_tiles(
                    &raw,
                    &market.id,
                    category,
                    &valid_from,
                    &valid_until,
                    &mut seen,
                    &mut offers,
                );
            }
        }

        Ok(offers)
    })
}

// Alle offerTiles einer Kategorie-Antwort in Offers übersetzen.
// `seen` dedupliziert nach Titel über Kategorien hinweg (top-angebote
// überschneidet sich mit den Themen-Kategorien).
pub fn parse_offer_tiles(
    raw: &serde_json::Value,
    market_id: &str,
    category: &str,
    valid_from: &str,
    valid_until: &str,
    seen: &mut HashSet<String>,
    offers: &mut Vec<Offer>,
) {
    let tiles = raw
        .get("offerTiles")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for tile in &tiles {
        let Some(title) = tile.get("title").and_then(|v| v.as_str()) else { continue };
        let title = title.to_string();

        // Kategorien überschneiden sich (z. B. top-angebote vs. food)
        if !seen.insert(title.to_lowercase()) {
            continue;
        }

        let subtitle = tile.get("quantity").and_then(|v| v.as_str()).map(String::from);
        let overline = tile
            .get("headline")
            .or_else(|| tile.get("actionMarker"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let price = tile.get("price").and_then(json_price);
        let regular_price = tile
            .get("listPrice")
            .and_then(json_price)
            .or_else(|| tile.get("crossOutPrice").and_then(json_price))
            .or_else(|| tile.get("originalPrice").and_then(json_price));

        let images: Vec<String> = tile
            .get("imageRendition")
            .and_then(|r| {
                ["tileLg", "tileMd", "tileSm", "tileXs"]
                    .iter()
                    .find_map(|k| r.get(k).and_then(|v| v.as_str()))
            })
            .map(|s| vec![s.to_string()])
            .unwrap_or_default();

        let id = Offer::build_id(market_id, &title, Some(valid_from));

        offers.push(Offer {
            id,
            market_id: market_id.to_string(),
            title,
            subtitle,
            overline,
            price,
            regular_price,
            category: Some(category.to_string()),
            nutri_score: None,
            valid_from: Some(valid_from.to_string()),
            valid_until: Some(valid_until.to_string()),
            images,
            biozid: false,
            flyer_page: None,
        });
    }
}

fn build_client() -> Result<reqwest::Client> {
    util::async_client()
}

async fn fetch_markets(client: &reqwest::Client) -> Result<Vec<serde_json::Value>> {
    let url = format!("{BASE_URL}/.rest/market");
    util::polite_pause(&url);
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| util::ctx("Penny", "Markt-Lookup", &url))?;

    if !resp.status().is_success() {
        bail!("[Penny] Markt-Lookup lieferte HTTP {}: {url}", resp.status());
    }

    let raw: serde_json::Value = resp
        .json()
        .await
        .with_context(|| util::ctx("Penny", "Markt-Lookup JSON parsen", &url))?;

    raw.as_array()
        .cloned()
        .context("Penny-Marktliste ist kein JSON-Array")
}

// Alle Werte von attr="..." aus dem HTML, dedupliziert in Reihenfolge des Auftretens
fn extract_attr_values(html: &str, attr: &str) -> Vec<String> {
    let needle = format!("{attr}=\"");
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut rest = html;
    while let Some(pos) = rest.find(&needle) {
        rest = &rest[pos + needle.len()..];
        if let Some(end) = rest.find('"') {
            let val = &rest[..end];
            if !val.is_empty() && seen.insert(val.to_string()) {
                out.push(val.to_string());
            }
            rest = &rest[end..];
        } else {
            break;
        }
    }
    out
}

// "2026-29" -> (Montag, Sonntag) der ISO-Woche als YYYY-MM-DD
fn week_dates(week: &str) -> Result<(String, String)> {
    let (year, wk) = week
        .split_once('-')
        .with_context(|| format!("Unerwartetes Wochenformat: {week}"))?;
    let year: i32 = year.parse().with_context(|| format!("Unerwartetes Wochenformat: {week}"))?;
    let wk: u32 = wk.parse().with_context(|| format!("Unerwartetes Wochenformat: {week}"))?;

    let monday = chrono::NaiveDate::from_isoywd_opt(year, wk, chrono::Weekday::Mon)
        .with_context(|| format!("Ungültige ISO-Woche: {week}"))?;
    let sunday = monday + chrono::Days::new(6);
    Ok((monday.format("%Y-%m-%d").to_string(), sunday.format("%Y-%m-%d").to_string()))
}

fn json_price(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| {
        // Aktionspreise kommen als String mit Fußnoten-Sternchen, z. B. "0.49*"
        let s = v.as_str()?;
        s.trim()
            .trim_end_matches(['*', '€'])
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
    })
}

#[cfg(test)]
mod price_tests {
    use super::json_price;
    use serde_json::json;

    #[test]
    fn parses_prices_with_footnote_marker() {
        assert_eq!(json_price(&json!("0.49*")), Some(0.49));
        assert_eq!(json_price(&json!("1,29 €")), Some(1.29));
        assert_eq!(json_price(&json!(2.99)), Some(2.99));
        assert_eq!(json_price(&json!("Aktion")), None);
    }
}

// main() ist synchron — eigener Runtime, damit die Aufrufer-API wie bei rewe.rs sync bleibt.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live-Test gegen penny.de: cargo test penny -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen penny.de"]
    fn live_fetch_offers_for_example_zip() {
        let market = find_market("01219").expect("Markt für 01219");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(15) {
            println!(
                "- {} | {:?} | {:?} € (statt {:?}) | {:?} | {} bis {}",
                o.title,
                o.subtitle,
                o.price,
                o.regular_price,
                o.category,
                o.valid_from.as_deref().unwrap_or("?"),
                o.valid_until.as_deref().unwrap_or("?"),
            );
        }
        assert!(!offers.is_empty(), "Keine Angebote geparst");
    }
}
