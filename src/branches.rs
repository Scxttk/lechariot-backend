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

/// Ein Gebiet, das geholt werden soll.
///
/// Bis v20 war das schlicht eine PLZ. Seit v21 kann stattdessen (oder
/// zusätzlich) die **Mitte** mitkommen, um die die App gesucht hat — und die
/// ist die verlässlichere Angabe: Die PLZ stammte aus der Ankerfiliale, und
/// die kann in der Nachbarstadt stehen (Ahlbeck 17419, Anker in Ueckermünde
/// 17373, 24,5 km — der Lauf am 2026-07-30 holte die falsche Stadt und meldete
/// Erfolg).
///
/// Die drei Felder, und warum jedes einzeln nötig ist:
///
/// * `coords` — der Suchmittelpunkt. Liegt er vor, gewinnt er; die PLZ für die
///   Textsuchen kommt dann aus dem Reverse-Geocoding.
/// * `plz` — ohne `coords` der Suchbegriff, mit `coords` der **Rückfall**,
///   falls Nominatim nichts liefert. Ein Teilausfall (Lidl/ALDI richtig,
///   Textsuchen auf der Anker-PLZ) ist besser als gar kein Lauf.
/// * `anchor_market_id` — die Ankerfiliale der Anforderung. Sie sagt, WELCHE
///   Filiale es gibt, nicht welches Gebiet gemeint ist: Seit Migration v22 ist
///   der Schlüssel einer Anforderung ihre Rasterzelle (`area_key`), und ein
///   Anker kann Zeilen in mehreren Zellen tragen — Penny Am Haff ist die
///   nächste Filiale sowohl für Ueckermünde als auch für Ahlbeck. Die
///   Fertigmeldung geht deshalb über [`AreaTarget::dedup_key`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AreaTarget {
    pub coords: Option<(f64, f64)>,
    pub plz: Option<String>,
    pub anchor_market_id: Option<String>,
}

impl AreaTarget {
    /// Der Weg vor v21 und der Weg des Sonntagslaufs: nur eine PLZ.
    pub fn from_plz(plz: impl Into<String>) -> Self {
        AreaTarget { coords: None, plz: Some(plz.into()), anchor_market_id: None }
    }

    /// Schlüssel, unter dem zwei Anforderungen als dasselbe Gebiet gelten.
    /// **Dieselbe Regel wie der Cooldown in Migration v21** — laufen die
    /// beiden auseinander, holt der Lauf ein Gebiet doppelt oder gar nicht.
    pub fn dedup_key(&self) -> String {
        match self.coords {
            Some((lat, lon)) => area_cell(lat, lon),
            None => format!("plz:{}", self.plz.as_deref().unwrap_or("")),
        }
    }
}

/// Rasterzelle als Text, auf 0,1° gerundet — der Rust-Spiegel des
/// Cooldown-Schlüssels aus Migration v21. Beide Seiten müssen dieselbe Zeichen-
/// kette erzeugen, und diese Funktion ist das Einzige daran, was ein Test
/// überhaupt fassen kann (für das SQL gibt es keine Testinfrastruktur).
///
/// 0,1° sind 11,1 km in der Breite und 6,5-7,6 km in der Länge über
/// Deutschland, also höchstens 13,5 km Diagonale — zwei Anforderungen in
/// derselben Zelle liegen im 25-km-Kreis (`AREA_RADIUS_KM`) des ersten Laufs.
///
/// `round()` rundet in Rust wie `round(numeric, 1)` in Postgres von der Null
/// weg; `+ 0.0` macht aus einer eventuellen -0.0 wieder 0.0.
pub fn area_cell(lat: f64, lon: f64) -> String {
    let round1 = |v: f64| (v * 10.0).round() / 10.0 + 0.0;
    format!("cell:{:.1},{:.1}", round1(lat), round1(lon))
}

pub struct DirectoryOptions {
    /// Gebiete, deren Umkreis geholt wird. Leer = nur die landesweiten Ketten.
    pub targets: Vec<AreaTarget>,
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
            targets: Vec::new(),
            national: true,
            radius_km: AREA_RADIUS_KM,
            cert: "cert.pem".to_string(),
            key: "private.key".to_string(),
            dry_run: false,
        }
    }
}

/// Was aus einem [`AreaTarget`] nach dem Geocoding geworden ist.
struct ResolvedArea {
    center: Option<(f64, f64)>,
    plz: Option<String>,
    city: Option<String>,
}

impl ResolvedArea {
    /// Kopf der Log-Zeilen. **Die Workflow-Zusammenfassung liest `[<PLZ>]` aus
    /// `branches.log`** — steht dort etwas anderes, fehlt die Zeile im
    /// Actions-Bericht, deshalb bleibt die PLZ die erste Wahl.
    fn label(&self) -> String {
        match (&self.plz, self.center) {
            (Some(plz), _) => plz.clone(),
            (None, Some((lat, lon))) => format!("{lat:.4},{lon:.4}"),
            (None, None) => "?".to_string(),
        }
    }
}

/// Mittelpunkt, PLZ und Stadt für ein Gebiet bestimmen.
///
/// Mit Koordinaten ist der Mittelpunkt gesetzt und die PLZ kommt aus dem
/// Reverse-Geocoding; scheitert das, **bleiben die Koordinaten** und die
/// mitgeschickte PLZ trägt die Textsuchen. Ohne Koordinaten der Weg von vor
/// v21: PLZ vorwärts geocodieren.
fn resolve_area(target: &AreaTarget) -> ResolvedArea {
    if let Some((lat, lon)) = target.coords {
        return match store_finder::reverse_geocode(lat, lon) {
            Ok((plz, city)) => {
                if plz.is_none() {
                    eprintln!(
                        "WARNUNG [{lat:.4},{lon:.4}] Reverse-Geocoding kennt keine PLZ — \
                         Textsuchen laufen auf der Anker-PLZ {:?}",
                        target.plz
                    );
                }
                ResolvedArea {
                    center: Some((lat, lon)),
                    plz: plz.or_else(|| target.plz.clone()),
                    city,
                }
            }
            Err(e) => {
                eprintln!("WARNUNG [{lat:.4},{lon:.4}] Reverse-Geocoding fehlgeschlagen: {e:#}");
                ResolvedArea { center: Some((lat, lon)), plz: target.plz.clone(), city: None }
            }
        };
    }

    // Ohne Koordinaten fallen die drei Umkreis-Ketten aus und der
    // Umkreisfilter kann nicht greifen; die Textsuchen laufen trotzdem,
    // sie brauchen nur die PLZ.
    let Some(plz) = target.plz.clone() else {
        return ResolvedArea { center: None, plz: None, city: None };
    };
    match store_finder::geocode_plz_with_city(&plz) {
        Ok((lat, lon, city)) => {
            ResolvedArea { center: Some((lat, lon)), plz: Some(plz), city }
        }
        Err(e) => {
            eprintln!("WARNUNG [{plz}] Geocoding fehlgeschlagen: {e:#}");
            ResolvedArea { center: None, plz: Some(plz), city: None }
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

    // Die Gebiete, die am Ende ihre Fertigmeldung bekommen. `synced_plzs`
    // bedient alle Zeilen eines Orts (v19), `synced_anchors` die eine Zeile,
    // die diesen Lauf ausgelöst hat — nötig, falls ihre PLZ nicht gesetzt
    // werden konnte und der Filter über die PLZ sie deshalb nicht trifft.
    let mut synced_plzs: Vec<String> = Vec::new();
    let mut synced_anchors: Vec<String> = Vec::new();

    for target in &opts.targets {
        let resolved = resolve_area(target);
        let area = resolved.label();
        println!("[{area}] Filialen im Umkreis von {:.0} km …", opts.radius_km);

        // Die aufgelöste PLZ **vor** den Ketten auf die Ankerzeile schreiben.
        // Bricht der Lauf mitten drin ab, steht dort dann trotzdem das echte
        // Gebiet — sonst läse der Sonntagslauf weiter die Anker-PLZ und holte
        // ein zweites Mal die falsche Stadt.
        if let (false, Some(anchor), Some(plz)) =
            (opts.dry_run, &target.anchor_market_id, &resolved.plz)
        {
            match mark_area_resolved(cfg, target, plz) {
                Ok(true) => println!("[{area}] Gebiet auf Anforderung {anchor} nachgetragen."),
                Ok(false) => {}
                Err(e) => eprintln!("WARNUNG [{area}] Gebiet nachtragen fehlgeschlagen: {e:#}"),
            }
        }
        if let Some(plz) = &resolved.plz {
            synced_plzs.push(plz.clone());
        }
        if let Some(anchor) = &target.anchor_market_id {
            synced_anchors.push(anchor.clone());
        }

        // Eigene Liste je Gebiet: Der Umkreisfilter unten darf nur die
        // Treffer dieses Gebiets sehen, nicht die landesweiten Ketten und
        // nicht die Nachbargebiete.
        let mut area_branches: Vec<Branch> = Vec::new();

        if let Some((lat, lon)) = resolved.center {
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

        // REWE, Netto und EDEKA sind **Textsuchen je PLZ** — ohne PLZ gibt es
        // für sie nichts zu fragen. Genau diese drei fehlten dem Tester in
        // Ahlbeck, weil sie mit „17373" statt „17419" gelaufen sind; sie still
        // ausfallen zu lassen wäre derselbe Fehler in leiser.
        if let Some(plz) = &resolved.plz {
            if rewe_available(&opts.cert, &opts.key) {
                // Zwei Suchen, und beide sind nötig:
                //  · Die PLZ nimmt REWE wörtlich — „01219" liefert fünf Filialen
                //    in 01257/01259/01277, also die Nachbarschaft, aber nur die.
                //  · Die Stadt liefert die Innenstadt (REWE am Postplatz und die
                //    anderen), deckelt aber bei 20 Treffern und schneidet damit in
                //    einer Großstadt den Rand ab.
                // Zusammen decken sie Umgebung und Zentrum ab; Dubletten fallen
                // ohnehin heraus, und der Umkreisfilter unten begrenzt das Ganze.
                let mut queries = vec![plz.clone()];
                queries.extend(resolved.city.clone().filter(|c| c != plz));
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
                netto::find_branches(plz, opts.radius_km as u32),
            );
            collect(&format!("[{area}] EDEKA"), &mut area_branches, edeka::find_branches(plz));
        } else {
            eprintln!(
                "WARNUNG [{area}] REWE, Netto und EDEKA übersprungen: keine PLZ für die Textsuche"
            );
        }

        // Erst hier filtern, nicht je Kette: Die Finder ziehen ihre Grenze
        // jeweils anders (Lidl serverseitig, Netto über einen eigenen
        // Radius-Parameter, EDEKA und REWE gar nicht). Ein Umkreis, der in
        // der App etwas bedeuten soll, muss an einer Stelle für alle gelten.
        if let Some((lat, lon)) = resolved.center {
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

    // Fertigmeldung für die Gebiete, die jemand angefordert hat (v19). Sie
    // ist der einzige Weg, auf dem die App erfährt, dass ihr Ort jetzt
    // vollständig ist — und weil die Zeile in der Datenbank steht und nicht
    // im Arbeitsspeicher, überlebt sie einen Neustart der App.
    //
    // Nur warnen, nicht abbrechen: Die Filialen sind geschrieben, das ist das
    // Ergebnis des Laufs. Eine fehlende Fertigmeldung kostet den Hinweis,
    // nicht die Daten — und der Sonntagslauf holt sie nach.
    if !synced_plzs.is_empty() {
        synced_plzs.sort();
        synced_plzs.dedup();
        match mark_areas_synced(cfg, &synced_plzs) {
            Ok(n) if n > 0 => println!("{n} Gebiets-Anforderung(en) als erledigt gemeldet."),
            Ok(_) => {}
            Err(e) => eprintln!("WARNUNG: Gebiete als erledigt melden fehlgeschlagen: {e:#}"),
        }
    }
    // Und dieselbe Meldung noch einmal über die Ankerzeile. Sie ist nicht
    // überflüssig: Konnte die aufgelöste PLZ oben nicht geschrieben werden,
    // trägt die Zeile weiter die des Ankers, und der Filter über die PLZ geht
    // an ihr vorbei — die Anforderung bliebe für immer offen und die App
    // wartete auf einen Hinweis, der nie kommt.
    if !synced_anchors.is_empty() {
        synced_anchors.sort();
        synced_anchors.dedup();
        if let Err(e) = mark_anchors_synced(cfg, &synced_anchors) {
            eprintln!("WARNUNG: Anforderungen als erledigt melden fehlgeschlagen: {e:#}");
        }
    }
    Ok(rows.len())
}

/// Meldet die angeforderten Gebiete als erledigt (`area_requests.last_synced`).
///
/// Über die **PLZ**, nicht über die Ankerfiliale: Zwei Nutzer desselben Ortes
/// wählen verschiedene Anker und meinen dasselbe Gebiet — ein Lauf bedient
/// beide, also müssen auch beide Zeilen ihre Meldung bekommen.
///
/// Liefert die Zahl der gemeldeten Zeilen.
pub fn mark_areas_synced(cfg: &PushConfig, areas: &[String]) -> Result<usize> {
    let list = areas.join(",");
    patch_area_requests(
        cfg,
        ("plz", format!("in.({list})")),
        serde_json::json!({ "last_synced": chrono::Utc::now().to_rfc3339() }),
        "Gebiete als erledigt melden",
    )
}

/// Trägt die **echte** PLZ auf der Anforderungszeile nach.
///
/// Die Zeile trägt bis hierher die PLZ der Ankerfiliale — die Migration setzt
/// sie beim Insert als Rückfall, weil zu dem Zeitpunkt niemand die richtige
/// kennt. Erst dieser Lauf hat sie (aus dem Reverse-Geocoding), und erst mit
/// ihr ist die Zeile in sich stimmig: Danach findet die Fertigmeldung über die
/// PLZ sie, das Sicherheitsnetz des Sonntagslaufs liest das richtige Gebiet,
/// und die App zeigt im Hinweis den Ort, in dem der Nutzer wirklich steht.
///
/// Liefert `true`, wenn eine Zeile geändert wurde.
pub fn mark_area_resolved(cfg: &PushConfig, target: &AreaTarget, plz: &str) -> Result<bool> {
    // **Über die Zelle, nicht über den Anker.** Vor Migration v22 war
    // `market_id` der Primärschlüssel, eine Filiale trug also höchstens eine
    // Zeile. Jetzt kann sie mehrere tragen, die zu verschiedenen Orten gehören
    // — die PLZ dieses Laufs auf alle zu schreiben, wäre der Fehler aus dem
    // Ahlbeck-Vorfall, nur andersherum.
    let key = target.dedup_key();
    let n = patch_area_requests(
        cfg,
        ("area_key", format!("eq.{key}")),
        serde_json::json!({ "plz": plz }),
        "Gebiet auf der Anforderung nachtragen",
    )?;
    if n == 0 {
        // Kein Abbruchgrund, aber sichtbar: Der Lauf lief für ein Gebiet, zu
        // dem keine offene Zeile passt. Still bliebe genau das unbemerkt.
        eprintln!("WARNUNG [Gebiet] keine Anforderungszeile mit area_key {key} gefunden");
    }
    Ok(n > 0)
}

/// Meldet die Anforderungen selbst als erledigt, über die Ankerfiliale.
///
/// Ergänzung zu [`mark_areas_synced`], kein Ersatz: Jene bedient alle Zeilen
/// eines Orts und ist deshalb der Normalweg. Diese hier fängt den Fall, dass
/// die PLZ auf der Zeile nicht die des gelaufenen Gebiets ist — dann trifft
/// der PLZ-Filter sie nicht, und ohne diese zweite Meldung bliebe sie offen.
pub fn mark_anchors_synced(cfg: &PushConfig, anchors: &[String]) -> Result<usize> {
    let list = anchors.join(",");
    patch_area_requests(
        cfg,
        ("market_id", format!("in.({list})")),
        serde_json::json!({ "last_synced": chrono::Utc::now().to_rfc3339() }),
        "Anforderungen als erledigt melden",
    )
}

/// PATCH auf `area_requests`, mit der Zahl der wirklich getroffenen Zeilen.
fn patch_area_requests(
    cfg: &PushConfig,
    filter: (&str, String),
    body: serde_json::Value,
    what: &str,
) -> Result<usize> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .patch(format!("{}/rest/v1/area_requests", cfg.base_url))
        .header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .header("Content-Type", "application/json")
        // `return=representation`, damit hier steht, wie viele Zeilen wirklich
        // getroffen wurden — ohne das antwortet PostgREST mit 204 und die
        // Meldung „0 Gebiete" wäre von „alle Gebiete" nicht zu unterscheiden.
        .header("Prefer", "return=representation")
        .query(&[filter])
        .json(&body)
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        let excerpt: String = text.chars().take(300).collect();
        anyhow::bail!("{what} fehlgeschlagen (HTTP {status}): {excerpt}");
    }
    let updated: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
    Ok(updated.len())
}

/// Die Gebiete, auf die jemand gerade wartet (`area_requests` ohne
/// `last_synced`).
///
/// Sicherheitsnetz für den Sonntagslauf: Der Trigger stößt bei jeder
/// Anforderung sofort einen Lauf an, aber er fängt seine eigenen Fehler ab
/// (sonst scheiterte die Anforderung am Dispatch). Geht ein Dispatch verloren,
/// bliebe die Zeile sonst für immer offen und der Nutzer wartete auf einen
/// Lauf, den niemand startet — genau der Fehler, der am 2026-07-25 schon
/// einmal bei den Filialen steckte.
/// Ab v21 kommen **Koordinaten und Anker** mit, nicht nur die PLZ — und das
/// ist der Unterschied zwischen einem Sicherheitsnetz und einer zweiten
/// Ausgabe desselben Fehlers. Läse dieser Weg wie früher nur `plz`, holte der
/// Sonntagslauf für Ahlbeck wieder Ueckermünde (das ist die PLZ, die beim
/// Insert als Rückfall auf der Zeile landet) und meldete die Anforderung als
/// erledigt. Grüner Lauf, falsche Stadt, geschlossene Anforderung — genau der
/// Vorfall vom 2026-07-30, nur eine Woche später und ohne Anlass, hinzusehen.
///
/// Der Filter `plz=not.is.null` ist deshalb weg: Eine Zeile mit Koordinaten
/// und ohne PLZ ist seit v21 ein gültiger Auftrag. Aussortiert wird in Rust.
pub fn targets_from_open_requests(cfg: &PushConfig) -> Result<Vec<AreaTarget>> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!("{}/rest/v1/area_requests", cfg.base_url))
        .header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .query(&[
            ("select", "market_id,plz,lat,lon"),
            ("last_synced", "is.null"),
            ("active", "eq.true"),
        ])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    if !resp.status().is_success() {
        anyhow::bail!("Offene Gebiets-Anforderungen laden fehlgeschlagen (HTTP {})", resp.status());
    }
    let rows: Vec<serde_json::Value> =
        resp.json().context("area_requests: Antwort ist kein gültiges JSON")?;

    let mut targets: Vec<AreaTarget> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in &rows {
        let text = |key: &str| row.get(key).and_then(|v| v.as_str()).map(String::from);
        let number = |key: &str| row.get(key).and_then(|v| v.as_f64());
        let target = AreaTarget {
            coords: match (number("lat"), number("lon")) {
                (Some(lat), Some(lon)) => Some((lat, lon)),
                _ => None,
            },
            plz: text("plz"),
            anchor_market_id: text("market_id"),
        };
        // Weder Ort noch Lage — daraus lässt sich keine Suche bauen.
        if target.coords.is_none() && target.plz.is_none() {
            continue;
        }
        if seen.insert(target.dedup_key()) {
            targets.push(target);
        }
    }
    Ok(targets)
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
///
/// Hier bleibt es bei der reinen PLZ, und das ist kein Versehen: Der Nutzer
/// hat **diese Filiale gewählt**, also ist ihre PLZ das Gebiet, das er benutzt
/// — die Verwechslung aus v21 entsteht nur, wo eine Filiale stellvertretend
/// für einen Ort steht, in dem sie gar nicht liegt.
pub fn targets_from_chosen_branches(cfg: &PushConfig) -> Result<Vec<AreaTarget>> {
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
    Ok(areas.into_iter().map(AreaTarget::from_plz).collect())
}
