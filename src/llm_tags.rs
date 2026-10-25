//! Angebote mit einem kleinen Sprachmodell taggen — einmal je Produkt, gecacht.
//!
//! Das Wörterbuch (`matching.rs`) bleibt der schnelle Weg und der Rückfall.
//! Das Modell sieht jedes verschiedene (Titel, Untertitel, Kategorie) genau
//! einmal; die Antwort liegt danach im Cache unter einem Schlüssel aus Modell,
//! Prompt (samt Vokabular) und Text. Daraus folgt das Determinismus-Tor aus
//! der LLM-Entscheidung (T3): Dieselbe Eingabe ergibt dieselben Tags, weil
//! sie nachgeschlagen und nicht neu erzeugt werden. Ändert sich Prompt,
//! Vokabular oder Modell, ist das ein neuer Schlüssel — also ein bewusstes
//! Neu-Taggen, kein stilles Driften.
//!
//! Ohne `ANTHROPIC_API_KEY` ist der Weg aus und alles bleibt beim Wörterbuch.
//! Jeder Fehler (Netz, Kontingent, unlesbare Antwort) lässt die betroffenen
//! Produkte beim Wörterbuch; ein Lauf scheitert nie am Modell.
//!
//! Rust hat kein offizielles Anthropic-SDK, deshalb roher HTTP-Aufruf der
//! Messages-API.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::matching::{self, NONFOOD_KEY};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-haiku-5-5";
const PROMPT: &str = include_str!("../docs/llm-tagging/prompt.txt");
/// Produkte je Anfrage. 50 halten die Antwort weit unter `MAX_TOKENS` und die
/// Anfrage unter 100 000 Prompt-Tokens, ab denen Haiku 5.5 teurer abrechnet.
const CHUNK: usize = 50;
const MAX_TOKENS: u32 = 4096;
const WORKERS: usize = 4;
/// Obergrenze neuer Produkte je Lauf, damit ein Fehler im Cache (leer, verloren)
/// nicht unbemerkt das Zehnfache kostet. Eine Woche Produktion: ~3 600.
const DEFAULT_MAX_PER_RUN: usize = 8000;
/// $/MTok für Haiku 5.5 bis 100 000 Prompt-Tokens (platform.claude.com/docs,
/// Stand 2026-10-20): Eingabe, Cache-Schreiben (5 min), Cache-Lesen, Ausgabe.
/// Nur für die Kostenzeile im Log — abgerechnet wird bei Anthropic.
const PREIS_HAIKU_5_5: [f64; 4] = [0.10, 0.125, 0.01, 0.50];

/// Wie die Antwort des Modells mit dem Wörterbuch zusammengeht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Nur wo das Wörterbuch nichts gefunden hat. Kann nichts verschlechtern,
    /// was heute stimmt — der Start, bis der Goldsatz gemessen ist.
    Fill,
    /// Das Modell hat das letzte Wort; das Wörterbuch zählt nur, wo das
    /// Modell keine Antwort geliefert hat.
    Replace,
}

/// Was das Modell über ein Angebot zu sehen bekommt.
#[derive(Debug, Clone)]
pub struct Input {
    pub title: String,
    pub subtitle: String,
    pub category: String,
    /// Tags des Wörterbuchs — als Hinweis im Prompt und als Rückfall.
    pub dict: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Usage {
    pub requests: usize,
    pub failed_requests: usize,
    pub input_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    pub fn dollars(&self) -> f64 {
        let [ein, schreiben, lesen, aus] = PREIS_HAIKU_5_5;
        (self.input_tokens as f64 * ein
            + self.cache_write_tokens as f64 * schreiben
            + self.cache_read_tokens as f64 * lesen
            + self.output_tokens as f64 * aus)
            / 1e6
    }
}

pub struct Tagger {
    model: String,
    api_key: String,
    pub mode: Mode,
    max_per_run: usize,
    cache_path: PathBuf,
    cache: BTreeMap<String, Vec<String>>,
    system: String,
    prompt_fp: String,
    vocab: HashSet<String>,
    client: reqwest::blocking::Client,
}

impl Tagger {
    /// None, wenn kein Schlüssel gesetzt ist oder `LLM_TAGS=off`.
    pub fn from_env() -> Option<Tagger> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        if api_key.trim().is_empty() || std::env::var("LLM_TAGS").as_deref() == Ok("off") {
            return None;
        }
        let mode = match std::env::var("LLM_TAGS_MODE").as_deref() {
            Ok("replace") => Mode::Replace,
            _ => Mode::Fill,
        };
        let cache_path = std::env::var("LLM_TAGS_CACHE").map(PathBuf::from).unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".local/share/lechariot/llm-tags.json")
        });
        let max_per_run = std::env::var("LLM_TAGS_MAX_PER_RUN")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_PER_RUN);
        let model = std::env::var("LLM_TAGS_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        Some(Tagger::new(model, api_key.trim().to_string(), mode, cache_path, max_per_run))
    }

    pub fn new(
        model: String,
        api_key: String,
        mode: Mode,
        cache_path: PathBuf,
        max_per_run: usize,
    ) -> Tagger {
        let system = system_prompt();
        let prompt_fp = hex(&Sha256::digest(system.as_bytes()))[..16].to_string();
        let cache = std::fs::read_to_string(&cache_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut vocab: HashSet<String> =
            matching::vocabulary().into_iter().map(|(t, _)| t).collect();
        vocab.insert(NONFOOD_KEY.to_string());
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("HTTP-Client");
        Tagger {
            model,
            api_key,
            mode,
            max_per_run,
            cache_path,
            cache,
            system,
            prompt_fp,
            vocab,
            client,
        }
    }

    fn key(&self, i: &Input) -> String {
        let roh = [self.model.as_str(), &self.prompt_fp, &i.title, &i.subtitle, &i.category]
            .join("\u{1f}");
        hex(&Sha256::digest(roh.as_bytes()))
    }

    /// Antwort des Modells für dieses Angebot, falls schon getaggt.
    pub fn cached(&self, i: &Input) -> Option<&Vec<String>> {
        self.cache.get(&self.key(i))
    }

    /// Endgültige Tags nach `mode`.
    pub fn apply(&self, i: &Input) -> Vec<String> {
        match (self.mode, self.cached(i)) {
            (Mode::Fill, Some(llm)) if i.dict.is_empty() => llm.clone(),
            (Mode::Replace, Some(llm)) => llm.clone(),
            _ => i.dict.clone(),
        }
    }

    /// Alle noch nicht gecachten Angebote taggen lassen und den Cache
    /// schreiben. Fehler landen im Log und in `Usage`, nie im Rückgabewert:
    /// Was nicht getaggt wurde, behält eben sein Wörterbuch-Tag.
    pub fn ensure(&mut self, inputs: &[Input]) -> Usage {
        let mut seen = HashSet::new();
        let mut offen: Vec<&Input> = inputs
            .iter()
            .filter(|i| !self.cache.contains_key(&self.key(i)) && seen.insert(self.key(i)))
            .collect();
        let gesamt = offen.len();
        if offen.len() > self.max_per_run {
            eprintln!(
                "WARNUNG LLM-Tags: {gesamt} neue Produkte, mehr als LLM_TAGS_MAX_PER_RUN={} — der Rest bleibt beim Wörterbuch.",
                self.max_per_run
            );
            offen.truncate(self.max_per_run);
        }
        let mut usage = Usage::default();
        if offen.is_empty() {
            return usage;
        }
        let chunks: Vec<&[&Input]> = offen.chunks(CHUNK).collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let ergebnisse = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..WORKERS.min(chunks.len()) {
                s.spawn(|| loop {
                    let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(chunk) = chunks.get(n) else { break };
                    let r = self.request_with_retry(chunk);
                    ergebnisse.lock().unwrap().push((n, r));
                });
            }
        });
        for (n, r) in ergebnisse.into_inner().unwrap() {
            usage.requests += 1;
            match r {
                Ok((tags, u)) => {
                    usage.input_tokens += u.input_tokens;
                    usage.cache_write_tokens += u.cache_creation_input_tokens;
                    usage.cache_read_tokens += u.cache_read_input_tokens;
                    usage.output_tokens += u.output_tokens;
                    for (i, input) in chunks[n].iter().enumerate() {
                        if let Some(t) = tags.get(&i) {
                            self.cache.insert(self.key(input), t.clone());
                        }
                    }
                }
                Err(e) => {
                    usage.failed_requests += 1;
                    eprintln!("WARNUNG LLM-Tags: Anfrage {n} fehlgeschlagen: {e:#}");
                }
            }
        }
        if let Err(e) = self.save() {
            eprintln!("WARNUNG LLM-Tags: Cache nicht geschrieben: {e:#}");
        }
        usage
    }

    fn save(&self) -> Result<()> {
        if let Some(dir) = self.cache_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.cache_path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(&self.cache)?)?;
        std::fs::rename(&tmp, &self.cache_path).context("Cache umbenennen")
    }

    fn request_with_retry(&self, chunk: &[&Input]) -> Result<(HashMap<usize, Vec<String>>, ApiUsage)> {
        let mut warte = 2u64;
        for versuch in 1..=4 {
            match self.request(chunk) {
                Ok(r) => return Ok(r),
                Err(Fehler::Wiederholbar(e, retry_after)) if versuch < 4 => {
                    let s = retry_after.unwrap_or(warte);
                    eprintln!("LLM-Tags: {e} — neuer Versuch in {s} s");
                    std::thread::sleep(Duration::from_secs(s));
                    warte *= 2;
                }
                Err(Fehler::Wiederholbar(e, _)) | Err(Fehler::Endgueltig(e)) => {
                    return Err(anyhow::anyhow!(e))
                }
            }
        }
        unreachable!()
    }

    fn request(&self, chunk: &[&Input]) -> Result<(HashMap<usize, Vec<String>>, ApiUsage), Fehler> {
        let user: Vec<serde_json::Value> = chunk
            .iter()
            .enumerate()
            .map(|(i, o)| {
                serde_json::json!({"i": i, "title": o.title, "sub": o.subtitle,
                                   "cat": o.category, "dict": o.dict})
            })
            .collect();
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": [{"type": "text", "text": self.system,
                        "cache_control": {"type": "ephemeral"}}],
            // Einordnen, nicht nachdenken: Haiku 5.5 erlaubt `disabled` bis
            // Aufwand `high`, und `low` hält die Ausgabe bei den Tags.
            "thinking": {"type": "disabled"},
            "output_config": {
                "effort": "low",
                "format": {"type": "json_schema", "schema": antwort_schema()}
            },
            "messages": [{"role": "user", "content": serde_json::Value::Array(user).to_string()}]
        });
        let resp = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .map_err(|e| Fehler::Wiederholbar(format!("Netz: {e}"), None))?;
        let status = resp.status();
        if !status.is_success() {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok());
            let text = resp.text().unwrap_or_default();
            let msg = format!("HTTP {status}: {}", text.chars().take(300).collect::<String>());
            return Err(if status.as_u16() == 429 || status.is_server_error() {
                Fehler::Wiederholbar(msg, retry_after)
            } else {
                Fehler::Endgueltig(msg)
            });
        }
        let r: ApiResponse =
            resp.json().map_err(|e| Fehler::Endgueltig(format!("Antwort unlesbar: {e}")))?;
        if r.stop_reason.as_deref() == Some("refusal") {
            return Err(Fehler::Endgueltig("Modell hat abgelehnt (refusal)".into()));
        }
        let text: String =
            r.content.iter().filter(|b| b.kind == "text").filter_map(|b| b.text.as_deref()).collect();
        let antwort: Antwort = serde_json::from_str(&text)
            .map_err(|e| Fehler::Endgueltig(format!("JSON unlesbar: {e}")))?;
        let tags = antwort
            .tags
            .into_iter()
            .filter(|e| e.i < chunk.len())
            .map(|e| (e.i, self.filter_vocab(e.t)))
            .collect();
        Ok((tags, r.usage))
    }

    /// Nur Begriffe, die das Wörterbuch kennt — ein erfundener Begriff fände
    /// in der App ohnehin keinen Suchenden, und `nonfood` steht allein.
    fn filter_vocab(&self, tags: Vec<String>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in tags {
            if self.vocab.contains(&t) && !out.contains(&t) {
                out.push(t);
            }
        }
        if out.iter().any(|t| t == NONFOOD_KEY) {
            return vec![NONFOOD_KEY.to_string()];
        }
        out
    }
}

enum Fehler {
    Wiederholbar(String, Option<u64>),
    Endgueltig(String),
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<Block>,
    stop_reason: Option<String>,
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
    tags: Vec<Eintrag>,
}

#[derive(Deserialize)]
struct Eintrag {
    i: usize,
    t: Vec<String>,
}

fn antwort_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "tags": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "i": {"type": "integer"},
                        "t": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["i", "t"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["tags"],
        "additionalProperties": false
    })
}

/// Prompt mit eingesetztem Vokabular. Die Vorlage liegt in
/// `docs/llm-tagging/prompt.txt`; ihre Beispiele dürfen nie aus dem Goldsatz
/// stammen, sonst misst der Goldsatz den Prompt.
pub fn system_prompt() -> String {
    let vocab = matching::vocabulary();
    let mut ober: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (t, o) in &vocab {
        if let Some(o) = o {
            ober.entry(o.as_str()).or_default().push(t.as_str());
        }
    }
    let ober_text = ober
        .iter()
        .map(|(o, kinder)| {
            let mut k = kinder.clone();
            k.sort();
            format!("  {o}: {}", k.join(", "))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut begriffe: Vec<&str> = vocab.iter().map(|(t, _)| t.as_str()).collect();
    begriffe.sort();
    PROMPT
        .replace("{OBERBEGRIFFE}", &ober_text)
        .replace("{VOKABULAR}", &format!("{}, {NONFOOD_KEY}", begriffe.join(", ")))
}

// ------------------------------------------------------------------ Goldsatz

/// Produkte je Woche in der Produktion, gezählt am 2026-10-20 (verschiedene
/// Titel mit `valid_until >= heute`). Grundlage der Monatsrechnung.
const PRODUKTE_JE_WOCHE: f64 = 3637.0;
/// Wie viele Produkte die beiden Schichten des Goldsatzes in der Produktion
/// vertreten (2026-10-20): `match_key` leer bzw. mit Begriff getaggt.
/// Der Goldsatz zieht aus beiden gleich viel, die Wirklichkeit nicht.
pub(crate) const SCHICHT: [(&str, f64); 2] = [("prod-leer", 567.0), ("prod-getaggt", 2191.0)];
/// Deutscher Angebotstext zerfällt dichter in Tokens als Englisch, und Haiku
/// 5.5 zählt laut Anthropic dieselbe Eingabe rund 30 % höher als Haiku 4.5.
/// 3,2 Zeichen je Token ist bewusst pessimistisch; ein echter Lauf ersetzt
/// die Schätzung durch die `usage`-Zahlen der API.
const ZEICHEN_JE_TOKEN: f64 = 3.2;

#[derive(Deserialize)]
struct Gold {
    set: String,
    title: String,
    #[serde(default)]
    sub: String,
    #[serde(default)]
    cat: String,
    dict: Vec<String>,
    gold: Option<Vec<String>>,
}

/// ok / fehlt / falsch / harmlos — ein Produkt gegen sein Goldetikett.
///
/// Gold `None`: der Text sagt nicht, was es ist; jedes Essens-Tag ist geraten.
/// Gold `["nonfood"]`: leer ist harmlos, ein Essens-Tag falsch.
/// Sonst muss jedes Tag ein Gold-Tag sein oder dessen Ober-/Unterbegriff.
pub(crate) fn urteil(tags: &[String], gold: &Option<Vec<String>>, ober: &HashMap<String, String>) -> &'static str {
    let nonfood = [NONFOOD_KEY.to_string()];
    match gold {
        None => if tags.is_empty() || tags == nonfood { "ok" } else { "falsch" },
        Some(g) if g.as_slice() == nonfood => {
            if tags == nonfood { "ok" } else if tags.is_empty() { "harmlos" } else { "falsch" }
        }
        Some(g) if g.is_empty() => if tags.is_empty() { "ok" } else { "falsch" },
        Some(_) if tags.is_empty() => "fehlt",
        Some(g) => {
            let nah = |t: &String| {
                g.contains(t)
                    || g.iter().any(|x| ober.get(x) == Some(t))
                    || ober.get(t).is_some_and(|o| g.contains(o))
            };
            if tags.iter().all(nah) { "ok" } else { "falsch" }
        }
    }
}

/// `lechariot llm-tags`: Goldsatz messen oder Kosten schätzen.
pub fn eval_gold(path: &str, estimate_only: bool) -> Result<()> {
    let gold: Vec<Gold> = serde_json::from_str(
        &std::fs::read_to_string(path).with_context(|| format!("{path} nicht lesbar"))?,
    )?;
    let inputs: Vec<Input> = gold
        .iter()
        .map(|g| Input {
            title: g.title.clone(),
            subtitle: g.sub.clone(),
            category: g.cat.clone(),
            dict: g.dict.clone(),
        })
        .collect();

    if estimate_only {
        return schaetzen(&inputs);
    }
    // Eigener Cache, damit eine Messung nie den der Nightly füllt oder liest.
    let cache = std::env::var("LLM_TAGS_CACHE").map(PathBuf::from).unwrap_or_else(|_| {
        std::env::temp_dir().join("lechariot-llm-tags-gold.json")
    });
    let key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    anyhow::ensure!(
        !key.trim().is_empty(),
        "ANTHROPIC_API_KEY fehlt — Schlüssel setzen oder --estimate benutzen"
    );
    let model = std::env::var("LLM_TAGS_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let mut t = Tagger::new(model, key.trim().to_string(), Mode::Fill, cache, usize::MAX);
    let u = t.ensure(&inputs);
    println!(
        "{} Anfragen ({} fehlgeschlagen) mit {}: Tokens ein {} / Cache geschrieben {} / gelesen {} / aus {} → ${:.4}",
        u.requests, u.failed_requests, t.model, u.input_tokens, u.cache_write_tokens,
        u.cache_read_tokens, u.output_tokens, u.dollars()
    );
    if u.requests > 0 {
        let je_produkt = u.dollars() / inputs.len() as f64;
        println!(
            "Hochgerechnet ({PRODUKTE_JE_WOCHE} Produkte/Woche, jedes neu): ${:.2}/Woche, ${:.2}/Monat",
            je_produkt * PRODUKTE_JE_WOCHE,
            je_produkt * PRODUKTE_JE_WOCHE * 52.0 / 12.0
        );
    }

    let ober: HashMap<String, String> =
        matching::vocabulary().into_iter().filter_map(|(t, o)| o.map(|o| (t, o))).collect();
    let mut stand: BTreeMap<(&str, &str), BTreeMap<&str, f64>> = BTreeMap::new();
    let mut ohne_antwort = 0;
    for (g, i) in gold.iter().zip(&inputs) {
        let gewicht = SCHICHT
            .iter()
            .find(|(s, _)| *s == g.set)
            .map(|(s, n)| n / gold.iter().filter(|x| x.set == *s).count() as f64)
            .unwrap_or(1.0);
        let mut zaehle = |wer: &'static str, tags: &[String]| {
            let u = urteil(tags, &g.gold, &ober);
            for set in [g.set.as_str(), "gewichtet"] {
                let w = if set == "gewichtet" { gewicht } else { 1.0 };
                *stand.entry((wer, set)).or_default().entry(u).or_default() += w;
            }
        };
        zaehle("wörterbuch", &i.dict);
        if t.cached(i).is_none() {
            ohne_antwort += 1;
        }
        t.mode = Mode::Fill;
        zaehle("llm fill", &t.apply(i));
        t.mode = Mode::Replace;
        zaehle("llm replace", &t.apply(i));
    }
    // Ohne „harmlos" gerechnet: Ein ungetaggtes Non-Food-Angebot findet
    // niemand, aber es schadet auch niemandem.
    for ((wer, set), c) in &stand {
        let n: f64 = c.iter().filter(|(k, _)| **k != "harmlos").map(|(_, v)| v).sum();
        let teil = |k: &str| c.get(k).copied().unwrap_or(0.0) / n * 100.0;
        println!(
            "{wer:12} {set:13} ok {:5.1} %  fehlt {:5.1} %  falsch {:5.1} %",
            teil("ok"),
            teil("fehlt"),
            teil("falsch")
        );
    }
    if ohne_antwort > 0 {
        println!("{ohne_antwort} Produkte ohne Modellantwort — sie zählen mit ihrem Wörterbuch-Tag.");
    }
    Ok(())
}

fn schaetzen(inputs: &[Input]) -> Result<()> {
    let system = system_prompt().chars().count() as f64 / ZEICHEN_JE_TOKEN;
    let chunks: Vec<&[Input]> = inputs.chunks(CHUNK).collect();
    let user: f64 = chunks
        .iter()
        .map(|c| {
            let v: Vec<_> = c
                .iter()
                .enumerate()
                .map(|(i, o)| serde_json::json!({"i": i, "title": o.title, "sub": o.subtitle,
                                                  "cat": o.category, "dict": o.dict}))
                .collect();
            serde_json::Value::Array(v).to_string().chars().count() as f64 / ZEICHEN_JE_TOKEN
        })
        .sum();
    // `{"i":12,"t":["käse","wurst"]},` ≈ 14 Tokens je Produkt.
    let aus = inputs.len() as f64 * 14.0;
    let n = chunks.len() as f64;
    let schreiben = system * n.min(WORKERS as f64);
    let lesen = system * (n - WORKERS as f64).max(0.0);
    let [p_ein, p_schreiben, p_lesen, p_aus] = PREIS_HAIKU_5_5;
    let dollar = (user * p_ein + schreiben * p_schreiben + lesen * p_lesen + aus * p_aus) / 1e6;
    let je_woche = dollar / inputs.len() as f64 * PRODUKTE_JE_WOCHE;
    println!(
        "{} Produkte, {} Anfragen; System-Prompt ≈ {system:.0} Tokens, Nutzertext ≈ {user:.0}, Ausgabe ≈ {aus:.0}",
        inputs.len(),
        chunks.len()
    );
    println!(
        "Haiku 5.5: ≈ ${dollar:.4} für diesen Satz; hochgerechnet ≈ ${je_woche:.3}/Woche, ≈ ${:.2}/Monat",
        je_woche * 52.0 / 12.0
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tagger(mode: Mode) -> Tagger {
        let dir = std::env::temp_dir().join(format!("llm-tags-test-{}", std::process::id()));
        Tagger::new("m".into(), "k".into(), mode, dir.join("c.json"), 10)
    }

    fn input(title: &str, dict: &[&str]) -> Input {
        Input {
            title: title.into(),
            subtitle: String::new(),
            category: String::new(),
            dict: dict.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn prompt_traegt_vokabular_und_keine_platzhalter() {
        let p = system_prompt();
        assert!(p.contains("käse") && p.contains("nonfood"));
        assert!(!p.contains("{VOKABULAR}") && !p.contains("{OBERBEGRIFFE}"));
    }

    #[test]
    fn fill_ueberschreibt_kein_woerterbuch_tag() {
        let mut t = tagger(Mode::Fill);
        let leer = input("Passionsfrüchte", &[]);
        let voll = input("Käsewiener", &["käse"]);
        t.cache.insert(t.key(&leer), vec!["passionsfrucht".into()]);
        t.cache.insert(t.key(&voll), vec!["würstchen".into()]);
        assert_eq!(t.apply(&leer), vec!["passionsfrucht"]);
        assert_eq!(t.apply(&voll), vec!["käse"]);
        t.mode = Mode::Replace;
        assert_eq!(t.apply(&voll), vec!["würstchen"]);
    }

    #[test]
    fn ohne_antwort_bleibt_das_woerterbuch() {
        let t = tagger(Mode::Replace);
        assert_eq!(t.apply(&input("Butter", &["butter"])), vec!["butter"]);
    }

    #[test]
    fn unbekannte_begriffe_fallen_weg_und_nonfood_steht_allein() {
        let t = tagger(Mode::Fill);
        assert_eq!(t.filter_vocab(vec!["käse".into(), "erfunden".into()]), vec!["käse"]);
        assert_eq!(t.filter_vocab(vec!["kerzen".into(), "nonfood".into()]), vec!["nonfood"]);
    }

    #[test]
    fn schluessel_haengt_an_modell_und_text() {
        let a = tagger(Mode::Fill);
        let b = Tagger::new("anderes".into(), "k".into(), Mode::Fill, PathBuf::from("/x"), 1);
        let i = input("Butter", &[]);
        assert_ne!(a.key(&i), b.key(&i));
        assert_ne!(a.key(&i), a.key(&input("Margarine", &[])));
    }
}
