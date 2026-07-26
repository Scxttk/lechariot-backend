//! Filialverzeichnis (`public.branches`) befüllen.
//!
//! Bis hierher kannte das Backend pro Kette und PLZ **eine** Filiale: die, die
//! der Store-Finder als nächste zur PLZ meldete. Alles andere aus der Antwort
//! wurde weggeworfen. Genau daraus entsteht der Eintrag „REWE in 01219",
//! obwohl die gefundene Filiale in 01257 steht — und genau daraus fehlt der
//! Netto in der Johannes-Paul-Thilman-Straße, obwohl der Finder ihn liefert.
//!
//! Dieses Modul holt die vollständigen Listen und schreibt sie nach Supabase.
//! Die Ketten sind dabei unterschiedlich teuer, und der Zuschnitt folgt dem:
//!
//! | Kette | Kosten | Wann |
//! |---|---|---|
//! | Kaufland | 1 Request für alle 787 Filialen Deutschlands | landesweit |
//! | Penny | 1 Request für alle 2120 Märkte | landesweit |
//! | Lidl, ALDI Nord/SÜD | Umkreissuche um Koordinaten | je Gebiet |
//! | REWE, Netto | Textsuche je PLZ | je Gebiet |
//! | EDEKA | Textsuche je PLZ **plus ein Redirect je Filiale** | je Gebiet |
//!
//! Deshalb ist das Verzeichnis nicht flächendeckend, sondern bedarfsgesteuert:
//! landesweit nur, wo es einen Request kostet, sonst für die Gebiete, die
//! tatsächlich jemand gewählt hat (`--from-branches`).

use anyhow::{Context, Result};

use crate::models::Branch;
use crate::push::PushConfig;
use crate::scrapers::{edeka, kaufland, netto, penny, rewe, store_finder};

/// Umkreis der Gebietssuche. Größer als `store_finder::CUTOFF_KM` (15 km):
/// Dort geht es um „ist die Kette in dieser Region vertreten", hier um „welche
/// Märkte kann der Nutzer sinnvoll ansteuern" — der Markt auf dem Arbeitsweg
/// darf weiter weg sein.
pub const AREA_RADIUS_KM: f64 = 25.0;

/// Obergrenze je Kette und Gebiet. Bremse gegen eine Finder-Antwort, die
/// plötzlich das halbe Bundesland liefert; in einer Großstadt liegen
/// realistisch 10 bis 30 Filialen einer Kette im Radius.
pub const MAX_PER_CHAIN: usize = 60;

/// Bis zu so vielen Zeilen listet der Dry-Run vollständig auf, darüber nur
/// eine Stichprobe.
const DRY_RUN_FULL_LIST: usize = 200;

pub struct DirectoryOptions {
    /// PLZ, deren Umkreis geholt wird. Leer = nur die landesweiten Ketten.
    pub areas: Vec<String>,
    /// Kaufland und Penny mitziehen (je ein Request für ganz Deutschland).
    pub national: bool,
    pub radius_km: f64,
    /// Pfad zum REWE-Zertifikat; ohne gültiges Paar wird REWE übersprungen.
    pub cert: String,
    pub key: String,
    /// Nur zeigen, was geschrieben würde — keine Supabase-Writes.
    pub dry_run: bool,
}

impl Default for DirectoryOptions {
    fn default() -> Self {
        DirectoryOptions {
            areas: Vec::new(),
            national: true,
            radius_km: AREA_RADIUS_KM,
            cert: "cert.pem".to_string(),
            key: "private.key".to_string(),
            dry_run: false,
        }
    }
}

/// Verzeichnis befüllen. Liefert die Zahl der geschriebenen Zeilen.
///
/// Fehler einzelner Ketten warnen nur: Ein Verzeichnis mit sieben von acht
/// Ketten ist brauchbar, ein Abbruch wegen EDEKA wäre es nicht.
pub fn sync(cfg: &PushConfig, opts: &DirectoryOptions) -> Result<usize> {
    let mut all: Vec<Branch> = Vec::new();

    if opts.national {
        collect("Kaufland (landesweit)", &mut all, kaufland::fetch_branches());
        collect("Penny (landesweit)", &mut all, penny::fetch_branches());
    }

    for area in &opts.areas {
        println!("[{area}] Filialen im Umkreis von {:.0} km …", opts.radius_km);
        // Ohne Koordinaten fallen die drei Umkreis-Ketten aus und der
        // Umkreisfilter kann nicht greifen; die Textsuchen laufen trotzdem,
        // sie brauchen nur die PLZ.
        let center = match store_finder::geocode_plz_with_city(area) {
            Ok((lat, lon, city)) => Some((lat, lon, city)),
            Err(e) => {
                eprintln!("WARNUNG [{area}] Geocoding fehlgeschlagen: {e:#}");
                None
            }
        };

        // Eigene Liste je Gebiet: Der Umkreisfilter unten darf nur die
        // Treffer dieses Gebiets sehen, nicht die landesweiten Ketten und
        // nicht die Nachbargebiete.
        let mut area_branches: Vec<Branch> = Vec::new();

        if let Some((lat, lon, _)) = &center {
            let (lat, lon) = (*lat, *lon);
            collect(
                &format!("[{area}] Lidl"),
                &mut area_branches,
                store_finder::lidl_branches(lat, lon, opts.radius_km, MAX_PER_CHAIN),
            );
            collect(
                &format!("[{area}] ALDI Nord"),
                &mut area_branches,
                store_finder::aldi_nord_branches(lat, lon, opts.radius_km, MAX_PER_CHAIN),
            );
            collect(
                &format!("[{area}] ALDI SÜD"),
                &mut area_branches,
                store_finder::aldi_sued_branches(lat, lon, opts.radius_km, MAX_PER_CHAIN),
            );
        }

        if rewe_available(&opts.cert, &opts.key) {
            // Zwei Suchen, und beide sind nötig:
            //  · Die PLZ nimmt REWE wörtlich — „01219" liefert fünf Filialen
            //    in 01257/01259/01277, also die Nachbarschaft, aber nur die.
            //  · Die Stadt liefert die Innenstadt (REWE am Postplatz und die
            //    anderen), deckelt aber bei 20 Treffern und schneidet damit in
            //    einer Großstadt den Rand ab.
            // Zusammen decken sie Umgebung und Zentrum ab; Dubletten fallen
            // ohnehin heraus, und der Umkreisfilter unten begrenzt das Ganze.
            let city = center.as_ref().and_then(|(_, _, city)| city.clone());
            let mut queries = vec![area.clone()];
            queries.extend(city.filter(|c| c != area));
            for query in queries {
                collect(
                    &format!("[{area}] REWE ({query})"),
                    &mut area_branches,
                    rewe::find_branches(&query, &opts.cert, &opts.key),
                );
            }
        } else {
            eprintln!("WARNUNG [{area}] REWE übersprungen: kein Zertifikat (--cert/--key)");
        }
        collect(
            &format!("[{area}] Netto"),
            &mut area_branches,
            netto::find_branches(area, opts.radius_km as u32),
        );
        collect(&format!("[{area}] EDEKA"), &mut area_branches, edeka::find_branches(area));

        // Erst hier filtern, nicht je Kette: Die Finder ziehen ihre Grenze
        // jeweils anders (Lidl serverseitig, Netto über einen eigenen
        // Radius-Parameter, EDEKA und REWE gar nicht). Ein Umkreis, der in
        // der App etwas bedeuten soll, muss an einer Stelle für alle gelten.
        if let Some((lat, lon, _)) = center {
            let before = area_branches.len();
            area_branches.retain(|b| within(b, (lat, lon), opts.radius_km));
            let dropped = before - area_branches.len();
            if dropped > 0 {
                println!(
                    "[{area}] {dropped} Filialen außerhalb von {:.0} km verworfen.",
                    opts.radius_km
                );
            }
        }
        all.extend(area_branches);
    }

    let rows = deduplicated(all);
    if rows.is_empty() {
        println!("Keine Filialen gefunden — nichts zu schreiben.");
        return Ok(0);
    }

    if opts.dry_run {
        // Ein Gebiet passt vollständig auf den Schirm, und genau darum geht es
        // beim Prüfen ("steht mein Markt drin?"). Die landesweiten Listen
        // haben Tausende Zeilen — dort ist eine Stichprobe das Höchste, was
        // jemand liest.
        let shown = if rows.len() <= DRY_RUN_FULL_LIST { rows.len() } else { 20 };
        println!("\nDry-Run: {} Filialen würden geschrieben.", rows.len());
        for branch in rows.iter().take(shown) {
            println!("  {:<12} {:<10} {}", branch.chain, branch.market_id, describe(branch));
        }
        if rows.len() > shown {
            println!("  … und {} weitere", rows.len() - shown);
        }
        return Ok(0);
    }

    crate::push::upsert_branches(cfg, &rows)?;
    println!("\n{} Filialen nach Supabase geschrieben.", rows.len());
    Ok(rows.len())
}

fn describe(branch: &Branch) -> String {
    let address = [branch.street.as_deref(), branch.plz.as_deref(), branch.city.as_deref()]
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    match (branch.lat, branch.lon) {
        (Some(lat), Some(lon)) => format!("{} ({address}) {lat:.4},{lon:.4}", branch.name),
        _ => format!("{} ({address}) — ohne Koordinaten", branch.name),
    }
}

/// Ergebnis einer Kette einsammeln; Fehler warnen und gehen weiter.
fn collect(label: &str, all: &mut Vec<Branch>, result: Result<Vec<Branch>>) {
    match result {
        Ok(branches) => {
            let with_geo = branches.iter().filter(|b| b.lat.is_some()).count();
            println!("{label}: {} Filialen ({with_geo} mit Koordinaten).", branches.len());
            all.extend(branches);
        }
        Err(e) => eprintln!("WARNUNG {label}: {e:#}"),
    }
}

/// Dieselbe Filiale erscheint in mehreren Gebietssuchen — benachbarte PLZ
/// überlappen sich im Radius. Ein Upsert würde die Dublette zwar schlucken,
/// aber sie erst hochladen; hier fällt sie vorher raus. Der erste Treffer
/// gewinnt, das ist die Suche, die am nächsten dran war.
pub fn deduplicated(branches: Vec<Branch>) -> Vec<Branch> {
    let mut seen = std::collections::HashSet::new();
    branches.into_iter().filter(|b| seen.insert(b.market_id.clone())).collect()
}

/// Liegt die Filiale im Umkreis? Zeilen **ohne** Koordinaten bleiben drin:
/// Eine Filiale, deren Lage wir nicht kennen, wegzuwerfen hieße, sie wegen
/// einer Datenlücke des Finders verschwinden zu lassen — und die Suche, die
/// sie geliefert hat, galt ja diesem Gebiet.
pub fn within(branch: &Branch, center: (f64, f64), radius_km: f64) -> bool {
    match (branch.lat, branch.lon) {
        (Some(lat), Some(lon)) => store_finder::distance_km(center, (lat, lon)) <= radius_km,
        _ => true,
    }
}

fn rewe_available(cert: &str, key: &str) -> bool {
    std::path::Path::new(cert).exists() && std::path::Path::new(key).exists()
}

/// Eine Filiale aus dem Verzeichnis holen. Fehlt sie, ist das ein Fehler und
/// keine leere Liste: Wer mit `--market-id` syncen will, hat eine bestimmte
/// Filiale gemeint, und eine unbekannte ID ist ein Tippfehler oder ein
/// Verzeichnis, das für dieses Gebiet noch nie gelaufen ist.
pub fn fetch_branch(cfg: &PushConfig, market_id: &str) -> Result<Branch> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!("{}/rest/v1/branches", cfg.base_url))
        .header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .query(&[("market_id", format!("eq.{market_id}"))])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let excerpt: String = body.chars().take(300).collect();
        anyhow::bail!("Filiale {market_id} laden fehlgeschlagen (HTTP {status}): {excerpt}");
    }
    let rows: Vec<Branch> =
        resp.json().context("Filial-Antwort passt nicht zum branches-Schema")?;
    rows.into_iter().next().with_context(|| {
        format!(
            "Filiale {market_id} steht nicht im Verzeichnis. \
             `lechariot branches-sync --area <PLZ>` füllt das Gebiet nach."
        )
    })
}

/// Die PLZ der Filialen, die jemand gewählt hat — das Verzeichnis wächst
/// dort, wo die App tatsächlich benutzt wird.
///
/// Bis Migration v16 kam diese Liste aus `regions`. Die Tabelle gibt es nicht
/// mehr; die Frage „wo wird die App benutzt" beantworten jetzt die gewählten
/// Filialen selbst. Das ist sogar genauer: Eine Region galt als aktiv,
/// sobald irgendwer sie einmal eingetragen hatte.
pub fn areas_from_chosen_branches(cfg: &PushConfig) -> Result<Vec<String>> {
    let ids = crate::sync::fetch_chosen_branches(cfg).context("Gewählte Filialen laden")?;
    let mut areas: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for market_id in ids {
        match fetch_branch(cfg, &market_id) {
            Ok(branch) => {
                if let Some(plz) = branch.plz {
                    areas.insert(plz);
                }
            }
            // Eine Filiale, die das Verzeichnis nicht kennt, ist kein Grund,
            // die übrigen Gebiete fallen zu lassen.
            Err(e) => eprintln!("WARNUNG: Gebiet zu Filiale {market_id} unklar: {e:#}"),
        }
    }
    Ok(areas.into_iter().collect())
}
