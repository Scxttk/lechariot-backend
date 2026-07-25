//! Filial-Lookup für die Ketten mit nationalem Angebotskatalog (Lidl,
//! ALDI Nord, ALDI SÜD). Deren Angebots-APIs sind bundesweit — ob die Kette
//! in einer Region überhaupt vertreten ist, klären die offiziellen
//! Store-Finder der Ketten:
//!
//!   Lidl:      Bing Spatial Data Service (spatial.virtualearth.net), Dataset
//!              Filialdaten-SEC — dieselbe Quelle nutzt der Filialfinder auf
//!              lidl.de. `spatialFilter=nearby(lat,lon,km)` filtert serverseitig.
//!   ALDI Nord/SÜD: Uberall-Locator (uberall.com/api/storefinders/<key>),
//!              die Plattform hinter den Filialfindern auf aldi-nord.de bzw.
//!              aldi-sued.de. Sucht per lat/lng, liefert `distance` in Metern.
//!
//! Beide brauchen Koordinaten statt PLZ; die PLZ geocodiert Nominatim
//! (nominatim.openstreetmap.org, 1 Request pro Region, mit eigenem UA laut
//! OSM-Policy). Alle drei Hosts sind nicht Akamai-geschützt — reqwest mit dem
//! gemeinsamen Browser-UA (util::blocking_client) reicht, anders als bei den
//! Angebots-Scrapern von Netto/ALDI SÜD (System-curl).
//!
//! Fehlerverhalten: Finder-Fehler (Netz, Formatänderung) fallen mit WARN auf
//! den nationalen Platzhalter zurück — lieber ein zu breiter Eintrag als eine
//! stumm verschwundene Kette. Nur ein *erfolgreicher* Lookup ohne Filiale im
//! Umkreis meldet die Kette als nicht vertreten.

use anyhow::{Context, Result, bail};

use crate::models::{Branch, Market};
use crate::scrapers::util;

/// Maximale Entfernung Filiale <-> PLZ-Zentrum, ab der die Kette als in der
/// Region vertreten gilt.
pub const CUTOFF_KM: f64 = 15.0;

// Öffentliche Keys der Filialfinder (in den Websites der Ketten eingebettet,
// Stand 2026-07; Quelle: alltheplaces-Spiders lidl_de / aldi_nord_de / aldi_sud_de).
const LIDL_DATASET: &str = "ab055fcbaac04ec4bc563e65ffa07097";
const LIDL_KEY: &str = "AnTPGpOQpGHsC_ryx9LY3fRTI27dwcRWuPrfg93-WZR2m-1ax9e9ghlD4s1RaHOq";
const ALDI_NORD_KEY: &str = "ALDINORDDE_UimhY3MWJaxhjK9QdZo3Qa4chq1MAu";
const ALDI_SUED_KEY: &str = "gqNws2nRfBBlQJS9UrA8zV9txngvET";

// ---------------------------------------------------------------- Geocoding

/// PLZ -> Koordinaten über Nominatim (OSM). Ein Request pro Region.
pub fn geocode_plz(plz: &str) -> Result<(f64, f64)> {
    geocode_plz_with_city(plz).map(|(lat, lon, _)| (lat, lon))
}

/// Wie [`geocode_plz`], zusätzlich die Stadt.
///
/// Die braucht das Filialverzeichnis für die Textsuchen: REWE nimmt die PLZ
/// wörtlich und liefert für „01219" fünf Filialen in 01257/01259/01277 —
/// die Suche nach „Dresden" dagegen die ganze Stadt, aus der dann der Umkreis
/// filtert.
pub fn geocode_plz_with_city(plz: &str) -> Result<(f64, f64, Option<String>)> {
    let url = format!(
        "https://nominatim.openstreetmap.org/search?postalcode={plz}&country=de&format=jsonv2&limit=1&addressdetails=1"
    );
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx("Store-Finder", "PLZ geocodieren", &url))?
        .error_for_status()
        .with_context(|| util::ctx("Store-Finder", "PLZ geocodieren (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx("Store-Finder", "Geocoding JSON parsen", &url))?;
    let (lat, lon) =
        parse_nominatim(&raw).with_context(|| format!("Nominatim kennt PLZ {plz} nicht"))?;
    Ok((lat, lon, parse_nominatim_city(&raw)))
}

/// Erstes Ergebnis einer Nominatim-Antwort als (lat, lon).
pub fn parse_nominatim(raw: &serde_json::Value) -> Option<(f64, f64)> {
    let first = raw.as_array()?.first()?;
    let lat = first.get("lat")?.as_str()?.parse().ok()?;
    let lon = first.get("lon")?.as_str()?.parse().ok()?;
    Some((lat, lon))
}

/// Stadt aus `addressdetails`. Auf dem Land steht dort statt `city` je nach
/// Gemeindegröße `town` oder `village`; ohne `addressdetails=1` fehlt der
/// Block ganz und das Ergebnis ist None (die Aufrufer fallen dann auf die
/// PLZ zurück).
pub fn parse_nominatim_city(raw: &serde_json::Value) -> Option<String> {
    let address = raw.as_array()?.first()?.get("address")?;
    ["city", "town", "village", "municipality"].iter().find_map(|key| {
        address
            .get(*key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

// ---------------------------------------------------------------- Lidl

/// Nächste Lidl-Filiale im Umkreis der PLZ; None, wenn keine existiert.
pub fn lidl_branch(plz: &str) -> Result<Option<Market>> {
    let (lat, lon) = geocode_plz(plz)?;
    let url = format!(
        "https://spatial.virtualearth.net/REST/v1/data/{LIDL_DATASET}/Filialdaten-SEC/Filialdaten-SEC\
         ?key={LIDL_KEY}&$filter=Adresstyp%20Eq%201&spatialFilter=nearby({lat},{lon},{CUTOFF_KM})\
         &$select=EntityID,ShownStoreName,Locality,Latitude,Longitude&$format=json&$top=1"
    );
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx("Lidl", "Filialsuche", &url))?
        .error_for_status()
        .with_context(|| util::ctx("Lidl", "Filialsuche (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx("Lidl", "Filialsuche JSON parsen", &url))?;
    parse_virtualearth(&raw)
}

/// Erste Filiale einer Bing-SDS-Antwort als Market; None bei leerer Liste,
/// Fehler bei unerwartetem Format (damit der Aufrufer auf den Platzhalter
/// zurückfällt statt die Kette fälschlich abzumelden).
pub fn parse_virtualearth(raw: &serde_json::Value) -> Result<Option<Market>> {
    let results = raw
        .pointer("/d/results")
        .and_then(|v| v.as_array())
        .context("Bing-SDS-Antwort ohne d.results")?;
    let Some(store) = results.first() else {
        return Ok(None);
    };
    let id = store.get("EntityID").and_then(|v| v.as_str()).context("EntityID fehlt")?;
    // ShownStoreName ist oft leer — dann die Stadt (Locality), damit nicht
    // "Lidl " als Filialname in der App landet.
    let name = store
        .get("ShownStoreName")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            store
                .get("Locality")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("Filiale");
    Ok(Some(
        Market::new(format!("LIDL_{id}"), format!("Lidl {name}")).with_geo(
            store.get("Latitude").and_then(|v| v.as_f64()),
            store.get("Longitude").and_then(|v| v.as_f64()),
        ),
    ))
}

/// Alle Lidl-Filialen im Umkreis, für das Verzeichnis (`public.branches`).
///
/// Derselbe Endpunkt wie [`lidl_branch`], nur ohne `$top=1`: Der Finder ist
/// seit jeher eine Umkreissuche, wir haben bloß immer den ersten Treffer
/// behalten und den Rest weggeworfen.
pub fn lidl_branches(lat: f64, lon: f64, radius_km: f64, limit: usize) -> Result<Vec<Branch>> {
    let url = format!(
        "https://spatial.virtualearth.net/REST/v1/data/{LIDL_DATASET}/Filialdaten-SEC/Filialdaten-SEC\
         ?key={LIDL_KEY}&$filter=Adresstyp%20Eq%201&spatialFilter=nearby({lat},{lon},{radius_km})\
         &$format=json&$top={limit}"
    );
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx("Lidl", "Filialverzeichnis", &url))?
        .error_for_status()
        .with_context(|| util::ctx("Lidl", "Filialverzeichnis (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx("Lidl", "Filialverzeichnis JSON parsen", &url))?;
    parse_virtualearth_branches(&raw)
}

/// Alle Filialen einer Bing-SDS-Antwort als Verzeichniseinträge.
///
/// Ohne `$select` liefert das Dataset knapp 60 Felder — genommen werden die
/// `Shown*`-Varianten, das ist die Adresse, die auch lidl.de anzeigt, mit
/// Rückfall auf die Rohfelder.
pub fn parse_virtualearth_branches(raw: &serde_json::Value) -> Result<Vec<Branch>> {
    let results = raw
        .pointer("/d/results")
        .and_then(|v| v.as_array())
        .context("Bing-SDS-Antwort ohne d.results")?;

    Ok(results
        .iter()
        .filter_map(|store| {
            let id = store.get("EntityID").and_then(|v| v.as_str())?;
            // Leer zählt als fehlend, sonst greift der Rückfall nicht: Das
            // Dataset trägt ShownStoreName oft als "" statt gar nicht, und
            // ein Filter *nach* dem find_map würde bei der ersten leeren
            // Variante aufhören statt die nächste zu probieren.
            let text = |keys: [&str; 2]| {
                keys.iter().find_map(|k| {
                    store
                        .get(*k)
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
            };
            let locality = text(["ShownLocality", "Locality"]);
            // ShownStoreName ist oft leer — dann der Ort, damit im Verzeichnis
            // nicht "Lidl " steht (dieselbe Regel wie in parse_virtualearth).
            let label = text(["ShownStoreName", "CityDistrict"])
                .or_else(|| locality.clone())
                .unwrap_or_else(|| "Filiale".to_string());
            Some(
                Branch::new(format!("LIDL_{id}"), "Lidl", format!("Lidl {label}"), "bing-sds")
                    .with_address(
                        text(["ShownAddressLine", "AddressLine"]),
                        text(["ShownPostalCode", "PostalCode"]),
                        locality,
                    )
                    .with_geo(
                        store.get("Latitude").and_then(|v| v.as_f64()),
                        store.get("Longitude").and_then(|v| v.as_f64()),
                    ),
            )
        })
        .collect())
}

// ---------------------------------------------------------------- ALDI (Uberall)

pub fn aldi_nord_branch(plz: &str) -> Result<Option<Market>> {
    uberall_branch(plz, ALDI_NORD_KEY, "ALDI Nord", "ALDI_NORD")
}

pub fn aldi_sued_branch(plz: &str) -> Result<Option<Market>> {
    uberall_branch(plz, ALDI_SUED_KEY, "ALDI SÜD", "ALDI_SUED")
}

fn uberall_branch(plz: &str, key: &str, chain: &str, id_prefix: &str) -> Result<Option<Market>> {
    let (lat, lon) = geocode_plz(plz)?;
    let url = format!("https://uberall.com/api/storefinders/{key}/locations?lat={lat}&lng={lon}&max=1");
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx(chain, "Filialsuche", &url))?
        .error_for_status()
        .with_context(|| util::ctx(chain, "Filialsuche (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx(chain, "Filialsuche JSON parsen", &url))?;
    parse_uberall(&raw, chain, id_prefix)
}

/// Nächste Filiale einer Uberall-Antwort als Market, sofern innerhalb des
/// Cutoffs (`distance` in Metern); None ohne Treffer im Umkreis.
pub fn parse_uberall(
    raw: &serde_json::Value,
    chain: &str,
    id_prefix: &str,
) -> Result<Option<Market>> {
    if raw.get("status").and_then(|v| v.as_str()) != Some("SUCCESS") {
        bail!("Uberall-Antwort ohne status=SUCCESS: {}", raw);
    }
    let locations = raw
        .pointer("/response/locations")
        .and_then(|v| v.as_array())
        .context("Uberall-Antwort ohne response.locations")?;
    let Some(store) = locations.first() else {
        return Ok(None);
    };
    if let Some(dist) = store.get("distance").and_then(|v| v.as_f64()) {
        if dist > CUTOFF_KM * 1000.0 {
            return Ok(None);
        }
    }
    let id = store
        .get("identifier")
        .and_then(|v| v.as_str())
        .map(|i| format!("{id_prefix}_{i}"))
        .unwrap_or_else(|| format!("{id_prefix}_DE"));
    let name = match store.get("city").and_then(|v| v.as_str()) {
        Some(city) => format!("{chain} {city}"),
        None => chain.to_string(),
    };
    Ok(Some(Market::new(id, name).with_geo(
        store.get("lat").and_then(|v| v.as_f64()),
        store.get("lng").and_then(|v| v.as_f64()),
    )))
}

/// Alle ALDI-Filialen im Umkreis, für das Verzeichnis.
///
/// Uberall kennt keinen Radius-Parameter, nur `max` — die Antwort ist nach
/// Entfernung sortiert und trägt `distance` in Metern. Der Radius wird
/// deshalb beim Parsen angewandt, sonst holt eine Suche für Dresden am Ende
/// Filialen in Chemnitz.
pub fn aldi_nord_branches(lat: f64, lon: f64, radius_km: f64, limit: usize) -> Result<Vec<Branch>> {
    uberall_branches(lat, lon, radius_km, limit, ALDI_NORD_KEY, "ALDI Nord", "ALDI_NORD")
}

pub fn aldi_sued_branches(lat: f64, lon: f64, radius_km: f64, limit: usize) -> Result<Vec<Branch>> {
    uberall_branches(lat, lon, radius_km, limit, ALDI_SUED_KEY, "ALDI SÜD", "ALDI_SUED")
}

fn uberall_branches(
    lat: f64,
    lon: f64,
    radius_km: f64,
    limit: usize,
    key: &str,
    chain: &str,
    id_prefix: &str,
) -> Result<Vec<Branch>> {
    let url =
        format!("https://uberall.com/api/storefinders/{key}/locations?lat={lat}&lng={lon}&max={limit}");
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx(chain, "Filialverzeichnis", &url))?
        .error_for_status()
        .with_context(|| util::ctx(chain, "Filialverzeichnis (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx(chain, "Filialverzeichnis JSON parsen", &url))?;
    parse_uberall_branches(&raw, chain, id_prefix, radius_km)
}

/// Alle Filialen einer Uberall-Antwort innerhalb des Radius.
///
/// Zeilen ohne `identifier` fallen raus: [`parse_uberall`] setzt für den
/// Angebots-Pfad ersatzweise den nationalen Platzhalter `<prefix>_DE`, aber
/// ein Verzeichniseintrag ohne echte Filial-ID wäre eine Filiale, die es
/// nirgends gibt.
pub fn parse_uberall_branches(
    raw: &serde_json::Value,
    chain: &str,
    id_prefix: &str,
    radius_km: f64,
) -> Result<Vec<Branch>> {
    if raw.get("status").and_then(|v| v.as_str()) != Some("SUCCESS") {
        bail!("Uberall-Antwort ohne status=SUCCESS: {}", raw);
    }
    let locations = raw
        .pointer("/response/locations")
        .and_then(|v| v.as_array())
        .context("Uberall-Antwort ohne response.locations")?;

    Ok(locations
        .iter()
        .filter(|store| {
            store
                .get("distance")
                .and_then(|v| v.as_f64())
                .is_none_or(|d| d <= radius_km * 1000.0)
        })
        .filter_map(|store| {
            let id = store.get("identifier").and_then(|v| v.as_str())?;
            let text = |key: &str| {
                store
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let city = text("city");
            let name = match &city {
                Some(c) => format!("{chain} {c}"),
                None => chain.to_string(),
            };
            Some(
                Branch::new(format!("{id_prefix}_{id}"), chain, name, "uberall")
                    .with_address(text("streetAndNumber"), text("zip"), city)
                    .with_geo(
                        store.get("lat").and_then(|v| v.as_f64()),
                        store.get("lng").and_then(|v| v.as_f64()),
                    ),
            )
        })
        .collect())
}

// ---------------------------------------------------------------- Fallback

/// Finder-Ergebnis auf das Sync-Verhalten abbilden: Treffer -> echte Filiale,
/// sauberes "keine Filiale" -> None (Kette wird für die Region nicht
/// registriert), Fehler -> WARN + nationaler Platzhalter.
/// Großkreis-Distanz zweier Koordinaten in km (Haversine).
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    const R: f64 = 6371.0;
    let (dlat, dlon) = ((b.0 - a.0).to_radians(), (b.1 - a.1).to_radians());
    let h = (dlat / 2.0).sin().powi(2)
        + a.0.to_radians().cos() * b.0.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * h.sqrt().asin()
}

pub fn resolve(
    chain: &str,
    found: Result<Option<Market>>,
    national: Market,
) -> Option<Market> {
    match found {
        Ok(Some(market)) => Some(market),
        Ok(None) => None,
        Err(e) => {
            eprintln!(
                "WARNUNG [{chain}] Filialsuche fehlgeschlagen ({e:#}) — nutze nationalen Platzhalter."
            );
            Some(national)
        }
    }
}
