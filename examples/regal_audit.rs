//! Zählt die Zeilen, die das Wörterbuch als Essen erkennt und die trotzdem
//! im Regal „Sonstiges" landen — dem Topf, den sich das Unsortierbare mit
//! der Aktionsware teilt.
//!
//! Seit [#64] entscheidet das Wörterbuch, was Essen ist, und `enrich` holt
//! sich das Regal über `kategorie_fuer_begriff`. Kennt die `RULES`-Tabelle
//! den Begriff nicht — oder gibt sie ihm ausdrücklich „Sonstiges" —, bleibt
//! die Zeile liegen. Genau diese Fälle listet der Lauf, nach Begriff
//! gruppiert, damit vorher und nachher vergleichbar sind.
//!
//! `cargo run --example regal_audit [pfad/zur/lechariot.db]`
use lechariot::enrich::enrich;
use lechariot::matching::{match_keys, NONFOOD_KEY};
use std::collections::BTreeMap;

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| {
        std::env::var("HOME").unwrap() + "/.local/share/lechariot/lechariot.db"
    });
    let conn = rusqlite::Connection::open(&db).unwrap();
    let mut stmt = conn
        .prepare(
            "select o.title, coalesce(o.subtitle,''), coalesce(o.category,'')
             from offers o order by o.id",
        )
        .unwrap();
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    let mut je_begriff: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut essen = 0usize;
    for (title, sub, cat) in &rows {
        let sub = (!sub.is_empty()).then_some(sub.as_str());
        let cat = (!cat.is_empty()).then_some(cat.as_str());
        let tags = match_keys(title, sub, cat);
        if tags.is_empty() || tags == [NONFOOD_KEY] {
            continue;
        }
        essen += 1;
        if enrich(title, sub, cat).category != "Sonstiges" {
            continue;
        }
        // Der Begriff, der das Regal hätte nennen müssen — bei mehreren Tags
        // stehen alle da, sonst liest sich die Ursache falsch.
        je_begriff.entry(tags.join("+")).or_default().push(title.clone());
    }

    let betroffen: usize = je_begriff.values().map(Vec::len).sum();
    println!("{} Angebote, {essen} davon vom Wörterbuch als Essen erkannt", rows.len());
    println!("{betroffen} Zeilen Essen stehen unter „Sonstiges\"\n");
    for (begriff, titel) in &je_begriff {
        println!("  {:24} {:2}  z.B. {}", begriff, titel.len(), titel[0]);
        for t in titel.iter().skip(1) {
            println!("  {:24}      {}", "", t);
        }
    }
}
