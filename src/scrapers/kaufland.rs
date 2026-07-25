use anyhow::{Context, Result, bail};
use chrono::Datelike;
use scraper::{ElementRef, Html, Selector};

use crate::models::{Branch, Market, Offer};
use crate::scrapers::util;

// Scraped von filiale.kaufland.de: der Store-Finder ist öffentliches JSON,
// die Angebotsseite ist server-seitig gerendert. Die Filiale wird über das
// Cookie `x-aem-variant=<store id>` gebunden (verifiziert 2026-07).

const STORE_FINDER_URL: &str = "https://filiale.kaufland.de/.klstorefinder.json";
const OFFERS_URL: &str = "https://filiale.kaufland.de/angebote/uebersicht.html";

fn client() -> Result<reqwest::blocking::Client> {
    util::blocking_client()
}

pub fn find_market(zip: &str) -> Result<Market> {
    util::polite_pause(STORE_FINDER_URL);
    let stores: Vec<serde_json::Value> = client()?
        .get(STORE_FINDER_URL)
        .send()
        .with_context(|| util::ctx("Kaufland", "Markt-Lookup", STORE_FINDER_URL))?
        .error_for_status()
        .with_context(|| util::ctx("Kaufland", "Markt-Lookup (HTTP-Status)", STORE_FINDER_URL))?
        .json()
        .with_context(|| util::ctx("Kaufland", "Markt-Lookup JSON parsen", STORE_FINDER_URL))?;

    // Exakter PLZ-Treffer, sonst nächste Filiale im Umkreis (Großstadt-PLZs
    // wie 80331 haben selten eine Filiale mit genau dieser PLZ)
    let store = match stores.iter().find(|s| s.get("pc").and_then(|v| v.as_str()) == Some(zip)) {
        Some(s) => s,
        None => nearest_store(&stores, zip)?
            .with_context(|| format!("Keine Kaufland-Filiale im Umkreis von PLZ {zip}"))?,
    };

    let id = store
        .get("n")
        .and_then(|v| v.as_str())
        .context("Filial-ID fehlt in der Store-Finder-Antwort")?;
    let name = store
        .get("cn")
        .and_then(|v| v.as_str())
        .unwrap_or("Kaufland");

    // Koordinaten kommen als Strings ("52.066…"); fehlend/kaputt -> None.
    let coord = |k: &str| store.get(k).and_then(|v| v.as_str()).and_then(|s| s.parse().ok());
    Ok(Market::new(id, name).with_geo(coord("lat"), coord("lng")))
}

/// Das komplette deutsche Kaufland-Verzeichnis — ein einziger Request.
///
/// `.klstorefinder.json` ist die Datei, aus der auch der Filialfinder auf
/// filiale.kaufland.de arbeitet: alle Filialen mit Adresse und Koordinaten,
/// ohne Suchparameter (787 Stück am 2026-07-25). Deshalb ist Kaufland die
/// einzige Kette, die im Verzeichnis flächendeckend billig ist.
pub fn fetch_branches() -> Result<Vec<Branch>> {
    util::polite_pause(STORE_FINDER_URL);
    let stores: Vec<serde_json::Value> = client()?
        .get(STORE_FINDER_URL)
        .send()
        .with_context(|| util::ctx("Kaufland", "Filialverzeichnis", STORE_FINDER_URL))?
        .error_for_status()
        .with_context(|| util::ctx("Kaufland", "Filialverzeichnis (HTTP-Status)", STORE_FINDER_URL))?
        .json()
        .with_context(|| util::ctx("Kaufland", "Filialverzeichnis JSON parsen", STORE_FINDER_URL))?;
    Ok(parse_branches(&stores))
}

/// Store-Finder-Einträge als Verzeichniszeilen.
///
/// Die Feldnamen sind Kürzel: `n` Filial-ID, `cn` Name, `sn` Straße,
/// `pc` PLZ, `t` Ort, `lat`/`lng` als Strings.
pub fn parse_branches(stores: &[serde_json::Value]) -> Vec<Branch> {
    stores
        .iter()
        .filter_map(|store| {
            let id = store.get("n").and_then(|v| v.as_str())?;
            let text = |key: &str| {
                store
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let coord =
                |key: &str| store.get(key).and_then(|v| v.as_str()).and_then(|s| s.parse().ok());
            let name = text("cn").unwrap_or_else(|| "Kaufland".to_string());
            Some(
                Branch::new(id, "Kaufland", name, "kaufland-storefinder")
                    .with_address(text("sn"), text("pc"), text("t"))
                    .with_geo(coord("lat"), coord("lng")),
            )
        })
        .collect()
}

/// Nächste Filiale zur geocodierten PLZ innerhalb von CUTOFF_KM; None, wenn
/// keine im Umkreis liegt.
fn nearest_store<'a>(
    stores: &'a [serde_json::Value],
    zip: &str,
) -> Result<Option<&'a serde_json::Value>> {
    use crate::scrapers::store_finder::{CUTOFF_KM, distance_km, geocode_plz};
    let center = geocode_plz(zip)?;
    let coords = |s: &serde_json::Value| -> Option<(f64, f64)> {
        let get = |k: &str| s.get(k)?.as_str()?.parse::<f64>().ok();
        Some((get("lat")?, get("lng")?))
    };
    Ok(stores
        .iter()
        .filter_map(|s| coords(s).map(|c| (s, distance_km(center, c))))
        .filter(|(_, d)| *d <= CUTOFF_KM)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(s, _)| s))
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    util::polite_pause(OFFERS_URL);
    let html = client()?
        .get(OFFERS_URL)
        .header("Cookie", format!("x-aem-variant={}", market.id))
        .send()
        .with_context(|| util::ctx("Kaufland", "Angebote laden", OFFERS_URL))?
        .error_for_status()
        .with_context(|| util::ctx("Kaufland", "Angebote laden (HTTP-Status)", OFFERS_URL))?
        .text()
        .with_context(|| util::ctx("Kaufland", "Angebote lesen", OFFERS_URL))?;

    let offers = parse_offers(&html, &market.id)?;
    if offers.is_empty() {
        bail!("Keine Angebote gefunden — Seitenstruktur hat sich möglicherweise geändert");
    }
    Ok(offers)
}

// Kaufland listet dasselbe Angebot in mehreren Kategorien (z. B. in der
// Warengruppe und zusätzlich in "Unsere Knüller"). Es wird bewusst nicht
// hier dedupliziert: die Duplikate teilen dieselbe Offer-ID und werden beim
// DB-Upsert zusammengeführt.
pub fn parse_offers(html: &str, market_id: &str) -> Result<Vec<Offer>> {
    let doc = Html::parse_document(html);
    let sel_section = sel("div.k-product-section");
    let sel_headline = sel("h2.k-product-section__headline");
    let sel_subheadline = sel("div.k-product-section__subheadline");
    let sel_tile = sel("a.k-product-tile");
    let sel_title = sel(".k-product-tile__title");
    let sel_subtitle = sel(".k-product-tile__subtitle");
    let sel_unit_price = sel(".k-product-tile__unit-price");
    let sel_base_price = sel(".k-product-tile__base-price");
    let sel_price = sel(".k-price-tag__price");
    let sel_old_price = sel(".k-price-tag__old-price-line-through");
    let sel_image = sel("img.k-product-tile__main-image");

    let mut offers = Vec::new();

    for section in doc.select(&sel_section) {
        let category = text_of(section, &sel_headline);
        let (valid_from, valid_until) = text_of(section, &sel_subheadline)
            .and_then(|s| parse_date_range(&s))
            .map_or((None, None), |(f, u)| (Some(f), Some(u)));

        for tile in section.select(&sel_tile) {
            // Markenlose Produkte haben einen leeren Titel und stehen nur im Subtitle.
            let mut subtitle = text_of(tile, &sel_subtitle);
            let title = match text_of(tile, &sel_title) {
                Some(t) => t,
                None => match subtitle.take() {
                    Some(s) => s,
                    None => continue,
                },
            };
            let overline = match (text_of(tile, &sel_unit_price), text_of(tile, &sel_base_price)) {
                (Some(unit), Some(base)) => Some(format!("{unit} {base}")),
                (unit, base) => unit.or(base),
            };

            let price = text_of(tile, &sel_price).and_then(|s| parse_price(&s));
            let regular_price = text_of(tile, &sel_old_price).and_then(|s| parse_price(&s));

            let mut images: Vec<String> = Vec::new();
            if let Some(img) = tile.select(&sel_image).next() {
                if let Some(src) = img.value().attr("src") {
                    images.push(src.to_string());
                }
            }

            // Kaufland: Titel ist die Marke, das Produkt steht im Untertitel —
            // ohne ihn kollidieren alle Angebote einer Marke auf derselben ID
            let id_key = match &subtitle {
                Some(s) if !s.is_empty() => format!("{title} {s}"),
                _ => title.clone(),
            };
            let id = Offer::build_id(market_id, &id_key, valid_from.as_deref());

            offers.push(Offer {
                id,
                market_id: market_id.to_string(),
                title,
                subtitle,
                overline,
                price,
                regular_price,
                category: category.clone(),
                nutri_score: None,
                valid_from: valid_from.clone(),
                valid_until: valid_until.clone(),
                images,
                biozid: false,
                flyer_page: None,
            });
        }
    }

    Ok(offers)
}

fn sel(css: &str) -> Selector {
    // Alle Selektoren sind statische Literale — parse kann nicht fehlschlagen.
    Selector::parse(css).expect("statischer CSS-Selektor")
}

fn text_of(el: ElementRef, selector: &Selector) -> Option<String> {
    let text: String = el.select(selector).next()?.text().collect();
    let text = text.trim();
    if text.is_empty() { None } else { Some(text.to_string()) }
}

/// Preise wie "0.99", "2.79*" oder "2.79**" (Fußnoten-Sternchen).
fn parse_price(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    cleaned.parse::<f64>().ok()
}

/// "Gültig vom 16.07. bis 22.07." -> ("2026-07-16", "2026-07-22"),
/// "Angebote gültig am 17.07." -> ("2026-07-17", "2026-07-17").
/// Das Jahr fehlt auf der Seite; es wird vom heutigen Datum übernommen,
/// mit Jahreswechsel-Korrektur für Bereiche über Silvester.
fn parse_date_range(s: &str) -> Option<(String, String)> {
    let mut days_months = Vec::new();
    let mut parts = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty());
    while let (Some(d), Some(m)) = (parts.next(), parts.next()) {
        let day: u32 = d.parse().ok()?;
        let month: u32 = m.parse().ok()?;
        if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
            return None;
        }
        days_months.push((day, month));
    }
    let ((fd, fm), (ud, um)) = match days_months.as_slice() {
        [single] => (*single, *single),
        [from, until] => (*from, *until),
        _ => return None,
    };

    let year = chrono::Local::now().year();
    let until_year = if um < fm { year + 1 } else { year };

    Some((
        format!("{year}-{fm:02}-{fd:02}"),
        format!("{until_year}-{um:02}-{ud:02}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_range_parsing() {
        assert_eq!(
            parse_date_range("Gültig vom 16.07. bis 22.07."),
            Some((
                format!("{}-07-16", chrono::Local::now().year()),
                format!("{}-07-22", chrono::Local::now().year())
            ))
        );
        assert_eq!(
            parse_date_range("Angebote gültig am 17.07."),
            Some((
                format!("{}-07-17", chrono::Local::now().year()),
                format!("{}-07-17", chrono::Local::now().year())
            ))
        );
        assert_eq!(parse_date_range("Unsere Knüller"), None);
    }

    #[test]
    fn price_parsing() {
        assert_eq!(parse_price("0.99"), Some(0.99));
        assert_eq!(parse_price("2.79*"), Some(2.79));
        assert_eq!(parse_price("nicht verfügbar"), None);
    }

    /// Live-Test gegen filiale.kaufland.de — bewusst ignored.
    /// Ausführen mit: cargo test -- --ignored --nocapture kaufland
    #[test]
    #[ignore]
    fn live_fetch_offers() {
        let market = find_market("01219").expect("Markt für PLZ 01219");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote gefunden.", offers.len());
        assert!(offers.len() >= 80, "erwartet >= 80 Angebote, waren {}", offers.len());

        for offer in offers.iter().take(5) {
            println!(
                "[{}] {} {} — {:?} (statt {:?})  [{:?} – {:?}]",
                offer.category.as_deref().unwrap_or("?"),
                offer.title,
                offer.subtitle.as_deref().unwrap_or(""),
                offer.price,
                offer.regular_price,
                offer.valid_from,
                offer.valid_until,
            );
        }
    }
}
