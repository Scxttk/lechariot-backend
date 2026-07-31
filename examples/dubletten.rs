//! Zählt die Dubletten in `public.offers` — **nur lesend**, kein Write, kein
//! DDL. Das Messwerkzeug zur Backlog-Frage „Dubletten-Check: prüfen, ob der
//! `market_id`-Schlüssel das erledigt hat".
//!
//! Die Einteilung kommt nicht aus einer nachgebauten Regel, sondern aus
//! [`lechariot::push::dedupe_rows`] — derselben Funktion, die der Lauf vor dem
//! Upsert benutzt, und derselben Regel wie Migration v23. Nachgebaut liefe die
//! Messung sonst irgendwann neben dem Code her, und genau das ist der Grund,
//! warum die alte Erklärung im Log („ein zweiter Sync-Lauf derselben KW")
//! zwei Wochen lang unwidersprochen falsch war.
//!
//!   SUPABASE_URL=… SUPABASE_SERVICE_KEY=… cargo run --example dubletten
//!
//! Ausgabe: Zeilen, Nenner (die „Faktor"-Zahlen aus dem Log), die drei Sorten
//! Mehrfachzeilen und was Migration v23 heute löschen würde.

use std::collections::{BTreeMap, HashMap, HashSet};

use lechariot::push::{SupabaseRow, config_from_env};

/// Was PostgREST je Seite höchstens liefert.
const PAGE: usize = 1000;

const COLUMNS: &str = "market_id,market,product,price,regular_price,unit,category,emoji,\
image_url,valid_from,valid_until,base_price,base_unit,brand,ean,source,nationwide,match_key,\
created_at";

fn main() -> anyhow::Result<()> {
    let cfg = config_from_env()?;
    let client = reqwest::blocking::Client::new();

    let mut rows: Vec<SupabaseRow> = Vec::new();
    // `created_at` gehört nicht zu SupabaseRow (die Spalte schreibt die
    // Datenbank), wird aber gebraucht, um zu sagen, WANN eine Dublette
    // entstanden ist. Deshalb daneben, über den Zeilenschlüssel.
    let mut created: HashMap<(String, String, String), String> = HashMap::new();
    let mut offset = 0usize;
    loop {
        let resp = client
            .get(format!("{}/rest/v1/offers", cfg.base_url))
            .header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .header("Range", format!("{offset}-{}", offset + PAGE - 1))
            .query(&[("select", COLUMNS), ("order", "id")])
            .send()?;
        if !resp.status().is_success() {
            anyhow::bail!("offers lesen fehlgeschlagen (HTTP {})", resp.status());
        }
        let page: Vec<serde_json::Value> = resp.json()?;
        let n = page.len();
        for value in page {
            let stamp =
                value.get("created_at").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let row: SupabaseRow = serde_json::from_value(value)?;
            created.insert(key_of(&row), stamp);
            rows.push(row);
        }
        offset += n;
        if n < PAGE {
            break;
        }
    }

    let total = rows.len();
    println!("Zeilen in public.offers: {total}");

    let filialen: HashSet<&str> = rows.iter().map(|r| r.market_id.as_str()).collect();
    let je_filiale: HashSet<(&str, &str)> =
        rows.iter().map(|r| (r.market_id.as_str(), r.product.as_str())).collect();
    let je_kette: HashSet<(&str, &str)> =
        rows.iter().map(|r| (r.market.as_str(), r.product.as_str())).collect();
    let produkte: HashSet<&str> = rows.iter().map(|r| r.product.as_str()).collect();

    println!("Filialen: {}", filialen.len());
    println!(
        "je Filiale (market_id, product): {} — Faktor {:.4}",
        je_filiale.len(),
        total as f64 / je_filiale.len() as f64
    );
    println!(
        "je Kette (market, product):      {} — Faktor {:.2}",
        je_kette.len(),
        total as f64 / je_kette.len() as f64
    );
    println!(
        "verschiedene Produkte:           {} — Faktor {:.2}",
        produkte.len(),
        total as f64 / produkte.len() as f64
    );
    println!(
        "  (die beiden letzten Faktoren messen die Streuung über die Filialen, \n   nicht Dubletten — der Nenner der Log-Zahlen 5,1 und 5,3.)"
    );

    // ---------------------------------------------------------------- Sorten
    let mut nach_kombination: BTreeMap<(&str, &str), Vec<&SupabaseRow>> = BTreeMap::new();
    for row in &rows {
        nach_kombination
            .entry((row.market_id.as_str(), row.product.as_str()))
            .or_default()
            .push(row);
    }
    let mehrfach: Vec<_> = nach_kombination.values().filter(|v| v.len() > 1).collect();
    let mehrfach_zeilen: usize = mehrfach.iter().map(|v| v.len()).sum();
    println!(
        "\nKombinationen mit mehr als einer Zeile: {} ({} Zeilen, {:.2} % der Tabelle)",
        mehrfach.len(),
        mehrfach_zeilen,
        100.0 * mehrfach_zeilen as f64 / total as f64
    );

    // ------------------------------------------------- Was v23 löschen würde
    //
    // Über dieselbe Funktion wie der Lauf: Was `dedupe_rows` wegnimmt, nimmt
    // auch die Migration weg.
    let vorher = rows.len();
    let bereinigt = lechariot::push::dedupe_rows(rows.clone());
    let bleibt: HashSet<(String, String, String)> = bereinigt.iter().map(key_of).collect();
    let entfernt: Vec<&SupabaseRow> =
        rows.iter().filter(|r| !bleibt.contains(&key_of(r))).collect();

    println!(
        "\nMigration v23 (überlappendes Fenster, gleicher Preis) würde {} Zeilen löschen\n  (von {vorher} auf {}).",
        entfernt.len(),
        bereinigt.len()
    );
    let mut je_kette_entfernt: BTreeMap<&str, usize> = BTreeMap::new();
    for row in &entfernt {
        *je_kette_entfernt.entry(row.market.as_str()).or_default() += 1;
    }
    for (kette, n) in &je_kette_entfernt {
        println!("  {kette}: {n}");
    }
    let jüngste = entfernt.iter().filter_map(|r| created.get(&key_of(r))).max();
    match jüngste {
        Some(stamp) => println!(
            "  Jüngste betroffene Zeile angelegt: {stamp}\n  (= wann der Schreibweg zuletzt eine Dublette erzeugt hat)"
        ),
        None => println!("  Keine — der Bestand ist sauber."),
    }

    // Der Rest der Mehrfachzeilen ist ausdrücklich in Ordnung und bleibt
    // stehen: echte Folgewochen und Aktionstage mit eigenem Preis.
    let betroffene_kombinationen: HashSet<(&str, &str)> =
        entfernt.iter().map(|r| (r.market_id.as_str(), r.product.as_str())).collect();
    println!(
        "\nUnangetastet: {} Kombinationen mit disjunkten Fenstern oder eigenem Preis\n  (Folgewochen, Aktionstage, Lidl-Sammeltitel).",
        mehrfach.len() - betroffene_kombinationen.len()
    );
    Ok(())
}

fn key_of(row: &SupabaseRow) -> (String, String, String) {
    (
        row.market_id.clone(),
        row.product.clone(),
        row.valid_from.clone().unwrap_or_default(),
    )
}
