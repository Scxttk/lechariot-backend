//! Upload gespeicherter Angebote in die Supabase-Tabelle `public.offers`
//! (PostgREST-API). Ersetzt den Python-Uploader `supabase_uploader.py`:
//! Upsert mit on_conflict=market,product,valid_from,region, Batches à 100,
//! vorheriges Löschen veralteter Wochen pro Markt, Region-Cache in
//! `public.regions`.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::models::{Market, Offer};
use crate::{db, enrich, storage, units};

pub const BATCH_SIZE: usize = 100;

/// Zeile im Supabase-Schema (schema.sql + migration_v2.sql + migration_regions.sql).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupabaseRow {
    pub market: String,
    pub product: String,
    pub price: f64,
    pub regular_price: Option<f64>,
    pub unit: String,
    pub category: Option<String>,
    pub emoji: Option<String>,
    pub image_url: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub base_price: Option<f64>,
    pub base_unit: Option<String>,
    pub brand: Option<String>,
    pub ean: Option<String>,
    pub source: String,
    pub region: Option<String>,
    /// Begriffs-Tags aus matching.rs (migration_v10); ["nonfood"] = Non-Food,
    /// leer = ungetaggt (Kandidat für die Review-Liste). default: die
    /// Region-Kopie in sync.rs liest Bestandszeilen, die vor der Migration
    /// gepusht wurden und das Feld noch nicht tragen.
    #[serde(default)]
    pub match_key: Vec<String>,
}

/// Ketten-Anzeigename für einen gespeicherten Markt. Maßgeblich ist die Kette,
/// die `stores::scrape_store` am Markt hinterlegt hat; nur Markt-Zeilen aus
/// lokalen DBs vor Schema v5 tragen sie nicht und werden über ID und
/// Filialnamen zugeordnet. Filialen ohne erkennbare Kette liefern None und
/// werden beim Push übersprungen.
pub fn chain_for(market: &Market) -> Option<&'static str> {
    use crate::stores::Store;
    if let Some(store) = market.chain.as_deref().and_then(Store::from_chain) {
        return Some(store.chain());
    }
    chain_from_name(market)
}

/// Fallback-Heuristik für Altbestand ohne `markets.chain`.
///
/// Die ALDI-Zuordnung hängt am ID-Präfix, das der Store-Finder garantiert
/// setzt (`ALDI_NORD_…` / `ALDI_SUED_…`, nationale Platzhalter eingeschlossen).
/// Der Filialname taugt dafür nicht: er trägt den Stadtteil, und ein freies
/// `contains("nord")` hat "ALDI SÜD Köln-Altstadt-Nord" als ALDI Nord
/// eingestuft — der komplette SÜD-Katalog landete unter der falschen Kette.
/// Deshalb hier nur die Kette als Präfix des Namens, nicht irgendwo darin.
fn chain_from_name(market: &Market) -> Option<&'static str> {
    use crate::stores::Store;
    let id = market.id.to_lowercase();
    let name = market.name.to_lowercase();
    let hay = format!("{id} {name}");
    if hay.contains("aldi") {
        if id.starts_with("aldi_nord") || name.starts_with("aldi nord") {
            return Some(Store::AldiNord.chain());
        }
        if id.starts_with("aldi_sued")
            || name.starts_with("aldi süd")
            || name.starts_with("aldi sued")
        {
            return Some(Store::AldiSued.chain());
        }
        // "aldi" ohne erkennbare Gesellschaft: lieber überspringen als raten.
        return None;
    }
    // Kanonischer Name kommt IMMER aus Store::chain() — `offers.market` muss
    // exakt zu markets.chain passen, die App filtert mit market=in.(…).
    let chains = [
        ("rewe", Store::Rewe.chain()),
        ("penny", Store::Penny.chain()),
        ("kaufland", Store::Kaufland.chain()),
        ("lidl", Store::Lidl.chain()),
        ("netto", Store::Netto.chain()),
        ("edeka", Store::Edeka.chain()),
        // EDEKA-Vertriebsmarken tragen "edeka" nicht immer im Namen
        ("e center", Store::Edeka.chain()),
        ("e-center", Store::Edeka.chain()),
        ("e neukauf", Store::Edeka.chain()),
        ("e aktiv", Store::Edeka.chain()),
        ("e xpress", Store::Edeka.chain()),
        ("marktkauf", Store::Edeka.chain()),
        ("nah & gut", Store::Edeka.chain()),
        ("nah und gut", Store::Edeka.chain()),
    ];
    chains.iter().find(|(n, _)| hay.contains(n)).map(|(_, c)| *c)
}

// Produktname: Titel, plus Untertitel wenn er echte Zusatzinfo trägt
// (kein reiner Mengen-Text, nicht leer, nicht schon im Titel enthalten).
fn product_name(offer: &Offer) -> String {
    match &offer.subtitle {
        Some(sub)
            if !sub.is_empty()
                && !is_pure_quantity(sub)
                && !offer.title.to_lowercase().contains(&sub.to_lowercase()) =>
        {
            format!("{} {}", offer.title, sub)
        }
        _ => offer.title.clone(),
    }
}

// Ein Untertitel ist nur dann reiner Mengen-Text, wenn er außer Zahlen nichts
// als Einheiten-/Verpackungswörter enthält ("je 250-g-Packg."). Texte wie
// "Rispentomaten, 500-g-Schale" tragen den Produktnamen und müssen erhalten
// bleiben — units::parse_quantity findet auch dort eine Menge und reicht als
// Kriterium deshalb nicht.
fn is_pure_quantity(text: &str) -> bool {
    const FILLER: &[&str] = &[
        "je", "ca", "x", "g", "kg", "mg", "ml", "cl", "l", "stück", "stk", "er", "packg",
        "packung", "pack", "dose", "flasche", "beutel", "schale", "netz", "becher", "glas",
        "tafel", "riegel", "rolle", "portion",
    ];
    text.to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .filter(|t| !t.is_empty())
        .all(|t| FILLER.contains(&t))
}

/// Lokales Angebot in eine Supabase-Zeile mappen. None bei Angeboten ohne
/// Preis — die kann die App nicht anzeigen.
pub fn map_offer(offer: &Offer, chain: &str, region: Option<&str>) -> Option<SupabaseRow> {
    let price = offer.price?;
    let enriched =
        enrich::enrich(&offer.title, offer.subtitle.as_deref(), offer.category.as_deref());
    let unit_price = units::derive_unit_price(
        offer.price,
        &[offer.subtitle.as_deref(), offer.overline.as_deref(), Some(&offer.title)],
    );
    Some(SupabaseRow {
        market: chain.to_string(),
        product: product_name(offer),
        price,
        regular_price: offer.regular_price,
        // Mengen-Untertitel ("je 12 x 1 l") wandert ins unit-Feld — sonst
        // wirkt ein Multipack-Preis in der App wie ein Einzelpreis.
        unit: match &offer.subtitle {
            Some(s) if !s.is_empty() && is_pure_quantity(s) => s.clone(),
            _ => "Stück".to_string(),
        },
        category: Some(enriched.category.to_string()),
        emoji: Some(enriched.emoji.to_string()),
        // Erste echte Bild-URL vom Scraper; Emoji bleibt als Fallback erhalten.
        image_url: offer.images.iter().find(|u| !u.is_empty()).cloned(),
        valid_from: offer.valid_from.clone(),
        valid_until: offer.valid_until.clone(),
        base_price: unit_price.map(|up| (up.eur * 100.0).round() / 100.0),
        base_unit: unit_price.map(|up| up.unit.label().to_string()),
        brand: None,
        ean: None,
        source: "smartshop-rust".to_string(),
        region: region.map(String::from),
        match_key: crate::matching::match_keys(
            &offer.title,
            offer.subtitle.as_deref(),
            offer.category.as_deref(),
        ),
    })
}

/// Duplikate auf dem Upsert-Schlüssel (market, product, valid_from, region)
/// entfernen; der erste Treffer gewinnt.
pub fn dedupe_rows(rows: Vec<SupabaseRow>) -> Vec<SupabaseRow> {
    let mut seen = HashSet::new();
    rows.into_iter()
        .filter(|r| {
            seen.insert((
                r.market.clone(),
                r.product.clone(),
                r.valid_from.clone(),
                r.region.clone(),
            ))
        })
        .collect()
}

/// Zeile für `public.price_history` (migration_v7): nur die Preis-relevanten
/// Spalten einer SupabaseRow; `recorded_at` setzt die Datenbank.
#[derive(Debug, Serialize)]
struct HistoryRow<'a> {
    market: &'a str,
    product: &'a str,
    region: Option<&'a str>,
    price: f64,
    regular_price: Option<f64>,
    base_price: Option<f64>,
    base_unit: Option<&'a str>,
    unit: &'a str,
    category: Option<&'a str>,
    valid_from: Option<&'a str>,
    valid_until: Option<&'a str>,
}

impl<'a> From<&'a SupabaseRow> for HistoryRow<'a> {
    fn from(r: &'a SupabaseRow) -> Self {
        HistoryRow {
            market: &r.market,
            product: &r.product,
            region: r.region.as_deref(),
            price: r.price,
            regular_price: r.regular_price,
            base_price: r.base_price,
            base_unit: r.base_unit.as_deref(),
            unit: &r.unit,
            category: r.category.as_deref(),
            valid_from: r.valid_from.as_deref(),
            valid_until: r.valid_until.as_deref(),
        }
    }
}

pub struct PushConfig {
    pub base_url: String,
    pub api_key: String,
}

pub fn config_from_env() -> Result<PushConfig> {
    let base_url = std::env::var("SUPABASE_URL")
        .context("Umgebungsvariable SUPABASE_URL fehlt (z. B. https://xyz.supabase.co)")?;
    let api_key = std::env::var("SUPABASE_SERVICE_KEY")
        .context("Umgebungsvariable SUPABASE_SERVICE_KEY fehlt (Service-Role-Key, nicht der anon key)")?;
    Ok(PushConfig { base_url: base_url.trim_end_matches('/').to_string(), api_key })
}

pub struct PushOptions {
    pub db_path: String,
    /// Nur diese Kette pushen (Anzeigename, z. B. "REWE"); None = alle.
    pub chain: Option<String>,
    /// PLZ, aus der die Angebote stammen. Pflicht außer bei --dry-run.
    pub region: Option<String>,
    pub dry_run: bool,
    /// Produktbilder in den Supabase-Storage-Bucket spiegeln (Händler-URL ->
    /// Bucket-URL). Bei --dry-run wird ohnehin nicht gespiegelt.
    pub mirror_images: bool,
    /// Bilder erst NACH dem Offers-Upsert spiegeln (On-Demand-Pfad): Phase 1
    /// upsertet sofort mit Händler-URL bzw. null (Emoji-Fallback der App),
    /// Phase 2 spiegelt und upsertet die gespiegelten Zeilen erneut. So sind
    /// Angebote in Sekunden sichtbar statt hinter Hunderten Bild-Uploads.
    pub defer_mirror: bool,
}

/// Upload-fertige Zeilen einer Kette plus Zähler der übersprungenen Angebote.
pub struct ChainRows {
    pub rows: Vec<SupabaseRow>,
    /// Angebote ohne Preis — die kann die App nicht anzeigen.
    pub no_price: usize,
    /// Angebote ohne valid_from — die App filtert sie serverseitig weg und
    /// der Upsert-Schlüssel (market, product, valid_from, region) ist nicht
    /// NULL-sicher, jeder Lauf würde sie duplizieren.
    pub no_date: usize,
}

/// Angebote aus der lokalen DB laden und pro Kette gruppieren (gemappt,
/// dedupliziert; Angebote ohne Preis oder ohne valid_from werden mitgezählt,
/// aber übersprungen).
fn load_grouped(opts: &PushOptions) -> Result<BTreeMap<&'static str, ChainRows>> {
    let conn = db::open(&opts.db_path)?;
    let markets = db::markets(&conn)?;
    let offers = db::export_offers(&conn, None)?;

    let chain_by_market: BTreeMap<&str, &'static str> = markets
        .iter()
        .filter_map(|m| chain_for(m).map(|c| (m.id.as_str(), c)))
        .collect();

    let mut groups: BTreeMap<&'static str, (Vec<Offer>, usize)> = BTreeMap::new();
    for offer in offers {
        let Some(&chain) = chain_by_market.get(offer.market_id.as_str()) else {
            continue;
        };
        if let Some(want) = &opts.chain {
            if chain != want {
                continue;
            }
        }
        let entry = groups.entry(chain).or_default();
        if offer.price.is_none() {
            entry.1 += 1;
        } else {
            entry.0.push(offer);
        }
    }

    Ok(groups
        .into_iter()
        .map(|(chain, (offers, no_price))| {
            let mapped: Vec<SupabaseRow> = offers
                .iter()
                .filter_map(|o| map_offer(o, chain, opts.region.as_deref()))
                .collect();
            let (rows, dateless): (Vec<_>, Vec<_>) =
                mapped.into_iter().partition(|r| r.valid_from.is_some());
            let no_date = dateless.len();
            if no_date > 0 {
                eprintln!(
                    "WARNUNG [{chain}] {no_date} Angebote ohne valid_from — nicht hochgeladen."
                );
            }
            (chain, ChainRows { rows: dedupe_rows(rows), no_price, no_date })
        })
        .collect())
}

fn skipped_note(g: &ChainRows) -> String {
    format!("{} ohne Preis, {} ohne Datum übersprungen", g.no_price, g.no_date)
}

fn dry_run_report(groups: &BTreeMap<&'static str, ChainRows>) -> Result<()> {
    println!("Dry-Run — es wird nichts hochgeladen.");
    for (chain, g) in groups {
        println!("  [{chain}] {} Zeilen ({})", g.rows.len(), skipped_note(g));
    }
    let samples: Vec<&SupabaseRow> = groups.values().flat_map(|g| &g.rows).take(3).collect();
    if !samples.is_empty() {
        println!("Beispiel-Zeilen:");
        println!("{}", serde_json::to_string_pretty(&samples)?);
    }
    Ok(())
}

fn check_response(what: &str, resp: reqwest::blocking::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().unwrap_or_default();
    let excerpt: String = body.chars().take(300).collect();
    bail!("{what} fehlgeschlagen (HTTP {status}): {excerpt}");
}

/// Händler-Bild-URLs in den Storage-Bucket spiegeln und die Zeilen auf die
/// Bucket-URL umschreiben. Bekannte Bilder liefert der lokale Cache ohne
/// Netzwerkzugriff; Fehler eines einzelnen Bildes lassen die Händler-URL stehen
/// (Emoji bleibt der letzte Fallback) und brechen den Push nicht ab.
/// Liefert (neu gespiegelt, aus Cache, fehlgeschlagen).
fn mirror_images(
    groups: &mut BTreeMap<&'static str, ChainRows>,
    cfg: &PushConfig,
    db_path: &str,
    client: &reqwest::blocking::Client,
) -> Result<(usize, usize, usize)> {
    let conn = db::open(db_path)?;
    let (mut fresh, mut cached, mut failed) = (0usize, 0usize, 0usize);

    for (_chain, g) in groups.iter_mut() {
        for row in g.rows.iter_mut() {
            let Some(src) = row.image_url.clone() else { continue };
            // Schon eine Bucket-URL (z. B. erneuter Lauf ohne DB-Cache-Treffer)?
            if src.contains(&format!("/{}/", storage::BUCKET)) {
                continue;
            }
            if let Some(hit) = db::cached_image_url(&conn, &src)? {
                row.image_url = Some(hit);
                cached += 1;
                continue;
            }
            match storage::mirror(client, cfg, &src) {
                Ok(public) => {
                    db::cache_image_url(&conn, &src, &public)?;
                    row.image_url = Some(public);
                    fresh += 1;
                }
                Err(e) => {
                    eprintln!("  Bild übersprungen ({src}): {e}");
                    failed += 1;
                }
            }
        }
    }
    Ok((fresh, cached, failed))
}

/// Alle gepushten Zeilen zusätzlich in `public.price_history` upserten
/// (Wochen-Schnappschuss für Preisverlaufs-Charts). Zeilen ohne Preis kommen
/// hier nie an — map_offer filtert sie bereits. Liefert die Zeilenzahl.
fn push_history(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    rows: &[&SupabaseRow],
) -> Result<usize> {
    let url = format!("{}/rest/v1/price_history", cfg.base_url);
    for batch in rows.chunks(BATCH_SIZE) {
        let payload: Vec<HistoryRow> = batch.iter().map(|r| HistoryRow::from(*r)).collect();
        let resp = client
            .post(&url)
            .header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .query(&[("on_conflict", "market,product,region,valid_from")])
            .header("Prefer", "resolution=merge-duplicates")
            .json(&payload)
            .send()
            .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
        check_response(&format!("Preis-Historie: Upsert von {} Zeilen", payload.len()), resp)?;
    }
    Ok(rows.len())
}

/// Filialen nach `public.branches` upserten (Konfliktschlüssel `market_id`,
/// siehe `supabase/migration_v12_branches.sql`).
///
/// Anders als die Preis-Historie ist das **kein** Best-Effort: Wenn das
/// Verzeichnis nicht geschrieben werden kann, hat der Lauf sein einziges Ziel
/// verfehlt und soll das auch melden.
pub fn upsert_branches(cfg: &PushConfig, branches: &[crate::models::Branch]) -> Result<()> {
    #[derive(Serialize)]
    struct BranchRow<'a> {
        market_id: &'a str,
        chain: &'a str,
        name: &'a str,
        street: Option<&'a str>,
        plz: Option<&'a str>,
        city: Option<&'a str>,
        lat: Option<f64>,
        lon: Option<f64>,
        source: &'a str,
        updated_at: String,
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("{}/rest/v1/branches", cfg.base_url);
    let now = chrono::Utc::now().to_rfc3339();
    for batch in branches.chunks(BATCH_SIZE) {
        let payload: Vec<BranchRow> = batch
            .iter()
            .map(|b| BranchRow {
                market_id: &b.market_id,
                chain: &b.chain,
                name: &b.name,
                street: b.street.as_deref(),
                plz: b.plz.as_deref(),
                city: b.city.as_deref(),
                lat: b.lat,
                lon: b.lon,
                source: &b.source,
                updated_at: now.clone(),
            })
            .collect();
        let resp = client
            .post(&url)
            .header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .query(&[("on_conflict", "market_id")])
            .header("Prefer", "resolution=merge-duplicates")
            .json(&payload)
            .send()
            .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
        check_response(&format!("Upsert von {} Filialen", payload.len()), resp)?;
    }
    Ok(())
}

/// Zeilen in Batches nach `public.offers` upserten (Konfliktschlüssel wie
/// Schema: market, product, valid_from, region).
pub fn upsert_offer_rows(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    rows: &[SupabaseRow],
    what: &str,
) -> Result<()> {
    let url = format!("{}/rest/v1/offers", cfg.base_url);
    for batch in rows.chunks(BATCH_SIZE) {
        let resp = client
            .post(&url)
            .header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .query(&[("on_conflict", "market,product,valid_from,region")])
            .header("Prefer", "resolution=merge-duplicates")
            .json(batch)
            .send()
            .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
        check_response(&format!("{what}: Upsert von {} Angeboten", batch.len()), resp)?;
    }
    Ok(())
}

pub fn run(opts: &PushOptions, cfg: Option<&PushConfig>) -> Result<()> {
    let mut groups = load_grouped(opts)?;
    if groups.is_empty() {
        println!("Keine passenden Angebote in '{}' gefunden.", opts.db_path);
        return Ok(());
    }

    if opts.dry_run {
        return dry_run_report(&groups);
    }

    let Some(region) = opts.region.as_deref() else {
        bail!("--region <PLZ> ist Pflicht (außer bei --dry-run): die App filtert Angebote pro Region.");
    };
    let cfg = match cfg {
        Some(c) => c,
        None => &config_from_env()?,
    };

    let client = reqwest::blocking::Client::new();

    // Eigener Client fürs Bild-Spiegeln: folgt bewusst KEINEN Redirects, damit
    // ein 3xx einer Händler-URL nicht auf ein internes Ziel umgelenkt werden
    // kann (SSRF). Der reguläre `client` für die Supabase-REST-Aufrufe bleibt
    // unverändert (folgt Redirects wie bisher).
    let mirror_client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("HTTP-Client fürs Bild-Spiegeln konnte nicht erstellt werden")?;

    // Produktbilder in den Storage-Bucket spiegeln, bevor die Zeilen hochgeladen
    // werden — so trägt image_url die stabile Bucket-URL statt der Händler-URL.
    // Im defer_mirror-Modus passiert das erst NACH dem Upsert (Phase 2 unten).
    if opts.mirror_images && !opts.defer_mirror {
        let (fresh, cached, failed) = mirror_images(&mut groups, cfg, &opts.db_path, &mirror_client)?;
        println!("Bilder: {fresh} neu gespiegelt, {cached} aus Cache, {failed} fehlgeschlagen.");
    }
    let offers_url = format!("{}/rest/v1/offers", cfg.base_url);
    let auth = |req: reqwest::blocking::RequestBuilder| {
        req.header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
    };

    let mut total = 0usize;
    for (chain, g) in &groups {
        let rows = &g.rows;
        if rows.is_empty() {
            eprintln!(
                "WARNUNG [{chain}] 0 Angebote hochladbar ({}) — Kette bleibt in dieser Region leer.",
                skipped_note(g)
            );
            continue;
        }

        // Veraltete Wochen dieses Markts in DIESER Region löschen — nur wenn
        // neue Daten mit Gültigkeitsdatum vorliegen. Legacy-Zeilen ohne
        // valid_from (alte Scraper-Läufe) matchen `lt.` nie und müssen
        // explizit mit weg. Der Region-Filter verhindert, dass der Push einer
        // Region die noch nicht neu gesyncten Wochen anderer Regionen löscht.
        if let Some(current) = rows.iter().filter_map(|r| r.valid_from.as_deref()).max() {
            let resp = auth(client.delete(&offers_url))
                .query(&[
                    ("market", format!("eq.{chain}")),
                    ("region", format!("eq.{region}")),
                    ("or", format!("(valid_from.lt.{current},valid_from.is.null)")),
                ])
                .send()
                .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
            check_response(&format!("[{chain}] Löschen veralteter Angebote"), resp)?;
        }

        upsert_offer_rows(&client, cfg, rows, &format!("[{chain}]"))?;
        total += rows.len();
        println!("  [{chain}] {} Angebote hochgeladen ({}).", rows.len(), skipped_note(g));
    }

    // Preis-Historie best-effort mitschreiben: Fehler (z. B. Tabelle noch
    // nicht migriert) warnen nur und lassen den Offers-Push erfolgreich.
    let history_rows: Vec<&SupabaseRow> = groups.values().flat_map(|g| &g.rows).collect();
    if !history_rows.is_empty() {
        match push_history(&client, cfg, &history_rows) {
            Ok(n) => println!("Preis-Historie: {n} Zeilen upsertet."),
            Err(e) => eprintln!("WARNUNG: Preis-Historie fehlgeschlagen: {e:#}"),
        }
    }

    // Region-Cache aktualisieren: diese PLZ wurde soeben gesynct.
    let resp = auth(client.post(format!("{}/rest/v1/regions", cfg.base_url)))
        .query(&[("on_conflict", "plz")])
        .header("Prefer", "resolution=merge-duplicates")
        .json(&serde_json::json!([{
            "plz": region,
            "last_synced": chrono::Utc::now().to_rfc3339(),
        }]))
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    check_response(&format!("Region {region} eintragen"), resp)?;

    println!("Fertig: {total} Angebote nach Supabase gepusht (Region {region}).");

    // Phase 2 (defer_mirror): Bilder jetzt spiegeln und die betroffenen Zeilen
    // erneut upserten — die App zeigt die Angebote längst, hier kommen nur noch
    // die stabilen Bucket-URLs nach.
    if opts.mirror_images && opts.defer_mirror {
        let (fresh, cached, failed) = mirror_images(&mut groups, cfg, &opts.db_path, &mirror_client)?;
        println!("Bilder (Phase 2): {fresh} neu gespiegelt, {cached} aus Cache, {failed} fehlgeschlagen.");
        let bucket_marker = format!("/{}/", storage::BUCKET);
        let mirrored: Vec<SupabaseRow> = groups
            .values()
            .flat_map(|g| &g.rows)
            .filter(|r| r.image_url.as_deref().is_some_and(|u| u.contains(&bucket_marker)))
            .cloned()
            .collect();
        if !mirrored.is_empty() {
            upsert_offer_rows(&client, cfg, &mirrored, "Bild-Nachtrag")?;
            println!("Bild-Nachtrag: {} Zeilen mit Bucket-URLs aktualisiert.", mirrored.len());
        }
    }
    Ok(())
}
