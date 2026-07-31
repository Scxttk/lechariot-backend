//! Filial-Lookup für die Ketten mit nationalem Angebotskatalog (Lidl,
//! ALDI Nord, ALDI SÜD, NORMA). Deren Angebots-APIs sind bundesweit — ob die
//! Kette in einer Region überhaupt vertreten ist, klären die offiziellen
//! Store-Finder der Ketten:
//!
//!   Lidl:      Bing Spatial Data Service (spatial.virtualearth.net), Dataset
//!              Filialdaten-SEC — dieselbe Quelle nutzt der Filialfinder auf
//!              lidl.de. `spatialFilter=nearby(lat,lon,km)` filtert serverseitig.
//!   ALDI Nord/SÜD: Uberall-Locator (uberall.com/api/storefinders/<key>),
//!              die Plattform hinter den Filialfindern auf aldi-nord.de bzw.
//!              aldi-sued.de. Sucht per lat/lng, liefert `distance` in Metern.
//!   NORMA:     der eigene Filialfinder auf norma-online.de. Ein POST-Formular
//!              mit PLZ und Radius, das seine Suche in einer PHP-Sitzung
//!              ablegt; die Antwort ist HTML statt JSON. Braucht keine
//!              Koordinaten — aber einen Handschlag für das Sitzungscookie
//!              (siehe `norma_search`).
//!
//! Lidl und ALDI brauchen Koordinaten statt PLZ; die besorgt Nominatim
//! (nominatim.openstreetmap.org, 1 Request pro Gebiet) in beide Richtungen:
//! `/search` macht aus einer PLZ Koordinaten, `/reverse` aus Koordinaten eine
//! PLZ. Nominatim läuft über `util::nominatim_client` — eigener UA und
//! 1 Request/Sekunde laut OSM-Policy; **das stand hier schon, bevor es
//! stimmte**, siehe den Kommentar in `util.rs`. Die Filialfinder-Hosts sind
//! nicht Akamai-geschützt, dort reicht der gemeinsame Browser-UA
//! (util::blocking_client), anders als bei den Angebots-Scrapern von
//! Netto/ALDI SÜD (System-curl).
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
    let raw: serde_json::Value = util::nominatim_client()?
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

/// Koordinaten -> (PLZ, Stadt) über Nominatim. Die Gegenrichtung zu
/// [`geocode_plz_with_city`], und der Kern der Gebiets-Anforderung ab v21:
/// Die App schickt die Mitte der Region, in der ihr Picker gesucht hat, und
/// **hier** wird daraus die PLZ — nicht mehr aus der Ankerfiliale, die 24 km
/// entfernt in der Nachbarstadt stehen kann (Ahlbeck/Ueckermünde, 2026-07-30).
///
/// Kein `zoom`-Parameter: Der Standard (18, Gebäudeebene) füllt
/// `address.postcode`. Mit `zoom=10` antwortet Nominatim auf Stadtebene und
/// lässt die PLZ weg — dann wäre der ganze Aufruf umsonst.
///
/// Beide Rückgaben sind optional, und der Aufrufer muss damit rechnen: Über
/// See, im Wald oder bei einer Lücke in OSM antwortet Nominatim mit
/// `{"error": "Unable to geocode"}`. Das ist kein Fehler des Laufs — dann
/// trägt die PLZ der Ankerfiliale die Textsuchen weiter, also genau das
/// Verhalten von vor v21.
pub fn reverse_geocode(lat: f64, lon: f64) -> Result<(Option<String>, Option<String>)> {
    let url = format!(
        "https://nominatim.openstreetmap.org/reverse?lat={lat}&lon={lon}&format=jsonv2&addressdetails=1"
    );
    util::polite_pause(&url);
    let raw: serde_json::Value = util::nominatim_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx("Store-Finder", "Koordinaten rückwärts geocodieren", &url))?
        .error_for_status()
        .with_context(|| util::ctx("Store-Finder", "Reverse-Geocoding (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx("Store-Finder", "Reverse-Geocoding JSON parsen", &url))?;
    Ok(parse_nominatim_reverse(&raw))
}

/// PLZ und Stadt aus einer `/reverse`-Antwort.
///
/// Eigener Parser, und das ist keine Doppelung: **`/reverse` liefert ein
/// Objekt, `/search` ein Array.** [`parse_nominatim`] und
/// [`parse_nominatim_city`] beginnen beide mit `as_array()?` und gäben für
/// eine Reverse-Antwort still `None` zurück — ein Fehler, der wie „diese
/// Gegend kennt Nominatim nicht" aussähe und nirgends auffiele.
pub fn parse_nominatim_reverse(raw: &serde_json::Value) -> (Option<String>, Option<String>) {
    let Some(address) = raw.get("address") else {
        return (None, None);
    };
    let field = |key: &str| {
        address
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let city = ["city", "town", "village", "municipality"].iter().find_map(|key| field(key));
    (field("postcode"), city)
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

/// Absatzregion („AR") der nächsten Lidl-Filiale.
///
/// Lidl schneidet seine Wochenprospekte nach Absatzregionen zu, und dasselbe
/// Filialdataset, das wir für die Filialsuche ohnehin abfragen, trägt die
/// Region im Feld `AR` (Dresden 20, Passau 31, Köln 42, München 9 — gemessen
/// 2026-07-25). Damit ist der Prospekt regionsgenau adressierbar, ohne eine
/// zweite Quelle. Genutzt von [`crate::scrapers::lidl_prospekt`].
///
/// Achtung: `$top=1` nimmt die *nächste* Filiale. In großen Städten können
/// Filialen im selben Umkreis verschiedene ARs tragen (Hamburg: 446 in
/// St. Georg, 15 in Eimsbüttel). Solange Angebote an der PLZ hängen, ist die
/// nächste Filiale die beste verfügbare Näherung; unter Phase 11 (Angebote
/// gehören der Filiale) wandert die AR an die einzelne Filiale.
pub fn lidl_region_code(plz: &str) -> Result<Option<String>> {
    let (lat, lon) = geocode_plz(plz)?;
    let url = format!(
        "https://spatial.virtualearth.net/REST/v1/data/{LIDL_DATASET}/Filialdaten-SEC/Filialdaten-SEC\
         ?key={LIDL_KEY}&$filter=Adresstyp%20Eq%201&spatialFilter=nearby({lat},{lon},{CUTOFF_KM})\
         &$select=EntityID,AR&$format=json&$top=1"
    );
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx("Lidl", "Absatzregion", &url))?
        .error_for_status()
        .with_context(|| util::ctx("Lidl", "Absatzregion (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx("Lidl", "Absatzregion JSON parsen", &url))?;
    Ok(parse_region_code(&raw))
}

/// AR-Feld der ersten Filiale. Bing liefert es je nach Datensatz als Zahl
/// oder als String, deshalb beide Formen.
pub fn parse_region_code(raw: &serde_json::Value) -> Option<String> {
    let store = raw.pointer("/d/results")?.as_array()?.first()?;
    match store.get("AR")? {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
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
    parse_uberall(&raw, (lat, lon), chain, id_prefix)
}

/// Nächste Filiale einer Uberall-Antwort als Market, sofern innerhalb des
/// Cutoffs; None ohne Treffer im Umkreis.
///
/// Die Entfernung kommt aus `distance` (Meter). Fehlt das Feld, wird sie aus
/// den Filialkoordinaten gegen `origin` (die Koordinaten der PLZ) gerechnet —
/// die Abfrage filtert serverseitig nicht nach Radius (`max=1` ohne
/// Umkreisparameter), ein ungeprüfter Treffer machte die Kette also überall
/// vertreten. Ohne beides ist die Antwort unbrauchbar: Err, damit `resolve`
/// mit dem nationalen Platzhalter greift statt still zu raten.
/// Entfernung eines Uberall-Treffers zum Suchmittelpunkt in km, oder None,
/// wenn sie sich nicht bestimmen lässt.
///
/// Uberall liefert `distance` in Metern — aber nicht immer, und die Abfrage
/// filtert serverseitig nicht nach Radius. Fehlt das Feld, wird die Entfernung
/// aus den Filialkoordinaten gerechnet. Beides zu haben ist der Normalfall;
/// keins von beidem heißt, dass über die Nähe dieser Filiale nichts bekannt
/// ist — was die beiden Aufrufer unterschiedlich behandeln müssen, siehe dort.
fn uberall_distance_km(store: &serde_json::Value, origin: (f64, f64)) -> Option<f64> {
    if let Some(meters) = store.get("distance").and_then(|v| v.as_f64()) {
        return Some(meters / 1000.0);
    }
    let lat = store.get("lat").and_then(|v| v.as_f64())?;
    let lng = store.get("lng").and_then(|v| v.as_f64())?;
    Some(distance_km(origin, (lat, lng)))
}

pub fn parse_uberall(
    raw: &serde_json::Value,
    origin: (f64, f64),
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
    let lat = store.get("lat").and_then(|v| v.as_f64());
    let lng = store.get("lng").and_then(|v| v.as_f64());
    let dist_km = uberall_distance_km(store, origin).with_context(|| {
        format!("Uberall-Treffer ohne distance und ohne Koordinaten — Entfernung nicht prüfbar: {store}")
    })?;
    if dist_km > CUTOFF_KM {
        return Ok(None);
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
    Ok(Some(Market::new(id, name).with_geo(lat, lng)))
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
    parse_uberall_branches(&raw, (lat, lon), chain, id_prefix, radius_km)
}

/// Alle Filialen einer Uberall-Antwort innerhalb des Radius.
///
/// Zeilen ohne `identifier` fallen raus: [`parse_uberall`] setzt für den
/// Angebots-Pfad ersatzweise den nationalen Platzhalter `<prefix>_DE`, aber
/// ein Verzeichniseintrag ohne echte Filial-ID wäre eine Filiale, die es
/// nirgends gibt.
pub fn parse_uberall_branches(
    raw: &serde_json::Value,
    origin: (f64, f64),
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
        // Dieselbe Entfernungsrechnung wie in `parse_uberall`, aber eine
        // andere Antwort auf "Entfernung unbekannt": Dort IST der eine
        // Treffer das Ergebnis, ein ungeprüfter macht die Kette in der Region
        // vertreten — also lieber ein Fehler. Hier ist er eine Zeile von
        // vielen, und die ganze Liste wegen einer Datenlücke fallen zu
        // lassen wäre schlimmer, als die Zeile zu behalten (`branches::within`
        // entscheidet später genauso).
        .filter(|store| {
            uberall_distance_km(store, origin).is_none_or(|km| km <= radius_km)
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

// ---------------------------------------------------------------- NORMA

const NORMA_SEARCH_URL: &str = "https://www.norma-online.de/de/filialfinder/";
/// Der einzige Endpunkt auf norma-online.de, der eine PHP-Sitzung eröffnet —
/// siehe [`norma_search`].
const NORMA_SESSION_URL: &str = "https://www.norma-online.de/ext/ajax/get_wishlist.php";

/// Nächste NORMA-Filiale zur PLZ, oder None ohne Filiale im Umkreis
/// (`CUTOFF_KM`). Für `find_market`: „ist die Kette hier vertreten".
pub fn norma_branch(plz: &str) -> Result<Option<Market>> {
    let html = norma_search(plz, CUTOFF_KM)?;
    Ok(parse_norma(&html, CUTOFF_KM).into_iter().next().map(|s| s.into_market()))
}

/// Alle NORMA-Filialen im Umkreis der PLZ, für das Verzeichnis.
pub fn norma_branches(plz: &str, radius_km: f64, limit: usize) -> Result<Vec<Branch>> {
    let html = norma_search(plz, radius_km)?;
    Ok(parse_norma(&html, radius_km).into_iter().take(limit).map(NormaStore::into_branch).collect())
}

/// Trefferseite des NORMA-Filialfinders zu einer PLZ, als HTML.
///
/// **Zwei Requests, und der erste ist kein Versehen.** Der Filialfinder ist ein
/// klassisches POST-Formular (`filialfinder[suche][plz]`, Radius in **Metern**);
/// die Suche landet in der PHP-Sitzung, und die Trefferseite hinter dem
/// 302-Redirect liest sie von dort. Ohne Sitzungscookie antwortet der Server
/// mit `?info=nosearch` — also mit einer leeren Seite und HTTP 200, nicht mit
/// einem Fehler. Und ein `PHPSESSID` bekommt man auf norma-online.de nirgends
/// sonst: Weder `/de/filialfinder/`, noch `/de/angebote/`, noch die
/// Trefferseite selbst setzen eines (gemessen 2026-07-31, alle vier geprüft) —
/// nur der Einkaufslisten-Endpunkt, den der Seitenkopf ohnehin auf jeder Seite
/// aufruft. Deshalb steht er hier als Handschlag davor.
///
/// Der ältere Weg über `suchergebnis?lat=…&lng=…&r=…` bleibt bewusst
/// ungenutzt: Er kommt ohne Sitzung aus, ignoriert aber den Radius und liefert
/// **immer nur die nächstgelegene** Filiale (2026-07-31 an sieben Orten
/// geprüft). Der POST-Weg liefert für dieselbe PLZ 01219 im 25-km-Kreis neun
/// Filialen und spart obendrein den Nominatim-Request, weil er die PLZ als
/// Text nimmt.
fn norma_search(plz: &str, radius_km: f64) -> Result<String> {
    util::polite_pause(NORMA_SESSION_URL);
    let client = util::blocking_client()?;
    let session = client
        .get(NORMA_SESSION_URL)
        .send()
        .with_context(|| util::ctx("NORMA", "Sitzung eröffnen", NORMA_SESSION_URL))?;
    let cookie = session
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.split(';').next())
        .filter(|c| c.starts_with("PHPSESSID="))
        .map(str::to_string)
        .with_context(|| {
            format!(
                "[NORMA] Filialsuche: {NORMA_SESSION_URL} hat kein PHPSESSID gesetzt — \
                 ohne Sitzung liefert die Suche eine leere Trefferseite"
            )
        })?;

    let radius_m = (radius_km * 1000.0).round() as u32;
    util::polite_pause(NORMA_SEARCH_URL);
    let html = client
        .post(NORMA_SEARCH_URL)
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("filialfinder[suche][land]", "Deutschland"),
            ("filialfinder[suche][radius]", &radius_m.to_string()),
            ("filialfinder[suche][plz]", plz),
            ("filialfinder[suche][stadt]", ""),
            ("filialfinder[suche][strasse]", ""),
        ])
        .send()
        .with_context(|| util::ctx("NORMA", "Filialsuche", NORMA_SEARCH_URL))?
        .error_for_status()
        .with_context(|| util::ctx("NORMA", "Filialsuche (HTTP-Status)", NORMA_SEARCH_URL))?
        .text()
        .with_context(|| util::ctx("NORMA", "Filialsuche lesen", NORMA_SEARCH_URL))?;
    Ok(html)
}

/// Eine Filiale aus der Filialfinder-Antwort.
pub struct NormaStore {
    pub id: String,
    pub street: Option<String>,
    pub plz: Option<String>,
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub distance_km: f64,
}

impl NormaStore {
    fn label(&self) -> String {
        match &self.city {
            Some(c) => format!("NORMA {c}"),
            None => "NORMA".to_string(),
        }
    }

    pub fn into_market(self) -> Market {
        Market::new(format!("NORMA_{}", self.id), self.label()).with_geo(self.lat, self.lon)
    }

    pub fn into_branch(self) -> Branch {
        let (id, name) = (format!("NORMA_{}", self.id), self.label());
        Branch::new(id, "NORMA", name, "norma-filialfinder")
            .with_address(self.street, self.plz, self.city)
            .with_geo(self.lat, self.lon)
    }
}

/// Filialen aus der HTML-Antwort des NORMA-Filialfinders.
///
/// Nicht über die `<div>`-Verschachtelung geparst, obwohl der
/// alltheplaces-Spider `norma_de` das tut: Jede Filialkarte trägt ihre Daten
/// **zusätzlich** als URL-kodiertes JSON im `showMap(...)`-Link — mit
/// Filial-ID, Straße, PLZ, Ort, Koordinaten und Entfernung in einem Stück.
/// Das ist die belastbarere Quelle; ein Umbau des Kachel-Layouts bricht sie
/// nicht.
///
/// `radius_km` wird hier **nochmals** angewandt, obwohl der POST-Weg den
/// Radius serverseitig respektiert (2026-07-31 gemessen: PLZ 01219 liefert bei
/// 15 km sieben, bei 25 km neun, bei 50 km 22 Filialen). Die Antwort ist die
/// Quelle für den Umkreis, aber `geoDistance` steht an jeder Filiale — sie
/// gegen den gefragten Radius zu prüfen kostet nichts und fängt den Tag ab, an
/// dem der Server den Parameter wieder ignoriert.
pub fn parse_norma(html: &str, radius_km: f64) -> Vec<NormaStore> {
    let mut out = Vec::new();
    for raw in html.split("showMap(").skip(1) {
        let Some(encoded) = raw.split(')').next() else { continue };
        let decoded = percent_decode(encoded);
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&decoded) else { continue };

        let text = |key: &str| {
            v.get(key)
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let Some(id) = text("fmsLocationId") else { continue };
        // geoDistance steht in Metern; fehlt sie, gilt die Filiale als nah —
        // dieselbe Abwägung wie bei Uberall (lieber ein zu breiter Eintrag als
        // eine stumm verschwundene Kette).
        let distance_km =
            v.get("geoDistance").and_then(|x| x.as_f64()).map_or(0.0, |m| m / 1000.0);
        if distance_km > radius_km {
            continue;
        }
        out.push(NormaStore {
            id,
            street: text("fmsGeoStreet"),
            plz: text("fmsPostalCode"),
            city: text("fmsCity"),
            lat: v.pointer("/geoCoordinate/latitude").and_then(|x| x.as_f64()),
            lon: v.pointer("/geoCoordinate/longitude").and_then(|x| x.as_f64()),
            distance_km,
        });
    }
    out.sort_by(|a, b| a.distance_km.total_cmp(&b.distance_km));
    out
}

/// Prozent-Dekodierung im Formular-Stil (`+` ist ein Leerzeichen).
///
/// Von Hand statt per Crate, weil es die einzige Stelle im Repo ist, die das
/// braucht — dieselbe Abwägung wie beim Jitter in `util.rs`, der ohne `rand`
/// auskommt. Ungültige Sequenzen bleiben unverändert stehen, damit ein
/// kaputtes Byte nicht die ganze Filiale kostet.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------- Fallback

/// Großkreis-Distanz zweier Koordinaten in km (Haversine).
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    const R: f64 = 6371.0;
    let (dlat, dlon) = ((b.0 - a.0).to_radians(), (b.1 - a.1).to_radians());
    let h = (dlat / 2.0).sin().powi(2)
        + a.0.to_radians().cos() * b.0.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * h.sqrt().asin()
}

/// Finder-Ergebnis auf das Sync-Verhalten abbilden: Treffer -> echte Filiale,
/// sauberes "keine Filiale" -> None (Kette wird für die Region nicht
/// registriert), Fehler -> WARN + nationaler Platzhalter.
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
