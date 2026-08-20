//! Schreibt das Feld `warengruppe` in `docs/matching-woerterbuch.json`.
//!
//! **Warum das Wörterbuch und keine zweite Datei.** Die App braucht für einen
//! Artikel ohne Wochenangebot ein Regal (App-#157) und für die Reihenfolge der
//! Treffer die Frage „ist das Angebot überhaupt die gesuchte Ware?" (App-#106).
//! Beides beantwortet `enrich::kategorie_fuer_begriff` schon — nur kam die
//! Antwort bisher nie über die Ketten-Angebote hinaus. Sie reist deshalb im
//! Wörterbuch mit, das ohnehin zwischen beiden Repos abgeglichen wird
//! (`woerterbuch-sync.yml`); eine eigene Datei wäre ein zweiter Abgleich und
//! die dritte Stelle, an der Warenkunde gepflegt wird.
//!
//! Geschrieben wird **nicht** von Hand: `cargo run --example warengruppen`
//! setzt das Feld neu, `tests/warengruppen.rs` hält es an der Funktion fest.
//!
//! Begriffe, denen die Tabelle kein Regal gibt, bekommen kein Feld — ein
//! leerer String wäre eine Behauptung, `null` eine Zeile mehr im Diff.

use std::fs;

use anyhow::{Context, Result};
use lechariot::enrich;
use serde_json::Value;

const PFAD: &str = "docs/matching-woerterbuch.json";

fn main() -> Result<()> {
    let roh = fs::read_to_string(PFAD).with_context(|| format!("{PFAD} lesen"))?;
    let mut wb: Value = serde_json::from_str(&roh).with_context(|| format!("{PFAD} parsen"))?;

    let begriffe = wb["begriffe"]
        .as_object_mut()
        .context("Sektion 'begriffe' fehlt")?;

    let (mut gesetzt, mut ohne) = (0usize, Vec::new());
    for (begriff, eintrag) in begriffe.iter_mut() {
        let Some(objekt) = eintrag.as_object_mut() else { continue };
        match enrich::kategorie_fuer_begriff(begriff) {
            Some(regal) => {
                objekt.insert("warengruppe".into(), Value::String(regal.to_string()));
                gesetzt += 1;
            }
            None => {
                objekt.remove("warengruppe");
                ohne.push(begriff.clone());
            }
        }
    }

    // Einrückung mit EINEM Leerzeichen — so liegt die Datei im Repo, und der
    // Abgleich mit der App-Kopie vergleicht Bytes.
    let mut ausgabe = Vec::new();
    let formatierer = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut ausgabe, formatierer);
    serde::Serialize::serialize(&wb, &mut ser)?;
    ausgabe.push(b'\n');
    fs::write(PFAD, &ausgabe).with_context(|| format!("{PFAD} schreiben"))?;

    println!("{gesetzt} Begriffe tragen jetzt eine Warengruppe, {} ohne.", ohne.len());
    if !ohne.is_empty() {
        println!("Ohne Regal: {}", ohne.join(", "));
    }
    Ok(())
}
