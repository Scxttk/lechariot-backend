use anyhow::{Context, Result, anyhow, bail};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;

use crate::models::{Branch, Market, Offer};
use crate::scrapers::util::{self, curl_get, curl_redirect_url};

// EDEKA über edeka.de (regionale Angebote, Markt über PLZ wie bei Rewe).
//
// Marktsuche (öffentliches JSON):
//   GET https://www.edeka.de/api/marketsearch/markets?searchstring=<PLZ>&size=<N>
//
// ACHTUNG, Seitengröße: Die Antwort MELDET `limit` und `offset`, nimmt aber
// `size` (und `page`) entgegen. Wer `limit=50` schickt, bekommt weiter 10
// Märkte und hält die beiden Felder für kaputt — genau daran hing der
// Backlog-Punkt „EDEKA-Marktsuche deckelt bei 10 Treffern". Gemessen am
// 2026-07-25 für 50667 Köln: ohne Parameter 10 von 17, mit `limit=50`
// ebenfalls 10, mit `size=50` alle 17.
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

/// Seitengröße der Marktsuche. Großzügig, aber nicht unbegrenzt: Der größte
/// gemessene Wert war 17 (Köln), und eine Suche nach einem Stadtnamen darf
/// nicht zur Volltextabfrage des halben Landes werden.
const MARKET_SEARCH_SIZE: usize = 100;

/// Die URL der Marktsuche — als eigene Funktion, damit ein Test die
/// Seitengröße festnagelt, ohne ans Netz zu gehen.
pub fn market_search_url(zip: &str) -> String {
    format!("{BASE}/api/marketsearch/markets?searchstring={zip}&size={MARKET_SEARCH_SIZE}")
}

fn market_search(zip: &str) -> Result<serde_json::Value> {
    let url = market_search_url(zip);
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
            //
            // Der Name gehört in die Meldung, nicht nur der Slug: Die
            // Zusammenfassung des Laufs zeigt seit #36 jede WARNUNG-Zeile an,
            // und „EDEKA Böse, Ahlbeck" ist das, wonach jemand sucht, der
            // seine Filiale vermisst — nicht `/eh/mv/edeka-boese-.../`.
            let target = curl_redirect_url(url, MARKET_PAGE_HEADERS).with_context(|| {
                format!(
                    "{} — Filiale „{}“{}",
                    util::ctx("EDEKA", "Markt-Redirect auflösen", url),
                    draft.name,
                    draft
                        .city
                        .as_deref()
                        .map(|c| format!(" in {c}"))
                        .unwrap_or_default()
                )
            })?;
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

pub fn offers_url(market_id: &str) -> String {
    format!("{BASE}/maerkte/{market_id}/angebote/")
}

/// Die Angebotsseite als Roh-HTML — **derselbe Weg, den `fetch_offers` geht**.
///
/// Eigene Funktion, damit eine Probe das Markup messen kann, ohne den
/// Header-Satz nachzubauen: Ein nachgebauter Aufruf misst die Probe, nicht die
/// Seite (siehe `vorschau_probe`, dessen erster Anlauf genau daran scheiterte).
pub fn fetch_offers_html(market_id: &str) -> Result<String> {
    let url = offers_url(market_id);
    curl_get(&url, OFFERS_HEADERS).with_context(|| util::ctx("EDEKA", "Angebote laden", &url))
}

const OFFERS_HEADERS: &[(&str, &str)] = &[
    ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    ("Sec-Fetch-Site", "none"),
    ("Sec-Fetch-Mode", "navigate"),
    ("Sec-Fetch-Dest", "document"),
    ("Sec-Fetch-User", "?1"),
    ("Upgrade-Insecure-Requests", "1"),
];

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    let url = offers_url(&market.id);
    let html = fetch_offers_html(&market.id)?;

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

    // Kacheln ohne Preis gibt es echt (siehe unten), aber **kein einziger**
    // Preis auf einer ganzen Angebotsseite ist keine Woche ohne Preise,
    // sondern ein Parser, der am Markup vorbeigreift. Genau dieser Zustand
    // hielt die Nightly vom 05.08. bis zum 09.08. rot, und die Meldung dazu
    // fiel erst ganz am Ende beim Ketten-Wächter — hier steht sie beim Markt,
    // der sie ausgelöst hat.
    if offers.iter().all(|o| o.price.is_none()) {
        eprintln!(
            "WARNUNG [EDEKA] Markt {} ({}): {} Angebote, aber keines mit Preis — \
             das Preis-Markup auf {url} hat sich vermutlich geändert.",
            market.name,
            market.id,
            offers.len()
        );
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
    // Der Preis steht in der Vorlesehilfe der Kachel, weil die sichtbare Zahl
    // `aria-hidden` ist. Bis Anfang August war das ein `<div class="sr-only">`,
    // seither ein `<span>` (gemessen 2026-08-09) — deshalb hier ohne Tag: der
    // Selektor beschreibt die Rolle, nicht das Element, und trägt beide
    // Fassungen.
    let sel_sronly = sel(".sr-only");
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

        // "Festpreis von 1.49€" / "App Preis von 5.99€" / "Rabattierter Preis
        // von 6.49€ (Insgesamt -35% Rabatt)".
        //
        // Trägt eine Kachel zwei Preise, gewinnt der erste — das ist der
        // App-Preis, und es bleibt damit bei der Auslegung von vor dem
        // Markup-Wechsel. Ob ein Preis, den nur die EDEKA-App hergibt, in der
        // Zeile stehen sollte, ist eine offene Frage und steht als solche im
        // Backlog; sie hier nebenbei anders zu beantworten hieße, den
        // Preisverlauf still zu verbiegen.
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

// "Festpreis von 3.99 €" -> 3.99
//
// Die Zahl wird **hinter dem Marker** gelesen, nicht als erstes zahlartiges
// Token der Zeile. Der Unterschied ist neu und wichtig: Seit Anfang August
// heißt die zweite Fassung „Rabattierter Preis von 1.49€ (Insgesamt -45%
// Rabatt)" — eine Zeile, die zwei Zahlen trägt. Und das Euro-Zeichen klebt
// jetzt an der Zahl ("1.49€"), weshalb ein Token-Parser hier gar nichts mehr
// findet: genau das hat die Nightly ab 05.08. auf „0 Angebote hochladbar"
// gesetzt.
fn parse_price(s: &str) -> Option<f64> {
    let rest = s.split_once("reis von").map(|(_, rest)| rest)?;
    let zahl: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    zahl.trim_end_matches(['.', ',']).replace(',', ".").parse().ok()
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

    /// Die Marktsuche nimmt `size`, nicht `limit` — und meldet in der Antwort
    /// trotzdem `limit` und `offset`. Genau diese Verwechslung hat sieben
    /// Kölner EDEKA-Märkte unsichtbar gemacht: gemessen am 2026-07-25 liefert
    /// `limit=50` weiterhin 10 von 17, `size=50` alle 17.
    #[test]
    fn market_search_asks_for_more_than_one_page() {
        let url = market_search_url("50667");
        assert!(url.contains("searchstring=50667"), "{url}");
        assert!(url.contains("size="), "ohne size deckelt die API bei 10: {url}");
        assert!(!url.contains("limit="), "limit wird ignoriert und täuscht nur: {url}");
        assert!(MARKET_SEARCH_SIZE >= 20, "17 Märkte in Köln sind der bisher größte Fall");
    }

    #[test]
    fn price_parsing() {
        // Alte Schreibweise (bis Anfang August): Leerzeichen vor dem Euro.
        assert_eq!(parse_price("Festpreis von 3.99 €"), Some(3.99));
        assert_eq!(parse_price("App-Preis von 0.88 €"), Some(0.88));
        assert_eq!(parse_price("kein Preis"), None);
    }

    /// Die drei Fassungen, die edeka.de seit dem Markup-Wechsel schreibt
    /// (gemessen 2026-08-09 an 021868/421696/421347). Das Euro-Zeichen klebt
    /// an der Zahl, der Bindestrich im App-Preis ist weg, und die
    /// rabattierte Zeile trägt hinter dem Preis eine zweite Zahl.
    #[test]
    fn price_parsing_neues_markup() {
        assert_eq!(parse_price("Festpreis von 1.49€"), Some(1.49));
        assert_eq!(parse_price("App Preis von 5.99€"), Some(5.99));
        assert_eq!(
            parse_price("Rabattierter Preis von 6.49€ (Insgesamt -35% Rabatt)"),
            Some(6.49),
            "der Rabatt hinter dem Preis darf die Zahl nicht kapern"
        );
        // Komma-Schreibweise, falls die Seite sie je zurückbringt.
        assert_eq!(parse_price("Festpreis von 1,49 €"), Some(1.49));
        // Ohne Marker kein Preis: „Angebot:" und Mengenangaben tragen Zahlen,
        // sind aber keine Preise.
        assert_eq!(parse_price("Angebot: 3 Stück"), None);
        assert_eq!(parse_price("Insgesamt -45% Rabatt"), None);
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
