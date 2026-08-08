//! Dumps the Rust tags per offer as TSV so the Python reference can be
//! diffed row by row. Measuring tool for the dictionary maintenance rounds —
//! it is what makes the per-row parity check repeatable.
use lechariot::matching::match_keys;

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| {
        std::env::var("HOME").unwrap() + "/.local/share/lechariot/lechariot.db"
    });
    // `--alle` misst den ganzen Korpus statt der heute gültigen Woche —
    // dieselbe Schaltung wie in der Python-Referenz, und aus demselben Grund:
    // Sobald die Eval-DB älter ist als die laufende Woche, findet der Filter
    // null Zeilen und der Vergleich vergleicht zwei leere Dateien.
    let alle = std::env::args().any(|a| a == "--alle");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let mut stmt = conn
        .prepare(&format!(
            "select o.title, coalesce(o.subtitle,''), coalesce(o.category,'')
             from offers o {} order by o.id",
            if alle { "" } else { "where o.valid_until >= date('now')" }
        ))
        .unwrap();
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for (title, sub, cat) in &rows {
        let mut k = match_keys(title, Some(sub), Some(cat));
        k.sort();
        println!(
            "{}\t{}\t{}\t{}",
            title.replace('\t', " "),
            sub.replace('\t', " "),
            cat.replace('\t', " "),
            k.join(",")
        );
    }
}
