use anyhow::{Context, Result, bail};

use crate::scrapers::util::{self, curl_get};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;

use crate::models::{Branch, Market, Offer};

// Netto Marken-Discount über netto-online.de (Intershop, Akamai-geschützt).
//
// Filialsuche (öffentliches JSON, kein Token nötig):
//   GET /INTERSHOP/web/WFS/Plus-NettoDE-Site/de_DE/-/EUR/
//       ViewMMPStoreFinder-GetStoreByPostcode?postalcode=<PLZ>&searchradius=25
//
// Filial-Angebote sind server-seitig gerendert; die Filiale wird allein über
// das Cookie `netto_user_stores_id=<store_id>` gebunden (verifiziert 2026-07):
//   GET /filialangebote/1   Wochenangebote
//   GET /filialangebote/2   Wochenendangebote
//   GET /filialangebote/4   Freitag ist Netto-Tag
//   GET /filialangebote/5   Samstagskracher
// (Unbekannte Seiten-IDs liefern Seite 1 — Dedup fängt das ab.)
//
// Achtung Akamai: Der Bot-Schutz fingerprintet den TLS-Stack — reqwest/rustls
// wird konsequent mit HTTP 403 geblockt, curl mit vollem Browser-Header-Satz
// (User-Agent + Accept + Sec-Fetch-*) kommt durch (verifiziert 2026-07).
// Deshalb laufen die Requests über util::curl_get (System-curl).

const BASE: &str = "https://www.netto-online.de";
const STORE_FINDER_PATH: &str =
    "/INTERSHOP/web/WFS/Plus-NettoDE-Site/de_DE/-/EUR/ViewMMPStoreFinder-GetStoreByPostcode";
const OFFER_PAGES: &[u32] = &[1, 2, 4, 5];

/// Header-Satz der Filialsuche — ohne den vollständigen XHR-Satz antwortet
/// Akamai mit 403.
const STORE_FINDER_HEADERS: &[(&str, &str)] = &[
    ("Accept", "application/json, text/javascript, */*; q=0.01"),
    ("X-Requested-With", "XMLHttpRequest"),
    ("Referer", "https://www.netto-online.de/filialangebote/"),
    ("Sec-Fetch-Site", "same-origin"),
    ("Sec-Fetch-Mode", "cors"),
    ("Sec-Fetch-Dest", "empty"),
];

pub fn find_market(zip: &str) -> Result<Market> {
    let url = format!("{BASE}{STORE_FINDER_PATH}?postalcode={zip}&searchradius=25");
    let body = curl_get(&url, STORE_FINDER_HEADERS)
        .with_context(|| util::ctx("Netto", "Markt-Lookup", &url))?;

    let raw: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| util::ctx("Netto", "Markt-Lookup JSON parsen", &url))?;

    // Wie bisher der erste Treffer; neu ist nur, dass Adresse und
    // Koordinaten nicht mehr verlorengehen. Für 01219 liefert der Finder
    // vier Filialen — bisher sah die App davon genau eine.
    let branch = parse_branches(&raw)?
        .into_iter()
        .next()
        .with_context(|| format!("Keine Netto-Filiale für PLZ {zip} gefunden"))?;
    Ok(branch.as_market())
}

/// Alle Filialen im Umkreis der PLZ, für das Verzeichnis.
pub fn find_branches(zip: &str, radius_km: u32) -> Result<Vec<Branch>> {
    let url = format!("{BASE}{STORE_FINDER_PATH}?postalcode={zip}&searchradius={radius_km}");
    let body = curl_get(&url, STORE_FINDER_HEADERS)
        .with_context(|| util::ctx("Netto", "Filialverzeichnis", &url))?;
    let raw: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| util::ctx("Netto", "Filialverzeichnis JSON parsen", &url))?;
    parse_branches(&raw)
}

/// Filialsuche-Antwort als Verzeichniszeilen.
///
/// Geschlossene Filialen (`is_closed`) fallen raus — sie stehen in der
/// Antwort, haben aber keine Angebote und wären im Onboarding eine
/// Einladung, den falschen Markt zu wählen.
pub fn parse_branches(raw: &serde_json::Value) -> Result<Vec<Branch>> {
    let stores = raw.as_array().context("Netto-Filialsuche ist kein JSON-Array")?;

    Ok(stores
        .iter()
        .filter(|store| store.get("is_closed").and_then(|v| v.as_bool()) != Some(true))
        .filter_map(|store| {
            let id = store.get("store_id").and_then(|v| v.as_str())?;
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
            let city = text("city");
            // store_name ist bei jeder Filiale "Netto Marken-Discount" — erst
            // der Ort macht daraus einen unterscheidbaren Namen.
            let name = match (text("store_name"), &city) {
                (Some(n), Some(c)) => format!("{n} {c}"),
                (Some(n), None) => n,
                _ => "Netto Marken-Discount".to_string(),
            };
            Some(
                Branch::new(id, "Netto", name, "netto-storefinder")
                    .with_address(text("street"), text("post_code"), city)
                    .with_geo(coord("coord_latitude"), coord("coord_longitude")),
            )
        })
        .collect())
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    let cookie = format!("netto_user_stores_id={}", market.id);

    let mut offers = Vec::new();
    let mut seen = HashSet::new();
    let mut abgewiesen: Vec<String> = Vec::new();

    for page in OFFER_PAGES {
        let url = format!("{BASE}/filialangebote/{page}");
        let html = match curl_get(
            &url,
            &[
                ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
                ("Cookie", cookie.as_str()),
                ("Sec-Fetch-Site", "none"),
                ("Sec-Fetch-Mode", "navigate"),
                ("Sec-Fetch-Dest", "document"),
                ("Sec-Fetch-User", "?1"),
            ],
        ) {
            Ok(h) => h,
            // Einzelne Kategorieseite nicht erreichbar -> überspringen statt
            // abbrechen. Der Grund wird aber mitgenommen: Bis 2026-08-01 fiel
            // er hier weg (`Err(_)`), und übrig blieb unten die Meldung über
            // die Seitenstruktur — eine Diagnose, die mit dem Ausfall vom
            // 01.08. (alle vier Seiten abgewiesen, Seiten völlig unverändert)
            // nichts zu tun hatte und die Suche in die falsche Richtung schickte.
            Err(e) => {
                abgewiesen.push(format!("Seite {page}: {e:#}"));
                continue;
            }
        };
        parse_page(&html, &market.id, &mut offers, &mut seen);
    }

    if offers.is_empty() {
        bail!("{}", kein_angebot_grund(&abgewiesen));
    }

    // Teilausfall: Angebote sind da, aber nicht von allen Seiten. Das ist
    // stiller Verlust — die Zahl stimmt dann nur scheinbar.
    if !abgewiesen.is_empty() {
        eprintln!(
            "WARNUNG [Netto] {} von {} Angebotsseiten abgewiesen, {} Angebote aus dem Rest — {}",
            abgewiesen.len(),
            OFFER_PAGES.len(),
            offers.len(),
            abgewiesen.join("; ")
        );
    }
    Ok(offers)
}

pub fn parse_page(html: &str, market_id: &str, offers: &mut Vec<Offer>, seen: &mut HashSet<String>) {
    let doc = Html::parse_document(html);
    let sel_period = sel("div.offer__period");
    let sel_tile = sel("div.js-store-product-tile");
    let sel_title = sel(".tc-product-name");
    let sel_bundle = sel(".product-property__bundle-text");
    let sel_desc = sel(".product-property__description-short");
    let sel_base = sel(".product-property__base-price");
    let sel_price = sel(".product__current-price");
    let sel_strike = sel(".product__strike-price");
    let sel_img = sel("img.tc-product-image");

    // "Wochenangebote gültig von Montag, 13.07.26 - Samstag, 18.07.26"
    let period = doc
        .select(&sel_period)
        .next()
        .map(|e| e.text().collect::<String>());
    let (category, valid_from, valid_until) = match period.as_deref() {
        Some(p) => {
            let category = p.split(" gültig").next().map(|s| s.trim().to_string());
            let (f, u) = parse_period_dates(p).map_or((None, None), |(f, u)| (Some(f), Some(u)));
            (category, f, u)
        }
        None => (None, None, None),
    };

    for tile in doc.select(&sel_tile) {
        let Some(title) = text_of(tile, &sel_title) else { continue };

        let subtitle = text_of(tile, &sel_bundle);
        let overline = match (text_of(tile, &sel_desc), text_of(tile, &sel_base)) {
            (Some(d), Some(b)) => Some(format!("{d} ({b})")),
            (d, b) => d.or(b),
        };

        let price = text_of(tile, &sel_price).and_then(|s| parse_price(&s));
        // "UVP 3.99" -> 3.99
        let regular_price = text_of(tile, &sel_strike).and_then(|s| parse_price(&s));

        let images = tile
            .select(&sel_img)
            .next()
            .and_then(|img| img.value().attr("src"))
            .map(|s| vec![s.to_string()])
            .unwrap_or_default();

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

/// Warum kein einziges Angebot herauskam — als reine Funktion, damit ein Test
/// sie ohne Netz fassen kann (dieselbe Bauart wie `util::redirect_outcome`).
///
/// Die Unterscheidung ist der ganze Punkt: „ich kam nicht ran" ist ein Ausfall
/// der Verbindung, „ich kam ran und fand nichts" einer der Seitenstruktur. Bis
/// 2026-08-01 sahen beide gleich aus, weil die abgewiesenen Seiten oben still
/// übersprungen wurden — die Meldung sprach immer von der Struktur. Im Ausfall
/// vom 01.08. (Lauf 30713329472: alle vier Seiten wiesen alle drei Versuche
/// ab, die Seiten selbst völlig unverändert) hat genau das die Suche einen
/// Abend lang in die falsche Richtung geschickt.
fn kein_angebot_grund(abgewiesen: &[String]) -> String {
    match abgewiesen.len() {
        0 => format!(
            "[Netto] Keine Angebote gefunden ({BASE}/filialangebote/…) — \
             Seitenstruktur hat sich möglicherweise geändert"
        ),
        n => format!(
            "[Netto] Keine Angebote gefunden — {n} von {} Seiten abgewiesen: {}",
            OFFER_PAGES.len(),
            abgewiesen.join("; ")
        ),
    }
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("statischer CSS-Selektor")
}

fn text_of(el: ElementRef, selector: &Selector) -> Option<String> {
    let text: String = el.select(selector).next()?.text().collect();
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { None } else { Some(text) }
}

// Preistexte wie "1. 79 *" (verschachtelte Spans) oder "UVP 3.99".
fn parse_price(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    cleaned.parse::<f64>().ok()
}

// "... von Montag, 13.07.26 - Samstag, 18.07.26" -> ("2026-07-13", "2026-07-18")
fn parse_period_dates(s: &str) -> Option<(String, String)> {
    let mut dates = Vec::new();
    let mut nums = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty());
    while let (Some(d), Some(m), Some(y)) = (nums.next(), nums.next(), nums.next()) {
        if d.len() > 2 || m.len() > 2 || (y.len() != 2 && y.len() != 4) {
            continue;
        }
        let (day, month, year): (u32, u32, u32) =
            (d.parse().ok()?, m.parse().ok()?, y.parse().ok()?);
        if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
            continue;
        }
        let year = if year < 100 { 2000 + year } else { year };
        dates.push(format!("{year}-{month:02}-{day:02}"));
    }
    match dates.as_slice() {
        [single] => Some((single.clone(), single.clone())),
        [from, .., until] => Some((from.clone(), until.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_parsing() {
        assert_eq!(
            parse_period_dates("Wochenangebote gültig von Montag, 13.07.26 - Samstag, 18.07.26"),
            Some(("2026-07-13".to_string(), "2026-07-18".to_string()))
        );
        assert_eq!(parse_period_dates("Wochenangebote"), None);
    }

    /// Der Ausfall vom 01.08.2026 in einem Test: Sieben Filialen meldeten
    /// „Seitenstruktur hat sich möglicherweise geändert", während die
    /// Seitenstruktur unverändert war und schlicht jede Anfrage abgewiesen
    /// wurde. Die Meldung muss sagen, welcher der beiden Fälle vorliegt.
    #[test]
    fn grund_unterscheidet_abweisung_von_seitenstruktur() {
        // Alle Seiten kamen an, nur ohne Kacheln — das ist die Struktur.
        let struktur = kein_angebot_grund(&[]);
        assert!(struktur.contains("Seitenstruktur"), "{struktur}");

        // Abgewiesene Seiten: Der HTTP-Status gehört in die Meldung, und der
        // Verdacht gegen den Parser hat dort nichts verloren.
        let abgewiesen = kein_angebot_grund(&[
            "Seite 1: https://…/1 in 3 Versuchen abgewiesen (HTTP 403/403/403)".to_string(),
            "Seite 2: https://…/2 in 3 Versuchen abgewiesen (HTTP 403/403/403)".to_string(),
        ]);
        assert!(abgewiesen.contains("403"), "{abgewiesen}");
        assert!(abgewiesen.contains("2 von 4"), "{abgewiesen}");
        assert!(!abgewiesen.contains("Seitenstruktur"), "{abgewiesen}");
    }

    #[test]
    fn price_parsing() {
        assert_eq!(parse_price("1. 79 *"), Some(1.79));
        assert_eq!(parse_price("UVP 3.99"), Some(3.99));
        assert_eq!(parse_price("—"), None);
    }

    /// Live-Test gegen netto-online.de: cargo test netto -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen netto-online.de"]
    fn live_fetch_offers() {
        let market = find_market("01219").expect("Markt");
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
