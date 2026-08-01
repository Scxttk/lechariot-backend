//! Wie `dump_tags`, aber mit der Marke als Rückfall — und es druckt **beide**
//! Ergebnisse je Zeile, damit der Vergleich nicht zwei Läufe braucht.
//!
//! Ausgabe: `titel \t untertitel \t marke \t vorher \t nachher`.
//! Der Nachweis ist der Vergleich **aller** Übergänge, nicht die Zahl der
//! Ungetaggten — der Fehler vom 01.08. war, „Tag verloren: 0" zu zählen und
//! damit nur „auf leer gefallen" zu meinen.
use lechariot::matching::{match_keys, match_keys_with_brand};

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| {
        std::env::var("HOME").unwrap() + "/.local/share/lechariot/lechariot.db"
    });
    let conn = rusqlite::Connection::open(&db).unwrap();
    let mut stmt = conn
        .prepare(
            "select o.title, coalesce(o.subtitle,''), coalesce(o.category,''),
                    coalesce(o.overline,''), m.chain
             from offers o join markets m on m.id = o.market_id
             where o.valid_until >= date('now') order by o.id",
        )
        .unwrap();
    let rows: Vec<(String, String, String, String, String)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    for (title, sub, cat, overline, chain) in &rows {
        // Dieselbe Regel wie `push::brand_of`: nur ALDI Nord legt in
        // `overline` wirklich die Marke ab.
        let brand = (chain == "ALDI Nord").then_some(overline.as_str()).filter(|s| !s.is_empty());
        let mut before = match_keys(title, Some(sub), Some(cat));
        let mut after = match_keys_with_brand(title, Some(sub), Some(cat), brand);
        before.sort();
        after.sort();
        println!(
            "{}\t{}\t{}\t{}\t{}",
            title.replace('\t', " "),
            sub.replace('\t', " "),
            brand.unwrap_or("").replace('\t', " "),
            before.join(","),
            after.join(",")
        );
    }
}
