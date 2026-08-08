//! Wie `dump_tags`, aber für das Regal: gibt je Angebot `titel \t kategorie
//! \t emoji` als TSV aus, damit sich ein Eingriff in die `RULES`-Tabelle
//! zeilenweise vorher/nachher vergleichen lässt.
//!
//! Die Zahl der Zeilen unter „Sonstiges" allein genügt als Nachweis nicht —
//! sie sagt nichts über die Zeilen, die dabei in ein **falsches** Regal
//! rutschen. Gemessen wird deshalb jeder Übergang.
use lechariot::enrich::enrich;

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
    for (title, sub, cat) in &rows {
        let e = enrich(
            title,
            (!sub.is_empty()).then_some(sub.as_str()),
            (!cat.is_empty()).then_some(cat.as_str()),
        );
        println!("{}\t{}\t{}", title.replace('\t', " "), e.category, e.emoji);
    }
}
