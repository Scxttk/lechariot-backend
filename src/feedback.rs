//! Feedback-Runde: aus Meldungen in `match_feedback` werden Wörterbuch-Änderungen.
//!
//! Die App fragt nach, wenn jemand einen Treffer weglegt (Tabelle
//! `match_feedback`). Bis zum 20.10. las `/pflegerunde` diese Meldungen von
//! Hand; seitdem erreichten sie nichts mehr. Diese Runde schließt die Schleife,
//! ohne dass irgendetwas ungeprüft ins Wörterbuch kommt:
//!
//! 1. Meldungen nach der letzten verarbeiteten ID holen, je (Suchwort, Titel)
//!    zu einem Fall bündeln, Vorlieben und Mengenfragen weglassen
//!    (`docs/feedback-auswertung.md`), und nur Fälle behalten, die das heutige
//!    Wörterbuch noch erzeugt.
//! 2. Claude Haiku schlägt je Fall genau eine Änderung vor: Sperrwort,
//!    Synonym, ein Wort streichen, Non-Food-Wort, Regal oder keine. Nur an vorhandenen Begriffen —
//!    ein neuer Begriff bräuchte in der App ein Artikelzeichen und eine
//!    Regalzeile, das ist Handarbeit.
//! 3. Jede Änderung wird einzeln auf eine Kopie gelegt und gemessen: Sie muss
//!    ihren gemeldeten Fall beheben, darf auf dem Goldsatz keine der drei
//!    Zahlen verschlechtern und darf im Wochenbestand nur wenige Zeilen
//!    bewegen. Was eine Bedingung reißt, fällt weg und steht im Bericht.
//! 4. Ergebnis sind drei Dateien: das Wörterbuch, die Liste der übernommenen
//!    Änderungen samt Stand (`matching-woerterbuch-feedback.json`, die
//!    Python-Referenz liest sie mit) und der PR-Text. Den PR öffnet der
//!    Workflow; gemergt wird von Hand.
//!
//! **Regal-Vorschläge werden nicht angewendet.** Das Feld `warengruppe` gehört
//! `enrich::kategorie_fuer_begriff` (`tests/warengruppen.rs`), also der
//! Keyword-Tabelle, die auch die Kategorie jedes importierten Angebots
//! bestimmt. Eine Regaländerung trifft damit den Import, nicht nur die Suche,
//! und der Goldsatz misst sie nicht. Sie stehen im PR zur Handprüfung.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::llm_tags::{self, SCHICHT, Usage};
use crate::matching::{NONFOOD_KEY, Woerterbuch};
use crate::push::PushConfig;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-haiku-5-5";
const PROMPT: &str = include_str!("../docs/llm-tagging/feedback-prompt.txt");
const PROMPT_PRUEFUNG: &str = include_str!("../docs/llm-tagging/feedback-gegenpruefung.txt");
/// So viele bewegte Titel je Änderung sieht die Gegenprüfung.
const BEWEGT_IN_PRUEFUNG: usize = 10;
/// Fälle je Anfrage. Ein Fall trägt seine Wörterbuch-Einträge mit, 20 Fälle
/// sind rund 6 000 Tokens — weit unter jeder Grenze, und ein Fehler kostet
/// höchstens 20 Fälle.
const FAELLE_JE_ANFRAGE: usize = 20;
const MAX_TOKENS: u32 = 8192;
/// Höchstens so viele Zeilen des Wochenbestands darf ein Sperr- oder
/// Non-Food-Wort ihren Begriff nehmen. Die gemeldete Zeile selbst steht oft
/// mehrfach drin (je Kette, je Größe); was darüber hinaus geht, ist kein
/// Einzelfall mehr, sondern ein Eingriff, den ein Mensch ansehen soll.
pub const MAX_STREUUNG_SPERRE: usize = 10;
/// Dasselbe für ein Synonym: Es darf mehr Zeilen bewegen, weil es nur dazu
/// gibt und nichts nimmt — aber ein Wort, das 50 Angebote neu taggt, ist eher
/// ein neuer Begriff als eine Wortform.
pub const MAX_STREUUNG_SYNONYM: usize = 25;
/// Höchstens so viele Kommentare je Fall gehen an das Modell, gekürzt.
const KOMMENTARE_JE_FALL: usize = 2;
const KOMMENTAR_ZEICHEN: usize = 200;

pub const WOERTERBUCH: &str = "docs/matching-woerterbuch.json";
pub const STAND: &str = "docs/matching-woerterbuch-feedback.json";

// ------------------------------------------------------------------ Daten

/// Eine Zeile aus `match_feedback`. `install_id` wird gar nicht erst gelesen.
#[derive(Debug, Clone, Deserialize)]
pub struct Meldung {
    pub id: i64,
    pub query: String,
    pub product_title: String,
    #[serde(default)]
    pub match_kind: Option<String>,
    pub reason: String,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Mehrere Meldungen zu einem (Suchwort, Titel). Zehn Meldungen zum selben
/// Fehltreffer sind ein Fall, nicht zehn.
#[derive(Debug, Clone)]
pub struct Fall {
    pub query: String,
    pub titel: String,
    /// `direct` oder `category` — wie die App den Treffer gefunden hat.
    pub weg: String,
    pub gruende: BTreeMap<String, usize>,
    pub ids: Vec<i64>,
    pub kommentare: Vec<String>,
    /// Begriffe des Suchworts, die den Titel heute noch treffen.
    pub offen: Vec<String>,
}

impl Fall {
    pub fn meldungen(&self) -> usize {
        self.ids.len()
    }
}

#[derive(Debug, Default)]
pub struct Zaehlung {
    pub meldungen: usize,
    /// `personal_taste`: eine Vorliebe, kein Fehler im Wörterbuch.
    pub vorliebe: usize,
    /// `wrong_size`: ein Mengenproblem, das kein Wort lösen kann.
    pub menge: usize,
    /// Fälle, die das heutige Wörterbuch schon nicht mehr erzeugt.
    pub erledigt: usize,
    /// Fälle, deren Suchwort keinen Begriff hat — es gibt keinen Eintrag,
    /// an dem eine Änderung ansetzen könnte.
    pub ohne_begriff: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Sperrwort,
    Synonym,
    /// Ein Wort aus der exact-Liste nehmen, das dort nicht hingehört
    /// (`obst` führte „pak choi").
    Streichen,
    Nonfood,
    Regal,
    Keine,
}

/// Was das Modell zu einem Fall sagt.
#[derive(Debug, Clone, Deserialize)]
pub struct Vorschlag {
    pub fall: usize,
    pub op: Op,
    #[serde(default)]
    pub begriff: String,
    #[serde(default)]
    pub wort: String,
    #[serde(default)]
    pub regal: String,
    #[serde(default)]
    pub grund: String,
}

/// Eine Änderung, gebündelt über alle Fälle, die sie vorschlagen.
#[derive(Debug, Clone)]
pub struct Kandidat {
    pub op: Op,
    pub begriff: String,
    pub wort: String,
    pub regal: String,
    pub grund: String,
    pub faelle: Vec<usize>,
}

/// Eine übernommene Änderung, so wie sie in `matching-woerterbuch-feedback.json`
/// steht. Die Python-Referenz liest `op`, `begriff` und `wort`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Eintrag {
    pub op: Op,
    pub begriff: String,
    pub wort: String,
    pub runde: String,
    pub meldungen: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stand {
    /// Höchste ID aus `match_feedback`, die eine Runde gesehen hat.
    pub letzte_id: i64,
    pub aenderungen: Vec<Eintrag>,
}

// ------------------------------------------------------------------ Fälle

/// Meldungen zu Fällen bündeln. Was das heutige Wörterbuch nicht mehr erzeugt,
/// fällt hier schon heraus — dafür muss niemand ein Modell fragen.
pub fn faelle(meldungen: &[Meldung], wb: &Woerterbuch) -> (Vec<Fall>, Zaehlung) {
    let mut z = Zaehlung { meldungen: meldungen.len(), ..Default::default() };
    let mut map: BTreeMap<(String, String), Fall> = BTreeMap::new();
    for m in meldungen {
        match m.reason.as_str() {
            "personal_taste" => {
                z.vorliebe += 1;
                continue;
            }
            "wrong_size" => {
                z.menge += 1;
                continue;
            }
            _ => {}
        }
        let key = (m.query.trim().to_lowercase(), m.product_title.trim().to_string());
        let fall = map.entry(key).or_insert_with(|| Fall {
            query: m.query.trim().to_string(),
            titel: m.product_title.trim().to_string(),
            weg: m.match_kind.clone().unwrap_or_default(),
            gruende: BTreeMap::new(),
            ids: Vec::new(),
            kommentare: Vec::new(),
            offen: Vec::new(),
        });
        *fall.gruende.entry(m.reason.clone()).or_default() += 1;
        fall.ids.push(m.id);
        if let Some(c) = m.comment.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            if fall.kommentare.len() < KOMMENTARE_JE_FALL {
                fall.kommentare.push(c.chars().take(KOMMENTAR_ZEICHEN).collect());
            }
        }
    }
    let mut out = Vec::new();
    for mut fall in map.into_values() {
        if suchbegriffe(wb, &fall.query).is_empty() {
            z.ohne_begriff += 1;
            continue;
        }
        fall.offen = offen(wb, &fall);
        if fall.offen.is_empty() {
            z.erledigt += 1;
        } else {
            out.push(fall);
        }
    }
    // Häufig gemeldete Fälle zuerst: Sie bekommen beim Messen den Vortritt.
    out.sort_by(|a, b| b.meldungen().cmp(&a.meldungen()).then_with(|| a.titel.cmp(&b.titel)));
    (out, z)
}

/// Begriffe, die die App unter dem Suchwort versteht (der engste, nicht jeder
/// Topf darüber — „Heidelbeeren" meint `heidelbeeren`, nicht `beeren`).
fn suchbegriffe(wb: &Woerterbuch, query: &str) -> Vec<String> {
    wb.bedeutung(query)
}

/// Begriffe des Suchworts, über die der gemeldete Titel heute noch hereinkommt.
///
/// `category`: das Angebot trägt den Begriff. Seit #101 kommen die Tags in
/// der Produktion von Haiku; das Wörterbuch ist dort Hinweis und Rückfall.
/// Gemessen wird hier, ob das *Wörterbuch* den Begriff noch vergibt.
/// `direct`: das Suchwort steht im Titel, und seit app#167 hält dann nur die
/// Sperrliste des Begriffs den Treffer auf — die liest die App selbst, eine
/// Sperre wirkt dort also sofort.
pub fn offen(wb: &Woerterbuch, fall: &Fall) -> Vec<String> {
    let such = suchbegriffe(wb, &fall.query);
    let titel = wb.match_keys(&fall.titel, None, None);
    if titel.iter().any(|t| t == NONFOOD_KEY) {
        return Vec::new();
    }
    if fall.weg == "direct" {
        such.into_iter().filter(|b| !wb.sperrt(b, &fall.titel)).collect()
    } else {
        such.into_iter().filter(|b| titel.contains(b)).collect()
    }
}

/// Behebt `k` den Fall `f`: offen auf `vorher`, behoben auf `nachher`? Ein
/// Fall, den eine früher übernommene Änderung schon behoben hat, zählt nicht —
/// sonst käme jedes weitere Sperrwort zum selben Fall ungeprüft mit durch.
fn behebt(vorher: &Woerterbuch, nachher: &Woerterbuch, k: &Kandidat, f: &Fall) -> bool {
    match k.op {
        Op::Sperrwort | Op::Streichen | Op::Nonfood => {
            offen(vorher, f).contains(&k.begriff) && !offen(nachher, f).contains(&k.begriff)
        }
        Op::Synonym => {
            let hat = |wb: &Woerterbuch| wb.match_keys(&f.titel, None, None).contains(&k.begriff);
            !hat(vorher) && hat(nachher)
        }
        Op::Regal | Op::Keine => false,
    }
}

// ------------------------------------------------------------------ Änderungen

fn wort_gueltig(w: &str) -> bool {
    w.chars().count() >= 3
        && !w.starts_with(' ')
        && !w.ends_with(' ')
        && !w.contains("  ")
        && w.chars().all(|c| matches!(c, 'a'..='z' | 'ä' | 'ö' | 'ü' | 'ß' | ' '))
}

/// Die Sorte unter `topf`, die `wort` heißt oder in ihrer exact-Liste führt.
///
/// Seit #92 trägt ein Salami-Angebot `salami` **und** `wurst`, und wer „Wurst"
/// sucht, soll die Salami sehen. Ein Sperrwort, das eine Sorte aus ihrem Topf
/// wirft, sähe auf dem Goldsatz harmlos aus (`salami` allein ist dort auch
/// richtig) und zerstörte genau diese Regel.
fn sorte_von(dict: &Value, topf: &str, wort: &str) -> Option<String> {
    dict["begriffe"].as_object()?.iter().find_map(|(k, e)| {
        let unter = e["oberbegriff"].as_str() == Some(topf);
        let fuehrt = k == wort || e["exact"].as_array().is_some_and(|a| a.iter().any(|v| v.as_str() == Some(wort)));
        (unter && fuehrt).then(|| k.clone())
    })
}

/// Legt eine Änderung auf das Wörterbuch-JSON. Ein Fehler heißt: Die Änderung
/// ist so nicht anwendbar (unbekannter Begriff, ungültiges Wort, steht schon da).
pub fn anwenden(dict: &mut Value, op: Op, begriff: &str, wort: &str) -> Result<()> {
    if !wort_gueltig(wort) {
        bail!("ungültiges Wort {wort:?}");
    }
    match op {
        Op::Streichen => {
            if let Some(sorte) = sorte_von(dict, begriff, wort) {
                bail!("{wort:?} gehört zur Sorte {sorte:?}, die unter {begriff:?} liegt (#92)");
            }
            let liste = dict["begriffe"]
                .get_mut(begriff)
                .with_context(|| format!("unbekannter Begriff {begriff:?}"))?["exact"]
                .as_array_mut()
                .with_context(|| format!("{begriff:?} hat kein Feld exact"))?;
            let vorher = liste.len();
            liste.retain(|v| v.as_str() != Some(wort));
            if liste.len() == vorher {
                bail!("{wort:?} steht nicht bei {begriff:?}");
            }
            if liste.is_empty() {
                bail!("{begriff:?} hätte danach keinen Treffer mehr");
            }
        }
        Op::Sperrwort | Op::Synonym => {
            let eintrag =
                dict["begriffe"].get(begriff).with_context(|| format!("unbekannter Begriff {begriff:?}"))?;
            let schon = |feld: &str| {
                eintrag[feld].as_array().is_some_and(|a| a.iter().any(|v| v.as_str() == Some(wort)))
            };
            let feld = if op == Op::Sperrwort { "block" } else { "exact" };
            if op == Op::Sperrwort && (schon("exact") || wort == begriff) {
                bail!("{wort:?} ist ein Treffer von {begriff:?}, kein Sperrwort");
            }
            if schon(feld) {
                bail!("{wort:?} steht schon bei {begriff:?}");
            }
            if op == Op::Sperrwort {
                if let Some(sorte) = sorte_von(dict, begriff, wort) {
                    bail!("{wort:?} gehört zur Sorte {sorte:?}, die unter {begriff:?} liegt (#92)");
                }
            }
            dict["begriffe"][begriff][feld]
                .as_array_mut()
                .with_context(|| format!("{begriff:?} hat kein Feld {feld}"))?
                .push(Value::String(wort.to_string()));
        }
        Op::Nonfood => {
            let muster = dict["nonfood_terms"].as_str().context("Feld nonfood_terms fehlt")?;
            if muster.split('|').any(|t| t == wort) {
                bail!("{wort:?} steht schon bei den Non-Food-Wörtern");
            }
            // Nur Buchstaben und Leerzeichen (wort_gueltig): nichts zu maskieren,
            // und Python wie Rust lesen dieselbe Alternative.
            dict["nonfood_terms"] = Value::String(format!("{muster}|{wort}"));
        }
        Op::Regal | Op::Keine => bail!("{op:?} ändert das Wörterbuch nicht"),
    }
    Ok(())
}

/// Dieselbe Formatierung wie `examples/warengruppen.rs`: ein Leerzeichen
/// Einrückung, Zeilenumbruch am Ende. Der Abgleich mit der App vergleicht Bytes.
pub fn als_text(dict: &Value) -> Result<String> {
    let mut ausgabe = Vec::new();
    let formatierer = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut ausgabe, formatierer);
    Serialize::serialize(dict, &mut ser)?;
    ausgabe.push(b'\n');
    Ok(String::from_utf8(ausgabe)?)
}

// ------------------------------------------------------------------ Messen

#[derive(Deserialize)]
pub struct Gold {
    set: String,
    title: String,
    #[serde(default)]
    sub: String,
    #[serde(default)]
    cat: String,
    gold: Option<Vec<String>>,
}

/// ok / fehlt / falsch in Prozent, auf die Produktion zurückgewichtet —
/// dieselbe Rechnung wie `lechariot llm-tags`, hier für das Wörterbuch allein.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoldWert {
    pub ok: f64,
    pub fehlt: f64,
    pub falsch: f64,
}

impl GoldWert {
    /// Welche Zahl ist schlechter als in `vorher`? Leer heißt: keine.
    fn schlechter_als(&self, vorher: &GoldWert) -> Vec<String> {
        const EPS: f64 = 1e-9;
        let mut out = Vec::new();
        if self.ok < vorher.ok - EPS {
            out.push(format!("Goldsatz ok {:.1} → {:.1} %", vorher.ok, self.ok));
        }
        if self.fehlt > vorher.fehlt + EPS {
            out.push(format!("Goldsatz fehlt {:.1} → {:.1} %", vorher.fehlt, self.fehlt));
        }
        if self.falsch > vorher.falsch + EPS {
            out.push(format!("Goldsatz falsch {:.1} → {:.1} %", vorher.falsch, self.falsch));
        }
        out
    }
}

pub fn gold_messen(wb: &Woerterbuch, gold: &[Gold], ober: &HashMap<String, String>) -> GoldWert {
    let mut c: HashMap<&str, f64> = HashMap::new();
    for g in gold {
        let gewicht = SCHICHT
            .iter()
            .find(|(s, _)| *s == g.set)
            .map(|(s, n)| n / gold.iter().filter(|x| x.set == *s).count() as f64)
            .unwrap_or(1.0);
        let sub = Some(g.sub.as_str()).filter(|s| !s.is_empty());
        let cat = Some(g.cat.as_str()).filter(|s| !s.is_empty());
        let tags = wb.match_keys(&g.title, sub, cat);
        *c.entry(llm_tags::urteil(&tags, &g.gold, ober)).or_default() += gewicht;
    }
    // Ohne „harmlos", wie in llm_tags::eval_gold.
    let n: f64 = c.iter().filter(|(k, _)| **k != "harmlos").map(|(_, v)| v).sum();
    let teil = |k: &str| c.get(k).copied().unwrap_or(0.0) / n.max(f64::MIN_POSITIVE) * 100.0;
    GoldWert { ok: teil("ok"), fehlt: teil("fehlt"), falsch: teil("falsch") }
}

/// Eine Zeile des Wochenbestands: Produkttitel und Kategorie der Kette.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Zeile {
    pub product: String,
    #[serde(default)]
    pub category: Option<String>,
}

fn korpus_tags(wb: &Woerterbuch, korpus: &[Zeile]) -> Vec<Vec<String>> {
    korpus.iter().map(|z| wb.match_keys(&z.product, None, z.category.as_deref())).collect()
}

/// Anteil der Essens-Zeilen mit mindestens einem Begriff.
fn abdeckung(tags: &[Vec<String>]) -> (usize, usize) {
    let essen: Vec<&Vec<String>> = tags.iter().filter(|t| t.as_slice() != [NONFOOD_KEY]).collect();
    (essen.iter().filter(|t| !t.is_empty()).count(), essen.len())
}

// ------------------------------------------------------------------ Auswahl

#[derive(Debug)]
pub struct Urteil {
    pub kandidat: Kandidat,
    /// Leer heißt übernommen.
    pub gruende: Vec<String>,
    /// Zeilen des Wochenbestands, deren Tags sich ändern (Titel).
    pub bewegt: Vec<String>,
}

pub struct Auswahl {
    pub dict: Value,
    pub urteile: Vec<Urteil>,
    pub gold_vorher: GoldWert,
    pub gold_nachher: GoldWert,
    pub abdeckung_vorher: (usize, usize),
    pub abdeckung_nachher: (usize, usize),
}

/// Kandidaten einzeln auf den jeweils letzten guten Stand legen und messen.
///
/// Gierig und in fester Reihenfolge (meiste Meldungen zuerst): Jede Änderung
/// wird gegen das Wörterbuch *mit* den schon übernommenen gemessen, damit zwei
/// Änderungen, die je allein harmlos sind, nicht zusammen den Goldsatz drücken.
pub fn auswaehlen(
    dict: &Value,
    kandidaten: Vec<Kandidat>,
    faelle: &[Fall],
    gold: &[Gold],
    korpus: &[Zeile],
) -> Result<Auswahl> {
    // Aus dem gemessenen Wörterbuch, nicht dem eingebetteten: Die Runde ändert
    // keine Oberbegriffe, aber ein Test misst ein ganz anderes.
    let ober: HashMap<String, String> = dict["begriffe"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(t, e)| Some((t.clone(), e["oberbegriff"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let mut stand = dict.clone();
    let mut wb_jetzt = Woerterbuch::parse(&stand.to_string())?;
    let gold_vorher = gold_messen(&wb_jetzt, gold, &ober);
    let tags_vorher = korpus_tags(&wb_jetzt, korpus);
    let mut gold_jetzt = gold_vorher;
    let mut tags_jetzt = tags_vorher.clone();
    let mut urteile = Vec::new();

    for k in kandidaten {
        let mut gruende = Vec::new();
        let mut bewegt = Vec::new();
        if matches!(k.op, Op::Regal | Op::Keine) {
            urteile.push(Urteil { kandidat: k, gruende: vec!["nicht automatisch".into()], bewegt });
            continue;
        }
        let mut kopie = stand.clone();
        if let Err(e) = anwenden(&mut kopie, k.op, &k.begriff, &k.wort) {
            urteile.push(Urteil { kandidat: k, gruende: vec![format!("{e}")], bewegt });
            continue;
        }
        // Ein einzelnes Wort, das den Begriff selbst trifft, ist ein Treffer und
        // kein Sperrwort: „bratwurst" endet auf das Suffix von `wurst`, und die
        // Sperre nähme jeder Bratwurst ihren Topf. Wendungen sind ausgenommen —
        // „milch schnitte" enthält „milch" und ist trotzdem die richtige Sperre.
        if k.op == Op::Sperrwort
            && !k.wort.contains(' ')
            && wb_jetzt.match_keys(&k.wort, None, None).contains(&k.begriff)
        {
            gruende.push(format!("„{}“ trifft selbst `{}`", k.wort, k.begriff));
        }
        let wb = match Woerterbuch::parse(&kopie.to_string()) {
            Ok(wb) => wb,
            Err(e) => {
                urteile.push(Urteil { kandidat: k, gruende: vec![format!("Wörterbuch ungültig: {e}")], bewegt });
                continue;
            }
        };
        if !k.faelle.iter().any(|&i| behebt(&wb_jetzt, &wb, &k, &faelle[i])) {
            gruende.push("behebt den gemeldeten Fall nicht".into());
        }
        let g = gold_messen(&wb, gold, &ober);
        gruende.extend(g.schlechter_als(&gold_jetzt));
        let tags = korpus_tags(&wb, korpus);
        let mut gesehen = HashSet::new();
        for (i, (a, b)) in tags_jetzt.iter().zip(&tags).enumerate() {
            if a != b && gesehen.insert(korpus[i].product.as_str()) {
                bewegt.push(korpus[i].product.clone());
            }
        }
        // Seit #92 trägt eine Sorte ihren Topf mit (Sekt → `sekt` und `wein`).
        // Eine Sperre, die den Topf nimmt und die Sorte stehen lässt, bricht
        // genau das — der Goldsatz sieht es nicht, weil `sekt` allein dort
        // auch richtig ist.
        let topf_weg = |vorher: &[String], nachher: &[String]| {
            vorher.contains(&k.begriff)
                && !nachher.contains(&k.begriff)
                && nachher.iter().any(|t| ober.get(t) == Some(&k.begriff))
        };
        let gemeldet_topf = k.faelle.iter().any(|&i| {
            let t = &faelle[i].titel;
            topf_weg(&wb_jetzt.match_keys(t, None, None), &wb.match_keys(t, None, None))
        });
        if gemeldet_topf || tags_jetzt.iter().zip(&tags).any(|(a, b)| topf_weg(a, b)) {
            gruende.push(format!("nimmt einer Sorte ihren Topf `{}`", k.begriff));
        }
        let grenze = if k.op == Op::Synonym { MAX_STREUUNG_SYNONYM } else { MAX_STREUUNG_SPERRE };
        if bewegt.len() > grenze {
            gruende.push(format!("bewegt {} Titel im Wochenbestand (Grenze {grenze})", bewegt.len()));
        }
        if gruende.is_empty() {
            stand = kopie;
            wb_jetzt = wb;
            gold_jetzt = g;
            tags_jetzt = tags;
        }
        urteile.push(Urteil { kandidat: k, gruende, bewegt });
    }
    Ok(Auswahl {
        dict: stand,
        urteile,
        gold_vorher,
        gold_nachher: gold_jetzt,
        abdeckung_vorher: abdeckung(&tags_vorher),
        abdeckung_nachher: abdeckung(&tags_jetzt),
    })
}

/// Vorschläge zu Kandidaten bündeln: Dieselbe Änderung aus drei Fällen ist
/// eine Änderung mit drei Fällen. `Keine` bleibt je Fall stehen (Bericht).
pub fn buendeln(vorschlaege: &[Vorschlag], faelle: &[Fall]) -> Vec<Kandidat> {
    let mut map: BTreeMap<(Op, String, String, String), Kandidat> = BTreeMap::new();
    for v in vorschlaege.iter().filter(|v| v.fall < faelle.len()) {
        let begriff = v.begriff.trim().to_lowercase();
        let wort = v.wort.trim().to_lowercase();
        let regal = v.regal.trim().to_string();
        let key = match v.op {
            Op::Keine => (v.op, String::new(), String::new(), v.fall.to_string()),
            _ => (v.op, begriff.clone(), wort.clone(), regal.clone()),
        };
        let k = map.entry(key).or_insert_with(|| Kandidat {
            op: v.op,
            begriff,
            wort,
            regal,
            grund: v.grund.trim().to_string(),
            faelle: Vec::new(),
        });
        if !k.faelle.contains(&v.fall) {
            k.faelle.push(v.fall);
        }
    }
    let mut out: Vec<Kandidat> = map.into_values().collect();
    let gewicht = |k: &Kandidat| k.faelle.iter().map(|&i| faelle[i].meldungen()).sum::<usize>();
    out.sort_by(|a, b| gewicht(b).cmp(&gewicht(a)).then_with(|| a.wort.cmp(&b.wort)));
    out
}

// ------------------------------------------------------------------ Modell

struct Modell {
    model: String,
    api_key: String,
    client: reqwest::blocking::Client,
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<Block>,
    stop_reason: Option<String>,
    #[serde(default)]
    usage: ApiUsage,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Deserialize, Default)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Deserialize)]
struct Antwort {
    vorschlaege: Vec<Vorschlag>,
}

fn antwort_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "vorschlaege": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "fall": {"type": "integer"},
                        "op": {"type": "string",
                               "enum": ["sperrwort", "synonym", "streichen", "nonfood", "regal", "keine"]},
                        "begriff": {"type": "string"},
                        "wort": {"type": "string"},
                        "regal": {"type": "string"},
                        "grund": {"type": "string"}
                    },
                    "required": ["fall", "op", "begriff", "wort", "regal", "grund"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["vorschlaege"],
        "additionalProperties": false
    })
}

/// Systemprompt: Regeln plus die 15 Regale. Gleich für jede Anfrage, damit
/// er im Prompt-Cache liegt.
fn system_prompt() -> String {
    format!("{PROMPT}\nDie Regale (Warengruppen): {}.\n", crate::enrich::CATEGORIES.join(", "))
}

/// Was das Modell zu einem Fall sieht: Meldung plus die beteiligten Einträge.
fn anfrage(dict: &Value, wb: &Woerterbuch, faelle: &[(usize, &Fall)]) -> Value {
    let mut eintraege = serde_json::Map::new();
    let mut liste = Vec::new();
    for (i, f) in faelle {
        let titel_begriffe = wb.match_keys(&f.titel, None, None);
        for b in f.offen.iter().chain(&titel_begriffe) {
            if let Some(e) = dict["begriffe"].get(b) {
                eintraege.entry(b.clone()).or_insert_with(|| e.clone());
            }
        }
        liste.push(serde_json::json!({
            "fall": i,
            "suchwort": f.query,
            "titel": f.titel,
            "weg": f.weg,
            "meldungen": f.meldungen(),
            "gruende": f.gruende,
            "kommentare": f.kommentare,
            "begriffe_des_suchworts": f.offen,
            "begriffe_des_titels": titel_begriffe,
        }));
    }
    serde_json::json!({"eintraege": eintraege, "faelle": liste})
}

#[derive(Deserialize)]
struct Pruefung {
    urteile: Vec<PruefUrteil>,
}

#[derive(Deserialize)]
struct PruefUrteil {
    i: usize,
    ok: bool,
    #[serde(default)]
    grund: String,
}

fn pruef_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "urteile": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "i": {"type": "integer"},
                        "ok": {"type": "boolean"},
                        "grund": {"type": "string"}
                    },
                    "required": ["i", "ok", "grund"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["urteile"],
        "additionalProperties": false
    })
}

/// Zweiter Blick auf die Änderungen, die beide Messungen bestanden haben.
///
/// Der Goldsatz hat 220 Produkte; eine Sperre wie `wurst` → „bratwurst"
/// bewegt dort nichts und im Wochenbestand zwei Zeilen, ist aber offensichtlich
/// falsch. Diese Prüfung sieht die Änderung zusammen mit den Titeln, die sie
/// bewegt, und darf nur ablehnen, nie etwas hinzufügen. Zurück kommt je
/// abgelehnter Änderung (Index in `urteile`) der Grund.
fn gegenpruefen(
    modell: &Modell,
    a: &Auswahl,
    faelle: &[Fall],
    dict: &Value,
    usage: &mut Usage,
) -> Result<HashMap<usize, String>> {
    let pruefen: Vec<(usize, &Urteil)> =
        a.urteile.iter().enumerate().filter(|(_, u)| u.gruende.is_empty()).collect();
    if pruefen.is_empty() {
        return Ok(HashMap::new());
    }
    let liste: Vec<Value> = pruefen
        .iter()
        .map(|(i, u)| {
            let k = &u.kandidat;
            serde_json::json!({
                "i": i,
                "aenderung": beschreibung(k),
                "op": k.op,
                "begriff": k.begriff,
                "wort": k.wort,
                "gemeldet": k.faelle.iter().map(|&f| format!("{} → {}", faelle[f].query, faelle[f].titel)).collect::<Vec<_>>(),
                "eintrag": dict["begriffe"].get(&k.begriff).cloned().unwrap_or(Value::Null),
                "bewegt": u.bewegt.iter().take(BEWEGT_IN_PRUEFUNG).collect::<Vec<_>>(),
            })
        })
        .collect();
    let p: Pruefung = modell.fragen(PROMPT_PRUEFUNG, &serde_json::json!({"aenderungen": liste}), pruef_schema(), usage)?;
    let gesehen: HashSet<usize> = p.urteile.iter().map(|u| u.i).collect();
    let mut veto: HashMap<usize, String> =
        p.urteile.into_iter().filter(|u| !u.ok).map(|u| (u.i, u.grund)).collect();
    // Ohne Urteil keine Freigabe.
    for (i, _) in &pruefen {
        if !gesehen.contains(i) {
            veto.insert(*i, "kein Urteil erhalten".into());
        }
    }
    Ok(veto)
}

impl Modell {
    fn from_env() -> Result<Modell> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        anyhow::ensure!(!api_key.trim().is_empty(), "ANTHROPIC_API_KEY fehlt");
        Ok(Modell {
            model: std::env::var("FEEDBACK_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
            api_key: api_key.trim().to_string(),
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(180))
                .build()?,
        })
    }

    fn fragen<T: for<'de> Deserialize<'de>>(
        &self,
        system: &str,
        user: &Value,
        schema: Value,
        usage: &mut Usage,
    ) -> Result<T> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}],
            "thinking": {"type": "disabled"},
            "output_config": {
                "effort": "medium",
                "format": {"type": "json_schema", "schema": schema}
            },
            "messages": [{"role": "user", "content": user.to_string()}]
        });
        let mut warte = 2u64;
        for versuch in 1..=4 {
            usage.requests += 1;
            let resp = self
                .client
                .post(API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send();
            let resp = match resp {
                Ok(r) => r,
                Err(e) if versuch < 4 => {
                    eprintln!("Feedback-Runde: Netz: {e} — neuer Versuch in {warte} s");
                    std::thread::sleep(Duration::from_secs(warte));
                    warte *= 2;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = resp.status();
            if (status.as_u16() == 429 || status.is_server_error()) && versuch < 4 {
                eprintln!("Feedback-Runde: HTTP {status} — neuer Versuch in {warte} s");
                std::thread::sleep(Duration::from_secs(warte));
                warte *= 2;
                continue;
            }
            if !status.is_success() {
                let text: String = resp.text().unwrap_or_default().chars().take(300).collect();
                bail!("HTTP {status}: {text}");
            }
            let r: ApiResponse = resp.json().context("Antwort unlesbar")?;
            usage.input_tokens += r.usage.input_tokens;
            usage.output_tokens += r.usage.output_tokens;
            usage.cache_write_tokens += r.usage.cache_creation_input_tokens;
            usage.cache_read_tokens += r.usage.cache_read_input_tokens;
            if r.stop_reason.as_deref() == Some("refusal") {
                bail!("Modell hat abgelehnt (refusal)");
            }
            let text: String =
                r.content.iter().filter(|b| b.kind == "text").filter_map(|b| b.text.as_deref()).collect();
            return serde_json::from_str(&text).context("JSON der Antwort unlesbar");
        }
        unreachable!()
    }
}

// ------------------------------------------------------------------ Supabase

fn auth(cfg: &PushConfig, req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
    req.header("apikey", &cfg.api_key).header("Authorization", format!("Bearer {}", cfg.api_key))
}

/// Seitenweise lesen; PostgREST gibt ohne Range höchstens 1000 Zeilen.
fn alle_seiten<T: for<'de> Deserialize<'de>>(
    cfg: &PushConfig,
    tabelle: &str,
    query: &[(&str, String)],
) -> Result<Vec<T>> {
    const SEITE: usize = 1000;
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build()?;
    let mut out: Vec<T> = Vec::new();
    loop {
        let mut q: Vec<(&str, String)> = query.to_vec();
        q.push(("offset", out.len().to_string()));
        q.push(("limit", SEITE.to_string()));
        let resp = auth(cfg, client.get(format!("{}/rest/v1/{tabelle}", cfg.base_url)))
            .query(&q)
            .send()
            .with_context(|| format!("Supabase nicht erreichbar ({tabelle})"))?;
        let status = resp.status();
        if !status.is_success() {
            let text: String = resp.text().unwrap_or_default().chars().take(300).collect();
            bail!("{tabelle} lesen fehlgeschlagen (HTTP {status}): {text}");
        }
        let seite: Vec<T> = resp.json().with_context(|| format!("{tabelle}: Antwort unlesbar"))?;
        let fertig = seite.len() < SEITE;
        out.extend(seite);
        if fertig {
            return Ok(out);
        }
    }
}

fn meldungen_holen(cfg: &PushConfig, nach_id: i64) -> Result<Vec<Meldung>> {
    alle_seiten(
        cfg,
        "match_feedback",
        &[
            ("select", "id,query,product_title,match_kind,reason,comment".into()),
            ("id", format!("gt.{nach_id}")),
            ("order", "id.asc".into()),
        ],
    )
}

/// Der Wochenbestand: jedes heute gültige (Titel, Kategorie) einmal.
fn korpus_holen(cfg: &PushConfig) -> Result<Vec<Zeile>> {
    let heute = chrono::Local::now().date_naive().to_string();
    let mut zeilen: Vec<Zeile> = alle_seiten(
        cfg,
        "offers",
        &[
            ("select", "product,category".into()),
            ("valid_until", format!("gte.{heute}")),
            ("order", "id.asc".into()),
        ],
    )?;
    zeilen.sort();
    zeilen.dedup();
    Ok(zeilen)
}

// ------------------------------------------------------------------ Lauf

pub struct Optionen {
    pub gold: String,
    pub bericht: PathBuf,
    /// Fälle zeigen, nichts fragen, nichts schreiben.
    pub trocken: bool,
}

pub fn run(opts: &Optionen) -> Result<()> {
    let cfg = crate::push::config_from_env()?;
    let mut stand: Stand = match std::fs::read_to_string(STAND) {
        Ok(s) => serde_json::from_str(&s).with_context(|| format!("{STAND} unlesbar"))?,
        Err(_) => Stand::default(),
    };
    let meldungen = meldungen_holen(&cfg, stand.letzte_id)?;
    let Some(hoechste) = meldungen.iter().map(|m| m.id).max() else {
        println!("Keine neuen Meldungen nach ID {}.", stand.letzte_id);
        return Ok(());
    };
    let roh = std::fs::read_to_string(WOERTERBUCH).with_context(|| format!("{WOERTERBUCH} lesen"))?;
    let dict: Value = serde_json::from_str(&roh)?;
    let wb = Woerterbuch::parse(&roh)?;
    let (faelle, z) = faelle(&meldungen, &wb);
    println!(
        "{} Meldungen (ID {}–{hoechste}): {} Vorliebe, {} Menge, {} Fälle schon behoben, {} ohne Begriff, {} offen.",
        z.meldungen,
        meldungen.iter().map(|m| m.id).min().unwrap_or(0),
        z.vorliebe,
        z.menge,
        z.erledigt,
        z.ohne_begriff,
        faelle.len()
    );
    if opts.trocken {
        for f in &faelle {
            println!("  {}× „{}“ → „{}“ ({}; {})", f.meldungen(), f.query, f.titel, f.weg, f.offen.join(", "));
        }
        return Ok(());
    }

    let mut usage = Usage::default();
    let mut vorschlaege = Vec::new();
    let modell = if faelle.is_empty() { None } else { Some(Modell::from_env()?) };
    if let Some(modell) = &modell {
        let system = system_prompt();
        let nummeriert: Vec<(usize, &Fall)> = faelle.iter().enumerate().collect();
        for teil in nummeriert.chunks(FAELLE_JE_ANFRAGE) {
            match modell.fragen::<Antwort>(&system, &anfrage(&dict, &wb, teil), antwort_schema(), &mut usage) {
                Ok(a) => vorschlaege.extend(a.vorschlaege),
                Err(e) => {
                    usage.failed_requests += 1;
                    eprintln!("WARNUNG Feedback-Runde: {} Fälle ohne Antwort: {e:#}", teil.len());
                }
            }
        }
        // Fehlt die Antwort für eine Anfrage, bleibt der Stand stehen: Diese
        // Meldungen sollen nächste Woche wiederkommen, nicht verloren gehen.
        anyhow::ensure!(
            usage.failed_requests == 0,
            "{} Anfragen ohne Antwort — Stand bleibt bei ID {}",
            usage.failed_requests,
            stand.letzte_id
        );
    }

    let gold: Vec<Gold> = serde_json::from_str(
        &std::fs::read_to_string(&opts.gold).with_context(|| format!("{} lesen", opts.gold))?,
    )?;
    let korpus = korpus_holen(&cfg)?;
    println!("Wochenbestand: {} verschiedene Titel.", korpus.len());
    let kandidaten = buendeln(&vorschlaege, &faelle);
    let erste = auswaehlen(&dict, kandidaten.clone(), &faelle, &gold, &korpus)?;
    let veto = match &modell {
        Some(m) => gegenpruefen(m, &erste, &faelle, &dict, &mut usage)?,
        None => HashMap::new(),
    };
    // Was die Gegenprüfung ablehnt, fällt heraus, und der Rest wird neu
    // gemessen: Jede Änderung stand auf den vor ihr übernommenen, und die
    // Zahlen im Bericht sollen zum Wörterbuch gehören, das wirklich entsteht.
    let abgelehnt: Vec<Urteil> = erste
        .urteile
        .into_iter()
        .enumerate()
        .filter_map(|(i, u)| {
            veto.get(&i).map(|g| Urteil { gruende: vec![format!("Gegenprüfung: {g}")], ..u })
        })
        .collect();
    let bleiben: Vec<Kandidat> = kandidaten
        .into_iter()
        .filter(|k| !abgelehnt.iter().any(|u| u.kandidat.op == k.op && u.kandidat.begriff == k.begriff && u.kandidat.wort == k.wort))
        .collect();
    let mut auswahl = auswaehlen(&dict, bleiben, &faelle, &gold, &korpus)?;
    auswahl.urteile.extend(abgelehnt);
    if usage.requests > 0 {
        println!(
            "Modell: {} Anfragen, Tokens ein {} / Cache geschrieben {} / gelesen {} / aus {} → ${:.4}; Gegenprüfung lehnt {} ab",
            usage.requests,
            usage.input_tokens,
            usage.cache_write_tokens,
            usage.cache_read_tokens,
            usage.output_tokens,
            usage.dollars(),
            veto.len()
        );
    }

    let runde = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
    for u in auswahl.urteile.iter().filter(|u| u.gruende.is_empty()) {
        stand.aenderungen.push(Eintrag {
            op: u.kandidat.op,
            begriff: u.kandidat.begriff.clone(),
            wort: u.kandidat.wort.clone(),
            runde: runde.clone(),
            meldungen: u.kandidat.faelle.iter().map(|&i| faelle[i].meldungen()).sum(),
        });
    }
    stand.letzte_id = hoechste;
    let von = meldungen.iter().map(|m| m.id).min().unwrap_or(0);
    let text = bericht(&auswahl, &faelle, &z, (von, hoechste), &usage, korpus.len());

    std::fs::write(WOERTERBUCH, als_text(&auswahl.dict)?)?;
    std::fs::write(STAND, serde_json::to_string_pretty(&stand)? + "\n")?;
    if let Some(dir) = opts.bericht.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&opts.bericht, &text)?;
    let n = auswahl.urteile.iter().filter(|u| u.gruende.is_empty()).count();
    println!(
        "{n} Änderungen übernommen. Goldsatz ok {:.1} → {:.1} %, falsch {:.1} → {:.1} %. Bericht: {}",
        auswahl.gold_vorher.ok,
        auswahl.gold_nachher.ok,
        auswahl.gold_vorher.falsch,
        auswahl.gold_nachher.falsch,
        opts.bericht.display()
    );
    Ok(())
}

// ------------------------------------------------------------------ Bericht

fn beschreibung(k: &Kandidat) -> String {
    match k.op {
        Op::Sperrwort => format!("`{}` sperrt „{}“", k.begriff, k.wort),
        Op::Synonym => format!("`{}` erkennt „{}“", k.begriff, k.wort),
        Op::Streichen => format!("`{}` führt „{}“ nicht mehr", k.begriff, k.wort),
        Op::Nonfood => format!("„{}“ ist Non-Food", k.wort),
        Op::Regal => format!("`{}` ins Regal „{}“", k.begriff, k.regal),
        Op::Keine => "keine Änderung".into(),
    }
}

fn fall_text(f: &Fall) -> String {
    format!("{}× „{}“ → „{}“", f.meldungen(), f.query, f.titel)
}

fn prozent(n: (usize, usize)) -> String {
    format!("{:.1} % ({} von {})", n.0 as f64 / n.1.max(1) as f64 * 100.0, n.0, n.1)
}

/// Der PR-Text; die erste Zeile ist der Titel. Er nennt Suchwort und Produkttitel (beides steht im Prospekt
/// bzw. ist ein Alltagswort), aber nie Install-IDs oder Kommentare.
pub fn bericht(
    a: &Auswahl,
    faelle: &[Fall],
    z: &Zaehlung,
    ids: (i64, i64),
    usage: &Usage,
    korpus: usize,
) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let uebernommen: Vec<&Urteil> = a.urteile.iter().filter(|u| u.gruende.is_empty()).collect();
    let regal: Vec<&Urteil> = a.urteile.iter().filter(|u| u.kandidat.op == Op::Regal).collect();
    let keine: usize = a.urteile.iter().filter(|u| u.kandidat.op == Op::Keine).count();
    let verworfen: Vec<&Urteil> = a
        .urteile
        .iter()
        .filter(|u| !u.gruende.is_empty() && !matches!(u.kandidat.op, Op::Regal | Op::Keine))
        .collect();

    let _ = writeln!(
        s,
        "# Feedback-Runde {}: {} Wörterbuch-Änderungen aus {} Meldungen\n",
        chrono::Local::now().format("%d.%m."),
        uebernommen.len(),
        z.meldungen
    );
    let _ = writeln!(
        s,
        "Feedback-Runde über die Meldungen {}–{} aus `match_feedback` ({} Stück). \
         Geöffnet von `lechariot feedback-runde`; jede Änderung unten ist einzeln \
         gegen Goldsatz und Wochenbestand gemessen und danach von einem zweiten \
         Modellaufruf gegengeprüft, der nur ablehnen kann.\n",
        ids.0, ids.1, z.meldungen
    );
    let _ = writeln!(
        s,
        "- nicht ausgewertet: {} Vorliebe, {} Menge\n- schon behoben, das heutige Wörterbuch erzeugt den Treffer nicht mehr: {} Fälle\n- Suchwort ohne Begriff: {} Fälle\n- an das Modell: {} Fälle, davon {} „keine Änderung“\n",
        z.vorliebe, z.menge, z.erledigt, z.ohne_begriff, faelle.len(), keine
    );

    let _ = writeln!(s, "## Messung\n");
    let _ = writeln!(s, "| | vorher | nachher |\n|---|---:|---:|");
    let g = |w: &GoldWert| format!("{:.1} / {:.1} / {:.1} %", w.ok, w.fehlt, w.falsch);
    let _ = writeln!(
        s,
        "| Goldsatz, Wörterbuch allein (ok / fehlt / falsch, gewichtet) | {} | {} |",
        g(&a.gold_vorher),
        g(&a.gold_nachher)
    );
    let _ = writeln!(
        s,
        "| Wochenbestand, Essen mit Begriff ({korpus} Titel) | {} | {} |\n",
        prozent(a.abdeckung_vorher),
        prozent(a.abdeckung_nachher)
    );
    let _ = writeln!(
        s,
        "Eine Sperre nimmt Zeilen ihren Begriff; dass die Abdeckung dabei leicht fällt, ist ihr Zweck. \
         Die Grenze ist pro Änderung: höchstens {MAX_STREUUNG_SPERRE} bewegte Titel je Sperr- oder \
         Non-Food-Wort, {MAX_STREUUNG_SYNONYM} je Synonym.\n\n\
         Haiku `replace` (Nightly) ändert sich dadurch nicht: Sein Cache-Schlüssel enthält die \
         Wörterbuch-Tags nicht. Wirksam wird eine Änderung in der App-Suche (Sperrlisten bei \
         Direkttreffern, Synonyme), als Rückfall ohne Modellantwort und als Hinweis für neue Produkte.\n"
    );

    let _ = writeln!(s, "## Übernommen ({})\n", uebernommen.len());
    for u in &uebernommen {
        let _ = writeln!(s, "- {} — {}", beschreibung(&u.kandidat), u.kandidat.grund);
        for &i in &u.kandidat.faelle {
            let _ = writeln!(s, "  - {}", fall_text(&faelle[i]));
        }
        if !u.bewegt.is_empty() {
            let beispiele: Vec<String> = u.bewegt.iter().take(5).map(|t| format!("„{t}“")).collect();
            let _ = writeln!(s, "  - bewegt {} Titel im Wochenbestand: {}", u.bewegt.len(), beispiele.join(", "));
        }
    }
    if !verworfen.is_empty() {
        let _ = writeln!(s, "\n## Verworfen ({})\n", verworfen.len());
        for u in &verworfen {
            let faelle_text: Vec<String> = u.kandidat.faelle.iter().map(|&i| fall_text(&faelle[i])).collect();
            let _ = writeln!(
                s,
                "- {} ({}) — {}",
                beschreibung(&u.kandidat),
                faelle_text.join("; "),
                u.gruende.join("; ")
            );
        }
    }
    if !regal.is_empty() {
        let _ = writeln!(s, "\n## Zur Handprüfung: Regal ({})\n", regal.len());
        let _ = writeln!(
            s,
            "Nicht angewendet: Das Regal gehört `enrich::kategorie_fuer_begriff` und damit auch dem Import.\n"
        );
        for u in &regal {
            let faelle_text: Vec<String> = u.kandidat.faelle.iter().map(|&i| fall_text(&faelle[i])).collect();
            let _ = writeln!(s, "- {} — {} ({})", beschreibung(&u.kandidat), u.kandidat.grund, faelle_text.join("; "));
        }
    }
    if usage.requests > 0 {
        let _ = writeln!(
            s,
            "\nModell: {} Anfragen, {} Tokens ein / {} aus, ${:.4}.",
            usage.requests,
            usage.input_tokens + usage.cache_write_tokens + usage.cache_read_tokens,
            usage.output_tokens,
            usage.dollars()
        );
    }
    s
}

pub fn bericht_pfad_standard() -> PathBuf {
    Path::new("target").join("feedback-runde.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein kleines Wörterbuch mit allen Pflichtfeldern.
    fn mini() -> Value {
        serde_json::json!({
            "begriffe": {
                "milch": {"exact": ["milch", "vollmilch"], "suffix": ["milch"], "prefix": [], "block": ["milchreis"]},
                "käse": {"exact": ["käse", "gouda"], "suffix": ["käse"], "prefix": [], "block": []},
                "reis": {"exact": ["reis", "basmati"], "suffix": [], "prefix": [], "block": []}
            },
            "marken": {},
            "kategorien": {},
            "nonfood_cat": "drogerie",
            "nonfood_terms": "lampe|socken",
            "food_cat": "lebensmittel"
        })
    }

    fn wb(v: &Value) -> Woerterbuch {
        Woerterbuch::parse(&v.to_string()).unwrap()
    }

    fn meldung(id: i64, q: &str, t: &str, reason: &str) -> Meldung {
        Meldung {
            id,
            query: q.into(),
            product_title: t.into(),
            match_kind: Some("category".into()),
            reason: reason.into(),
            comment: None,
        }
    }

    #[test]
    fn faelle_buendeln_und_vorlieben_fallen_weg() {
        let v = mini();
        let m = vec![
            meldung(1, "Milch", "MILCH-SCHNITTE Snack", "wrong_product"),
            meldung(2, "milch", "MILCH-SCHNITTE Snack", "wrong_product"),
            meldung(3, "Milch", "Vollmilch 3,5 %", "personal_taste"),
            meldung(4, "Milch", "Milchreis Klassik", "wrong_product"),
            meldung(5, "Zahnpasta", "Elmex", "wrong_product"),
        ];
        let (f, z) = faelle(&m, &wb(&v));
        assert_eq!(z.vorliebe, 1);
        // Milchreis sperrt `milch` schon: erledigt, ohne dass jemand fragt.
        assert_eq!(z.erledigt, 1);
        assert_eq!(z.ohne_begriff, 1);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].meldungen(), 2);
        assert_eq!(f[0].offen, vec!["milch"]);
    }

    #[test]
    fn direkttreffer_haelt_nur_die_sperrliste_auf() {
        let mut v = mini();
        let mut f = Fall {
            query: "Milch".into(),
            titel: "Milch Schnitte".into(),
            weg: "direct".into(),
            gruende: BTreeMap::new(),
            ids: vec![1],
            kommentare: vec![],
            offen: vec![],
        };
        assert_eq!(offen(&wb(&v), &f), vec!["milch"]);
        anwenden(&mut v, Op::Sperrwort, "milch", "schnitte").unwrap();
        f.offen = offen(&wb(&v), &f);
        assert!(f.offen.is_empty());
    }

    #[test]
    fn anwenden_prueft_begriff_wort_und_doppelte() {
        let mut v = mini();
        assert!(anwenden(&mut v, Op::Sperrwort, "brot", "schnitte").is_err());
        assert!(anwenden(&mut v, Op::Sperrwort, "milch", "Schnitte!").is_err());
        assert!(anwenden(&mut v, Op::Sperrwort, "milch", "milchreis").is_err());
        assert!(anwenden(&mut v, Op::Sperrwort, "milch", "vollmilch").is_err(), "Treffer sperrt sich nicht selbst");
        assert!(anwenden(&mut v, Op::Regal, "milch", "egal").is_err());
        v["begriffe"]["gouda"] = serde_json::json!({"exact": ["gouda", "goudakäse"], "suffix": [], "prefix": [], "block": [], "oberbegriff": "käse"});
        assert!(anwenden(&mut v, Op::Sperrwort, "käse", "goudakäse").is_err(), "Sorte bleibt im Topf");
        anwenden(&mut v, Op::Synonym, "käse", "bergkäse").unwrap();
        anwenden(&mut v, Op::Streichen, "käse", "bergkäse").unwrap();
        assert!(anwenden(&mut v, Op::Streichen, "käse", "bergkäse").is_err());
        anwenden(&mut v, Op::Synonym, "käse", "bergkäse").unwrap();
        assert_eq!(v["begriffe"]["käse"]["exact"].as_array().unwrap().last().unwrap(), "bergkäse");
        anwenden(&mut v, Op::Nonfood, "", "bodenwischer").unwrap();
        assert_eq!(v["nonfood_terms"], "lampe|socken|bodenwischer");
        assert!(anwenden(&mut v, Op::Nonfood, "", "socken").is_err());
        assert_eq!(wb(&v).match_keys("VILEDA Bodenwischer-Set", None, None), vec![NONFOOD_KEY]);
    }

    fn gold(title: &str, tags: &[&str]) -> Gold {
        Gold {
            set: "x".into(),
            title: title.into(),
            sub: String::new(),
            cat: String::new(),
            gold: Some(tags.iter().map(|t| t.to_string()).collect()),
        }
    }

    fn fall(q: &str, t: &str, n: usize) -> Fall {
        Fall {
            query: q.into(),
            titel: t.into(),
            weg: "category".into(),
            gruende: BTreeMap::new(),
            ids: (0..n as i64).collect(),
            kommentare: vec![],
            offen: vec![],
        }
    }

    fn kandidat(op: Op, begriff: &str, wort: &str, faelle: Vec<usize>) -> Kandidat {
        Kandidat { op, begriff: begriff.into(), wort: wort.into(), regal: String::new(), grund: String::new(), faelle }
    }

    /// Die Sicherung, um die es geht: Eine Sperre, die den gemeldeten Fall
    /// behebt, aber auf dem Goldsatz einen echten Treffer verliert, fällt weg —
    /// und die harmlose daneben kommt trotzdem durch.
    #[test]
    fn verschlechterung_auf_dem_goldsatz_wird_verworfen() {
        let v = mini();
        let faelle = vec![fall("Milch", "Milch Schnitte", 3), fall("Milch", "Bio Frischmilch Schnitte", 1)];
        let gold = vec![gold("Bio Vollmilch frisch", &["milch"]), gold("Gouda jung", &["käse"])];
        let korpus = vec![Zeile { product: "Milch Schnitte".into(), category: None }];
        let k = vec![
            kandidat(Op::Sperrwort, "milch", "bio", vec![1]),
            kandidat(Op::Sperrwort, "milch", "schnitte", vec![0]),
            kandidat(Op::Sperrwort, "milch", "gouda", vec![0]),
        ];
        let a = auswaehlen(&v, k, &faelle, &gold, &korpus).unwrap();
        let urteil = |w: &str| a.urteile.iter().find(|u| u.kandidat.wort == w).unwrap();
        assert!(urteil("bio").gruende.iter().any(|g| g.contains("Goldsatz")), "{:?}", urteil("bio").gruende);
        assert!(urteil("schnitte").gruende.is_empty(), "{:?}", urteil("schnitte").gruende);
        assert_eq!(urteil("schnitte").bewegt, vec!["Milch Schnitte"]);
        assert!(urteil("gouda").gruende.iter().any(|g| g.contains("nicht")));
        assert_eq!(a.dict["begriffe"]["milch"]["block"], serde_json::json!(["milchreis", "schnitte"]));
        assert_eq!(a.gold_vorher, a.gold_nachher);
    }

    #[test]
    fn eine_sorte_behaelt_ihren_topf() {
        let mut v = mini();
        v["begriffe"]["wein"] = serde_json::json!({"exact": ["wein", "sekt"], "suffix": [], "prefix": [], "block": []});
        v["begriffe"]["sekt"] = serde_json::json!({"exact": ["sekt"], "suffix": [], "prefix": [], "block": [], "oberbegriff": "wein"});
        let faelle = vec![fall("Wein", "JULES MUMM Sekt", 1)];
        let k = vec![kandidat(Op::Sperrwort, "wein", "mumm", vec![0])];
        let a = auswaehlen(&v, k, &faelle, &[], &[]).unwrap();
        assert!(a.urteile[0].gruende.iter().any(|g| g.contains("Topf")), "{:?}", a.urteile[0].gruende);
    }

    #[test]
    fn zu_grosse_streuung_wird_verworfen() {
        let v = mini();
        let faelle = vec![fall("Reis", "Puffreis Schoko", 1)];
        let korpus: Vec<Zeile> = (0..=MAX_STREUUNG_SPERRE)
            .map(|i| Zeile { product: format!("Basmati Reis Schoko {i}"), category: None })
            .collect();
        let k = vec![kandidat(Op::Sperrwort, "reis", "schoko", vec![0])];
        let a = auswaehlen(&v, k, &faelle, &[], &korpus).unwrap();
        assert!(a.urteile[0].gruende.iter().any(|g| g.contains("Grenze")), "{:?}", a.urteile[0].gruende);
        assert_eq!(a.dict, v);
    }

    #[test]
    fn buendeln_fasst_gleiche_vorschlaege_zusammen() {
        let faelle = vec![fall("Milch", "A", 1), fall("Milch", "B", 5), fall("Milch", "C", 1)];
        let v = |fall: usize, op: Op, wort: &str| Vorschlag {
            fall,
            op,
            begriff: "Milch".into(),
            wort: wort.into(),
            regal: String::new(),
            grund: String::new(),
        };
        let k = buendeln(&[v(0, Op::Sperrwort, "x"), v(1, Op::Sperrwort, "x "), v(2, Op::Keine, ""), v(9, Op::Keine, "")], &faelle);
        assert_eq!(k.len(), 2);
        assert_eq!(k[0].faelle, vec![0, 1]);
        assert_eq!(k[0].begriff, "milch");
        assert_eq!(k[0].wort, "x");
    }

    #[test]
    fn ausgabe_ist_byte_gleich_mit_dem_repo() {
        let roh = include_str!("../docs/matching-woerterbuch.json");
        let v: Value = serde_json::from_str(roh).unwrap();
        assert_eq!(als_text(&v).unwrap(), roh);
    }

    #[test]
    fn prompt_nennt_alle_regale() {
        let p = system_prompt();
        for c in crate::enrich::CATEGORIES {
            assert!(p.contains(c));
        }
    }
}
