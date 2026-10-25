//! Stichprobe: Sprachmodell gegen Wörterbuch für die Angebote einer Kette.
//!
//! Ruft `fetch` vorher in eine eigene Datenbank ab und schreibt jedes Angebot,
//! dessen Tags (`replace`) als Menge vom Wörterbuch abweichen, als JSON nach
//! stdout. Eigener Cache, damit die Nightly davon nichts sieht.
//!
//! ```
//! lechariot fetch --store penny --zip 80331 --db /tmp/stichprobe.db
//! cargo run --release --example llm_stichprobe -- /tmp/stichprobe.db /tmp/cache.json Penny
//! ```
use std::collections::BTreeSet;

use lechariot::llm_tags::{Mode, Tagger};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let [_, db, cache, chain] = a.as_slice() else {
        panic!("Aufruf: llm_stichprobe <db> <cache.json> <Kette>");
    };
    let conn = lechariot::db::open(db).unwrap();
    let inputs: Vec<_> = lechariot::db::export_offers(&conn, None)
        .unwrap()
        .iter()
        .filter(|o| o.price.is_some())
        .map(|o| lechariot::push::llm_input(o, chain))
        .collect();
    let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY fehlt");
    let mut t = Tagger::new("claude-haiku-5-5".into(), key, Mode::Replace, cache.into(), 8000);
    let u = t.ensure(&inputs);
    let menge = |v: &[String]| v.iter().cloned().collect::<BTreeSet<_>>();
    let abweichend: Vec<_> = inputs
        .iter()
        .filter(|i| menge(&t.apply(i)) != menge(&i.dict))
        .map(|i| serde_json::json!({"title": i.title, "sub": i.subtitle, "cat": i.category,
                                     "dict": i.dict, "llm": t.apply(i)}))
        .collect();
    eprintln!(
        "{} Anfragen ({} fehlgeschlagen), ${:.4}; {} von {} Angeboten weichen ab.",
        u.requests, u.failed_requests, u.dollars(), abweichend.len(), inputs.len()
    );
    println!("{}", serde_json::to_string_pretty(&abweichend).unwrap());
}
