use anyhow::{Context, Result, bail};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;

use crate::models::{Market, Offer};
use crate::scrapers::{store_finder, util};

// NORMA über norma-online.de — server-seitig gerendertes HTML, kein Akamai,
// kein Cookie, kein API-Key (verifiziert 2026-07-31).
//
// Zwei Ebenen:
//   /de/angebote/                              Index, verlinkt alle Themenseiten
//   /de/angebote/ab-montag,-03.08.26/foo-t-123/ eine Themenseite mit den Kacheln
//
// NORMA fährt **drei Angebotszyklen pro Woche** (Montag, Mittwoch, Freitag),
// und der Index zeigt immer die laufenden *und* die kommenden Termine. Am
// 2026-07-31 waren das vier Termine mit zusammen 22 Themenseiten und 216
// Angeboten, 157 davon am Montagstermin. Ein Lauf kostet also rund 23
// Requests — der Index plus eine Seite je Thema.
//
// Der Angebotskatalog ist bundesweit; `find_market` liefert deshalb einen
// synthetischen National-Markt (wie aldi_nord.rs), und `Store::NORMA` steht in
// `stores_nationally()`.
//
// Zwei Eigenheiten, die beim Parsen Zeit kosten würden:
//
// 1. **Der Preis steht doppelt im Markup**, und nur eine Fassung ist
//    maschinenlesbar. Sichtbar ist „–,99" (Gedankenstrich statt führender
//    Null, Sternchen als Fußnote); daneben steht `aria-label="0,99 Euro"`.
//    Wir lesen das aria-label — dieselbe Steilvorlage wie EDEKAs sr-only-Div.
//    Der **Streichpreis** hat kein aria-label, und NORMA schreibt ihn in
//    derselben Gedankenstrich-Schreibweise („UVP –,99"). `parse_price` muss
//    sie deshalb doch beherrschen, siehe dort.
// 2. **Marke und Produkt stehen getrennt** (`strong.supplier` = „Sheba",
//    `h3` = „Katzennassnahrung"). Beide gehören in den Titel, sonst fallen
//    zwei Marken desselben Produkts auf dieselbe Offer-ID — am 2026-07-31
//    standen Sheba und Whiskas mit je einer „Katzennassnahrung" im selben
//    Prospekt. Dieselbe Falle wie bei Kaufland, siehe dort.

const BASE: &str = "https://www.norma-online.de";
const INDEX_PATH: &str = "/de/angebote/";

/// Echte Filiale über den Filialfinder; None ohne Filiale im Umkreis der PLZ.
/// Angebote bleiben der nationale Katalog (siehe store_finder.rs).
pub fn find_market(zip: &str) -> Result<Option<Market>> {
    Ok(store_finder::resolve("NORMA", store_finder::norma_branch(zip), national()))
}

/// Der bundesweite Katalog-Markt — die Filiale, unter der NORMA gespeichert wird.
pub fn national() -> Market {
    Market::new("NORMA_DE", "NORMA Deutschland")
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    let index_url = format!("{BASE}{INDEX_PATH}");
    let index = get(&index_url, "Angebotsübersicht")?;

    let themes = parse_index(&index);
    if themes.is_empty() {
        bail!(
            "[NORMA] Keine Themenseiten in {index_url} gefunden — Seitenstruktur hat sich möglicherweise geändert"
        );
    }

    let mut offers = Vec::new();
    let mut seen = HashSet::new();
    for path in &themes {
        let url = format!("{BASE}{path}");
        // Eine unerreichbare Themenseite darf nicht den ganzen Lauf kippen —
        // dieselbe Regel wie bei Nettos Kategorieseiten.
        let Ok(html) = get(&url, "Themenseite") else { continue };
        parse_theme(&html, path, &market.id, &mut offers, &mut seen);
    }

    if offers.is_empty() {
        bail!(
            "[NORMA] Keine Angebote in {} Themenseiten gefunden — Seitenstruktur hat sich möglicherweise geändert",
            themes.len()
        );
    }
    Ok(offers)
}

fn get(url: &str, step: &str) -> Result<String> {
    util::polite_pause(url);
    util::blocking_client()?
        .get(url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "de-DE,de;q=0.9")
        .send()
        .with_context(|| util::ctx("NORMA", step, url))?
        .error_for_status()
        .with_context(|| util::ctx("NORMA", &format!("{step} (HTTP-Status)"), url))?
        .text()
        .with_context(|| util::ctx("NORMA", &format!("{step} lesen"), url))
}

/// Themenseiten-Pfade aus dem Index, in Reihenfolge und ohne Dubletten.
///
/// Muster: `/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/`. Der
/// Index verlinkt dieselbe Seite mehrfach (Kachel, Slider, Navigation), teils
/// mit `?pk_campaign=…` — Query-Teil abschneiden und dedupen.
///
/// Die Artikel-Detailseiten (`…-t-379172/pils-i-378766/`) fallen raus: Sie
/// tragen keine Angebote, die nicht schon auf der Themenseite stehen. Sie
/// haben ein Segment mehr, daran sind sie zu erkennen.
pub fn parse_index(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let sel = sel("a[href]");
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for a in doc.select(&sel) {
        let Some(href) = a.value().attr("href") else { continue };
        let path = href.split('?').next().unwrap_or(href);
        if !path.starts_with("/de/angebote/ab-") {
            continue;
        }
        // ["", "de", "angebote", "ab-montag,-03.08.26", "thema-t-123", ""]
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != 6 || !parts[4].contains("-t-") {
            continue;
        }
        if seen.insert(path.to_string()) {
            out.push(path.to_string());
        }
    }
    out
}

/// Angebote einer Themenseite. `path` liefert Termin und Thema — beides steht
/// nur im URL-Pfad zuverlässig, nicht im Seitentext.
pub fn parse_theme(
    html: &str,
    path: &str,
    market_id: &str,
    offers: &mut Vec<Offer>,
    seen: &mut HashSet<String>,
) {
    let valid_from = valid_from_from_path(path);
    let valid_until = valid_until_from_path(path);

    let doc = Html::parse_document(html);
    let category = category_of(&doc, path);
    let sel_tile = sel("article.produktBoxContainer");
    let sel_brand = sel("strong.supplier");
    let sel_title = sel("h3.produktBox-txt-headline");
    let sel_desc = sel("p.produktBox-txt-description");
    let sel_amount = sel("li.produktBox-txt-inh");
    let sel_base = sel("li.produktBox-txt-price");
    let sel_strike = sel(STRIKE_SELECTOR);
    let sel_price = sel("li.produktBox-cont-wrapper-price span[aria-label]");
    let sel_img = sel("div.produktBox-img img");

    for tile in doc.select(&sel_tile) {
        let Some(headline) = text_of(tile, &sel_title) else { continue };
        let brand = text_of(tile, &sel_brand);

        // Marke vor den Produktnamen, weil der Produktname allein nicht
        // eindeutig ist (siehe Modulkommentar). Steht die Marke schon im
        // Namen, bleibt es beim Namen.
        let title = match &brand {
            Some(b) if !headline.to_lowercase().contains(&b.to_lowercase()) => {
                format!("{b} {headline}")
            }
            _ => headline,
        };

        let price = tile
            .select(&sel_price)
            .next()
            .and_then(|e| e.value().attr("aria-label"))
            .and_then(parse_price);
        let regular_price = text_of(tile, &sel_strike).as_deref().and_then(parse_price);

        // Der Untertitel trägt allein die Menge ("je 225 g"), damit
        // `push::is_pure_quantity` ihn erkennt und als Einheit übernimmt,
        // statt ihn an den Produktnamen zu hängen.
        let subtitle = text_of(tile, &sel_amount).as_deref().map(strip_deposit);
        let overline = match (text_of(tile, &sel_desc), text_of(tile, &sel_base)) {
            (Some(d), Some(b)) => Some(format!("{d} ({b})")),
            (d, b) => d.or(b),
        };

        let images = tile
            .select(&sel_img)
            .next()
            .and_then(|img| img.value().attr("src"))
            .map(absolute_url)
            .into_iter()
            .collect();

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
        })
    }
}

/// Die Zeile über dem Preis — aber nur, solange sie wirklich einen Preis trägt.
///
/// NORMA benutzt dieselbe Klasse `produktBox-cont-wrapper-uvp` für **zwei**
/// Zeilen. Die reine Klasse ist der Streichpreis („statt 1,59", „UVP 2,49",
/// „UVP = 13,95"). Kommt `produktBox-cont-wrapper-uvp-info` dazu, steht dort
/// ein Hinweis statt eines Preises — gemessen am 2026-07-31 in 25 Kacheln:
///
/// ```text
/// Ständig im Sortiment          14x   (keine Ziffer, harmlos)
/// Zum tagesaktuellen Tiefpreis   5x
/// NEU im Sortiment               3x
/// 25% billiger                   1x   -> las sich als "statt 25,00"
/// z.B. 1,1 kg                    1x   -> las sich als "statt 1,10"
/// z.B. 909 g                     1x   -> las sich als "statt 909,00"
/// ```
///
/// Die letzten drei sind Beispiel-Gewichte und ein Rabattband, keine früheren
/// Preise. Zwei davon standen so in der Produktion („4,99 statt 909",
/// „5,99 statt 1,10"); die Maggi-Kachel trägt gar keinen Preis und fiel
/// deshalb schon vorher aus dem Upload. Die dritte Klasse `…-uvp-spacer` ist
/// ein eigener Klassenname und trifft den Selektor ohnehin nicht.
const STRIKE_SELECTOR: &str =
    "li.produktBox-cont-wrapper-uvp:not(.produktBox-cont-wrapper-uvp-info)";

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("statischer CSS-Selektor")
}

/// Text eines Treffers, Textknoten mit Leerzeichen verbunden.
///
/// Das `join` statt `collect` ist kein Schönheitsfehler: NORMA setzt in eine
/// Zeile mehrere Knoten ohne Leerraum dazwischen („…Tipo" + „je 520 g"), und
/// ein bloßes Aneinanderhängen macht daraus „Tipoje 520 g" — gemessen am
/// 2026-07-31 an der Dr.-Oetker-Pizza.
fn text_of(el: ElementRef, selector: &Selector) -> Option<String> {
    let node = el.select(selector).next()?;
    let text = node
        .text()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { None } else { Some(text) }
}

/// Preistexte aus dem aria-label ("0,99 Euro") und aus den Streichpreis-Zeilen
/// ("statt 1,59", "UVP 2,49", "UVP = 13,95", "UVP –,99").
///
/// Bewusst über die *letzte* Zahl im Text: „1 l = –,79 ohne App" und
/// „UVP = 13,95" tragen beide ein Gleichheitszeichen, und der NormaPlus-Zusatz
/// „ohne App" hängt hinten dran.
///
/// **Der Gedankenstrich ist eine Null.** Unter einem Euro druckt NORMA
/// „–,99" statt „0,99" — im Preis selbst ist das folgenlos (dort steht das
/// aria-label daneben), in der UVP-Zeile nicht: Sie hat kein aria-label, und
/// ohne diese Regel wurde aus „UVP –,99" ein Streichpreis von 99 €.
/// Ein Trennzeichen darf deshalb eine Zahl eröffnen, wenn unmittelbar davor
/// ein Strich steht.
fn parse_price(s: &str) -> Option<f64> {
    let mut found = None;
    let mut current = String::new();
    // Steht der Strich direkt vor dem Komma, ist er die weggelassene Null.
    let mut dash = false;
    for c in s.chars() {
        match c {
            '0'..='9' => {
                current.push(c);
                dash = false;
            }
            ',' | '.' if !current.contains('.') && (!current.is_empty() || dash) => {
                if current.is_empty() {
                    current.push('0');
                }
                current.push('.');
                dash = false;
            }
            _ => {
                if let Ok(v) = current.parse::<f64>() {
                    found = Some(v);
                }
                current.clear();
                dash = matches!(c, '-' | '–' | '—' | '−' | '‒');
            }
        }
    }
    if let Ok(v) = current.parse::<f64>() {
        found = Some(v);
    }
    found
}

/// Pfand aus der Mengenangabe schneiden: "je 1,25 l, zzgl. –,25 Pfand"
/// -> "je 1,25 l". Sonst hält `push::is_pure_quantity` die Zeile für einen
/// Produktnamen und hängt sie an den Titel.
///
/// NORMA schreibt das Komma mal mit und mal ohne Leerzeichen davor
/// („je 6 x 0,5 l,zzgl. 6 x –,25 Pfand"), deshalb wird nach dem Wort selbst
/// gesucht und der Trenner danach abgeschnitten.
fn strip_deposit(s: &str) -> String {
    let cut = ["zzgl", "inkl. Pfand"]
        .iter()
        .filter_map(|marker| s.find(marker))
        .min()
        .unwrap_or(s.len());
    s[..cut].trim().trim_end_matches([',', ';', '·']).trim().to_string()
}

/// Laufzeit eines NORMA-Termins in Tagen, Starttag mitgezählt.
///
/// **Abgeleitet, nicht von der Themenseite abgelesen** — dort steht nur „ab
/// Montag, 03.08.". Gemessen am 2026-07-31 auf den Artikel-Detailseiten des
/// Montagstermins: „Aktionszeitraum: 03.08. bis 09.08.2026", also sieben Tage
/// (Mo–So). Für die Mittwochs- und Freitagstermine gibt es diese Angabe auf
/// keiner erreichbaren Seite; sie bekommen dasselbe Fenster.
///
/// Warum überhaupt abgeleitet wird, statt `valid_until` leer zu lassen: Die
/// App liest die Spalte als **nicht-optionales** `Date`
/// (`ios/LeChariot/Models/Offer.swift`). Ein `null` bricht nicht nur die
/// NORMA-Zeile, sondern das Decoding der **ganzen** Angebotsantwort.
///
/// `valid_until_is_the_published_window` prüft die Ableitung live gegen eine
/// Detailseite und schlägt an, wenn NORMA den Rhythmus ändert.
const TERM_DAYS: i64 = 7;

/// "/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/" -> "2026-08-03"
fn valid_from_from_path(path: &str) -> Option<String> {
    date_from_path(path).map(|d| d.to_string())
}

/// Ende des Termins: Start + [`TERM_DAYS`] − 1.
fn valid_until_from_path(path: &str) -> Option<String> {
    let start = date_from_path(path)?;
    Some((start + chrono::Duration::days(TERM_DAYS - 1)).to_string())
}

fn date_from_path(path: &str) -> Option<chrono::NaiveDate> {
    let termin = path.split('/').nth(3)?;
    let datum = termin.rsplit(',').next()?.trim_start_matches('-');
    let mut parts = datum.split('.');
    let (d, m, y) = (parts.next()?, parts.next()?, parts.next()?);
    let (day, month, year): (u32, u32, u32) =
        (d.parse().ok()?, m.parse().ok()?, y.parse().ok()?);
    let year = if year < 100 { 2000 + year } else { year };
    chrono::NaiveDate::from_ymd_opt(year as i32, month, day)
}

/// Themenname als Kategorie — die einzige, die NORMA je Angebot hergibt.
/// `matching::match_keys` liest sie als drittes Signal, `enrich` macht daraus
/// die gespeicherte Kategorie und das Emoji.
///
/// Erste Wahl ist der Seitentitel („NORMA … | Mehr fürs Geld."), denn nur dort
/// steht der Name so, wie NORMA ihn schreibt. Der URL-Slug ist der Rückfall
/// und schlechter: Er hat keine Umlaute („unser-obst-und-gemuese").
fn category_of(doc: &Html, path: &str) -> Option<String> {
    let from_title = text_of(doc.root_element(), &sel("title"))
        .and_then(|t| t.rsplit('|').next().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty() && !s.to_lowercase().starts_with("norma"));
    from_title.or_else(|| category_from_path(path))
}

/// "/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/"
/// -> "genuss mit nuss"
///
/// Rückfall ohne Seitentitel. Bewusst ohne kosmetische Großschreibung: Die
/// Kategorie ist ein Signal für Matching und `enrich` (beide vergleichen
/// normalisiert), kein Text, den jemand liest — ein aufgehübschter Slug würde
/// nur vortäuschen, er sei der echte Themenname.
fn category_from_path(path: &str) -> Option<String> {
    let slug = path.split('/').nth(4)?;
    let name = slug.rsplit_once("-t-").map(|(name, _)| name).unwrap_or(slug);
    let words: Vec<&str> = name.split(['-', '_']).filter(|w| !w.is_empty()).collect();
    if words.is_empty() { None } else { Some(words.join(" ")) }
}

fn absolute_url(src: &str) -> String {
    if src.starts_with("http") { src.to_string() } else { format!("{BASE}{src}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_parsing() {
        assert_eq!(parse_price("0,99 Euro"), Some(0.99));
        assert_eq!(parse_price("15,99 Euro"), Some(15.99));
        assert_eq!(parse_price("statt 1,59"), Some(1.59));
        assert_eq!(parse_price("UVP 2,49"), Some(2.49));
        // Gleichheitszeichen und Nachsatz: die letzte Zahl gewinnt nicht
        // blind — "UVP = 13,95" hat nur eine.
        assert_eq!(parse_price("UVP = 13,95"), Some(13.95));
        assert_eq!(parse_price(""), None);
        assert_eq!(parse_price("Zum tagesaktuellen Tiefpreis"), None);

        // Der Gedankenstrich ist die weggelassene Null — ohne diese Regel
        // wurde aus "UVP –,99" ein Streichpreis von 99 €.
        assert_eq!(parse_price("UVP –,99"), Some(0.99));
        assert_eq!(parse_price("1 l = –,79 ohne App"), Some(0.79));
        // Auch mit dem einfachen Bindestrich, falls NORMA das Zeichen tauscht.
        assert_eq!(parse_price("UVP -,49"), Some(0.49));
        // Umgekehrt: der Strich *hinter* dem Komma ist ein glatter Betrag,
        // und die Zahl davor darf nicht verloren gehen ("1 kg = 399,–").
        assert_eq!(parse_price("1 kg = 399,–"), Some(399.0));
        // Ein Strich ohne Zahl bleibt kein Preis.
        assert_eq!(parse_price("–"), None);
        assert_eq!(parse_price("Ständig im Sortiment"), None);
    }

    #[test]
    fn deposit_is_cut_from_the_amount() {
        assert_eq!(strip_deposit("je 1,25 l, zzgl. –,25 Pfand"), "je 1,25 l");
        assert_eq!(strip_deposit("je 225 g"), "je 225 g");
        assert_eq!(strip_deposit("je 0,5 l zzgl. Pfand"), "je 0,5 l");
        // Ohne Leerzeichen vor dem Komma — so steht es bei Beck's Pils.
        assert_eq!(strip_deposit("je 6 x 0,5 l,zzgl. 6 x –,25 Pfand"), "je 6 x 0,5 l");
    }

    #[test]
    fn text_nodes_do_not_grow_together() {
        // "…Tipo" und "je 520 g" stehen als zwei Knoten ohne Leerraum in
        // derselben Zeile; ohne das Trennzeichen entstand "Tipoje 520 g".
        let doc = Html::parse_document(
            "<li class=\"produktBox-txt-inh\">z.B. Calabrese &amp; 'Nduja Tipo<br>je 520 g</li>",
        );
        assert_eq!(
            text_of(doc.root_element(), &sel("li.produktBox-txt-inh")).as_deref(),
            Some("z.B. Calabrese & 'Nduja Tipo je 520 g")
        );
    }

    #[test]
    fn date_from_the_url_slug() {
        assert_eq!(
            valid_from_from_path("/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/")
                .as_deref(),
            Some("2026-08-03")
        );
        assert_eq!(
            valid_from_from_path("/de/angebote/ab-freitag,-31.07.26/wochenend-spezial-t-378267/")
                .as_deref(),
            Some("2026-07-31")
        );
        assert_eq!(valid_from_from_path("/de/angebote/"), None);
        // Kein Kalender-Unsinn: der 31. Februar ist kein Datum.
        assert_eq!(valid_from_from_path("/de/angebote/ab-montag,-31.02.26/x-t-1/"), None);
    }

    #[test]
    fn term_runs_a_full_week() {
        // Gemessen: "Aktionszeitraum: 03.08. bis 09.08.2026" (Mo bis So).
        let path = "/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/";
        assert_eq!(valid_from_from_path(path).as_deref(), Some("2026-08-03"));
        assert_eq!(valid_until_from_path(path).as_deref(), Some("2026-08-09"));
        // Über den Monatswechsel hinweg.
        let ende = "/de/angebote/ab-mittwoch,-29.07.26/mittwochs-clou-t-378263/";
        assert_eq!(valid_until_from_path(ende).as_deref(), Some("2026-08-04"));
    }

    #[test]
    fn category_prefers_the_page_title_over_the_slug() {
        let path = "/de/angebote/ab-montag,-03.08.26/unser-obst-und-gemuese-t-379181/";

        // Der echte Titel trägt Umlaute und die Schreibweise von NORMA.
        let doc = Html::parse_document(
            "<html><head><title>NORMA - Ihr Lebensmittel-Discounter  | Unser Obst &amp; Gemüse</title></head></html>",
        );
        assert_eq!(category_of(&doc, path).as_deref(), Some("Unser Obst & Gemüse"));

        // Ohne brauchbaren Titel der Slug — kleingeschrieben und ohne Umlaute,
        // aber als Matching-Signal genauso gut.
        let bare = Html::parse_document("<html><head><title>NORMA</title></head></html>");
        assert_eq!(category_of(&bare, path).as_deref(), Some("unser obst und gemuese"));
    }

    #[test]
    fn index_keeps_only_theme_pages() {
        let html = r#"<html><body>
            <a href="/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/">Thema</a>
            <a href="/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/?pk_campaign=Slider">nochmal</a>
            <a href="/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/pils-i-378766/">Artikel</a>
            <a href="/de/angebote/ab-montag,-03.08.26/">nur der Termin</a>
            <a href="/at/angebote/ab-montag,-03.08.26/oesterreich-t-1/">Österreich</a>
            <a href="/de/filialfinder/">woanders</a>
        </body></html>"#;
        assert_eq!(
            parse_index(html),
            vec!["/de/angebote/ab-montag,-03.08.26/genuss-mit-nuss-t-379172/"]
        );
    }

    /// Live-Test gegen norma-online.de: cargo test norma -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen norma-online.de"]
    fn live_fetch_offers() {
        let market = national();
        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(5) {
            println!(
                "- {} | {:?} | {:?} € (statt {:?}) | {:?} | ab {:?}",
                o.title, o.subtitle, o.price, o.regular_price, o.category, o.valid_from
            );
        }
        let mit_preis = offers.iter().filter(|o| o.price.is_some()).count();
        let mit_streichpreis = offers.iter().filter(|o| o.regular_price.is_some()).count();
        println!("davon {mit_preis} mit Preis, {mit_streichpreis} mit Streichpreis");

        assert!(offers.len() >= 100, "Erwartet >= 100 Angebote, war {}", offers.len());
        assert!(
            mit_preis * 10 >= offers.len() * 9,
            "Erwartet >= 90 % mit Preis, war {mit_preis}/{}",
            offers.len()
        );
        assert!(mit_streichpreis > 0, "Kein einziger Streichpreis — Selektor kaputt?");
    }

    /// Wächter über [`TERM_DAYS`]: Die Laufzeit ist abgeleitet, weil sie auf
    /// den Themenseiten nicht steht — die **Artikel**-Detailseiten nennen sie
    /// aber ("Aktionszeitraum: 03.08. bis 09.08.2026"). Dieser Test sucht eine
    /// solche Seite und rechnet die Ableitung dagegen.
    ///
    /// Nicht jede Detailseite trägt die Angabe (am 2026-07-31 zwei von
    /// achtzehn geprüften Themen). Findet der Test keine, sagt er das, statt
    /// grün zu behaupten, er habe etwas geprüft.
    ///
    /// cargo test valid_until_is_the_published_window -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen norma-online.de"]
    fn valid_until_is_the_published_window() {
        let index = get(&format!("{BASE}{INDEX_PATH}"), "Angebotsübersicht").expect("Index");

        // Artikelseiten hängen an den Themenseiten, nicht am Index — deshalb
        // erst die Themen, dann deren Artikellinks einsammeln.
        let sel_link = sel("a[href]");
        let mut artikel: Vec<String> = Vec::new();
        for theme in parse_index(&index) {
            let Ok(page) = get(&format!("{BASE}{theme}"), "Themenseite") else { continue };
            let doc = Html::parse_document(&page);
            for a in doc.select(&sel_link) {
                let Some(href) = a.value().attr("href") else { continue };
                let path = href.split('?').next().unwrap_or(href);
                // Artikelseite: ein Segment tiefer als die Themenseite.
                if path.starts_with("/de/angebote/ab-") && path.split('/').count() == 7 {
                    artikel.push(path.to_string());
                }
            }
        }
        artikel.sort();
        artikel.dedup();
        println!("{} Artikelseiten gefunden", artikel.len());

        let mut geprueft = 0;
        for path in &artikel {
            let path = path.as_str();
            let Ok(html) = get(&format!("{BASE}{path}"), "Artikelseite") else { continue };
            let text: String = Html::parse_document(&html)
                .root_element()
                .text()
                .collect::<Vec<_>>()
                .join(" ");
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let Some(rest) = text.split("Aktionszeitraum:").nth(1) else { continue };
            let genannt: String = rest.chars().take(30).collect();

            let erwartet = valid_until_from_path(path).expect("Termin im Pfad");
            // "03.08. bis 09.08.2026" -> Tag und Monat des Endes
            let (tag, monat) = {
                let ende = genannt.split("bis").nth(1).expect("Enddatum").trim();
                let mut p = ende.split('.');
                (p.next().unwrap().to_string(), p.next().unwrap().to_string())
            };
            println!("{path}\n  genannt: {genannt}\n  abgeleitet: {erwartet}");
            assert!(
                erwartet.ends_with(&format!("-{monat}-{tag}")),
                "NORMA nennt '{genannt}', TERM_DAYS={TERM_DAYS} ergibt {erwartet} — Rhythmus geändert?"
            );
            geprueft += 1;
            if geprueft == 2 {
                return;
            }
        }
        panic!("Keine Detailseite mit 'Aktionszeitraum' gefunden — Ableitung ungeprüft");
    }

    /// Live-Test des Filialfinders: cargo test norma_live_branch -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen norma-online.de"]
    fn norma_live_branch() {
        let market = find_market("01219").expect("Filialsuche").expect("Filiale");
        println!("Markt: {} ({})", market.name, market.id);
        assert!(market.id.starts_with("NORMA_"));
    }

    /// Live-Test des Verzeichniswegs — der andere Weg als `norma_live_branch`:
    /// Der schreibt eine Filiale in den Push, dieser die Liste, aus der die App
    /// ihre Auswahl baut. Eine Zeile ohne Koordinaten wäre dort unbrauchbar.
    ///
    /// cargo test norma_live_directory -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen norma-online.de"]
    fn norma_live_directory() {
        for plz in ["01219", "90402"] {
            let branches = store_finder::norma_branches(
                plz,
                crate::branches::AREA_RADIUS_KM,
                crate::branches::MAX_PER_CHAIN,
            )
            .expect("Filialverzeichnis");
            let mit_geo = branches.iter().filter(|b| b.lat.is_some() && b.lon.is_some()).count();
            let mit_strasse = branches.iter().filter(|b| b.street.is_some()).count();
            println!(
                "{plz}: {} Filialen, {mit_geo} mit Koordinaten, {mit_strasse} mit Straße",
                branches.len()
            );
            assert!(!branches.is_empty(), "{plz}: keine Filiale — Formularsuche kaputt?");
            assert_eq!(mit_geo, branches.len(), "{plz}: Zeile ohne Koordinaten");
            assert_eq!(mit_strasse, branches.len(), "{plz}: Zeile ohne Straße");
        }
    }
}
