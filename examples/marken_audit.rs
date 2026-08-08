//! Prüflauf über den riskantesten Weg des Wörterbuchs: den Marken-Rückfall.
//!
//! Die Markenliste vergibt ein Tag anhand eines Herstellernamens statt anhand
//! des Produkts. Sie wird von niemandem gemeldet, weil ihre Fehler plausibel
//! aussehen — „lor" (L'OR) machte fünf Lorenz-Zeilen zu Kaffee, und das fiel
//! erst beim Messen auf, nicht im Feedback. Dieser Lauf macht den Weg
//! sichtbar, statt auf eine Meldung zu warten.
//!
//! Drei Abschnitte, alle über den ganzen Korpus:
//!
//! 1. **Teilzeichenketten-Fallen** — Marken, die im Titel nur als Wortteil
//!    stecken. Seit [#61] greift der Rückfall nur auf Wortgrenze; hier steht,
//!    was diese Regel jeweils verhindert. Wächst die Liste, ist eine neue
//!    Marke zu kurz oder zu gewöhnlich geraten.
//! 2. **Kurze Marken** (≤ 4 Zeichen) — die Klasse, die fast immer eine Falle
//!    ist, mit ihren echten Treffern im Korpus.
//! 3. **Alle greifenden Marken** — jede Zeile, die ihr Tag wirklich über die
//!    Marke bekommen hat, mit Titel und Tag zum Gegenlesen.
//!
//! `cargo run --example marken_audit [pfad/zur/lechariot.db]`
use lechariot::matching::{match_keys, NONFOOD_KEY};
use std::collections::BTreeMap;

/// Normalisierung wie `matching::norm` — der Vergleich muss auf demselben
/// Text stattfinden wie im Rückfall, sonst misst der Lauf etwas anderes als
/// die Nightly taggt.
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.to_lowercase().chars() {
        match c {
            '®' | '*' | '™' => {}
            '-' => out.push(' '),
            'é' | 'è' | 'ê' => out.push('e'),
            'á' | 'à' | 'â' => out.push('a'),
            'í' | 'ì' => out.push('i'),
            'ó' | 'ò' => out.push('o'),
            'ú' | 'ù' => out.push('u'),
            'a'..='z' | 'ä' | 'ö' | 'ü' | 'ß' | ' ' => out.push(c),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn als_wort(ntext: &str, nadel: &str) -> bool {
    if nadel.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(pos) = ntext[from..].find(nadel) {
        let start = from + pos;
        let end = start + nadel.len();
        let davor = ntext[..start].chars().next_back();
        let danach = ntext[end..].chars().next();
        if !davor.is_some_and(char::is_alphabetic) && !danach.is_some_and(char::is_alphabetic) {
            return true;
        }
        from = start + nadel.chars().next().map_or(1, char::len_utf8);
    }
    false
}

fn main() {
    let db = std::env::args().nth(1).filter(|a| !a.starts_with("--")).unwrap_or_else(|| {
        std::env::var("HOME").unwrap() + "/.local/share/lechariot/lechariot.db"
    });
    let dict: serde_json::Value =
        serde_json::from_str(include_str!("../docs/matching-woerterbuch.json")).unwrap();
    // Dieselbe Reihenfolge wie im Rückfall: längste Marke zuerst.
    let mut marken: Vec<(String, String)> = dict["marken"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(b, t)| {
            let b = norm(b);
            (!b.is_empty()).then(|| (b, t.as_str().unwrap_or("").to_string()))
        })
        .collect();
    marken.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));

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

    let mut fallen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut treffer: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut ueber_marke = 0usize;

    for (title, sub, cat) in &rows {
        let sub_o = (!sub.is_empty()).then_some(sub.as_str());
        let cat_o = (!cat.is_empty()).then_some(cat.as_str());
        let text = match sub_o {
            Some(s) => format!("{title} {s}"),
            None => title.clone(),
        };
        let ntext = norm(&text);
        let tags = match_keys(title, sub_o, cat_o);

        // Abschnitt 1: was die Wortgrenze abfängt. Die Marke steckt im Text,
        // aber nicht als Wort — ohne die Regel hätte sie das Tag gestellt.
        for (marke, term) in &marken {
            if ntext.contains(marke.as_str()) && !als_wort(&ntext, marke) {
                fallen
                    .entry(format!("{marke} → {term}"))
                    .or_default()
                    .push(title.clone());
            }
        }

        // Abschnitt 3: hat diese Zeile ihr Tag WIRKLICH über die Marke? Der
        // Rückfall läuft nur, wenn Titel und Untertitel nichts hergeben —
        // nachgestellt, indem ohne Kategorie und ohne Marke gefragt wird.
        if tags.len() == 1 && tags[0] != NONFOOD_KEY {
            if let Some((marke, term)) = marken
                .iter()
                .find(|(m, t)| *t != "NONFOOD" && *t == tags[0] && als_wort(&ntext, m))
            {
                // Nur zählen, wenn der Titel den Begriff nicht selbst nennt —
                // „Milka Schokolade" trägt sein Tag über das Wort Schokolade.
                if !ntext.contains(term.as_str()) {
                    ueber_marke += 1;
                    treffer
                        .entry(marke.clone())
                        .or_default()
                        .push((title.clone(), term.clone()));
                }
            }
        }
    }

    println!("== Abschnitt 1: Teilzeichenketten-Fallen (von der Wortgrenze abgefangen) ==");
    if fallen.is_empty() {
        println!("  keine");
    }
    for (marke, titel) in &fallen {
        println!("  {:28} {:2}x  z.B. {}", marke, titel.len(), titel[0]);
    }

    println!("\n== Abschnitt 2: Marken mit höchstens 4 Zeichen ==");
    for (marke, term) in marken.iter().filter(|(m, _)| m.chars().count() <= 4) {
        let t = treffer.get(marke).map(Vec::len).unwrap_or(0);
        let bsp = treffer
            .get(marke)
            .and_then(|v| v.first())
            .map(|(t, _)| t.as_str())
            .unwrap_or("—");
        println!("  {:10} → {:14} {:2} Treffer  {}", marke, term, t, bsp);
    }

    println!("\n== Abschnitt 3: {ueber_marke} Zeilen tragen ihr Tag über die Marke ==");
    for (marke, zeilen) in &treffer {
        for (title, term) in zeilen {
            println!("  {:16} → {:14} {}", marke, term, title);
        }
    }
}
