//! Regelbasiertes Angebots-Tagging mit Alltagsbegriffen (`match_key`).
//!
//! Port der Python-Referenz `docs/matching-woerterbuch-eval.py` — das
//! Wörterbuch `docs/matching-woerterbuch.json` ist die gemeinsame Quelle
//! und wird zur Compile-Zeit eingebettet. Bei Änderungen an Wörterbuch
//! oder Matching-Regeln IMMER beide Seiten anfassen und den Ignore-Test
//! `parity_with_eval_db` gegen die lokale Nightly-DB laufen lassen.
//!
//! Ergebnis pro Angebot: Liste von Begriffs-Tags ("käse", "tomaten", …),
//! `["nonfood"]` für erkanntes Non-Food, leer für ungetaggt (Review-Liste).

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

pub const NONFOOD_KEY: &str = "nonfood";

const DICT_JSON: &str = include_str!("../docs/matching-woerterbuch.json");

// Ketten-Marketing-Kategorien, die klar Non-Food sind.
const NONFOOD_CAT: &str = r"(?i)mode|style|heim|haus|garten|haustier|tierbedarf|tiernahrung|pflanzen|angeln|elektro|medien|kinderzimmer|wäschepflege|schulstart|kochen-und-grillen|drogerie|spielzeug|alltagshelfer|technik|spielwaren|baumarkt|multimedia|bekleidung|schuhe|camping|auto|buero|non.?food";

// Non-Food-Begriffe im Titel (fängt Non-Food in Food-Kategorien wie „Wochenangebote").
const FOOD_CAT: &str = r"(?i)obst|gemüse|fleisch|geflügel|wurst|molkerei|fette|getränke|feinkost|konserven|kaffee|tee|süßwaren|knabber|grundnahrung|fisch|bäckerei|backwaren|tiefkühl";

const NONFOOD_TERMS: &str = r"(?i)lichterkette|lampion|wäschest|wäscheklammer|wäschekorb|kettensäge|akku|werkzeug|kinderbuch|spielzeug|rosen\b|blumen|pflanze|socken|shorts|shirt|cap\b|hose|schuhe|handtuch|bettwäsche|pfannen?\b|topf\b|löffel|messer|grill\b|kohle|batterie|lampe|leuchte|katzen|hunde|tiernahrung|nassfutter|trockenfutter|snack für|rasenkanten|solar|deko|kissen|matratze|drucker|kopfhörer|wc-|reiniger|megaperls|oxi action|schreibwaren|mikrofon|duschregal|sonnensegel|wäscheparf|karaoke|trinkzubehör|wäschetrockner|weißer riese|sonnenspray|duftspüler|sonnencreme|feuchttücher|servietten|haushaltstücher|klumpstreu|geschirrtücher|platzset|schlafsack|fusselrolle|bügeleisen|glasschüssel|lautsprecher|geräusche-box|fliegengitter|kajak|husarenknöpfchen|lavendel|bilderbuch|wecker|hairstyler|bastelkoffer|kochgeschirr|grillplatte|boombox|fliegenfalle|mottenabwehr|badvorleger|schrubber|kosmetikspiegel|shorty|plaid|fototafel|komfort-bh|pantoletten|spannbetttuch|küchentücher|sneaker|hoodie|bodyspray|deospray|sonnenschutz|dutch oven|gläsersortiment|sonnenschirm|tischdecke|fleece|wellnessbürste|maniküre|pediküre|teppich|taillenslip|haftcreme|wasserballon|corega|axe ";

// Tokens, bei denen Suffix-Matching generell verboten ist (falsche Komposita).
const SUFFIX_STOP: &[&str] = &[
    "reis", "preis", "schwein", "schweine", "kreis", "eis", "wein", "hackfleisch", "gehacktes",
    "abwaschbecken",
];

struct Term {
    key: String,
    /// Einwort-Begriffe: Token-Gleichheit. Mehrwort-Begriffe: Substring in ntext.
    exact: Vec<String>,
    /// Komposita-Suffixe (nur ab 4 Zeichen wirksam).
    suffix: Vec<String>,
    block_words: Vec<String>,
    block_phrases: Vec<String>,
}

struct Dict {
    terms: Vec<Term>,
    /// Marke (normalisiert) → Begriff bzw. NONFOOD_KEY; Reihenfolge = JSON-Reihenfolge.
    brands: Vec<(String, String)>,
    nonfood_cat: Regex,
    nonfood_terms: Regex,
    food_cat: Regex,
}

fn dict() -> &'static Dict {
    static DICT: OnceLock<Dict> = OnceLock::new();
    DICT.get_or_init(|| {
        let v: serde_json::Value =
            serde_json::from_str(DICT_JSON).expect("matching-woerterbuch.json ungültig");
        let terms = v["begriffe"]
            .as_object()
            .expect("Sektion 'begriffe' fehlt")
            .iter()
            .map(|(key, def)| {
                let list = |field: &str| -> Vec<String> {
                    def[field]
                        .as_array()
                        .map(|a| {
                            a.iter().filter_map(|s| s.as_str()).map(norm).collect()
                        })
                        .unwrap_or_default()
                };
                let (block_phrases, block_words) =
                    list("block").into_iter().partition(|b| b.contains(' '));
                Term {
                    key: key.clone(),
                    exact: list("exact"),
                    suffix: list("suffix").into_iter().filter(|s| s.chars().count() >= 4).collect(),
                    block_words,
                    block_phrases,
                }
            })
            .collect();
        let brands = v["marken"]
            .as_object()
            .expect("Sektion 'marken' fehlt")
            .iter()
            .filter_map(|(brand, term)| {
                let b = norm(brand);
                let t = term.as_str()?;
                if b.is_empty() {
                    return None;
                }
                let key = if t == "NONFOOD" { NONFOOD_KEY.to_string() } else { t.to_string() };
                Some((b, key))
            })
            .collect();
        // Regexe kommen aus der JSON (eine Quelle mit der Python-Referenz);
        // die Konstanten sind nur Fallback für alte JSON-Stände. Python-
        // Patterns tragen kein eingebettetes (?i), daher hier ergänzen.
        let rx = |field: &str, fallback: &str| -> Regex {
            match v[field].as_str() {
                Some(p) => Regex::new(&format!("(?i){p}")).unwrap(),
                None => Regex::new(fallback).unwrap(),
            }
        };
        Dict {
            terms,
            brands,
            nonfood_cat: rx("nonfood_cat", NONFOOD_CAT),
            nonfood_terms: rx("nonfood_terms", NONFOOD_TERMS),
            food_cat: rx("food_cat", FOOD_CAT),
        }
    })
}

/// Normalisierung wie in der Python-Referenz: lowercase, ®*™ raus,
/// Bindestrich = Leerzeichen, Akzente flachziehen (Chicorée), alles außer
/// a-zäöüß zu Leerzeichen, Whitespace kollabieren.
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

/// Tokens ab 3 Zeichen, plus Plural-Varianten ohne Endungs-s/-n/-e.
fn tokens(ntext: &str) -> Vec<String> {
    let base: Vec<String> = ntext.split(' ').filter(|t| t.chars().count() > 2).map(String::from).collect();
    let mut all = base.clone();
    for t in &base {
        if t.chars().count() > 4 {
            if let Some(last) = t.chars().last() {
                if matches!(last, 's' | 'n' | 'e') {
                    all.push(t[..t.len() - last.len_utf8()].to_string());
                }
            }
        }
    }
    all
}

/// `match_key`-Tags für ein Angebot: Begriffs-Tags, `["nonfood"]` für
/// erkanntes Non-Food, leer für ungetaggt.
pub fn match_keys(title: &str, subtitle: Option<&str>, category: Option<&str>) -> Vec<String> {
    let d = dict();
    let text = match subtitle {
        Some(sub) if !sub.is_empty() => format!("{title} {sub}"),
        _ => title.to_string(),
    };
    // Kategorie-Nonfood nur, wenn die Kategorie keinen Food-Marker trägt —
    // Kauflands Obsttheke heißt „Obst, Gemüse, Pflanzen" und flog sonst
    // komplett über das Wort „Pflanzen" raus (Fund 2026-07-22).
    let cat = category.unwrap_or("");
    if (d.nonfood_cat.is_match(cat) && !d.food_cat.is_match(cat)) || d.nonfood_terms.is_match(&text)
    {
        return vec![NONFOOD_KEY.to_string()];
    }
    let ntext = norm(&text);
    let toks: HashSet<String> = tokens(&ntext).into_iter().collect();

    let mut hits: Vec<String> = Vec::new();
    for term in &d.terms {
        if term.block_phrases.iter().any(|b| ntext.contains(b.as_str()))
            || term.block_words.iter().any(|b| toks.contains(b))
        {
            continue;
        }
        let exact_hit = term
            .exact
            .iter()
            .any(|e| toks.contains(e) || (e.contains(' ') && ntext.contains(e.as_str())));
        let suffix_hit = || {
            term.suffix.iter().any(|sfx| {
                toks.iter().any(|t| {
                    t.ends_with(sfx.as_str())
                        && !SUFFIX_STOP.contains(&t.as_str())
                        && !term.block_words.contains(t)
                })
            })
        };
        if exact_hit || suffix_hit() {
            hits.push(term.key.clone());
        }
    }
    if hits.is_empty() {
        // Marken-Fallback: erste passende Marke gewinnt (JSON-Reihenfolge).
        for (brand, key) in &d.brands {
            if ntext.contains(brand.as_str()) {
                return vec![key.clone()];
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(title: &str) -> Vec<String> {
        match_keys(title, None, None)
    }

    /// Die drei Beobachtungen aus dem Backlog, abgearbeitet am 2026-07-26.
    /// Alle drei kamen aus echten Fehltreffern, nicht aus dem Kopf.
    #[test]
    fn woerterbuch_beobachtungen_2026_07() {
        // „Käse" traf „Laugenstange mit Käse" — ein DIREKTtreffer, das Wort
        // stand wörtlich im Titel. Eine Blockliste half hier zunächst nicht,
        // weil sie am Begriff hängt und nicht am Produkt; jetzt hängt das
        // Backwerk selbst darin.
        assert!(keys("Laugenstange mit Käse").is_empty(), "{:?}", keys("Laugenstange mit Käse"));
        assert_eq!(keys("Gouda am Stück"), vec!["käse"], "echter Käse bleibt");

        // `arla → milch` war mehrdeutig: Arla macht Milch UND Käse UND Butter.
        // Statt der Marke stehen jetzt die Produktlinien im Wörterbuch —
        // „ARLA Kærgården" ist Butter, „ARLA Finello" ist Käse.
        assert_eq!(keys("ARLA Kærgården"), vec!["butter"]);
        assert_eq!(keys("ARLA Finello"), vec!["käse"]);
    }

    #[test]
    fn regressionsfaelle() {
        assert_eq!(keys("Nadler Edle Matjesfilets"), vec!["fisch"]);
        assert_eq!(keys("Tomatenmark"), vec!["konserven"]);
        let ts = keys("Thunfisch-Salat");
        assert!(ts.contains(&"fisch".to_string()) && !ts.contains(&"salat".to_string()), "{ts:?}");
        assert!(keys("Kirschtomaten").contains(&"tomaten".to_string()));
        assert!(keys("Milka Schokolade").contains(&"schokolade".to_string()));
        assert!(keys("Chicorée").contains(&"brokkoli".to_string()));
        assert!(keys("Mini-Pak-Choi").contains(&"obst".to_string()));
        // Aus der Feedback-Schleife (docs/feedback-auswertung.md): „Käse“ traf
        // ein Schinken-Käse-Croissant. `croissant` steht seither auf der
        // Blockliste — echter Käse darf davon nichts merken.
        let croissant = keys("Schinken-Käse-Croissant");
        assert!(!croissant.contains(&"käse".to_string()), "{croissant:?}");
        assert!(keys("Gouda jung 48% Fett i. Tr.").contains(&"käse".to_string()));
        // Aus der Feedback-Schleife (Pflegerunde 2026-07-22): „Milch" traf
        // Gezuckerte Kondensmilch (Netto) und Sonnenmilch (Netto). Beide
        // stehen seither auf der Blockliste — echte Milch bleibt unberührt.
        let kondens = keys("Gezuckerte Kondensmilch");
        assert!(!kondens.contains(&"milch".to_string()), "{kondens:?}");
        let sonnen = keys("Sonnenmilch");
        assert!(!sonnen.contains(&"milch".to_string()), "{sonnen:?}");
        assert!(keys("Haltbare Milch").contains(&"milch".to_string()));
        // Aus dem proaktiven Angebots-Audit (2026-07-22): Fehl-Tags, die kein
        // Nutzer mehr melden muss. Jeweils mit Gegenprobe, dass der echte
        // Treffer den Tag behält.
        assert!(!keys("Leberkäse").contains(&"käse".to_string()));
        assert!(keys("Leberkäse").contains(&"wurst".to_string()));
        assert!(!keys("Schweine-Filet").contains(&"fisch".to_string()));
        assert!(keys("Schweine-Filet").contains(&"schwein".to_string()));
        assert!(keys("Doradenfilets").contains(&"fisch".to_string()));
        assert!(!keys("Kasseler Lachs XXL").contains(&"fisch".to_string()));
        assert!(keys("Lamm-Lachs mariniert").contains(&"lamm".to_string()));
        assert!(!keys("Schweinemedaillons").contains(&"hähnchen".to_string()));
        assert!(keys("Hähnchenmedaillons").contains(&"hähnchen".to_string()));
        assert!(!keys("Tafeltrauben dunkel").contains(&"bier".to_string()));
        assert!(keys("Lausitzer Dunkel").contains(&"bier".to_string()));
        assert!(keys("Kokosnussmilch").contains(&"kokosmilch".to_string()));
        assert!(!keys("Milch-Schnitte").contains(&"milch".to_string()));
        // Aus dem Alle-Regionen-Audit (2026-07-22, frische KW nach Neu-Scrape):
        // echte Food-Lücken geschlossen.
        assert!(keys("Zwetschgen, lose").contains(&"pfirsich".to_string()));
        assert!(keys("De Cecco italienische Teigwaren").contains(&"nudeln".to_string()));
        assert!(keys("Monopole Blue Top Champagner Brut").contains(&"wein".to_string()));
        assert!(keys("Norwegischer Räucherlachs XXL").contains(&"fisch".to_string()));
        assert!(keys("Tomatenketchup").contains(&"soßen".to_string()));
        assert!(keys("Skyr Natur").contains(&"quark".to_string()));
        assert!(keys("Süßrahmbutter").contains(&"butter".to_string()));
        assert!(keys("Gezuckerte Kondensmilch").contains(&"kondensmilch".to_string()));
        // Aus der Feedback-Schleife (Pflegerunde 2026-07-30, 173 Rückmeldungen).
        // Jeweils mit Gegenprobe: der echte Treffer behält seinen Tag.
        //
        // Das Suffix `filet` von „fisch" nahm Geflügel mit — der mit Abstand
        // größte gemeldete Block. „Hähnchen" blockt die Bindestrich-Formen,
        // `putenbrustfilet` das zusammengeschriebene Wort.
        assert!(!keys("Hähnchen-Brustfilet").contains(&"fisch".to_string()));
        assert!(keys("Hähnchen-Brustfilet").contains(&"hähnchen".to_string()));
        assert!(!keys("Putenbrustfilet XXL").contains(&"fisch".to_string()));
        assert!(keys("Putenbrustfilet XXL").contains(&"pute".to_string()));
        assert!(keys("Seelachsfilet").contains(&"fisch".to_string()));
        // Das Suffix `reis` traf „Puffreis" und — weniger offensichtlich —
        // „Wassereis", das auf dieselben vier Buchstaben endet.
        assert!(!keys("NIPPON Puffreis in Schokolade").contains(&"reis".to_string()));
        assert!(!keys("MERONG BAR Wassereis").contains(&"reis".to_string()));
        assert!(keys("Basmatireis").contains(&"reis".to_string()));
        // Molkerei-Marken auf -milch tragen auch Käse. Geblockt wird das
        // Produktwort, nicht die Marke — sonst verlöre echte Milch derselben
        // Marke ihren Tag.
        assert!(!keys("SALZBURGMILCH Bergkäse").contains(&"milch".to_string()));
        assert!(keys("SALZBURGMILCH Bergkäse").contains(&"käse".to_string()));
        assert!(!keys("Kärntnermilch Käsescheiben").contains(&"milch".to_string()));
        assert!(keys("Kärntnermilch Vollmilch").contains(&"milch".to_string()));
        // Fleischkäse ist Wurst, kein Käse — dieselbe Familie wie Leberkäse.
        assert!(!keys("Delikatess-Fleischkäse").contains(&"käse".to_string()));
        assert!(keys("Gouda am Stück").contains(&"käse".to_string()));
        // `nutella` steht bei „marmelade" im exact; die Marke steht aber auch
        // auf Eis und Keksen.
        assert!(!keys("Nutella Eisbecher").contains(&"marmelade".to_string()));
        assert!(!keys("Nutella Biscuits").contains(&"marmelade".to_string()));
        assert!(keys("Nutella 450g").contains(&"marmelade".to_string()));
        // Tortilla-Fladen sind kein Fertiggericht, und Antipasti sind eine
        // Vorspeise — vier Meldungen aus vier Märkten, deshalb ganz raus.
        assert!(!keys("Tortilla Wraps Weizen").contains(&"fertiggericht".to_string()));
        assert!(!keys("Antipasti Creme").contains(&"fertiggericht".to_string()));
        assert!(keys("Maultaschen").contains(&"fertiggericht".to_string()));
        // Aus dem proaktiven Audit über 11 Regionen (2026-07-30, 3.245 Produkte).
        //
        // Der teuerste Fund: Der Markenschlüssel „5,0 original" (eine Biermarke)
        // verliert unter `norm()` Ziffer und Komma und blieb als nacktes
        // „original" stehen — ein Wort, das auf sehr vielen Verpackungen steht.
        // Es taggte quer durch das Sortiment Bier: Snickers-Eis, McCain-Frites,
        // Miracel Whip, sogar den Schreibwarenhersteller STABILO. Der Schlüssel
        // ist raus, Löwenbräu dafür als eigene Marke drin — es hing vorher
        // ausschließlich an diesem kaputten Eintrag.
        assert!(!keys("Snickers Original Ice Cream").contains(&"bier".to_string()));
        assert!(!keys("McCain 1-2-3 Frites Original").contains(&"bier".to_string()));
        assert!(!keys("Miracel Whip Salatcreme Original").contains(&"bier".to_string()));
        assert!(keys("Löwenbräu Original").contains(&"bier".to_string()));
        // „adler" (Käsemarke) steckt als Teilstring in NADLER — einer Fisch-
        // und Salatmarke. Marken werden per Teilstring gesucht, also raus.
        assert!(!keys("NADLER Sahne Hering filets XXL").contains(&"käse".to_string()));
        assert!(keys("NADLER Sahne Hering filets XXL").contains(&"fisch".to_string()));
        // Tierfutter stand mitten in den Lebensmitteln.
        assert_eq!(keys("Vitakraft Beef Stick"), vec![NONFOOD_KEY]);
        // Das Suffix `medaillons` von hähnchen nahm irisches Rind mit.
        assert!(!match_keys("Teres Major", Some("irisches Rindfleisch, ideal als Medaillons"), None)
            .contains(&"hähnchen".to_string()));
        // Barista-Hafderdrink ist Milchersatz, kein Kaffee.
        assert!(!keys("Oatly Haferdrink Barista").contains(&"kaffee".to_string()));
        assert!(keys("Oatly Haferdrink Barista").contains(&"milch".to_string()));
        // Cheestrings ist Käse; als exact schlägt es den Marken-Fallback
        // „bauer" -> joghurt, der vorher gewann.
        assert!(keys("BAUER Cheestrings").contains(&"käse".to_string()));
        // Das Wörterbuch kannte nur `mc cain` mit Leerzeichen, der Prospekt
        // schreibt MCCAIN.
        assert!(keys("MCCAIN 1-2-3 Frites Original").contains(&"pommes".to_string()));
    }

    #[test]
    fn marken_fallback() {
        assert_eq!(keys("Fruchtzwerge"), vec!["joghurt"]);
        assert_eq!(keys("Bitburger Premium"), vec!["bier"]);
    }

    #[test]
    fn nonfood_und_ungetaggt() {
        assert_eq!(match_keys("Duschbad", None, Some("drogerie")), vec![NONFOOD_KEY]);
        // Kauflands Obsttheke heißt „Obst, Gemüse, Pflanzen" — der Food-Marker
        // in der Kategorie schlägt das „Pflanzen" (Fund 2026-07-22).
        assert_eq!(
            match_keys("Dtsch. Zwetschgen, lose", None, Some("Obst, Gemüse, Pflanzen")),
            vec!["pfirsich"]
        );
        assert_eq!(
            match_keys("Duschbad", None, Some("Drogerie, Tiernahrung")),
            vec![NONFOOD_KEY]
        );
        assert_eq!(keys("Sagrotan Hygiene-Spray 2in1"), vec!["windeln/hygiene"]);
        assert_eq!(keys("Crivit Trekkingstöcke"), vec![NONFOOD_KEY]);
        assert!(keys("Ciolino").is_empty()); // kontextloser Flyer-Titel → Review-Liste
    }

    /// Paritäts-Check gegen die Python-Referenz auf der lokalen Nightly-DB:
    /// `cargo test parity_with_eval_db -- --ignored --nocapture`
    /// Die Zahlen müssen zur Ausgabe von `python3 docs/matching-woerterbuch-eval.py` passen.
    #[test]
    #[ignore]
    fn parity_with_eval_db() {
        let path = std::env::var("HOME").unwrap() + "/.local/share/lechariot/lechariot.db";
        let conn = rusqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare(
                "select o.title, coalesce(o.subtitle,''), coalesce(o.category,'')
                 from offers o join markets m on m.id=o.market_id
                 where o.valid_until >= date('now')",
            )
            .unwrap();
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let (mut nonfood, mut tagged, mut untagged) = (0, 0, 0);
        for (title, sub, cat) in &rows {
            let k = match_keys(title, Some(sub), Some(cat));
            if k == [NONFOOD_KEY] {
                nonfood += 1;
            } else if k.is_empty() {
                untagged += 1;
            } else {
                tagged += 1;
            }
        }
        println!(
            "gesamt {} | nonfood {} | getaggt {} | ungetaggt {}",
            rows.len(),
            nonfood,
            tagged,
            untagged
        );
    }
}
