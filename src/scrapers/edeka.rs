use anyhow::{Context, Result, anyhow, bail};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;

use crate::models::{Branch, Market, Offer};
use crate::scrapers::util::{self, curl_get, curl_redirect_url};

// EDEKA über edeka.de (regionale Angebote, Markt über PLZ wie bei Rewe).
//
// Marktsuche (öffentliches JSON):
//   GET https://www.edeka.de/api/marketsearch/markets?searchstring=<PLZ>
// Die Antwort trägt noch die alte Markt-URL (/eh/<region>/<slug>/index.jsp);
// deren 308-Redirect zeigt auf die neue Seite /maerkte/<id>/ — diese ID
// braucht die Angebotsseite. (Die alte /api/offers-Schnittstelle ist tot
// und antwortet nur noch mit einem Scherz-JSON.)
//
// Angebote sind server-seitig gerendert:
//   GET https://www.edeka.de/maerkte/<id>/angebote/
// Ein <article> pro Angebot; Titel im Anker a[href^="#angebot-"] (mit
// sr-only-Präfix "Angebot:", Überschrift h2/h3/h4 je nach Kachelgröße),
// Preis maschinenlesbar in einem sr-only-Div ("Festpreis von 3.99 €" bzw.
// "App-Preis von 0.88 €"), Beschreibung in p.line-clamp-2, Gültigkeit als
// Seitentext "Gültig ab 13.07.2026" / "gültig bis Samstag, den 18.07.2026".
//
// Akamai-Bot-Schutz wie bei Netto/ALDI Süd -> util::curl_get statt reqwest.

const BASE: &str = "https://www.edeka.de";

const MARKET_SEARCH_HEADERS: &[(&str, &str)] = &[
    ("Accept", "application/json, text/plain, */*"),
    ("Referer", "https://www.edeka.de/marktsuche.jsp"),
    ("Sec-Fetch-Site", "same-origin"),
    ("Sec-Fetch-Mode", "cors"),
    ("Sec-Fetch-Dest", "empty"),
];

const MARKET_PAGE_HEADERS: &[(&str, &str)] = &[
    ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    ("Sec-Fetch-Site", "none"),
    ("Sec-Fetch-Mode", "navigate"),
    ("Sec-Fetch-Dest", "document"),
    ("Sec-Fetch-User", "?1"),
    ("Upgrade-Insecure-Requests", "1"),
];

pub fn find_market(zip: &str) -> Result<Market> {
    let raw = market_search(zip)?;
    let drafts = parse_branch_drafts(&raw);
    if drafts.is_empty() {
        bail!("Kein EDEKA-Markt für PLZ {zip} gefunden");
    }

    // Nicht blind den ersten Treffer nehmen: Die Marktsuche stellt auch
    // Märkte ohne Marktseite nach vorn (siehe [`resolve`]) — für 50667 steht
    // genau so einer auf Platz 1. Der erste Markt, dessen Scrape-ID sich
    // auflösen lässt, ist der erste, mit dem sich überhaupt etwas anfangen
    // lässt.
    let mut last_err = None;
    for draft in drafts {
        match resolve(draft) {
            Ok(branch) => return Ok(branch.as_market()),
            Err(e) => {
                // Die Meldung nennt den Markt bereits.
                eprintln!("WARNUNG [EDEKA] Markt übersprungen: {e:#}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("Kein EDEKA-Markt für PLZ {zip} gefunden")))
        .with_context(|| format!("Kein auflösbarer EDEKA-Markt für PLZ {zip}"))
}

/// Alle Filialen der Marktsuche, für das Verzeichnis.
///
/// Teuerste Kette im Verzeichnis: Jede Filiale kostet zusätzlich einen
/// Redirect-Request, siehe [`resolve`]. Für ein Stadtgebiet sind das gut ein
/// Dutzend — vertretbar; bundesweit wären es zehntausende, und genau deshalb
/// wird EDEKA nur gebietsweise auf Anforderung geholt.
pub fn find_branches(zip: &str) -> Result<Vec<Branch>> {
    let raw = market_search(zip)?;
    let mut branches = Vec::new();
    for draft in parse_branch_drafts(&raw) {
        match resolve(draft) {
            Ok(branch) => branches.push(branch),
            // Eine Filiale, deren ID sich nicht auflösen lässt, ist kein
            // Grund, die anderen zwölf fallen zu lassen.
            Err(e) => eprintln!("WARNUNG [EDEKA] Filiale übersprungen: {e:#}"),
        }
    }
    Ok(branches)
}

fn market_search(zip: &str) -> Result<serde_json::Value> {
    let url = format!("{BASE}/api/marketsearch/markets?searchstring={zip}");
    let body = curl_get(&url, MARKET_SEARCH_HEADERS)
        .with_context(|| util::ctx("EDEKA", "Markt-Lookup", &url))?;
    serde_json::from_str(&body)
        .with_context(|| util::ctx("EDEKA", "Markt-Lookup JSON parsen", &url))
}

/// Eine Filiale der Marktsuche, deren **Scrape-ID noch fehlt**.
///
/// EDEKA nennt in der Marktsuche eine andere ID (`id`, z. B. 10004808) als
/// der Angebots-Pfad (`/maerkte/<id>/`), und letztere steht nur hinter dem
/// Redirect der Markt-URL. Ein Verzeichniseintrag ohne Scrape-ID wäre
/// nutzlos, deshalb sind die beiden Schritte getrennt: [`parse_branch_drafts`]
/// liest die Adressdaten ohne Netz, [`resolve`] holt die ID nach.
///
/// `url` ist optional, weil die Marktsuche das Feld nicht bei jedem Markt
/// füllt — siehe [`parse_branch_drafts`].
pub struct BranchDraft {
    pub name: String,
    pub street: Option<String>,
    pub plz: Option<String>,
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub url: Option<String>,
}

/// Adressdaten aller Märkte der Antwort — **auch derer ohne `url`**.
///
/// Ein Teil der Märkte kommt ohne `url`-Feld, weil es zu ihnen auf edeka.de
/// keine Marktseite gibt (gemessen 2026-07-25: 50667 Köln einer von zehn,
/// 94032 Passau einer von zehn, 01219 Dresden keiner). Diese Einträge hier
/// stillschweigend wegzuwerfen hat zwei Folgen: Das Verzeichnis verliert die
/// Filiale ohne jede Meldung, und [`find_market`] verschiebt seine Auswahl
/// unbemerkt auf den nächsten Markt — in Köln steht genau so ein Eintrag an
/// erster Stelle. Sie kommen deshalb mit `url: None` durch; erst [`resolve`]
/// lehnt sie mit Begründung ab, und die Aufrufer melden das.
pub fn parse_branch_drafts(raw: &serde_json::Value) -> Vec<BranchDraft> {
    let Some(markets) = raw.get("markets").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    markets
        .iter()
        .map(|market| {
            let url = market.get("url").and_then(|v| v.as_str()).map(str::to_string);
            let text = |pointer: &str| {
                market
                    .pointer(pointer)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            // Koordinaten kommen als Strings ("51.00879").
            let coord = |pointer: &str| {
                market.pointer(pointer).and_then(|v| v.as_str()).and_then(|s| s.parse().ok())
            };
            BranchDraft {
                name: text("/name").unwrap_or_else(|| "EDEKA".to_string()),
                street: text("/contact/address/street"),
                plz: text("/contact/address/city/zipCode"),
                city: text("/contact/address/city/name"),
                lat: coord("/coordinates/lat"),
                lon: coord("/coordinates/lon"),
                url,
            }
        })
        .collect()
}

/// Scrape-ID nachschlagen und den Entwurf zur Verzeichniszeile machen.
pub fn resolve(draft: BranchDraft) -> Result<Branch> {
    // Ohne Markt-URL gibt es keinen Weg zur Scrape-ID: Weder
    // /maerkte/<marktsuche-id>/ noch die aus Region, Name und Straße
    // zusammengesetzte /eh/-URL existieren (beide 404, geprüft 2026-07-25 an
    // „EDEKA Heyßel“, ID 10008482). `branches.market_id` ist der
    // Scrape-Schlüssel und Primärschlüssel — eine Zeile mit geratener ID wäre
    // schlimmer als keine. Also abbrechen, aber hörbar: Die Aufrufer melden
    // den übersprungenen Markt, statt ihn wie bisher stumm zu verlieren.
    let url = draft.url.as_deref().with_context(|| {
        format!(
            "EDEKA-Markt „{}“ ohne Markt-URL in der Marktsuche — zu diesem \
             Markt gibt es auf edeka.de keine Marktseite und damit keine \
             Scrape-ID",
            draft.name
        )
    })?;

    // Neue URLs (https://www.edeka.de/maerkte/<id>/) tragen die ID schon —
    // dort gibt es keinen Redirect mehr, den man auflösen könnte.
    let id = match market_id_from_url(url) {
        Some(id) => id.to_string(),
        None => {
            // Alte URL -> 308-Redirect -> https://www.edeka.de/maerkte/<id>/
            let target = curl_redirect_url(url, MARKET_PAGE_HEADERS)
                .with_context(|| util::ctx("EDEKA", "Markt-Redirect auflösen", url))?;
            market_id_from_url(&target)
                .with_context(|| format!("Unerwartetes Redirect-Ziel für EDEKA-Markt: {target}"))?
                .to_string()
        }
    };
    Ok(Branch::new(id, "EDEKA", draft.name, "edeka-marktsuche")
        .with_address(draft.street, draft.plz, draft.city)
        .with_geo(draft.lat, draft.lon))
}

/// Numerische Markt-ID aus einer https://www.edeka.de/maerkte/<id>/-URL.
fn market_id_from_url(url: &str) -> Option<&str> {
    url.split("/maerkte/")
        .nth(1)
        .map(|rest| rest.trim_matches('/'))
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    let url = format!("{BASE}/maerkte/{}/angebote/", market.id);
    let html = curl_get(
        &url,
        &[
            ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
            ("Sec-Fetch-Site", "none"),
            ("Sec-Fetch-Mode", "navigate"),
            ("Sec-Fetch-Dest", "document"),
            ("Sec-Fetch-User", "?1"),
            ("Upgrade-Insecure-Requests", "1"),
        ],
    )
    .with_context(|| util::ctx("EDEKA", "Angebote laden", &url))?;

    let offers = parse_offers(&html, &market.id)
        .with_context(|| util::ctx("EDEKA", "Angebote parsen", &url))?;
    if offers.is_empty() {
        // Zwei sehr verschiedene Fälle, die bisher dieselbe (irreführende)
        // Meldung bekamen. Märkte ohne Angebotsseite liefern unter
        // /angebote/ keine 404, sondern mit HTTP 200 die Marktseite — die
        // trägt keinen einzigen "#angebot-"-Anker. Fehlt der Anker komplett,
        // ist es dieser Markt und nicht das Markup, das sich geändert hat.
        // (Geprüft 2026-07-25: 070992 und 070538 in Köln.)
        if !html.contains("#angebot-") {
            bail!(
                "[EDEKA] Markt {} ({}) veröffentlicht auf edeka.de keine Angebote — \
                 {url} liefert die Marktseite statt einer Angebotsliste",
                market.name,
                market.id
            );
        }
        bail!("[EDEKA] Keine Angebote gefunden ({url}) — Seitenstruktur hat sich möglicherweise geändert");
    }
    Ok(offers)
}

// NULL-Preise sind hier echt: "Tagespreis"-Kacheln und reine
// PAYBACK-Extra-Punkte-Kacheln tragen weder in der Kachel noch im
// zugehörigen Dialog einen Preis (~20-25 Angebote pro Woche, verifiziert
// 2026-07 am Roh-HTML). Sie werden bewusst mit price = None übernommen.
pub fn parse_offers(html: &str, market_id: &str) -> Result<Vec<Offer>> {
    let doc = Html::parse_document(html);
    let sel_article = sel("article");
    // Highlight-Kacheln nutzen h2, normale Kacheln h4 (vereinzelt h3);
    // der Anker "#angebot-<uuid>" unterscheidet Angebote von anderen <article>s.
    let sel_title = sel(r##"a[href^="#angebot-"]"##);
    let sel_desc = sel("p.line-clamp-2");
    let sel_sronly = sel("div.sr-only");
    let sel_img = sel("img");

    // Seitenweite Gültigkeit: "Gültig ab 13.07.2026" ... "gültig bis ..., den 18.07.2026"
    let page_text: String = doc.root_element().text().collect();
    let valid_from = find_date_after(&page_text, "Gültig ab ");
    let valid_until = find_date_after(&page_text, "gültig bis ");

    let mut offers = Vec::new();
    let mut seen = HashSet::new();

    for article in doc.select(&sel_article) {
        // "Angebot: Kulturheidelbeeren" -> "Kulturheidelbeeren"
        let Some(title) = text_of(article, &sel_title)
            .map(|t| t.trim_start_matches("Angebot:").trim().to_string())
            .filter(|t| !t.is_empty())
        else {
            continue;
        };

        // "Festpreis von 3.99 €" / "App-Preis von 0.88 €"
        let price = article
            .select(&sel_sronly)
            .map(|e| e.text().collect::<String>())
            .find(|t| t.contains("reis von"))
            .and_then(|t| parse_price(&t));

        let subtitle = text_of(article, &sel_desc);

        let images = article
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
            overline: None,
            price,
            regular_price: None,
            category: None,
            nutri_score: None,
            valid_from: valid_from.clone(),
            valid_until: valid_until.clone(),
            images,
            biozid: false,
            flyer_page: None,
        });
    }

    Ok(offers)
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("statischer CSS-Selektor")
}

fn text_of(el: ElementRef, selector: &Selector) -> Option<String> {
    let text: String = el.select(selector).next()?.text().collect();
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { None } else { Some(text) }
}

// "Festpreis von 3.99 €" -> 3.99 (erste als Zahl parsbare Token)
fn parse_price(s: &str) -> Option<f64> {
    s.split_whitespace()
        .find_map(|tok| tok.replace(',', ".").parse::<f64>().ok())
}

// Erstes "dd.mm.yyyy" nach dem Marker -> "yyyy-mm-dd"
fn find_date_after(text: &str, marker: &str) -> Option<String> {
    let idx = text.find(marker)? + marker.len();
    let window = &text[idx..text.len().min(idx + 60)];
    let mut nums = window
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty());
    loop {
        let d = nums.next()?;
        let m = nums.next()?;
        let y = nums.next()?;
        if d.len() <= 2 && m.len() <= 2 && y.len() == 4 {
            let (day, month, year): (u32, u32, u32) =
                (d.parse().ok()?, m.parse().ok()?, y.parse().ok()?);
            if (1..=31).contains(&day) && (1..=12).contains(&month) {
                return Some(format!("{year}-{month:02}-{day:02}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_parsing() {
        assert_eq!(parse_price("Festpreis von 3.99 €"), Some(3.99));
        assert_eq!(parse_price("App-Preis von 0.88 €"), Some(0.88));
        assert_eq!(parse_price("kein Preis"), None);
    }

    #[test]
    fn date_after_marker() {
        assert_eq!(
            find_date_after("... Gültig ab 13.07.2026 ...", "Gültig ab "),
            Some("2026-07-13".to_string())
        );
        assert_eq!(
            find_date_after("gültig bis Samstag, den 18.07.2026, KW 29", "gültig bis "),
            Some("2026-07-18".to_string())
        );
        assert_eq!(find_date_after("nichts", "Gültig ab "), None);
    }

    /// Live-Test gegen edeka.de: cargo test edeka -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen edeka.de"]
    fn live_fetch_offers() {
        let market = find_market("01219").expect("Markt");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(5) {
            println!(
                "- {} | {:?} | {:?} € | {:?} bis {:?}",
                o.title, o.subtitle, o.price, o.valid_from, o.valid_until
            );
        }
        assert!(offers.len() >= 80, "Erwartet >= 80 Angebote, war {}", offers.len());
    }
}
