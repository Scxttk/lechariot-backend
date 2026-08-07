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

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;

pub const NONFOOD_KEY: &str = "nonfood";

const DICT_JSON: &str = include_str!("../docs/matching-woerterbuch.json");

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
    /// Komposita-Präfixe (nur ab 4 Zeichen wirksam, Token muss LÄNGER sein).
    /// Fehlt die Sektion, bleibt die Liste leer und das Verhalten ist exakt
    /// das vor dieser Regel.
    prefix: Vec<String>,
    block_words: Vec<String>,
    block_phrases: Vec<String>,
}

struct Dict {
    terms: Vec<Term>,
    /// Normalisierte Kategorie → Begriff. Letzter Ausweg, wenn Titel und
    /// Untertitel nichts hergeben; gepflegte Zuordnung, kein Regex — die
    /// Begründung steht bei `KAT_ROH` in der Python-Referenz.
    categories: HashMap<String, String>,
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
                    prefix: list("prefix").into_iter().filter(|s| s.chars().count() >= 4).collect(),
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
        // Längste Marke zuerst. Der Fallback prüft `contains`, und in
        // JSON-Reihenfolge gewann die **kürzere**: „MILKANA" (Schmelzkäse)
        // enthält „milka" und bekam `schokolade` statt `käse` — dieselbe
        // Teilzeichenketten-Falle wie `reis` in „P**reis**". Gefunden am
        // 01.08. beim Messen von [EXTRAKT-1], vorhanden war sie vorher.
        let mut brands: Vec<(String, String)> = brands;
        brands.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        // Regexe kommen aus der JSON — eine Quelle mit der Python-Referenz.
        // Python-Patterns tragen kein eingebettetes (?i), daher hier ergänzen.
        //
        // Fehlt ein Feld, bricht das ab, statt still auf eine Konstante
        // zurückzufallen. Die Fallbacks standen bis zum 2026-08-01 hier und
        // waren um 46 Kategorie- und 180 Titelmuster ärmer als die JSON — ein
        // umbenannter Schlüssel hätte den Filter halbiert, ohne dass irgendwo
        // etwas fehlgeschlagen wäre.
        let rx = |field: &str| -> Regex {
            let pattern = v[field]
                .as_str()
                .unwrap_or_else(|| panic!("matching-woerterbuch.json: Feld `{field}` fehlt"));
            Regex::new(&format!("(?i){pattern}")).unwrap()
        };
        // Fehlt die Sektion (alter JSON-Stand), bleibt die Zuordnung leer und
        // das Verhalten ist exakt das vor dieser Regel.
        let categories = v["kategorien"]
            .as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(cat, term)| Some((norm(cat), term.as_str()?.to_string())))
                    .filter(|(c, _)| !c.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Dict {
            terms,
            categories,
            brands,
            nonfood_cat: rx("nonfood_cat"),
            nonfood_terms: rx("nonfood_terms"),
            food_cat: rx("food_cat"),
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

/// Wie [`match_keys`], zieht aber die **Marke** heran, wenn Titel und
/// Untertitel allein nichts ergeben.
///
/// Der Fall: ALDI Nords Untertitel ist oft nur eine Variante („Berry",
/// „Medium", „Zero Sugar"), und der Produktname steht in der Marke — `Berry`
/// ist *JACK DANIEL'S*, `Medium` ist *GEROLSTEINER*, `Original` ist *LÄTTA*.
/// 27 der 62 ungetaggten Zeilen mit reiner Packungsangabe kamen so zustande
/// (gemessen 01.08. am 11-Regionen-Korpus).
///
/// **Nur als Rückfall**, und das ist die Lehre aus der Messung: Die Marke
/// immer mitzugeben ändert Tags in gesunden Zeilen. Wer schon ein Tag hat,
/// bekommt die Marke gar nicht erst zu sehen — dann kann sie auch nichts
/// kaputt machen.
pub fn match_keys_with_brand(
    title: &str,
    subtitle: Option<&str>,
    category: Option<&str>,
    brand: Option<&str>,
) -> Vec<String> {
    let keys = match_keys(title, subtitle, category);
    if !keys.is_empty() {
        return keys;
    }
    let Some(brand) = brand.map(str::trim).filter(|b| !b.is_empty()) else {
        return keys;
    };
    match_keys(&format!("{brand} {title}"), subtitle, category)
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
        // Spiegelbild des Suffix-Wegs: das Deutsche schreibt das bekannte
        // Grundwort mal hinten („Steinofen*baguette*") und mal vorne
        // („*Kartoffel*taschen"). Dieselben Wächter — vier Zeichen,
        // SUFFIX_STOP, Blockliste — und zusätzlich muss das Token länger sein
        // als der Präfix, sonst wäre es ein exact und kein Kompositum.
        let prefix_hit = || {
            term.prefix.iter().any(|pfx| {
                toks.iter().any(|t| {
                    t.starts_with(pfx.as_str())
                        && t.len() > pfx.len()
                        && !SUFFIX_STOP.contains(&t.as_str())
                        && !term.block_words.contains(t)
                })
            })
        };
        if exact_hit || suffix_hit() || prefix_hit() {
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
        // Letzter Ausweg: die Kategorie der Kette. Sie steht erst hier, damit
        // ein Titel-Treffer nie überstimmt wird.
        //
        // Die Blockliste des Begriffs gilt auch auf diesem Weg, und das ist
        // keine Vorsichtsmaßnahme, sondern ein gefundener Fehler: „Erdnuss-
        // butter" steht auf der Blockliste von `butter` und bekam den Tag
        // über die Kategorie „Butter" zurück — die Blockliste wäre auf dem
        // neuen Weg schlicht wirkungslos gewesen.
        if let Some(key) = d.categories.get(norm(cat).as_str()) {
            let blocked = d.terms.iter().filter(|t| t.key == *key).any(|t| {
                t.block_phrases.iter().any(|b| ntext.contains(b.as_str()))
                    || t.block_words.iter().any(|b| toks.contains(b))
            });
            if !blocked {
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

    fn keys_cat(title: &str, cat: &str) -> Vec<String> {
        match_keys(title, None, Some(cat))
    }

    /// Die Kategorie-Zuordnung, gebaut am 2026-07-31 aus einer Messung über
    /// 414 Food-Angebote (Abdeckung 88 % → 97 %). Jeder Fall hier stand in
    /// dieser Messung; keiner ist ausgedacht.
    #[test]
    fn kategorie_als_letzter_ausweg() {
        // Titel nichtssagend, Kategorie eindeutig — genau dafür ist die Regel.
        assert_eq!(keys_cat("Die Extrazarte", "Butter"), vec!["butter"]);
        assert_eq!(keys_cat("Grande Réserve", "Champagner"), vec!["wein"]);
        assert_eq!(keys_cat("Froop", "Joghurt"), vec!["joghurt"]);
        assert_eq!(keys_cat("Naturelle", "Wasser"), vec!["wasser"]);
        // Kategorie mit Akzent und Komma: der Schlüssel wird normalisiert
        // verglichen, nicht wörtlich.
        assert_eq!(keys_cat("Bio Feine Creme zum Kochen", "Sahne, Schmand und Crème fraîche"), vec!["sahne"]);

        // Wörterbuch-Runde 2026-07-31, Op 5: Lidls Aufstrich-/Dip-Regale
        // sind zugeordnet — erledigt die letzte ungetaggte Zeile der
        // Eval-DB („Antipasti Creme").
        assert_eq!(keys_cat("Antipasti Creme", "herzhafte Aufstriche"), vec!["soßen"]);
        // Titel-Treffer bleiben unberührt von der Kategorie.
        assert_eq!(keys_cat("Zaziki", "Dips"), vec!["soßen"]);

        // Bewusst NICHT zugeordnet — die freie Wörterbuchsuche über die
        // Kategorie hätte hier danebengegriffen, und das ist der teurere
        // Fehler: ein falsches Tag legt jemandem das falsche Produkt in den
        // Einkauf.
        assert!(keys_cat("Not Milk", "Veganes").is_empty());          // Haferdrink, kein Tofu
        // Spitzkohl stand hier bis zum 07.08. als „bewusst nicht zugeordnet",
        // weil es keinen Begriff für Kohl gab und die Kategorie „Brokkoli und
        // Kohl" ihn sonst zu Brokkoli gemacht hätte. Seit Tranche 2 gibt es
        // `kohl`, und damit lässt sich die Absicht des Falls — **Kohl, kein
        // Brokkoli** — endlich positiv hinschreiben.
        assert_eq!(keys_cat("Spitzkohl", "Brokkoli und Kohl"), vec!["kohl"]);
        assert!(keys_cat("Ganzes Kaninchen", "Geflügel").is_empty());   // Kaninchen ist kein Geflügel

        // Ein Titel-Treffer wird nie überstimmt, und eine Blockliste auch
        // nicht: „Erdnussbutter" steht auf der Blockliste von „butter" und
        // darf sie nicht über die Kategorie zurückbekommen.
        //
        // **Die Absicht war „keine Butter", nicht „gar nichts".** Bis Tranche
        // 7 gab es keinen Begriff für Erdnussbutter, und leer war die einzige
        // Art, das hinzuschreiben. Jetzt gibt es einen — und die Sperre bei
        // `butter` gilt unverändert.
        assert_eq!(keys_cat("Erdnussbutter", "Butter"), vec!["erdnussbutter"]);
        // Und die Gegenprobe zur Gegenprobe: echte Butter behält ihren Tag,
        // auch ohne Kategorie.
        assert_eq!(keys("Deutsche Markenbutter"), vec!["butter"]);
    }

    /// Wörterbuch-Runde 2026-08-01, Op 2: zwölf weitere Kategorien. Jeder Fall
    /// hier ist eine Zeile, die im 11-Regionen-Korpus vorher ungetaggt war —
    /// keiner ist ausgedacht. Aufnahmekriterium war, dass JEDES Korpus-Produkt
    /// unter der Kategorie zum Begriff gehört; die Zahlen stehen bei `KAT_ROH`.
    #[test]
    fn kategorie_runde_2026_08_01() {
        assert_eq!(keys_cat("Haake-Beck", "Bier"), vec!["bier"]);
        assert_eq!(keys_cat("Kosmonaut Hell", "Bier"), vec!["bier"]);
        assert_eq!(keys_cat("Flusskrebssalat", "Fisch"), vec!["fisch"]);
        assert_eq!(keys_cat("Plus Pack 156 g, Würzig", "Schnittkäse"), vec!["käse"]);
        assert_eq!(keys_cat("Crema e Aroma 1 kg", "Kaffee"), vec!["kaffee"]);
        // Bis Tranche 3 war „wurst" die feinste Antwort, die es gab. Jetzt
        // gibt es `bacon`, und der Fall sagt, was er immer meinte.
        assert_eq!(keys_cat("Bacon 150 g", "Schinken"), vec!["bacon"]);
        assert_eq!(keys_cat("Dinkel-Bürli", "Bäckerei"), vec!["backwaren"]);
        assert_eq!(keys_cat("Kreatinpulver", "Sportnahrung"), vec!["protein/fitness"]);
        assert_eq!(keys_cat("Grissotti 200 g, Sesam", "Chips & Knabbereien"), vec!["chips"]);
        assert_eq!(keys_cat("Fruchtgummi 175 g, Quaxi", "Lakritz & Fruchtgummi"), vec!["schokolade"]);
        // Bis Tranche 4 war „nudeln" die feinste Antwort — und sie kam über
        // die Kategorie, nicht über den Titel. Jetzt trägt der Titel selbst
        // einen Begriff, und der Fall sagt, was er immer meinte.
        assert_eq!(keys_cat("Lasagneblätter 500 g", "Nudeln & Pasta"), vec!["lasagneblätter"]);
        // Bis Tranche 6 gab es keinen Begriff für Preiselbeeren, und die
        // Kategorie „Nüsse & Trockenfrüchte" war die feinste Antwort. Eine
        // Cranberry ist keine Nuss — jetzt sagt der Fall das auch.
        assert_eq!(keys_cat("Getrocknete Cranberries 200 g", "Nüsse & Trockenfrüchte"), vec!["preiselbeeren"]);
        // Seit Tranche 7 gibt es `hamburger` — die Frikadelle steht neben
        // dem groben „rind", nicht statt seiner.
        assert_eq!(keys_cat("Rind Hamburger 400 g", "Rindfleisch"), vec!["rind", "hamburger"]);

        // Gegenprobe 1: die Kategorie bleibt der LETZTE Ausweg. „Plus Pack
        // 156 g, Chili Paprika" steht auch unter *Schnittkäse* und behält
        // seinen Titel-Treffer — die Kategorie überstimmt ihn nicht.
        assert_eq!(keys_cat("Plus Pack 156 g, Chili Paprika", "Schnittkäse"), vec!["paprika"]);
        // Gegenprobe 2: Substring reicht nicht, verglichen wird auf Gleichheit
        // der normalisierten Kategorie. Sonst zöge „Bier" auch „Bier & Wein"
        // und „Alkoholfreies Bier und mehr" an sich.
        assert!(keys_cat("Irgendwas", "Bier & Wein").is_empty());
        assert!(keys_cat("Irgendwas", "Fischers Fritz").is_empty());
        // Gegenprobe 3: die Blockliste des Begriffs gilt auch auf diesem Weg —
        // „Käsekuchen" ist gesperrt und darf `käse` nicht über *Schnittkäse*
        // zurückbekommen.
        assert!(keys_cat("Käsekuchen nach Omas Art", "Schnittkäse")
            .iter()
            .all(|k| k != "käse"));
        // Gegenprobe 4: bewusst NICHT zugeordnet, weil die Kategorie zwei
        // Familien nennt — ein Begriff daraus wäre für den Rest falsch.
        assert!(keys_cat("PUSTERIA", "Molkereiprodukte, Fette").is_empty());
        assert!(keys_cat("Hella Near Water", "Alkoholfreie Getränke").is_empty());
        // Neutraler Titel, damit der Fall die Kategorie prüft und nicht das
        // Wort: „Fleisch & Fisch" nennt zwei Familien und bleibt unzugeordnet.
        assert!(keys_cat("Der Beste", "Frische-Aktion: Fleisch & Fisch").is_empty());
    }

    /// Wörterbuch-Runde 2026-08-01, Op 3: die neuen Alltagswörter. Jede Zeile
    /// hier war im 11-Regionen-Korpus ungetaggt; die zweite Hälfte des Tests
    /// ist der Preis dafür, gemessen im selben Lauf.
    #[test]
    fn alltagswoerter_2026_08_01() {
        let hat = |titel: &str, tag: &str| {
            let k = keys(titel);
            assert!(k.contains(&tag.to_string()), "{titel:?} -> {k:?}, erwartet {tag}");
        };
        hat("Makrele", "fisch");
        hat("Costa Pacific Prawns", "fisch");
        hat("Brimi Burrata", "käse");
        hat("Brimi Mascarpone", "käse");
        hat("EDEKA Herzstücke Pecorino Romano", "käse");
        hat("Harzer Minis", "käse");
        hat("Brimi Mozzarelline", "mozzarella");
        hat("Berchtesgadener Land Schlagrahm", "sahne");
        hat("Berchtesgadener Land frischer Sauerrahm", "sahne");
        hat("demeter Berchtesgadener Land Feinster Bio-Schlag-Rahm", "sahne");
        hat("MILBONA Fettarmer Kefir", "joghurt");
        hat("Halberstädter 5 Bockwürste", "wurst");
        hat("Gmyrek Oberschlesische Frankfurter", "wurst");
        hat("Weißwürste", "wurst");
        hat("Leberpastete", "wurst");
        hat("Hofglück Pfefferbeißer", "wurst");
        hat("Recla - Südtiroler Speck", "wurst");
        hat("Fleischkäse", "wurst");
        hat("Krustenbraten", "schwein");
        hat("Bauchrippe", "schwein");
        hat("BBQ Ribs", "schwein");
        hat("Rouladen vom Rind", "rind");
        hat("Wade vom Jungbullen", "rind");
        hat("Lauch", "brokkoli");
        hat("Bio Staudensellerie", "brokkoli");
        hat("Grapefruit", "obst");
        hat("Passionsfrucht", "obst");
        hat("Petersilie", "gewürze");
        hat("KRINI Hülsenfrüchte", "konserven");
        hat("Hummus XXL", "soßen");
        hat("SCHAMEL Meerrettich", "soßen");
        hat("Apfelmus ohne Zuckerzusatz", "marmelade");
        hat("Rote Grütze XXL", "pudding");
        hat("Sonnen Bassermann Eintöpfe", "eintopf");
        hat("SEEBERGER Apfelringe", "nüsse");
        hat("Sweet Popcorn XXL", "chips");
        hat("DELUXE Baklava Pistazie", "backwaren");
        hat("Schweinsöhrchen", "backwaren");
        hat("Brioche Burger Buns", "brot");
        hat("Früh Kölsch", "bier");
        hat("SCHWABENBRÄU Das echte Märzen", "bier");
        hat("Kosmonaut Hell", "bier");
        hat("Don Simon Sangria Premium", "wein");
        hat("Riunite Lambrusco Emilia Frizzante", "wein");
        hat("Grauer Burgunder trocken", "wein");
        hat("MALTESERKREUZ Aquavit", "spirituosen");
        hat("Bionade", "limonade");
        hat("Mio Mio Mate", "limonade");
        hat("Stieleis", "eis");
        hat("Dicke Fritte", "pommes");
        hat("Mazola Keimöl", "öl");
        hat("EDEKA Bio Chai Classic", "tee");
        hat("Rainbow Mystery Dumpling", "fertiggericht");

        // Der Preis, gemessen im selben Lauf über 3.474 Korpus-Produkte. Ohne
        // diese fünf Sperren kosten die Wörter oben je einen Fehltreffer.
        let ohne = |titel: &str, tag: &str| {
            let k = keys(titel);
            assert!(!k.contains(&tag.to_string()), "{titel:?} -> {k:?}, {tag} unerwünscht");
        };
        // `rahm` allein stand zwischendurch im Wörterbuch und war dreimal falsch.
        ohne("Iglo Rahm-Spinat", "sahne");
        ohne("KNORR Rahm Soße", "sahne");
        ohne("Allgäuer Rahm-Torte", "sahne");
        // Schöfferhofer Grapefruit ist Bier, der Naturradler Grapefruit auch.
        ohne("SCHÖFFERHOFER Grapefruit", "obst");
        assert_eq!(keys("SCHÖFFERHOFER Grapefruit"), vec!["bier"]);
        ohne("Lübzer Premium Pils oder Naturradler Grapefruit", "obst");
        // „Antipasti Creme" ist ein Aufstrich, keine Konserve.
        ohne("ERGÜLLÜ Antipasti Creme", "konserven");
        // Mascarpone-Joghurt ist Joghurt.
        ohne("Mascarpone-Joghurt", "käse");
        // „hell" ist Biersorte — auf Brötchen und auf Trauben aber nicht.
        ohne("Rosenbrötchen hell", "bier");
        ohne("Tafeltrauben hell", "bier");
        // „Speck" ist Wurst, der Speck-Käse-Twister bleibt Backwerk.
        ohne("Speck-Käse-Twister", "wurst");
    }

    /// Wörterbuch-Runde 2026-08-01, Op 4: Komposita, in denen ein längst
    /// bekanntes Wort steckt. Neu ist der Präfix-Weg neben dem Suffix-Weg;
    /// beide sind pro Begriff gepflegt und nie pauschal.
    #[test]
    fn komposita_2026_08_01() {
        let hat = |titel: &str, tag: &str| {
            let k = keys(titel);
            assert!(k.contains(&tag.to_string()), "{titel:?} -> {k:?}, erwartet {tag}");
        };
        // Suffix: das bekannte Wort steht hinten.
        hat("Steinofenbaguette", "brot");
        hat("Laugenbaguette", "brot");
        hat("Mehrkornbaguette", "brot");
        hat("Linzergebäck", "backwaren");
        hat("Apfelrotkohl", "konserven");
        hat("Karamellwaffeln XXL", "kekse");
        hat("Bratheringe", "fisch");
        hat("Lachsforelle", "fisch");
        hat("Stangenbrokkoli", "brokkoli");
        hat("Berchtesgadener Land Bio-Alpenbutter", "butter");
        hat("Delikat Käsewiener", "wurst");
        hat("Grau-, Weiß- oder Spätburgunder", "wein");
        // Präfix: das bekannte Wort steht vorne.
        hat("Lachsfiletseite", "fisch");
        hat("Garnelenspieße", "fisch");
        hat("Kartoffeltaschen", "kartoffeln");
        hat("PFANNI Kartoffelpüree", "kartoffeln");
        hat("Frische Hähnchenunterkeulen", "hähnchen");
        hat("Frische Putenoberkeulen", "pute");
        hat("Ponnath Gourmet Schinkenplatte", "wurst");
        hat("Wurstspezialität", "wurst");
        hat("Bio-Wienerle", "wurst");
        hat("Frisches Schweinegeschnetzeltes", "schwein");
        hat("Steakhüfte aus eigener Herstellung", "rind");
        hat("Hackfleischspieße Döner Art", "hackfleisch");
        hat("Gut & Günstig Beerenmischung", "beeren");
        hat("Frischkäsecreme", "frischkäse");
        hat("Schmelzkäsezubereitung", "käse");
        hat("CHEF SELECT MozzarellaSticks", "mozzarella");
        hat("SETTELE Teigwarenspezialität", "nudeln");
        hat("Bratwurstschnecke", "bratwurst");
        hat("ALESTO CashewkerneXXL", "nüsse");
        hat("YOPRO Joghurtdrink", "joghurt");
        hat("Surig Essigessenz", "essig");
        hat("Lammkeulenscheibe", "lamm");
        hat("Bio-Obstsortiment", "obst");
        hat("Cremig und Quarkig", "quark");
        hat("Deutschland - Weingenuss", "wein");
        hat("Butaris feines Butterschmalz", "butter");
        hat("REWE Regional, GQB, Salatmischung", "salat");
        hat("CAMPO VERDE Demeter Gemüsekonserven", "tiefkühlgemüse");
        hat("KIDSWORLD Puddingdessert", "pudding");

        // Der Preis, gemessen im selben Lauf: neun Sperren. Ohne sie erzeugt
        // der Kompositum-Weg genau diese neun Fehltreffer — das ist die
        // Sperrliste, von der „Reis"/„Preis" und „Wein"/„Schwein" handeln.
        let ohne = |titel: &str, tag: &str| {
            let k = keys(titel);
            assert!(!k.contains(&tag.to_string()), "{titel:?} -> {k:?}, {tag} unerwünscht");
        };
        ohne("Radeberger Premium-Lachsschinken", "fisch");
        ohne("Lachsrolle vom Schweinerücken", "fisch");
        ohne("NATURGUT Bio Gemüsebrühe", "tiefkühlgemüse");
        ohne("EHRMANN Obstgarten", "obst");
        ohne("POM-BÄR Kartoffelsnacks", "kartoffeln");
        ohne("MCCAIN Kartoffelprodukt", "kartoffeln");
        ohne("Quarktasche", "quark");
        ohne("EDEKA Herzstücke Reiswaffeln", "kekse");
        ohne("K-BIO Bio-Mini-Maiswaffeln", "kekse");
        ohne("Steakhouse Pommes 750 g, Rustikal", "rind");
        ohne("Salatgurke", "salat");
        // Und die Wächter, die es schon gab, gelten auch für den neuen Weg:
        // „Schweinsöhrchen" ist Backwerk (deshalb Präfix „schweine", nicht
        // „schwein"), HACKER-PSCHORR ist Bier (deshalb „hackfleisch", nicht
        // „hack"), und die Fischer-E-Bikes sind der Grund, warum „fisch"
        // gar nicht erst als Präfix dasteht.
        ohne("Schweinsöhrchen", "schwein");
        ohne("HACKER-PSCHORR Münchner Hell", "hackfleisch");
        ohne("Fischer E-Bike Trekking", "fisch");
        // Gegenprobe: der Präfix greift nur bei echten Komposita — ein Token,
        // das dem Präfix gleicht, kommt weiter über das exact.
        assert!(keys("Lachs").contains(&"fisch".to_string()));
    }

    /// Die drei Beobachtungen aus dem Backlog, abgearbeitet am 2026-07-26.
    /// Alle drei kamen aus echten Fehltreffern, nicht aus dem Kopf.
    #[test]
    fn woerterbuch_beobachtungen_2026_07() {
        // „Käse" traf „Laugenstange mit Käse" — ein DIREKTtreffer, das Wort
        // stand wörtlich im Titel. Eine Blockliste half hier zunächst nicht,
        // weil sie am Begriff hängt und nicht am Produkt; jetzt hängt das
        // Backwerk selbst darin.
        // Seit der Runde 2026-08-01 kennt `backwaren` das Wort „Laugenstange"
        // (es stand als Angebot im Korpus). Der Punkt des Falls bleibt
        // unberührt: `käse` darf es nicht sein.
        let laugen = keys("Laugenstange mit Käse");
        assert!(!laugen.contains(&"käse".to_string()), "{laugen:?}");
        assert_eq!(laugen, vec!["backwaren"]);
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
        // Wörterbuch-Runde 2026-07-31, Op 6: Diese Zeile war nicht bloß ein
        // toter Eintrag, sondern die eine echte Abweichung zwischen den
        // Maschinen — Rust blockte hier, Python nicht (Details bei
        // `parity_with_eval_db`). Der Fix steht in der Python-Referenz; was
        // Rust tut, stand schon immer hier und bleibt der Maßstab.
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
        // Wörterbuch-Runde 2026-07-31, Op 3: Suffix `nuggets` flog aus
        // `schwein` — im Korpus für 100 % seiner Treffer falsch (sieben
        // Hähnchen, ein veganes, null Schwein). Damit verliert auch das
        // Rügenwalder Veganes Mühlen-Schnitzel seinen Schweinefleisch-Tag.
        let nuggets = keys("Chicken Nuggets XXL");
        assert!(!nuggets.contains(&"schwein".to_string()), "{nuggets:?}");
        assert!(nuggets.contains(&"hähnchen".to_string()));
        let veggie = keys("Rügenwalder Mühle Veganes Mühlen Schnitzel*, Nuggets*");
        assert!(!veggie.contains(&"schwein".to_string()), "{veggie:?}");
        assert!(veggie.contains(&"tofu".to_string()));
        // Gegenprobe: echtes Schwein hängt an exacts, nicht am Suffix.
        assert!(keys("Schweineschnitzel").contains(&"schwein".to_string()));
        assert!(!keys("Tafeltrauben dunkel").contains(&"bier".to_string()));
        assert!(keys("Lausitzer Dunkel").contains(&"bier".to_string()));
        // Wörterbuch-Runde 2026-07-31, Op 2: `weizen` flog aus dem bier-exact —
        // im ganzen 11-Regionen-Korpus existiert kein Weizenbier-Angebot, die
        // zwei Treffer waren Brötchen und Mehl. Gegenprobe: Weizenbier käme
        // über das Suffix `bier` weiter an, Weißbier steht im exact.
        let broetchen = keys("Weizen-Brötchen");
        assert!(!broetchen.contains(&"bier".to_string()), "{broetchen:?}");
        assert!(broetchen.contains(&"brot".to_string()));
        let wmehl = keys("Alnatura Weizen Mehl");
        assert!(!wmehl.contains(&"bier".to_string()), "{wmehl:?}");
        assert!(wmehl.contains(&"mehl".to_string()));
        assert!(keys("Erdinger Weizenbier").contains(&"bier".to_string()));
        assert!(keys("Paulaner Weißbier").contains(&"bier".to_string()));
        assert!(keys("Kokosnussmilch").contains(&"kokosmilch".to_string()));
        assert!(!keys("Milch-Schnitte").contains(&"milch".to_string()));
        // Aus dem Alle-Regionen-Audit (2026-07-22, frische KW nach Neu-Scrape):
        // echte Food-Lücken geschlossen.
        assert!(keys("Zwetschgen, lose").contains(&"pfirsich".to_string()));
        // Wörterbuch-Runde 2026-07-31, Op 4 — die Antwort auf die alte
        // `pflaumen`-Frage: Der Begriff fehlt nicht, `pfirsich` fasst
        // Steinobst bewusst zusammen. Der Defekt war die Kollision mit
        // Pflaumentomaten; die zwei Blockeinträge lösen genau sie.
        let minipfl = keys("Minipflaumen Tomaten");
        assert!(!minipfl.contains(&"pfirsich".to_string()), "{minipfl:?}");
        assert!(minipfl.contains(&"tomaten".to_string()));
        let pfltom = keys("Mini Pflaumentomaten");
        assert!(!pfltom.contains(&"pfirsich".to_string()), "{pfltom:?}");
        assert!(pfltom.contains(&"tomaten".to_string()));
        // Gegenprobe: echtes Steinobst behält `pfirsich`.
        assert_eq!(keys("Pflaumen"), vec!["pfirsich"]);
        assert_eq!(keys("Zwetschgen*"), vec!["pfirsich"]);
        // Wörterbuch-Runde 2026-07-31, Op 7: Zwei Blocklisten führten je einen
        // Fließtext statt eines Wortes („kartoffelchips fällt unter chips",
        // „buttergemüse zulässig") — tote Einträge, denn eine Blockliste
        // vergleicht Wörter, keine Sätze. Sie sind jetzt Kommentare; hier
        // steht, was sie behaupteten, als Prüfung statt als Prosa.
        let chips = keys("Kartoffelchips Paprika");
        assert!(chips.contains(&"chips".to_string()), "{chips:?}");
        assert!(!chips.contains(&"kartoffeln".to_string()), "{chips:?}");
        let buttergem = keys("Buttergemüse");
        assert!(buttergem.contains(&"tiefkühlgemüse".to_string()), "{buttergem:?}");
        assert!(!buttergem.contains(&"butter".to_string()), "{buttergem:?}");
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
        // Warengruppen der Ketten, die klar Non-Food sind und bisher in den
        // Lebensmitteln standen. Über 11 Regionen gemessen: E-Bikes,
        // Staubsauger, Unterwäsche, Geschirr, Batterien, Nähmaschinen.
        // Das Food-Veto sticht weiterhin — deshalb die Gegenproben unten.
        for (titel, kat) in [
            ("FISCHER E-MTB Montis 2.2 29 Zoll", "E-Bikes"),
            ("Bodenstaubsauger", "Reinigungsgeräte"),
            ("ESMARA MEN Herren Boxer, 10 Stück", "Unterhemden"),
            ("Tellerset Magic Black Square 16-tlg.", "Essgeschirr"),
            ("Knopfzellen", "Batterien"),
            ("Computer Nähmaschine Serenade 660L", "Nähmaschinen"),
            ("Küchenmaschine", "Küchengeräte"),
        ] {
            assert_eq!(
                match_keys(titel, None, Some(kat)),
                vec![NONFOOD_KEY],
                "sollte Non-Food sein: {titel} [{kat}]"
            );
        }
        // Gegenprobe: Ein Food-Marker in der Kategorie schlägt die neuen
        // Non-Food-Muster weiterhin — sonst risse „Küchengeräte" auch
        // „Fleisch, Geflügel, Wurst" mit, wo beides in einer Zeile steht.
        assert!(match_keys("Gouda am Stück", None, Some("Käsetheke"))
            .contains(&"käse".to_string()));
        assert!(match_keys("Rinderhack", None, Some("Fleisch, Geflügel, Wurst"))
            .contains(&"hackfleisch".to_string()));
        // Wörterbuch-Runde 2026-07-31, Op 1: „Alles für die Schule" ist eine
        // Non-Food-Warengruppe (79 der 515 ungetaggten Produkte des
        // 11-Regionen-Korpus: EDURINO, LAMY, TIPP-EX, HERLITZ). Nebenbei
        // verlieren zwei MATTEL-Spielzeuge ihr falsches `limonade` (Marke
        // „monster" traf Monster Trucks / Monster High).
        assert_eq!(
            match_keys("EDURINO", Some("App-Lernspiel »Wörter & Sätze«"), Some("Alles für die Schule")),
            vec![NONFOOD_KEY]
        );
        assert_eq!(
            match_keys("MATTEL", Some("Haustier-Schlüsselanhänger »Monster High«"), Some("Alles für die Schule")),
            vec![NONFOOD_KEY]
        );
        // Beim Ausbau der Non-Food-Warengruppen einmal zu weit gegriffen und
        // gleich wieder zurückgenommen: „Sportnahrung" klingt nach Zubehör,
        // steht bei Lidl aber über Proteinriegeln, Protein-Chips und
        // Protein-Sahne. Das ist Essen und muss Essen bleiben.
        assert!(match_keys("Proteinriegel", None, Some("Sportnahrung"))
            .contains(&"protein/fitness".to_string()));
        assert!(match_keys("Premium High Protein-Chips", None, Some("Sportnahrung"))
            .contains(&"chips".to_string()));
        assert!(match_keys("Protein Coffee", None, Some("Sportnahrung"))
            .contains(&"kaffee".to_string()));
    }

    /// Zwei Meldungen aus `match_feedback` vom 2026-07-31, beide vom selben
    /// Tester binnen einer Minute, beide vom Kompositum-Typ: Ein Wort steckt
    /// im Titel, meint dort aber etwas anderes.
    ///
    /// Die Gegenproben stehen bewusst daneben. Eine Sperre, die „Butter" oder
    /// „Brot" insgesamt schwächt, wäre teurer als der Fehltreffer, den sie
    /// abstellt — ein fehlendes Tag kostet einen Treffer, ein falsches legt
    /// jemandem das falsche Produkt in den Einkauf.
    #[test]
    fn tester_meldungen_31_07() {
        // Süßware, keine Butter.
        assert!(keys("KORO Bio-Nut-Butter-Cups").is_empty());
        // Gebäck, kein Brot — und über die Marke landet es sogar richtig.
        assert_eq!(keys("BAHLSEN ABC Russisch Brot*"), vec!["kekse"]);

        // Gegenproben: echte Butter und echtes Brot bleiben unberührt.
        assert_eq!(keys("Kerrygold Original Irische Butter"), vec!["butter"]);
        // Vollkornbrot ist seit Tranche 3 zusätzlich `roggenbrot` — das grobe
        // „brot" bleibt daneben stehen, es wird nicht verdrängt.
        assert_eq!(keys("HARRY Vollkornbrot"), vec!["brot", "roggenbrot"]);
        assert_eq!(keys("GOLDEN TOAST Toastbrot*"), vec!["brot"]);
    }

    /// Fünf Lücken im Non-Food-Filter, gemessen am 11-Regionen-Korpus
    /// (2026-08-01). Sie sind die teuersten Fehler des Wörterbuchs, weil sie
    /// nicht bloß fehlen, sondern Treffer verschmutzen: **Zwei davon trugen
    /// vorher `kaffee`** — wer Kaffee suchte, bekam Haarspray und Waschpulver
    /// angeboten.
    ///
    /// „Chrysantheme" stand nur im Plural in der Liste; die vier anderen
    /// standen gar nicht darin.
    #[test]
    fn nonfood_luecken_korpus() {
        for titel in [
            "Chrysantheme",
            "L’Oréal Paris Elnett Haarspray",
            "Wilkinson Hydro 5 Rasierklingen",
            "Waschpulver Color",
            "Lego Set",
        ] {
            assert_eq!(keys(titel), vec![NONFOOD_KEY], "{titel} ist kein Lebensmittel");
        }

        // Gegenproben: Der Filter darf nichts Essbares mitnehmen.
        assert_eq!(keys("Kerrygold Original Irische Butter"), vec!["butter"]);
        assert_eq!(keys("Speisekartoffeln festkochend"), vec!["kartoffeln"]);
    }

    /// **Das Food-Veto greift auf dem Wort, nicht auf der Zeichenkette**
    /// (2026-08-01).
    ///
    /// Das Veto gibt es, weil Kauflands Obsttheke „Obst, Gemüse, Pflanzen"
    /// heißt und sonst über „Pflanzen" komplett herausflöge. Es kippte aber
    /// auch andersherum: In „Elektro, Kaffeemaschinen" trifft `nonfood_cat`
    /// das „Elektro" — und `kaffee` als Teilzeichenkette von „Kaffeemaschinen"
    /// legte sein Veto ein. Ein Kaffeevollautomat, dessen Titel das Wort nicht
    /// nennt, fiel damit durch in die normale Suche.
    ///
    /// **Auf dem 11-Regionen-Korpus ändert das null Zeilen** — dort ist
    /// „Obst, Gemüse, Pflanzen" die einzige Kategorie, in der beide Muster
    /// treffen. Eine schlafende Lücke, kein gemessener Ertrag; dieser Test ist
    /// der einzige Ort, an dem sie sichtbar bleibt.
    #[test]
    fn food_veto_greift_nur_auf_dem_ganzen_wort() {
        for kategorie in ["Elektro, Kaffeemaschinen", "Haushalt: Kaffeemaschinen"] {
            assert_eq!(
                match_keys("SKV 1200 A1", None, Some(kategorie)),
                vec![NONFOOD_KEY],
                "{kategorie} ist keine Lebensmittel-Warengruppe"
            );
        }
        // Gegenproben: Das Veto muss bleiben, wo es gebaut wurde.
        assert_eq!(
            match_keys("Dtsch. Zwetschgen, lose", None, Some("Obst, Gemüse, Pflanzen")),
            vec!["pfirsich"]
        );
        assert!(match_keys("Jacobs Krönung", None, Some("Kaffee, Tee, Süßwaren"))
            .contains(&"kaffee".to_string()));
        assert!(match_keys("Dallmayr prodomo", None, Some("Kaffee"))
            .contains(&"kaffee".to_string()));
    }

    /// **Drogerie-Grundbedarf ist nicht `nonfood`** (Runde 2026-08-01).
    ///
    /// Am 11-Regionen-Korpus gemessen: 28 Produkte bekamen zum ersten Mal ein
    /// Tag, ungetaggt 247 → 219 (7,7 % → 6,9 %), **keine einzige Food-Zeile
    /// wurde zu Non-Food umgestuft**.
    ///
    /// Die Trennung ist die eigentliche Entscheidung: Wer „Zahncreme" auf die
    /// Liste schreibt, will einen Treffer. `nonfood` hieße, das Produkt aus der
    /// Suche zu werfen — deshalb geht der Grundbedarf zu `windeln/hygiene`, und
    /// nur das, was niemand auf einen Einkaufszettel schreibt, wird Non-Food.
    /// Zwei Colgate-Zeilen wandern dabei bewusst von `nonfood` nach
    /// `windeln/hygiene`: Sie waren vorher unfindbar.
    #[test]
    fn drogerie_grundbedarf_bleibt_findbar() {
        for titel in [
            "Listerine Mundspülung",
            "elmex oder aronal Zahncreme",
            "fit Geschirrspülmittel",
            "Calgon Wasserenthärter",
            "Tesori d‘Oriente Weichspüler",
            "COLGATE Mundspülung*",
        ] {
            // Seit Tranche 8 tragen einige davon zusätzlich einen feineren
            // Begriff (`zahnpasta`, `shampoo`, `waschmittel`). Geprüft wird
            // hier, was der Fall immer meinte: **auffindbar bleiben**, nicht
            // „genau ein Tag haben".
            assert!(keys(titel).contains(&"windeln/hygiene".to_string()),
                    "{titel} muss auffindbar bleiben — bekam {:?}", keys(titel));
        }

        for titel in [
            "Treteimer \"Paso\"",
            "Wäschesammler",
            "Vorlesebuch",
            "Elektrische Zitruspresse",
            "Gaming Zubehör",
            "Silikonformen für Heißluftfritteusen",
            "Damen Shaping-Short oder Slip",
            "GÖTZBURG Nachtwäsche*",
            "All-in-1-Pods Color",
            "Feuchte Allzwecktücher XXL",
        ] {
            assert_eq!(keys(titel), vec![NONFOOD_KEY], "{titel} ist kein Lebensmittel");
        }

        // Gegenproben: nichts Essbares mitnehmen — und „Büro" (Kaufland
        // schreibt es mit ü, die Liste kannte nur „buero") ist keine
        // Lebensmittelgruppe.
        assert_eq!(keys("Kerrygold Original Irische Butter"), vec!["butter"]);
        assert_eq!(keys("Speisekartoffeln festkochend"), vec!["kartoffeln"]);
        assert_eq!(match_keys("Basics für die Schule", None, Some("Büro")), vec![NONFOOD_KEY]);
    }

    /// **Tranche 9: Baumarkt, Garten, Tierbedarf** (2026-08-07).
    ///
    /// 26 Begriffe, und der NONFOOD-Riegel hält **vollständig**: Von 669
    /// Angeboten wechselte keines die Seite, die Food-Zahl blieb bei 416.
    /// Rasenmäher, Heckenschere und Schrauben stehen in `NONFOOD_TERMS` und
    /// bleiben dort — ein Begriff macht sie **auf der Liste** auffindbar,
    /// holt sie aber nicht in den Angebotsvergleich zurück. Genau die
    /// Trennung, die ich für unmöglich gehalten hatte.
    #[test]
    fn artikelzeichen_tranche_9() {
        // Auf der Liste auffindbar …
        assert_eq!(keys("Gartenschere Bypass"), vec!["gartenwerkzeug"]);
        // Katzenstreu bleibt `nonfood` — „klumpstreu" steht in NONFOOD_TERMS,
        // und der Riegel greift **vor** dem Wörterbuch. Das ist genau die
        // gewünschte Arbeitsteilung, nur an einem Beispiel, das ich falsch
        // gewählt hatte.
        assert_eq!(keys("Katzenfutter Adult 400 g"), vec!["katzenfutter"]);

        // … und trotzdem nicht im Vergleich: Der Riegel greift vor dem
        // Wörterbuch, wo die Kategorie ihn ruft.
        assert_eq!(match_keys("Rasenmäher 1400 W", None, Some("Baumarkt")), vec![NONFOOD_KEY]);
    }

    /// **Tranche 8: Haushalt und Pflege** (2026-08-07).
    ///
    /// 28 Begriffe — und damit fällt der Riegel, an dem Non-Food seit dem
    /// 06.08. hing. **Er hielt von selbst, das war nachzumessen statt zu
    /// glauben:** `windeln/hygiene` ist längst ein Non-Food-Begriff, und
    /// `NONFOOD_CAT`/`NONFOOD_TERMS` sortieren echte Nicht-Lebensmittel
    /// weiter vorher aus. Von 669 Angeboten wechselten **zwei** die Seite —
    /// Zahnbürsten und Mundspülung von „nonfood" zu `zahnpasta`, und das ist
    /// richtig: Wer Zahnpasta aufschreibt, will genau die Angebote sehen.
    ///
    /// Was am 06.08. entschieden wurde, bleibt unberührt: Die Aktionsware
    /// fliegt in der **Anzeige** aus den Top-Angeboten (`Categories
    /// .middleAisle`), nicht im Wörterbuch.
    #[test]
    fn artikelzeichen_tranche_8() {
        // Der Gewinn: Drogerie-Grundbedarf trägt jetzt einen genauen Begriff.
        assert!(keys("COLGATE Zahnbürsten*").contains(&"zahnpasta".to_string()));

        // Der Riegel hält: echte Nicht-Lebensmittel bleiben aussortiert.
        assert_eq!(match_keys("Bosch Staubsauger", None, Some("Elektro")), vec![NONFOOD_KEY]);
        assert_eq!(keys("Akku-Bohrhammer"), vec![NONFOOD_KEY]);
    }

    /// **Tranche 7: die letzten Lebensmittel** (2026-08-07).
    ///
    /// 25 Begriffe — damit ist Bring!s Lebensmittelteil weitgehend
    /// abgedeckt. Darunter `erdnussbutter`, das in Tranche 1 **zurückgezogen**
    /// werden musste: Die Synonyme „peanut butter" und „nut butter" rissen
    /// „KORO Bio-Nut-Butter-Cups" mit, eine Süßware. Ohne sie trägt der
    /// Begriff, und der Testermeldung vom 31.07. geschieht nichts.
    #[test]
    fn artikelzeichen_tranche_7() {
        assert_eq!(keys("Erdnussmus crunchy 500 g"), vec!["erdnussbutter"]);
        // Die Meldung vom 31.07. bleibt erfüllt.
        assert!(keys("KORO Bio-Nut-Butter-Cups").is_empty());

        // Feiner neben dem groben.
        assert_eq!(keys_cat("Rind Hamburger 400 g", "Rindfleisch"), vec!["rind", "hamburger"]);

        // Die Sperren gegen die nahen Verwandten.
        assert!(!keys("Pfefferminztee 20 Beutel").contains(&"pfeffer".to_string()));
        assert!(!keys("Paprikapulver edelsüß").contains(&"pfeffer".to_string()));
    }

    /// **Tranche 6: der Rest aus Obst & Gemüse** (2026-08-07).
    ///
    /// 23 Begriffe. Ein gepinnter Fall wird dabei **richtiger, nicht nur
    /// feiner**: „Getrocknete Cranberries" hieß über die Kategorie „nüsse",
    /// weil es keinen Begriff für Preiselbeeren gab. Eine Cranberry ist keine
    /// Nuss.
    #[test]
    fn artikelzeichen_tranche_6() {
        assert_eq!(keys_cat("Getrocknete Cranberries 200 g", "Nüsse & Trockenfrüchte"),
                   vec!["preiselbeeren"]);
        // Gegenprobe: echte Nüsse bleiben Nüsse.
        // Der grobe Begriff bleibt daneben stehen — „nüsse" trägt das Suffix
        // „nüsse" und trifft die Haselnuss zu Recht mit.
        assert_eq!(keys("Ganze Haselnusskerne 200 g"), vec!["nüsse", "haselnüsse"]);
        assert_eq!(keys("Cashewkerne geröstet 150 g"), vec!["nüsse"]);

        // Die Sperre: Kokosmilch ist keine Kokosnuss.
        assert!(!keys("Kokosmilch 400 ml").contains(&"kokosnuss".to_string()));
    }

    /// **Tranche 5: Zutaten, Gewürze, Tiefkühl** (2026-08-07).
    ///
    /// 28 Begriffe. Die Abdeckung bleibt bei 98,3 % — Gewürze und Backzutaten
    /// stehen selten im Prospekt, aber oft auf einer Einkaufsliste. **Das ist
    /// der Teil des Wortschatzes, den nur die Liste braucht und der Vergleich
    /// nicht**, und damit der erste Vorgriff auf die Trennung, die für
    /// Non-Food ohnehin kommen muss.
    #[test]
    fn artikelzeichen_tranche_5() {
        assert_eq!(keys("Dr. Oetker Backpulver 10er"), vec!["backpulver"]);
        assert_eq!(keys("Bourbon Vanilleschote"), vec!["vanille"]);

        // Die Sperren, die die feineren Begriffe von ihren Verwandten trennen:
        // Vanillezucker ist nicht Vanille, Zimtschnecken sind nicht Zimt.
        assert!(!keys("Dr. Oetker Vanillezucker 10er").contains(&"vanille".to_string()));
        assert!(!keys("Zimtschnecken 4 Stück").contains(&"zimt".to_string()));
    }

    /// **Tranche 4: Getränke, Süßwaren, Getreide** (2026-08-07).
    ///
    /// 25 Begriffe, Abdeckung 98,1 % → **98,3 %**. Wieder überwiegend feiner
    /// statt mehr: Kaffeekapseln waren „kaffee", Gin war „spirituosen",
    /// Mie-Nudeln waren „nudeln".
    #[test]
    fn artikelzeichen_tranche_4() {
        assert_eq!(keys("NESCAFÉ Farmers Origins Kaffeekapseln*"), vec!["kaffee", "kaffeepads"]);
        assert_eq!(keys("GORDON'S London Dry Gin*"), vec!["spirituosen", "schnaps"]);
        assert_eq!(keys("NATURGUT Bio Mie-Nudeln*"), vec!["nudeln", "glasnudeln"]);

        // Gegenproben: die groben Begriffe bleiben, wo nichts Feineres passt.
        assert_eq!(keys("Barilla Spaghetti No. 5"), vec!["nudeln"]);
        assert_eq!(keys("Dallmayr prodomo gemahlen"), vec!["kaffee"]);
    }

    /// **Tranche 3: Brot, Milchprodukte, Fleisch & Fisch** (2026-08-07).
    ///
    /// 23 Begriffe. Die Abdeckung bleibt bei 98,1 % — hier geht es nicht um
    /// mehr Treffer, sondern um **feinere**: 18 Titel bekommen zusätzlich zum
    /// groben Begriff einen genauen. Ein Hüftsteak war „rind", ein Reibekäse
    /// „käse", ein Serrano „wurst".
    #[test]
    fn artikelzeichen_tranche_3() {
        // Feiner **neben** dem groben, nicht statt seiner.
        assert_eq!(keys("BUTCHER'S Frisches Hüftsteak*"), vec!["rind", "steak"]);
        assert_eq!(keys("KERRYGOLD Reibekäse*"), vec!["käse", "reibekäse"]);
        assert_eq!(keys("Croissant XXL"), vec!["backwaren", "croissant"]);

        // **Die Sperre, die eine Messung gefordert hat:** „vegan Schnitzel"
        // ist der Fall vom 21.07. und gehört zu `tofu`, nicht zu `schnitzel`.
        assert!(!keys("Vegan Schnitzel").contains(&"schnitzel".to_string()));

        // Gegenprobe: ein echtes Schnitzel behält seinen Begriff.
        assert_eq!(keys("MÜHLENHOF Frische Puten-Schnitzel*"), vec!["pute", "schnitzel"]);
    }

    /// **Tranche 2: Obst, Gemüse, Kräuter** (2026-08-07).
    ///
    /// 24 Begriffe aus Bring!s größter Kategorie. **Der erste Zug, der die
    /// Abdeckung hebt** — 97,8 % → 98,1 %, weil „Spitzkohl" vorher durch alle
    /// Raster fiel. Acht Titel wechseln die Zuordnung; die drei, die etwas
    /// über die neuen Begriffe aussagen, stehen hier.
    #[test]
    fn artikelzeichen_tranche_2() {
        // Der Gewinn: Kohl war eine Lücke, keine Entscheidung.
        assert_eq!(keys_cat("Spitzkohl", "Brokkoli und Kohl"), vec!["kohl"]);
        // Feiner statt gröber: eine Birne ist nicht mehr nur „obst".
        assert_eq!(keys("Birnen*"), vec!["birnen"]);
        assert_eq!(keys("XXL-Basilikum"), vec!["basilikum"]);

        // **Zwei Sperren, die Messungen erzwungen haben.**
        // Ein Brot mit Feigen darin darf nicht „feigen" heißen. Der Titel
        // allein trägt gar keinen Begriff — sein „brot" kommt aus der
        // Kategorie, und genau die soll er behalten.
        assert!(!keys("Couronne Feigen-Walnuss").contains(&"feigen".to_string()));
        // (Welche Kategorie ihn trägt, hängt am Prospekt; geprüft wird hier
        // nur, dass die Sperre greift.)
        // Und „chili" gehört nicht in `peperoni`: Damit trug ein Käse
        // plötzlich zwei Tags.
        assert_eq!(keys_cat("Plus Pack 156 g, Chili Paprika", "Schnittkäse"), vec!["paprika"]);

        // Gegenproben: die groben Begriffe bleiben, wo nichts Feineres passt.
        assert_eq!(keys("Obst der Saison"), vec!["obst"]);
    }

    /// **Tranche 1 zum Artikelzeichen-Vorhaben** (2026-08-07).
    ///
    /// 23 Begriffe für Wörter, die bisher auf einer Blockliste standen und
    /// keinem Begriff gehörten — sie trafen also nichts. Der Zug ist additiv;
    /// gemessen an der letzten vollen Woche (669 Angebote) blieb die Abdeckung
    /// bei 97,8 %, und genau **vier** Titel wechselten ihre Zuordnung. Alle
    /// vier stehen hier, damit sie nicht unbemerkt zurückkippen.
    #[test]
    fn artikelzeichen_tranche_1() {
        // Ein Riegel ist kein Müsli, und eine Brühe kein Eintopf.
        assert_eq!(keys("CORNY Müsliriegel*"), vec!["müsliriegel"]);
        assert_eq!(keys("NATURGUT Bio Gemüsebrühe*"), vec!["brühe"]);
        // Kuchen kommt zum groben „backwaren" dazu, ohne es zu verdrängen.
        assert_eq!(keys("JOMO Kuchen*"), vec!["backwaren", "kuchen"]);

        // Gegenproben: Was die neuen Begriffe **nicht** anfassen dürfen.
        assert_eq!(keys("Bio Müsli Knusper"), vec!["müsli"]);
        assert_eq!(keys("Gulaschsuppe im Glas"), vec!["eintopf"]);

        // **Die Sperre, die eine Messung erzwungen hat.** Ohne sie holte sich
        // „Süßkartoffel Chips" den Begriff `süßkartoffeln`, und wer
        // Süßkartoffeln aufschreibt, bekäme eine Tüte Chips vorgeschlagen.
        assert_eq!(keys("NATURGUT Bio Süßkartoffel Chips*"), vec!["chips"]);

        // Und die Gegenprobe dazu: die echte Knolle behält ihren Begriff.
        assert_eq!(keys("Süßkartoffeln lose"), vec!["süßkartoffeln"]);
    }

    #[test]
    fn marken_fallback() {
        assert_eq!(keys("Fruchtzwerge"), vec!["joghurt"]);
        assert_eq!(keys("Bitburger Premium"), vec!["bier"]);
    }

    /// **Die längere Marke gewinnt.** In JSON-Reihenfolge bekam „MILKANA"
    /// (Schmelzkäse) `schokolade`, weil der Name „milka" enthält und der
    /// Fallback beim ersten `contains` stehenblieb. Gegen den Stand vor der
    /// Sortierung fällt dieser Test mit `schokolade`.
    #[test]
    fn die_laengere_marke_gewinnt_gegen_die_teilzeichenkette() {
        assert_eq!(keys("MILKANA"), vec!["käse"]);
        assert_eq!(keys("MILKANA Tolle Rolle!"), vec!["käse"]);
        // Und die kurze Marke behält, was ihr gehört.
        assert_eq!(keys("Milka Alpenmilch"), vec!["schokolade"]);
    }

    /// Die Marke zieht **nur**, wenn Titel und Untertitel nichts ergeben.
    ///
    /// ALDI Nords Untertitel ist oft bloß eine Variante („Zero Sugar",
    /// „Medium"), und der Produktname steckt in der Marke. Gemessen am
    /// 11-Regionen-Korpus: **72 Zeilen neu getaggt (12 verschiedene Produkte),
    /// 0 Zeilen mit verändertem oder verlorenem Tag.** Der naive Weg — die
    /// ganze Überzeile an den Untertitel hängen — änderte 48.
    #[test]
    fn die_marke_taggt_nur_was_sonst_leer_bliebe() {
        // Ohne Marke ungetaggt, mit Marke richtig.
        assert!(match_keys("Zero Sugar", Some("2-L-Flasche"), None).is_empty());
        assert_eq!(
            match_keys_with_brand("Zero Sugar", Some("2-L-Flasche"), None, Some("COCA-COLA")),
            vec!["limonade"]
        );
        assert_eq!(
            match_keys_with_brand("Medium", Some("1,5-L-Flasche"), None, Some("GEROLSTEINER")),
            vec!["wasser"]
        );

        // Und der Gegenbeweis: Wo schon ein Tag steht, kommt die Marke gar
        // nicht erst zum Zug — auch dann nicht, wenn sie ein anderes brächte.
        let ohne = match_keys("Vollmilch", Some("1 l"), None);
        assert_eq!(ohne, vec!["milch"]);
        assert_eq!(
            match_keys_with_brand("Vollmilch", Some("1 l"), None, Some("Milka")),
            ohne,
            "Die Marke darf ein vorhandenes Tag nicht überschreiben"
        );
    }

    /// Eine leere oder fehlende Marke ändert nichts — sonst hinge das Ergebnis
    /// daran, ob ein Scraper `None` oder `Some("")` liefert.
    #[test]
    fn leere_marke_aendert_nichts() {
        for brand in [None, Some(""), Some("   ")] {
            assert!(match_keys_with_brand("Zero Sugar", Some("2-L-Flasche"), None, brand).is_empty());
        }
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

    /// Zeilenweiser Paritäts-Check gegen die Python-Referenz:
    /// `cargo test parity_with_eval_db -- --ignored --nocapture`
    /// Andere Basis: `LECHARIOT_PARITY_DB=~/.local/share/lechariot/testkorpus.db`.
    ///
    /// Hier standen bis 2026-07-31 drei Summen (nonfood/getaggt/ungetaggt),
    /// die jemand von Hand mit der Ausgabe des Python-Skripts verglich. Summen
    /// können strukturell nicht sehen, dass beide Maschinen DERSELBEN Zeile
    /// verschiedene Tags geben: solange die Zeile hüben wie drüben als
    /// „getaggt" zählt, bleibt jede Summe gleich. Genau so lag `thunfisch-salat`
    /// — Python entschied „Phrase oder Wort" am Rohstring, Rust an der
    /// normalisierten Form, „Thunfisch-Salat" bekam hier `fisch` und dort
    /// `fisch, salat`, und die Summen schwiegen.
    ///
    /// Geprüft werden zwei Mengen, und die zweite ist nicht schmückendes
    /// Beiwerk: Die Angebotszeilen allein hätten genau diesen Fund NICHT
    /// gefunden. Im Korpus heißt das Produkt „Thunfischsalat" in einem Wort,
    /// den Bindestrich trägt nur der Wörterbuch-Eintrag. Deshalb geht jeder
    /// Eintrag des Wörterbuchs zusätzlich als eigener Fall durch beide
    /// Maschinen — dort, in den Einträgen selbst, sitzt diese Fehlerklasse.
    #[test]
    #[ignore]
    fn parity_with_eval_db() {
        const QUERY: &str = "select o.title, coalesce(o.subtitle,''), coalesce(o.category,'') \
                             from offers o where o.valid_until >= date('now') order by o.id";
        let root = env!("CARGO_MANIFEST_DIR");
        let db = std::env::var("LECHARIOT_PARITY_DB").unwrap_or_else(|_| {
            std::env::var("HOME").unwrap() + "/.local/share/lechariot/lechariot.db"
        });

        // Der Treiber ruft `match_keys` der Referenz, nicht `term_hits`: sonst
        // bliebe der Marken- und der Kategorie-Weg ungeprüft, und die stehen
        // hinter jeder dritten getaggten Zeile. Er gibt den Fall mit aus, damit
        // beide Maschinen nachweislich denselben Eingaben begegnen — verglichen
        // wird die Ausgabe, nicht die Fähigkeit, dieselbe Liste zu bauen.
        let driver = format!(
            r#"
import importlib.util, sqlite3, sys
spec = importlib.util.spec_from_file_location("ev", r"{root}/docs/matching-woerterbuch-eval.py")
ev = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ev)
faelle = [("db",) + r for r in sqlite3.connect(sys.argv[1]).execute("""{QUERY}""")]
for begriff, (exact, suffix, block) in ev.V.items():
    eintraege = list(exact) + list(suffix) + list(block) + list(ev.PRAEFIX.get(begriff, []))
    faelle += [("dict", e, "", "") for e in eintraege]
for quelle, title, sub, cat in faelle:
    title, sub, cat = (f.replace("\t", " ") for f in (title, sub, cat))
    tags = ",".join(sorted(ev.match_keys(title, sub, cat)[0]))
    print("\t".join((quelle, title, sub, cat, tags)))
"#
        );
        let out = std::process::Command::new("python3")
            .arg("-c")
            .arg(&driver)
            .arg(&db)
            .output()
            .expect("python3 fehlt — ohne die Referenz gibt es keine Parität zu prüfen");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8(out.stdout).unwrap();

        // Verglichen wird sortiert: die Reihenfolge der Tags ist nirgends
        // zugesichert, ihre Menge schon.
        let (mut aus_db, mut aus_dict) = (0usize, 0usize);
        let mut abweichungen = Vec::new();
        for line in stdout.lines() {
            let f: Vec<&str> = line.splitn(5, '\t').collect();
            assert_eq!(f.len(), 5, "unerwartete Zeile der Referenz: {line}");
            let (quelle, title, sub, cat, py) = (f[0], f[1], f[2], f[3], f[4]);
            if quelle == "db" { aus_db += 1 } else { aus_dict += 1 }
            let mut keys = match_keys(title, Some(sub), Some(cat));
            keys.sort();
            let rs = keys.join(",");
            if rs != py {
                abweichungen.push(format!(
                    "  [{quelle}] „{title}“ | {sub} | {cat}\n      rust [{rs}]  python [{py}]"
                ));
            }
        }

        // Gegenprobe, dass die Referenz überhaupt gearbeitet hat: eine leere
        // oder halbe Ausgabe wäre sonst der grünste Test der Welt.
        let conn = rusqlite::Connection::open(&db).unwrap();
        let erwartet: usize = conn
            .query_row(&format!("select count(*) from ({QUERY})"), [], |r| r.get(0))
            .map(|n: i64| n as usize)
            .unwrap();
        assert_eq!(aus_db, erwartet, "die Referenz sah nicht dieselben Angebotszeilen");
        assert!(aus_dict > 0, "kein einziger Wörterbuch-Eintrag geprüft");

        println!("{aus_db} Angebotszeilen aus {db} + {aus_dict} Wörterbuch-Einträge verglichen");
        assert!(
            abweichungen.is_empty(),
            "{} von {} Fällen weichen ab:\n{}",
            abweichungen.len(),
            aus_db + aus_dict,
            abweichungen.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
        );
    }
}
