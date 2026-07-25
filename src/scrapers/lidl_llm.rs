//! Lidl-Prospekt: Angebote aus der Textebene per Sprachmodell herauslesen.
//!
//! **Warum überhaupt ein Modell?** Der Prospekt enthält die Daten vollständig —
//! am 2026-07-25 nachgemessen: **365 von 375 marktguru-Angeboten** derselben
//! Woche stehen mit ihrem Preis im PDF-Text, die übrigen zehn sind
//! Onlineshop-Möbel aus dem `products`-Feld des Prospekt-JSON. Es fehlt also
//! keine Quelle. Was fehlt, ist die *Zuordnung*: Welcher Preis gehört zu
//! welchem Produktnamen. Der geometrische Extraktor in
//! [`crate::scrapers::lidl_prospekt`] kommt damit auf rund 240 Angebote, und
//! die Lücke besteht vor allem aus Obst und Gemüse und Non-Food, wo die
//! Kachel keine saubere Marke-Name-Beschreibung-Struktur hat.
//!
//! Namen an Preise zu binden ist Sprachverständnis, kein Geometrieproblem.
//! Deshalb hier ein zweiter Weg: dieselbe Textebene, aber seitenweise durch
//! ein Modell.
//!
//! **Was das Modell nicht darf: sich Preise ausdenken.** Jedes zurückgegebene
//! Angebot wird gegengeprüft — der Preis muss wörtlich im Seitentext stehen
//! ([`price_is_grounded`]), und wo Packungsgröße und Grundpreis vorliegen,
//! muss die Rechnung aufgehen (dieselbe Probe wie im geometrischen Weg).
//! Was das nicht besteht, fliegt raus. Ein erfundener Preis ist in einer
//! Preisvergleichs-App schlimmer als ein fehlendes Produkt.
//!
//! **Warum GitHub Models?** Der Weg ist ein Zusatz, kein Fundament — der
//! deterministische Extraktor deckt bereits 96 % der marktguru-Preise ab.
//! Ein Zusatz darf keine laufenden Kosten verursachen, und GitHub Models ist
//! im GitHub-Student-Paket enthalten. Der Preis dafür sind harte
//! Ratenlimits (Freistufe: rund 15 Anfragen pro Minute), weshalb die Seiten
//! nacheinander statt parallel laufen — ein Wochenprospekt braucht damit
//! wenige Minuten. Für einen wöchentlichen Lauf ist das egal.
//!
//! Der Token kommt aus `GITHUB_MODELS_TOKEN`, sonst `GITHUB_TOKEN`, sonst
//! aus `gh auth token` — lokal ist also nichts einzurichten, wenn die
//! GitHub-CLI ohnehin angemeldet ist.

use std::collections::HashSet;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use crate::models::{Market, Offer};
use crate::scrapers::lidl_prospekt;

const API_URL: &str = "https://models.github.ai/inference/chat/completions";
/// Über `LIDL_LLM_MODEL` austauschbar, falls der Katalog sich ändert.
const DEFAULT_MODEL: &str = "openai/gpt-4.1-mini";
const MAX_TOKENS: u32 = 8_000;
/// Mindestabstand zwischen zwei Anfragen. Die Freistufe erlaubt rund 15 pro
/// Minute; 4 Sekunden halten sicheren Abstand, ohne dass ein Prospekt
/// unzumutbar lange braucht.
const REQUEST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4);
/// Nach einem 429 einmal deutlich länger warten und den Versuch wiederholen.
const RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(30);

/// Seiten ohne Sternpreis tragen keine Angebote — Titelbild, Imageseiten,
/// Rechtstext. Die spart der Vorfilter, bevor ein Request entsteht.
static HAS_STAR_PRICE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{1,3}[.,]\d{2}\s*\*").unwrap());

const SYSTEM_PROMPT: &str = "\
Du liest die Textebene einer Seite aus einem Lidl-Wochenprospekt und gibst die \
Angebote strukturiert zurück. Der Text stammt aus `pdftotext -layout`, die \
Spaltenausrichtung der gedruckten Seite ist also grob erhalten, aber Kacheln \
können ineinanderlaufen.

So ist eine Angebotskachel im Prospekt aufgebaut:
- Marke in Großbuchstaben (ARLA, MILBONA, GRILLMEISTER, COMBINO)
- darunter der Produktname
- eine Beschreibung wie \"Versch. Sorten. Gekühlt. Je 400 g\"
- der Grundpreis als \"1 kg = 6.23\" oder \"1 l = -.68\" (der Bindestrich steht \
für Beträge unter einem Euro, also 0,68 €)
- der Angebotspreis mit einem Stern: \"2.49*\"
- optional ein Streichpreis als \"UVP 3.19\" oder \"Normalpreis: 6.99\"
- optional der Hinweis \"Mit Lidl Plus\" — dann gilt der Preis nur mit Kundenkarte

Regeln:
1. Gib NUR Angebote zurück, deren Preis wörtlich im Text steht. Rechne keine \
Preise aus und rate nie. Wenn du unsicher bist, lass das Angebot weg.
2. Der Angebotspreis ist der Preis mit Stern. Streichpreis und Grundpreis sind \
NICHT der Angebotspreis.
3. Der Titel ist NUR Marke plus Produktname — hoechstens fuenf Woerter. Er \
enthaelt KEINE Beschreibung, KEINE Mengenangabe, KEINEN Grundpreis, KEINEN \
Rabatt, KEINEN Preis und NICHT \"Mit Lidl Plus\". Aus \"ARLA Kaergarden XXL \
Versch. Sorten. Gekuehlt. Je 400 g 1 kg = 6.23\" wird also der Titel \"ARLA \
Kaergarden XXL\"; Menge und Grundpreis gehoeren in die Felder quantity und \
base_price. Bei Obst und Gemuese ohne Marke reicht die Sortenbezeichnung \
(\"Rote Aepfel\", \"Zucchini\", \"Heidelbeeren\").
4. Überspringe alles, was kein kaufbarer Artikel ist: Werbeclaims, Fußnoten, \
Rechtstexte, Öffnungszeiten, Druckkennungen wie \"LHZ – 30/2026 – BE/BY\", \
Seitenzahlen, Hinweise auf die Lidl-App oder den Onlineshop.
5. Non-Food-Artikel (Möbel, Werkzeug, Textilien, Deko) gehören ebenfalls dazu, \
solange sie einen Sternpreis haben.
6. Ein Artikel, der in mehreren Größen zu verschiedenen Preisen angeboten wird, \
ergibt mehrere Einträge.";

const USER_PREFIX: &str = "Seitentext:\n\n";

// ------------------------------------------------------------------ Antwort

#[derive(Debug, Deserialize)]
struct LlmOffer {
    title: String,
    price: f64,
    #[serde(default)]
    regular_price: Option<f64>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    base_price: Option<String>,
    #[serde(default)]
    lidl_plus: bool,
}

#[derive(Debug, Deserialize)]
struct LlmPage {
    offers: Vec<LlmOffer>,
}

/// JSON-Schema für `output_config.format`. Erzwingt gültiges JSON, sodass es
/// keinen Parser für halb ausgegebene Antworten braucht.
fn response_schema() -> serde_json::Value {
    let nullable_number = serde_json::json!({"anyOf": [{"type": "number"}, {"type": "null"}]});
    let nullable_string = serde_json::json!({"anyOf": [{"type": "string"}, {"type": "null"}]});
    serde_json::json!({
        "type": "object",
        "properties": {
            "offers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "price": {"type": "number"},
                        "regular_price": nullable_number,
                        "quantity": nullable_string,
                        "base_price": nullable_string,
                        "lidl_plus": {"type": "boolean"}
                    },
                    "required": [
                        "title", "price", "regular_price",
                        "quantity", "base_price", "lidl_plus"
                    ],
                    "additionalProperties": false
                }
            }
        },
        "required": ["offers"],
        "additionalProperties": false
    })
}

fn request_body(page_text: &str) -> serde_json::Value {
    let model = std::env::var("LIDL_LLM_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    serde_json::json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        // Auslesearbeit, keine Kreativaufgabe.
        "temperature": 0,
        // `strict` erzwingt gültiges JSON nach Schema — erspart einen Parser
        // für halb ausgegebene Antworten.
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "lidl_offers",
                "strict": true,
                "schema": response_schema()
            }
        },
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": format!("{USER_PREFIX}{page_text}")}
        ]
    })
}

// ------------------------------------------------------------------ Prüfung

/// Steht dieser Preis wörtlich auf der Seite?
///
/// Die einzige wirksame Bremse gegen erfundene Preise. Der Prospekt schreibt
/// mit Punkt („2.49"), im Fließtext kommt aber auch das Komma vor — beide
/// Schreibweisen zählen.
pub fn price_is_grounded(price: f64, page_text: &str) -> bool {
    let dot = format!("{price:.2}");
    let comma = dot.replace('.', ",");
    // Beträge unter einem Euro schreibt Lidl als „-.68"
    let dash = dot.strip_prefix('0').map(|rest| format!("-{rest}"));
    page_text.contains(&dot)
        || page_text.contains(&comma)
        || dash.is_some_and(|d| page_text.contains(&d))
}

/// Beschreibung abschneiden, falls das Modell sie trotz Anweisung in den
/// Titel gezogen hat. Live beobachtet am 2026-07-25: „ARLA Kaergarden XXL
/// Gekühlt. Je 400 g 1 kg = 6.23" statt „ARLA Kaergarden XXL".
fn clean_title(title: &str) -> String {
    const CUTS: &[&str] = &[
        "Versch.",
        "Gekühlt",
        "Tiefgefroren",
        " Je ",
        "1 kg =",
        "1 l =",
        "1 Stk =",
        "zzgl.",
        "Mit Lidl Plus",
        "UVP",
        "Normalpreis",
    ];
    let mut end = title.len();
    for cut in CUTS {
        if let Some(at) = title.find(cut)
            && at > 0
            && at < end
        {
            end = at;
        }
    }
    title[..end]
        .trim_matches(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == '-')
        .to_string()
}

/// Titel, die kein Produkt benennen — dieselbe Filterliste wie im
/// geometrischen Weg, damit beide Wege dieselben Zeilen verwerfen.
fn title_is_usable(title: &str) -> bool {
    let trimmed = title.trim();
    trimmed.chars().count() >= 3 && lidl_prospekt::is_plausible_title(trimmed)
}

/// Ein Modell-Ergebnis in ein `Offer` übersetzen — oder verwerfen.
fn build_offer(
    raw: &LlmOffer,
    page_text: &str,
    page: usize,
    market_id: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
) -> Option<Offer> {
    if !(0.10..=10_000.0).contains(&raw.price) || !price_is_grounded(raw.price, page_text) {
        return None;
    }
    let title = clean_title(&raw.title);
    if !title_is_usable(&title) {
        return None;
    }

    // Rechenprobe, wo sie möglich ist: Menge × Grundpreis muss den Preis
    // ergeben. Beim Modell ist das nicht nur ein Kachel-Check, sondern die
    // Kontrolle, ob es Größe und Preis derselben Kachel entnommen hat.
    let context = format!(
        "{} {} {}",
        raw.quantity.as_deref().unwrap_or(""),
        raw.base_price.as_deref().unwrap_or(""),
        title
    );
    if lidl_prospekt::arithmetic_check(&context, raw.price) == Some(false) {
        return None;
    }

    let regular = raw.regular_price.filter(|r| *r > raw.price);
    let mut parts: Vec<String> = Vec::new();
    if let Some(q) = raw
        .quantity
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(q.to_string());
    }
    if let Some(b) = raw
        .base_price
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(b.to_string());
    }
    if raw.lidl_plus {
        parts.push("nur mit Lidl Plus".to_string());
    }
    Some(Offer {
        id: Offer::build_id(market_id, &format!("{title}_{:.2}", raw.price), valid_from),
        market_id: market_id.to_string(),
        title,
        subtitle: (!parts.is_empty()).then(|| parts.join(" · ")),
        overline: None,
        price: Some(raw.price),
        regular_price: regular,
        category: None,
        nutri_score: None,
        valid_from: valid_from.map(str::to_string),
        valid_until: valid_until.map(str::to_string),
        images: Vec::new(),
        biozid: false,
        flyer_page: Some(page as i64),
    })
}

// ------------------------------------------------------------------ Netzwerk

/// Token für GitHub Models: erst die Umgebung, dann die angemeldete
/// GitHub-CLI. Letzteres macht den lokalen Lauf voraussetzungsfrei, solange
/// `gh auth login` einmal gelaufen ist.
fn api_key() -> Result<String> {
    for var in ["GITHUB_MODELS_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(var)
            && !token.trim().is_empty()
        {
            return Ok(token);
        }
    }
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .context(
            "Kein GITHUB_MODELS_TOKEN/GITHUB_TOKEN gesetzt und `gh` nicht gefunden — \
             für LIDL_SOURCE=prospekt-llm wird eines von beiden benötigt",
        )?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || token.is_empty() {
        bail!(
            "`gh auth token` lieferte kein Token — bitte `gh auth login` ausführen \
             oder GITHUB_MODELS_TOKEN setzen"
        );
    }
    Ok(token)
}

/// Eine Seite auslesen.
///
/// Zwei Versuche, und zwar für beide Sorten Fehlschlag: das Ratenlimit (429)
/// und abgerissene Verbindungen. Letzteres ist kein theoretischer Fall — beim
/// ersten Lauf über 46 Seiten kam auf Seite 44 ein „Connection reset by peer".
/// Ohne Wiederholung verliert man dafür eine ganze Prospektseite.
async fn extract_page(
    client: &reqwest::Client,
    token: &str,
    page_text: &str,
) -> Result<Vec<LlmOffer>> {
    for attempt in 0..2 {
        let sent = client
            .post(API_URL)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .json(&request_body(page_text))
            .send()
            .await;
        let resp = match sent {
            Ok(resp) => resp,
            Err(e) if attempt == 0 => {
                eprintln!("  Verbindung abgerissen ({e}) — neuer Versuch in 5s");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            Err(e) => return Err(e).context("Anfrage an GitHub Models fehlgeschlagen"),
        };

        let status = resp.status();
        if status.as_u16() == 429 && attempt == 0 {
            eprintln!("  Ratenlimit erreicht — warte {}s", RETRY_PAUSE.as_secs());
            tokio::time::sleep(RETRY_PAUSE).await;
            continue;
        }
        // Erst als Text lesen, dann parsen: Fehlerantworten der Freistufe
        // kommen mitunter als Klartext oder leer, und `resp.json()` würde
        // daraus ein nichtssagendes „expected value at line 1 column 1"
        // machen — ohne zu verraten, was tatsächlich zurückkam.
        let raw = resp
            .text()
            .await
            .context("Antwort von GitHub Models konnte nicht gelesen werden")?;
        if !status.is_success() {
            let snippet: String = raw.chars().take(200).collect();
            bail!("GitHub Models antwortete mit HTTP {status}: {snippet}");
        }
        let body: serde_json::Value = serde_json::from_str(&raw).with_context(|| {
            format!(
                "Antwort war kein JSON (HTTP {status}): {}",
                raw.chars().take(200).collect::<String>()
            )
        })?;

        // Der Inhaltsfilter kann eine Seite ablehnen — dann liefert sie eben
        // nichts, statt den ganzen Prospekt zu kippen.
        if body
            .pointer("/choices/0/message/refusal")
            .is_some_and(|v| !v.is_null())
        {
            eprintln!("WARNUNG [Lidl] Seite vom Inhaltsfilter abgelehnt — übersprungen.");
            return Ok(Vec::new());
        }

        let text = body
            .pointer("/choices/0/message/content")
            .and_then(|t| t.as_str())
            .context("Antwort ohne Inhalt")?;
        let page: LlmPage =
            serde_json::from_str(text).context("Antwort war kein gültiges Angebots-JSON")?;
        return Ok(page.offers);
    }
    bail!("Ratenlimit auch nach Wartezeit nicht frei")
}

/// Alle Seiten mit Sternpreisen durch das Modell schicken und die Ergebnisse
/// prüfen.
pub fn extract_offers(
    pages: &[String],
    market_id: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
) -> Result<Vec<Offer>> {
    let key = api_key()?;
    let client = crate::scrapers::util::async_client()?;

    // Seiten ohne Sternpreis kosten nur Tokens.
    let candidates: Vec<(usize, String)> = pages
        .iter()
        .enumerate()
        .filter(|(_, text)| HAS_STAR_PRICE.is_match(text))
        .map(|(i, text)| (i + 1, text.clone()))
        .collect();
    println!(
        "  {} von {} Seiten tragen Sternpreise — nur die gehen ans Modell",
        candidates.len(),
        pages.len()
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Tokio-Runtime konnte nicht gestartet werden")?;

    // Nacheinander mit festem Abstand: Die Freistufe von GitHub Models
    // erlaubt nur wenige Anfragen pro Minute, und ein wöchentlicher Lauf darf
    // ruhig ein paar Minuten dauern.
    let results: Vec<(usize, String, Vec<LlmOffer>)> = runtime.block_on(async {
        let mut out = Vec::new();
        for (index, (page, text)) in candidates.iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(REQUEST_INTERVAL).await;
            }
            match extract_page(&client, &key, text).await {
                Ok(offers) => {
                    println!("  Seite {page}: {} Angebote", offers.len());
                    out.push((*page, text.clone(), offers));
                }
                // Eine gescheiterte Seite darf den Prospekt nicht kippen.
                Err(e) => eprintln!("WARNUNG [Lidl] Seite {page} übersprungen: {e:#}"),
            }
        }
        out
    });

    let mut offers = Vec::new();
    let mut seen = HashSet::new();
    let (mut proposed, mut rejected) = (0usize, 0usize);
    for (page, text, raw_offers) in &results {
        for raw in raw_offers {
            proposed += 1;
            match build_offer(raw, text, *page, market_id, valid_from, valid_until) {
                Some(offer) if seen.insert(offer.id.clone()) => offers.push(offer),
                Some(_) => {}
                None => rejected += 1,
            }
        }
    }
    println!(
        "  {} Angebote vom Modell, {rejected} bei der Gegenprobe verworfen, {} übernommen",
        proposed,
        offers.len()
    );
    if offers.is_empty() {
        bail!("Modell hat keine prüfbaren Angebote geliefert");
    }
    Ok(offers)
}

/// Wie [`crate::scrapers::lidl_prospekt::fetch_offers`], nur mit dem Modell
/// als Extraktor. Filialsuche, Absatzregion, Variantenwahl, PDF und Laufzeiten
/// sind dieselben — getauscht wird nur der Schritt „Text zu Angeboten".
pub fn fetch_offers(market: &Market, zip: &str) -> Result<Vec<Offer>> {
    let (flyer, pages) = lidl_prospekt::fetch_layout_pages(zip)?;
    println!(
        "  Gültig {} bis {}",
        flyer.offer_start_date.as_deref().unwrap_or("?"),
        flyer.offer_end_date.as_deref().unwrap_or("?")
    );
    let mut offers = extract_offers(
        &pages,
        &market.id,
        flyer.offer_start_date.as_deref(),
        flyer.offer_end_date.as_deref(),
    )?;
    offers.extend(lidl_prospekt::merge_products(&flyer, &market.id, &offers));
    Ok(offers)
}

pub fn find_market(zip: &str) -> Result<Option<Market>> {
    lidl_prospekt::find_market(zip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_price_counts_as_grounded_in_every_notation_lidl_prints() {
        let page = "ARLA Kaergarden 2.49* und BARESA Pesto 0,99 und Wasser 1 l = -.68";
        assert!(price_is_grounded(2.49, page));
        assert!(price_is_grounded(0.99, page), "Komma-Schreibweise");
        assert!(price_is_grounded(0.68, page), "Lidl-Schreibweise -.68");
        // Genau das ist der Halluzinationsfall: plausibler Preis, steht aber
        // nirgends auf der Seite.
        assert!(!price_is_grounded(3.49, page));
    }

    #[test]
    fn the_description_is_cut_off_the_title() {
        // Live beobachtet: Das Modell zieht die Beschreibung in den Titel.
        assert_eq!(
            clean_title("ARLA Kaergarden XXL Gekühlt. Je 400 g 1 kg = 6.23"),
            "ARLA Kaergarden XXL"
        );
        assert_eq!(
            clean_title("BARESA Pesto Versch. Sorten. Je 190 g"),
            "BARESA Pesto"
        );
        // Ein sauberer Titel bleibt unangetastet.
        assert_eq!(clean_title("MILBONA Speisequark"), "MILBONA Speisequark");
    }

    #[test]
    fn the_response_schema_forbids_extra_fields() {
        let schema = response_schema();
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
        assert_eq!(
            schema["properties"]["offers"]["items"]["additionalProperties"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn an_invented_price_is_dropped_even_when_everything_else_looks_right() {
        let page = "MILBONA Speisequark Je 500 g 1 kg = 2.76 1.38*";
        let raw = LlmOffer {
            title: "MILBONA Speisequark".into(),
            price: 1.99, // steht nicht auf der Seite
            regular_price: None,
            quantity: Some("500 g".into()),
            base_price: Some("1 kg = 2.76".into()),
            lidl_plus: false,
        };
        assert!(build_offer(&raw, page, 3, "LIDL_1", None, None).is_none());

        let honest = LlmOffer { price: 1.38, ..raw };
        let offer = build_offer(&honest, page, 3, "LIDL_1", None, None).unwrap();
        assert_eq!(offer.price, Some(1.38));
        assert_eq!(offer.flyer_page, Some(3));
    }

    #[test]
    fn a_price_that_contradicts_the_base_price_is_dropped() {
        // 500 g zu 2.76 €/kg sind 1.38 € — 2.76 € wäre die ganze Kachel daneben.
        let page = "MILBONA Speisequark Je 500 g 1 kg = 2.76 1.38* 2.76";
        let raw = LlmOffer {
            title: "MILBONA Speisequark".into(),
            price: 2.76,
            regular_price: None,
            quantity: Some("500 g".into()),
            base_price: Some("1 kg = 2.76".into()),
            lidl_plus: false,
        };
        assert!(build_offer(&raw, page, 3, "LIDL_1", None, None).is_none());
    }
}
