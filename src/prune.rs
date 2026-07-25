//! Aufräumen: `prune-images` für den Storage-Bucket, `prune-history` für die
//! Preis-Historie. Beide folgen derselben Regel — Standard ist Dry-Run,
//! gelöscht wird erst mit `--execute`.
//!
//! `prune-images`: verwaiste Bilder aus dem Storage-Bucket `offer-images`
//! entfernen. Angebote rotieren wöchentlich (der Push löscht veraltete
//! offer-Zeilen), ihre gespiegelten Bilder blieben aber bisher für immer im
//! Bucket liegen — der Bucket wächst unbegrenzt, während `public.offers` klein
//! bleibt. Genau das sprengt irgendwann das Storage-Kontingent.
//!
//! Ablauf: alle Bucket-Objekte auflisten, die von `public.offers.image_url`
//! noch referenzierten Pfade sammeln und nur die Objekte löschen, die kein
//! aktuelles Angebot mehr braucht. Geteilte Bilder (mehrere Angebote, gleiche
//! content-adressierte URL) bleiben unangetastet, weil mindestens eine
//! offer-Zeile sie referenziert.
//!
//! Zwei Sicherheitsnetze:
//!   * Standard ist Dry-Run — es wird nichts gelöscht, nur gezeigt. Löschen
//!     erst mit `--execute`.
//!   * Ein Altersfilter (`--min-age-days`, Standard 7) schützt frisch
//!     hochgeladene Bilder: ein gleichzeitig laufender Push/Sync lädt erst das
//!     Bild hoch und schreibt danach die offer-Zeile — ohne den Filter könnte
//!     ein gerade gespiegeltes Bild im Zwischenmoment als „verwaist“ gelöscht
//!     werden.
//!
//! Braucht `SUPABASE_URL` und `SUPABASE_SERVICE_KEY` (Service-Role-Key) in der
//! Umgebung — dieselben Variablen wie `push`/`sync-regions`.

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::push::{PushConfig, config_from_env};
use crate::storage::BUCKET;

/// PostgREST-Seitengröße beim Einlesen der referenzierten Bild-URLs.
const OFFERS_PAGE: usize = 1000;
/// Storage-List-Seitengröße.
const LIST_PAGE: usize = 1000;
/// Objekte pro Batch-Delete-Request.
const DELETE_BATCH: usize = 100;

pub struct PruneOptions {
    /// false = Dry-Run (nur zeigen, nichts löschen).
    pub execute: bool,
    /// Objekte, die jünger als so viele Tage sind, werden nie gelöscht.
    pub min_age_days: i64,
}

#[derive(Deserialize)]
struct OfferImage {
    image_url: Option<String>,
}

#[derive(Deserialize)]
struct StorageObject {
    name: String,
    /// Echte Objekte tragen eine id; ein `null` markiert einen Ordner-/Prefix-
    /// Platzhalter, den wir nicht als löschbares Objekt behandeln.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    metadata: Option<ObjectMetadata>,
}

#[derive(Deserialize)]
struct ObjectMetadata {
    #[serde(default)]
    size: Option<u64>,
}

fn obj_size(o: &StorageObject) -> Option<u64> {
    o.metadata.as_ref().and_then(|m| m.size)
}

pub fn run(opts: &PruneOptions, cfg: Option<&PushConfig>) -> Result<()> {
    let owned;
    let cfg = match cfg {
        Some(c) => c,
        None => {
            owned = config_from_env()?;
            &owned
        }
    };
    let client = reqwest::blocking::Client::new();

    let mode = if opts.execute { "LÖSCHEN" } else { "Dry-Run" };
    println!(
        "prune-images ({mode}) auf Bucket '{BUCKET}', Altersschutz {} Tage.\n",
        opts.min_age_days
    );

    // 1. Von offers referenzierte Bucket-Pfade sammeln.
    let referenced = referenced_paths(&client, cfg)?;
    println!(
        "Referenziert: {} Bild(er) über public.offers.image_url.",
        referenced.len()
    );

    // 2. Alle Objekte im Bucket auflisten.
    let objects = list_bucket_objects(&client, cfg)?;
    let total_bytes: u64 = objects.iter().filter_map(obj_size).sum();
    println!(
        "Im Bucket: {} Objekt(e) ({}).",
        objects.len(),
        human_size(total_bytes)
    );

    // 3. Verwaiste bestimmen: nicht referenziert. Junge Objekte schützt der
    //    Altersfilter (evtl. gerade von einem laufenden Push gespiegelt).
    let cutoff = Utc::now() - Duration::days(opts.min_age_days.max(0));
    let mut orphans: Vec<&StorageObject> = Vec::new();
    let mut protected = 0usize;
    let mut protected_bytes = 0u64;
    for o in &objects {
        if referenced.contains(&o.name) {
            continue;
        }
        let young = match o.created_at {
            Some(ts) => ts > cutoff,
            None => true, // ohne Zeitstempel auf Nummer sicher: behalten
        };
        if young {
            protected += 1;
            protected_bytes += obj_size(o).unwrap_or(0);
            continue;
        }
        orphans.push(o);
    }
    let orphan_bytes: u64 = orphans.iter().copied().filter_map(obj_size).sum();

    println!(
        "\nVerwaist & löschbar (älter als {} Tage): {} Objekt(e) ({}).",
        opts.min_age_days,
        orphans.len(),
        human_size(orphan_bytes)
    );
    if protected > 0 {
        println!(
            "  Geschützt (verwaist, aber jünger als {} Tage): {} Objekt(e) ({}).",
            opts.min_age_days,
            protected,
            human_size(protected_bytes)
        );
    }

    // Beispiel-Liste, größte zuerst.
    let mut sample = orphans.clone();
    sample.sort_by_key(|o| std::cmp::Reverse(obj_size(o).unwrap_or(0)));
    for o in sample.iter().take(20) {
        println!("  - {} ({})", o.name, human_size(obj_size(o).unwrap_or(0)));
    }
    if sample.len() > 20 {
        println!("  … und {} weitere.", sample.len() - 20);
    }

    if !opts.execute {
        println!("\nDry-Run — es wurde nichts gelöscht. Zum tatsächlichen Löschen `--execute` anhängen.");
        return Ok(());
    }
    if orphans.is_empty() {
        println!("\nNichts zu löschen.");
        return Ok(());
    }

    // 4. In Batches löschen.
    println!();
    let names: Vec<&str> = orphans.iter().map(|o| o.name.as_str()).collect();
    let mut deleted = 0usize;
    for batch in names.chunks(DELETE_BATCH) {
        delete_objects(&client, cfg, batch)
            .with_context(|| format!("Batch-Delete fehlgeschlagen (nach {deleted} gelöschten)"))?;
        deleted += batch.len();
        println!("  gelöscht: {deleted}/{}", names.len());
    }
    println!(
        "\nFertig: {deleted} verwaiste(s) Objekt(e) gelöscht, ~{} freigegeben.",
        human_size(orphan_bytes)
    );
    Ok(())
}

/// Alle von `public.offers.image_url` referenzierten Bucket-Objektpfade
/// einsammeln (nur URLs, die auf DIESEN Bucket zeigen). Paginiert bis leer.
fn referenced_paths(client: &reqwest::blocking::Client, cfg: &PushConfig) -> Result<HashSet<String>> {
    let marker = format!("/{BUCKET}/");
    let url = format!("{}/rest/v1/offers", cfg.base_url);
    let mut set = HashSet::new();
    let mut offset = 0usize;
    loop {
        let resp = client
            .get(&url)
            .header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .query(&[
                ("select", "image_url".to_string()),
                ("limit", OFFERS_PAGE.to_string()),
                ("offset", offset.to_string()),
            ])
            .send()
            .with_context(|| format!("offers lesen fehlgeschlagen ({})", cfg.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("offers lesen: HTTP {status}: {}", excerpt(&body));
        }
        let page: Vec<OfferImage> = resp.json().context("offers-Antwort nicht parsebar")?;
        let n = page.len();
        if n == 0 {
            break;
        }
        for row in page {
            if let Some(u) = row.image_url {
                if let Some(path) = bucket_path(&u, &marker) {
                    set.insert(path.to_string());
                }
            }
        }
        offset += n;
    }
    Ok(set)
}

/// Alle Objekte im Bucket auflisten (paginiert bis leer). Ordner-/Prefix-
/// Platzhalter (id == null) werden ausgelassen.
fn list_bucket_objects(client: &reqwest::blocking::Client, cfg: &PushConfig) -> Result<Vec<StorageObject>> {
    let url = format!("{}/storage/v1/object/list/{BUCKET}", cfg.base_url);
    let mut all = Vec::new();
    let mut offset = 0usize;
    loop {
        let body = serde_json::json!({
            "prefix": "",
            "limit": LIST_PAGE,
            "offset": offset,
            "sortBy": { "column": "name", "order": "asc" }
        });
        let resp = client
            .post(&url)
            .header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .json(&body)
            .send()
            .with_context(|| format!("Bucket-Liste fehlgeschlagen ({})", cfg.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            bail!("Bucket-Liste: HTTP {status}: {}", excerpt(&text));
        }
        let page: Vec<StorageObject> = resp.json().context("Bucket-Liste nicht parsebar")?;
        let n = page.len();
        if n == 0 {
            break;
        }
        all.extend(page.into_iter().filter(|o| o.id.is_some()));
        offset += n;
    }
    Ok(all)
}

/// Objekte per Batch-Delete entfernen (`DELETE /storage/v1/object/{bucket}`
/// mit `{"prefixes": [...]}`).
fn delete_objects(client: &reqwest::blocking::Client, cfg: &PushConfig, names: &[&str]) -> Result<()> {
    let url = format!("{}/storage/v1/object/{BUCKET}", cfg.base_url);
    let body = serde_json::json!({ "prefixes": names });
    let resp = client
        .delete(&url)
        .header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .json(&body)
        .send()
        .with_context(|| format!("Löschen fehlgeschlagen ({})", cfg.base_url))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        bail!("Löschen: HTTP {status}: {}", excerpt(&text));
    }
    Ok(())
}

/// Objektpfad aus einer öffentlichen Bucket-URL ziehen: der Teil hinter dem
/// letzten `/{bucket}/`. Nicht-Bucket-URLs (Händler-CDN) liefern None.
fn bucket_path<'a>(url: &'a str, marker: &str) -> Option<&'a str> {
    url.rsplit_once(marker)
        .map(|(_, p)| p)
        .filter(|p| !p.is_empty())
}

fn excerpt(s: &str) -> String {
    s.chars().take(200).collect()
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

// ============================ prune-history ============================

/// Standard-Aufbewahrung der Preis-Historie in Wochen. Ein halbes Jahr ist
/// die kürzeste Spanne, in der ein Preisverlauf noch etwas aussagt — kürzer
/// und der Chart im Angebots-Sheet zeigt nur Rauschen, länger und die Tabelle
/// wächst schneller, als sie jemand liest.
pub const HISTORY_WEEKS: i64 = 26;

pub struct HistoryPruneOptions {
    /// false = Dry-Run (nur zählen, nichts löschen).
    pub execute: bool,
    /// Wochen, die aufbewahrt werden.
    pub weeks: i64,
}

/// Alte Wochen aus `public.price_history` löschen.
///
/// Anders als `offers` rotiert die Historie nicht: Jeder Push schreibt eine
/// weitere Wochen-Zeile je Produkt und Filiale, gelöscht wurde nie. Mit dem
/// Filial-Schlüssel aus v13 multipliziert sich das zusätzlich mit jeder
/// Filiale, die jemand anfordert — Stand 2026-07-25 stehen dort 62.793 Zeilen
/// aus wenigen Wochen und 106 Filialen.
///
/// Zwei Löschläufe, weil zwei Sorten Zeilen alt sein können:
///   * die mit `valid_from` vor dem Stichtag — der Normalfall,
///   * Altbestand ohne `valid_from`, der über `recorded_at` datiert wird.
/// Ohne den zweiten Lauf bliebe genau der Teil für immer liegen, den niemand
/// mehr zuordnen kann.
pub fn run_history(opts: &HistoryPruneOptions, cfg: Option<&PushConfig>) -> Result<()> {
    let owned;
    let cfg = match cfg {
        Some(c) => c,
        None => {
            owned = config_from_env()?;
            &owned
        }
    };
    if opts.weeks < 1 {
        bail!("--weeks muss mindestens 1 sein (sonst löscht der Lauf die ganze Historie).");
    }
    let client = reqwest::blocking::Client::new();

    let cutoff = Utc::now() - Duration::weeks(opts.weeks);
    let cutoff_date = cutoff.format("%Y-%m-%d").to_string();
    let cutoff_ts = cutoff.to_rfc3339();
    let mode = if opts.execute { "LÖSCHEN" } else { "Dry-Run" };
    println!(
        "prune-history ({mode}): behalte {} Wochen, Stichtag {cutoff_date}.",
        opts.weeks
    );

    let before = count_history(&client, cfg, &[])?;
    let dated = count_history(&client, cfg, &[("valid_from", format!("lt.{cutoff_date}"))])?;
    let undated = count_history(
        &client,
        cfg,
        &[("valid_from", "is.null".to_string()), ("recorded_at", format!("lt.{cutoff_ts}"))],
    )?;

    println!("In der Historie: {before} Zeile(n).");
    println!("  älter als der Stichtag: {dated}");
    println!("  ohne valid_from und vor dem Stichtag aufgezeichnet: {undated}");

    if !opts.execute {
        println!("\nDry-Run — es wird nichts gelöscht. Mit --execute wirklich löschen.");
        return Ok(());
    }
    if dated + undated == 0 {
        println!("\nNichts zu löschen.");
        return Ok(());
    }

    delete_history(&client, cfg, &[("valid_from", format!("lt.{cutoff_date}"))])?;
    delete_history(
        &client,
        cfg,
        &[("valid_from", "is.null".to_string()), ("recorded_at", format!("lt.{cutoff_ts}"))],
    )?;

    let after = count_history(&client, cfg, &[])?;
    println!("\nGelöscht: {} Zeile(n). Historie jetzt: {after}.", before.saturating_sub(after));
    Ok(())
}

fn history_request(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    method: reqwest::Method,
    filters: &[(&str, String)],
) -> reqwest::blocking::RequestBuilder {
    let mut req = client
        .request(method, format!("{}/rest/v1/price_history", cfg.base_url))
        .header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key));
    for (key, value) in filters {
        req = req.query(&[(*key, value)]);
    }
    req
}

/// Zeilen zählen, ohne sie zu laden: `count=exact` plus ein Ein-Zeilen-Range,
/// die Zahl steht im `content-range`-Header.
fn count_history(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    filters: &[(&str, String)],
) -> Result<usize> {
    let resp = history_request(client, cfg, reqwest::Method::GET, filters)
        .query(&[("select", "id")])
        .header("Prefer", "count=exact")
        .header("Range", "0-0")
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let excerpt: String = body.chars().take(300).collect();
        bail!("Historie zählen fehlgeschlagen (HTTP {status}): {excerpt}");
    }
    let header = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .context("Antwort ohne content-range — Zeilenzahl nicht ermittelbar")?;
    parse_count(&header).with_context(|| format!("content-range unlesbar: {header}"))
}

/// `0-0/62793` -> 62793. `*/0` (leeres Ergebnis) -> 0.
fn parse_count(content_range: &str) -> Option<usize> {
    content_range.rsplit('/').next()?.trim().parse().ok()
}

fn delete_history(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    filters: &[(&str, String)],
) -> Result<()> {
    let resp = history_request(client, cfg, reqwest::Method::DELETE, filters)
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let excerpt: String = body.chars().take(300).collect();
        bail!("Historie löschen fehlgeschlagen (HTTP {status}): {excerpt}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_path_extracts_object_name() {
        let marker = "/offer-images/";
        assert_eq!(
            bucket_path(
                "https://p.supabase.co/storage/v1/object/public/offer-images/abc123.png",
                marker
            ),
            Some("abc123.png")
        );
    }

    #[test]
    fn bucket_path_ignores_foreign_urls() {
        let marker = "/offer-images/";
        // Händler-CDN-URL (noch nicht gespiegelt) -> kein Bucket-Objekt.
        assert_eq!(bucket_path("https://cdn.penny.de/foo/bar.jpg", marker), None);
        // Leerer Pfad hinter dem Marker -> None.
        assert_eq!(
            bucket_path("https://p.supabase.co/storage/v1/object/public/offer-images/", marker),
            None
        );
    }

    #[test]
    fn human_size_scales() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
