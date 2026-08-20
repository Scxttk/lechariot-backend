//! Das Feld `warengruppe` im Wörterbuch gehört `enrich::kategorie_fuer_begriff`.
//!
//! Geschrieben wird es von `examples/warengruppen.rs`, gelesen von der App
//! (App-#157: ein Artikel ohne Wochenangebot bekommt trotzdem sein Regal;
//! App-#106: Treffer ordnen sich danach, ob sie die gesuchte Ware *sind*).
//! Zwischen Schreiben und Lesen liegt ein Repo-Wechsel — deshalb hält dieser
//! Test die Datei an der Funktion fest, statt sich auf einen Lauf von Hand zu
//! verlassen.

use lechariot::enrich;
use serde_json::Value;

const DICT_JSON: &str = include_str!("../docs/matching-woerterbuch.json");

fn begriffe() -> serde_json::Map<String, Value> {
    let v: Value = serde_json::from_str(DICT_JSON).expect("Wörterbuch ungültig");
    v["begriffe"].as_object().expect("Sektion 'begriffe' fehlt").clone()
}

/// Jeder Begriff trägt genau das Regal, das die Keyword-Tabelle ihm gibt —
/// und keins, wo sie schweigt. Läuft der Test rot, ist die Datei alt:
/// `cargo run --example warengruppen` schreibt sie neu.
#[test]
fn jede_warengruppe_stammt_aus_der_keyword_tabelle() {
    let mut abweichungen = Vec::new();
    for (begriff, eintrag) in begriffe() {
        let gespeichert = eintrag.get("warengruppe").and_then(|v| v.as_str());
        let erwartet = enrich::kategorie_fuer_begriff(&begriff);
        if gespeichert != erwartet {
            abweichungen.push(format!("{begriff}: Datei {gespeichert:?}, Tabelle {erwartet:?}"));
        }
    }
    assert!(
        abweichungen.is_empty(),
        "{} Begriffe weichen ab (cargo run --example warengruppen):\n{}",
        abweichungen.len(),
        abweichungen.join("\n")
    );
}

/// Ein Regal, das die App nicht kennt, ist schlimmer als keins: Sie sortiert
/// danach in einen Abschnitt, den es in ihrer Liste nicht gibt.
#[test]
fn warengruppen_sind_die_fuenfzehn_bekannten() {
    for (begriff, eintrag) in begriffe() {
        let Some(regal) = eintrag.get("warengruppe").and_then(|v| v.as_str()) else { continue };
        assert!(
            enrich::CATEGORIES.contains(&regal),
            "{begriff} zeigt auf '{regal}', das keine der 15 Kategorien ist"
        );
    }
}

/// Der gemeldete Fall aus App-#157, als Zusicherung festgehalten: „Eier" ohne
/// Ei-Angebot in der Woche stand unter „Noch nicht einsortiert", obwohl jeder
/// weiß, wohin Eier gehören.
#[test]
fn eier_kennen_ihr_regal_auch_ohne_angebot() {
    let b = begriffe();
    let regal = |name: &str| {
        b.get(name)
            .and_then(|e| e.get("warengruppe"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    assert_eq!(regal("eier").as_deref(), Some("Molkerei & Eier"));
    assert_eq!(regal("käse").as_deref(), Some("Molkerei & Eier"));
    assert_eq!(regal("brot").as_deref(), Some("Backwaren"));
}
