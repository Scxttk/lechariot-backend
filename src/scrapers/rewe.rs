use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

use crate::models::{Branch, Market, Offer};

// Calls the `rewerse` Go CLI (https://github.com/ByteSizedMarius/rewerse-engineering)
// Requires: cert.pem and private.key in the working directory (extracted once from Rewe APK)

fn rewerse_cmd(cert: &str, key: &str) -> Command {
    let mut cmd = Command::new("rewerse");
    cmd.arg("-cert").arg(cert).arg("-key").arg(key);
    cmd
}

pub fn find_market(zip: &str, cert: &str, key: &str) -> Result<Market> {
    check_certs(cert, key)?;
    // -json ist ein globales Flag und muss VOR dem Kommando stehen (rewerse v1.2.0).
    let output = rewerse_cmd(cert, key)
        .args(["-json", "markets", "search", "-query", zip])
        .output()
        .context("rewerse CLI nicht gefunden — bitte installieren (siehe README)")?;

    if !output.status.success() {
        bail!(
            "[REWE] Markt-Lookup fehlgeschlagen (rewerse markets search -query {zip}):\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("[REWE] Markt-Lookup JSON parsen fehlgeschlagen (rewerse markets search)")?;

    // Unverändert der erste Treffer — geändert hat sich nur, dass die
    // Koordinaten nicht mehr verlorengehen: die Antwort trägt sie in
    // `location`, gelesen wurden bisher nur wwIdent und name.
    let branch = parse_branches(&raw)?
        .into_iter()
        .next()
        .context("Kein Markt für diese PLZ gefunden")?;
    Ok(branch.as_market())
}

/// Alle Filialen einer `rewerse markets search`-Antwort.
///
/// Die Suche nimmt einen freien Text (PLZ, Stadt) und liefert eine Liste mit
/// voller Adresse und Koordinaten — für „01219" am 2026-07-25 fünf Filialen,
/// keine davon in 01219, alle in 01257/01259/01277. Genau daher stammt der
/// Eintrag „REWE in 01219", den die App bisher anzeigt.
pub fn find_branches(query: &str, cert: &str, key: &str) -> Result<Vec<Branch>> {
    check_certs(cert, key)?;
    let output = rewerse_cmd(cert, key)
        .args(["-json", "markets", "search", "-query", query])
        .output()
        .context("rewerse CLI nicht gefunden — bitte installieren (siehe README)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // "no markets found" ist eine Antwort, kein Fehler.
        if stderr.contains("no markets found") {
            return Ok(Vec::new());
        }
        bail!("[REWE] Filialsuche fehlgeschlagen (query {query}):\n{stderr}");
    }

    let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("[REWE] Filialsuche JSON parsen fehlgeschlagen")?;
    parse_branches(&raw)
}

pub fn parse_branches(raw: &serde_json::Value) -> Result<Vec<Branch>> {
    let markets = raw
        .as_array()
        .or_else(|| raw.get("markets").and_then(|v| v.as_array()))
        .context("Keine Märkte in der Antwort")?;

    Ok(markets
        .iter()
        .filter_map(|market| {
            // rewerse v1.2.0 nennt die Markt-ID "wwIdent" (früher: marketId/id).
            let id = ["wwIdent", "marketId", "id"].iter().find_map(|k| {
                market.get(*k).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty())
            })?;
            let text = |key: &str| {
                market
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let name = text("name").unwrap_or_else(|| "REWE".to_string());
            Some(
                Branch::new(id, "REWE", name, "rewerse")
                    .with_address(text("street"), text("zipCode"), text("city"))
                    .with_geo(
                        market.pointer("/location/latitude").and_then(|v| v.as_f64()),
                        market.pointer("/location/longitude").and_then(|v| v.as_f64()),
                    ),
            )
        })
        .collect())
}

pub fn fetch_offers(market: &Market, cert: &str, key: &str) -> Result<Vec<Offer>> {
    check_certs(cert, key)?;
    // -json global vor das Kommando (rewerse v1.2.0).
    let output = rewerse_cmd(cert, key)
        .args(["-json", "discounts", "-market", &market.id])
        .output()
        .context("rewerse CLI nicht gefunden")?;

    if !output.status.success() {
        bail!(
            "[REWE] Angebote laden fehlgeschlagen (rewerse discounts -market {}):\n{}",
            market.id,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("[REWE] Angebote JSON parsen fehlgeschlagen (rewerse discounts)")?;

    let mut offers = parse_offers(raw, &market.id)?;
    if crate::preview::enabled() {
        offers.extend(fetch_next_week(market, cert, key));
    }
    Ok(offers)
}

/// Die Angebote der Folgewoche — aus **derselben** Antwort wie die laufenden.
///
/// `stationary-offers/<markt>` liefert `data.offers.current` **und**
/// `data.offers.next`; `rewerse discounts` wirft eine der beiden weg, je
/// nachdem, was `defaultWeek` sagt. `-raw` gibt beide heraus. Damit ist die
/// REWE-Vorschau derselbe Aufruf mit demselben Zertifikat — kein zweiter
/// Endpunkt, keine Marktwahl im Browser, nichts Neues zu authentifizieren.
///
/// Der Aufruf läuft trotzdem ein zweites Mal statt die laufende Woche auf
/// `-raw` umzustellen: Die laufende Woche soll byte-genau bleiben, was sie
/// war. Eine Vorschau darf sie nie kosten.
///
/// Gibt bei jedem Fehler eine leere Liste zurück statt `Err` — wie bei
/// ALDI Nord und Lidl.
/// Die ungefilterte Antwort des Angebots-Endpunkts (`-raw`).
///
/// `-json` liefert eine flachgeklopfte Sicht von rewerse v1.2.0 und lässt
/// dabei Felder liegen, die die API sehr wohl schickt (`overline`,
/// `priceData.regularPrice`, `rawValues.flyerPage`). Für Messungen an der
/// Quelle ist deshalb `-raw` der Zeuge, nicht `-json`.
pub fn fetch_raw(market_id: &str, cert: &str, key: &str) -> Result<serde_json::Value> {
    check_certs(cert, key)?;
    let output = rewerse_cmd(cert, key)
        .args(["-json", "discounts", "-market", market_id, "-raw"])
        .output()
        .context("rewerse CLI nicht gefunden")?;
    if !output.status.success() {
        bail!(
            "[REWE] Rohantwort laden fehlgeschlagen (rewerse discounts -market {market_id} -raw):\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("[REWE] Rohantwort ist kein JSON")
}

pub fn fetch_next_week(market: &Market, cert: &str, key: &str) -> Vec<Offer> {
    let output = match rewerse_cmd(cert, key)
        .args(["-json", "discounts", "-market", &market.id, "-raw"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            eprintln!(
                "WARNUNG [REWE] Vorschau übersprungen (rewerse -raw): {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return Vec::new();
        }
        Err(e) => {
            eprintln!("WARNUNG [REWE] Vorschau übersprungen (rewerse nicht aufrufbar): {e}");
            return Vec::new();
        }
    };

    let raw: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("WARNUNG [REWE] Vorschau übersprungen (JSON): {e}");
            return Vec::new();
        }
    };

    let today = chrono::Utc::now()
        .with_timezone(&chrono_tz::Europe::Berlin)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let offers = parse_next_week_offers(&raw, &market.id, &today);
    println!("  Vorschau: {} Angebote der Folgewoche", offers.len());
    offers
}

/// `data.offers.next` in Angebote übersetzen.
///
/// Kein `Result`: Baut REWE den Block um, soll der Lauf die laufende Woche
/// trotzdem hochladen.
///
/// Zwei Bedingungen, und beide sind nötig. `available` sagt, ob REWE die
/// Folgewoche überhaupt schon freigegeben hat (`nextWeekAvailableFrom` steht
/// auf „saturday"). Und `fromDate` muss **nach heute** liegen — am Samstag
/// selbst kann `next` bereits die Woche sein, die morgen läuft; ohne diese
/// Prüfung stünde ein Preis der laufenden Woche in der Vorschau.
pub fn parse_next_week_offers(
    raw: &serde_json::Value,
    market_id: &str,
    today: &str,
) -> Vec<Offer> {
    let Some(next) = raw.pointer("/data/offers/next") else {
        eprintln!("WARNUNG [REWE] Vorschau: data.offers.next fehlt.");
        return Vec::new();
    };
    if next.get("available").and_then(|v| v.as_bool()) != Some(true) {
        return Vec::new();
    }
    let from = next.get("fromDate").and_then(|v| v.as_str()).unwrap_or_default();
    if from.is_empty() || from <= today {
        return Vec::new();
    }
    let until = next.get("untilDate").and_then(|v| v.as_str());

    let mut offers = Vec::new();
    for cat in next.get("categories").and_then(|v| v.as_array()).into_iter().flatten() {
        let category = cat.get("title").and_then(|v| v.as_str()).map(String::from);
        for raw_offer in cat.get("offers").and_then(|v| v.as_array()).into_iter().flatten() {
            // Nur echte Angebotskacheln; alles andere sind Banner und Bühnen.
            if raw_offer.get("cellType").and_then(|v| v.as_str()) != Some("DEFAULT") {
                continue;
            }
            let Some(title) = raw_offer.get("title").and_then(|v| v.as_str()) else {
                continue;
            };
            let text = |key: &str| {
                raw_offer.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
            };
            let price_at = |path: &str| {
                raw_offer.pointer(path).and_then(|v| v.as_str()).and_then(parse_price_str)
            };

            offers.push(Offer {
                id: Offer::build_id(market_id, title, Some(from)),
                market_id: market_id.to_string(),
                title: title.to_string(),
                subtitle: text("subtitle"),
                // Die Marke steht bei REWE im Overline — sie muss mit, sonst
                // fehlt sie dem Matcher (siehe [EXTRAKT-1]).
                overline: text("overline"),
                price: price_at("/priceData/price"),
                // `regularPrice` trägt auch Etiketten wie „Knaller"; was sich
                // nicht als Preis lesen lässt, bleibt leer.
                regular_price: price_at("/priceData/regularPrice"),
                category: category.clone(),
                nutri_score: raw_offer
                    .pointer("/detail/nutriScore")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from),
                valid_from: Some(from.to_string()),
                valid_until: until.map(String::from),
                images: raw_offer
                    .get("images")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).map(String::from).collect())
                    .unwrap_or_default(),
                biozid: raw_offer.get("biozid").and_then(|v| v.as_bool()).unwrap_or(false),
                flyer_page: raw_offer.pointer("/rawValues/flyerPage").and_then(|v| v.as_i64()),
            });
        }
    }
    offers
}

fn check_certs(cert: &str, key: &str) -> Result<()> {
    if !Path::new(cert).exists() {
        bail!("Zertifikat nicht gefunden: {cert}\nBitte Einrichtung in README befolgen.");
    }
    if !Path::new(key).exists() {
        bail!("Private Key nicht gefunden: {key}\nBitte Einrichtung in README befolgen.");
    }
    Ok(())
}

pub fn parse_offers(raw: serde_json::Value, market_id: &str) -> Result<Vec<Offer>> {
    let mut offers = Vec::new();

    // rewerse v1.2.0 discounts -json liefert:
    //   { "validUntil": "2026-07-18T00:00:00Z",
    //     "categories": [ { "id", "title", "index",
    //       "offers": [ { "title", "subtitle", "images", "priceRaw", "price",
    //                     "priceParseFail", "nutriScore", ... } ] } ] }
    // Kein current/next-Wrapper, kein fromDate/regularPrice/overline/flyerPage/biozid mehr.
    let valid_until = raw
        .get("validUntil")
        .and_then(|v| v.as_str())
        // Zeitanteil abschneiden: "2026-07-18T00:00:00Z" -> "2026-07-18".
        .map(|s| s.split('T').next().unwrap_or(s).to_string());

    // Die v1.2.0-API liefert kein Startdatum mehr — valid_from ist eine Ableitung:
    // Rewe-Wochenangebote gelten Mo–Sa, also Montag der Woche von validUntil.
    // Ohne validUntil: laufende Woche (heute in Europe/Berlin) als Fallback.
    let (valid_from, valid_until) = match valid_until
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
    {
        Some(until) => (
            Some(monday_of_week(until).format("%Y-%m-%d").to_string()),
            valid_until,
        ),
        None => {
            let today = chrono::Utc::now().with_timezone(&chrono_tz::Europe::Berlin).date_naive();
            let (mon, sat) = week_bounds(today);
            (
                Some(mon.format("%Y-%m-%d").to_string()),
                Some(sat.format("%Y-%m-%d").to_string()),
            )
        }
    };

    let categories = raw
        .get("categories")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for cat in &categories {
        // Kategorie-Titel sind echte Produktgruppen ("Obst und Gemüse", "Kühlung",
        // "Drogerie", ...) und dienen der Kategorisierung in enrich.rs.
        let category = cat.get("title").and_then(|v| v.as_str()).map(String::from);
        let raw_offers = cat
            .get("offers")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for raw_offer in &raw_offers {
            let title = match raw_offer.get("title").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => continue,
            };

            let subtitle = raw_offer.get("subtitle").and_then(|v| v.as_str()).map(String::from);

            // rewerse parst den Preis bereits als Float (price); bei priceParseFail
            // (price == 0) auf den Rohtext (priceRaw, z. B. "2,22 €") zurückfallen.
            let price = raw_offer
                .get("price")
                .and_then(|v| v.as_f64())
                .filter(|p| *p > 0.0)
                .or_else(|| {
                    raw_offer.get("priceRaw").and_then(|v| v.as_str()).and_then(parse_price_str)
                });

            // nutriScore ist häufig "" -> als None behandeln.
            let nutri_score = raw_offer
                .get("nutriScore")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);

            let images: Vec<String> = raw_offer
                .get("images")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).map(String::from).collect())
                .unwrap_or_default();

            let id = Offer::build_id(market_id, &title, valid_until.as_deref());

            offers.push(Offer {
                id,
                market_id: market_id.to_string(),
                title,
                subtitle,
                overline: None,
                price,
                regular_price: None,
                category: category.clone(),
                nutri_score,
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

/// Montag der ISO-Woche, in der `date` liegt (Rewe-Angebote gelten Mo–Sa).
pub fn monday_of_week(date: chrono::NaiveDate) -> chrono::NaiveDate {
    use chrono::Datelike;
    date - chrono::Days::new(u64::from(date.weekday().num_days_from_monday()))
}

/// Fallback ohne API-Datum: (Montag, Samstag) der Woche von `today`.
pub fn week_bounds(today: chrono::NaiveDate) -> (chrono::NaiveDate, chrono::NaiveDate) {
    let mon = monday_of_week(today);
    (mon, mon + chrono::Days::new(5))
}

fn parse_price_str(s: &str) -> Option<f64> {
    // "2,22 €" / "1.299,00 €" -> Float: alles außer Ziffern/Komma/Punkt entfernen,
    // dann deutschen Tausenderpunkt weg und Komma zum Dezimalpunkt machen.
    let cleaned: String =
        s.chars().filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
    cleaned.replace('.', "").replace(',', ".").parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::{monday_of_week, week_bounds};
    use chrono::NaiveDate;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn montag_bleibt_montag() {
        assert_eq!(monday_of_week(d("2026-07-13")), d("2026-07-13"));
    }

    #[test]
    fn sonntag_gehoert_zur_selben_iso_woche() {
        assert_eq!(monday_of_week(d("2026-07-19")), d("2026-07-13"));
    }

    #[test]
    fn jahreswechsel() {
        // 2027-01-01 ist ein Freitag -> Montag 2026-12-28.
        assert_eq!(monday_of_week(d("2027-01-01")), d("2026-12-28"));
    }

    #[test]
    fn week_bounds_montag_bis_samstag() {
        assert_eq!(week_bounds(d("2026-07-15")), (d("2026-07-13"), d("2026-07-18")));
    }
}
