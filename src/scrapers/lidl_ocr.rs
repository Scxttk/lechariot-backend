//! Texterkennung für Lidl-Prospektseiten ohne Textebene.
//!
//! Seit dem Prospekt vom 05.10.2026 zeichnet Lidl die Schrift als Pfade.
//! `pdftotext` findet auf 1–3 von ~72 Seiten noch Wörter, und aus ~330
//! Sternpreisen je Woche wurden 8–48 (nachgezählt in 120 Nightly-Logs). Die
//! Seitenbilder im Prospekt-JSON (`pages[].zoom`, 1415×2400 JPEG) tragen den
//! Text aber lesbar. Dieses Modul liest sie per OCR
//! (`scripts/lidl_ocr.py`, RapidOCR) und schreibt das Ergebnis als
//! Wortkoordinaten in die leeren Seiten der `-bbox-layout`-Ausgabe, markiert
//! mit `ocr="1"`. Der Parser in [`super::lidl_prospekt`] liest sie wie die
//! Textebene; nur bei der Zuordnung Preis ↔ Name gelten auf diesen Seiten
//! zwei zusätzliche Regeln (siehe `carries_own_name` dort).
//!
//! Gemessen am Prospekt vom 26.10.2026: 19 Angebote aus der Textebene, mit
//! OCR 351. Auf acht von Hand abgelesenen Seiten (60 Angebote) kamen 53
//! heraus, keines mit dem Preis eines anderen Produkts; 6 davon tragen nur
//! die Marke als Titel („FAVORINA").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::scrapers::util;

/// Ab dieser Zeilenhöhe (Anteil der Seitenhöhe) ist eine erkannte Zahl ein
/// Sternpreis. Die großen Preise liegen bei 0,040–0,052, alles andere mit
/// Komma (Grundpreise, Streichpreise, „1 kg = 8.13") unter 0,012 (gemessen
/// am 26.10.2026 über alle 72 Seiten).
const PRICE_LINE_SHARE: f64 = 0.035;

/// Schriftgrade, die der Parser erwartet (gemessen an der Textebene vom
/// 27.07.2026): Produktnamen 11,96 pt, Fließtext 9,2 pt, Banner über 13 pt
/// (siehe `MIN_NAME_PT`/`MAX_NAME_PT` in [`super::lidl_prospekt`]).
///
/// Aus der OCR-Zeilenhöhe lassen sie sich nicht ablesen — alle drei kommen
/// bei ~8,9 pt heraus. Deshalb entscheidet die Strichstärke (fett = Name),
/// und unter den fetten Zeilen die Polarität: Banner stehen hell auf dunkel,
/// wo der Fließtext ringsum dunkel auf hell steht, oder umgekehrt.
const NAME_PT: f64 = 11.96;
const BODY_PT: f64 = 9.2;
const BANNER_PT: f64 = 16.0;
/// Ab dieser Strichstärke (Anteil der Bildhöhe) ist eine Zeile fett: 2,8 px
/// bei 2400 px. Fette Namen liegen bei 3,1–3,6 px, Fließtext bei 2,1–2,4 px.
const BOLD_STROKE: f64 = 2.8 / 2400.0;
/// So viele Nachbarzeilen in normaler Schrift bestimmen, welche Polarität
/// an einer Stelle der Seite die des Fließtexts ist.
const POLARITY_NEIGHBOURS: usize = 6;
/// Linke Kanten, die so nah beieinander liegen (Anteil der Bildbreite), gelten
/// als bündig — 14 px bei 1415 px.
const ALIGN_TOLERANCE: f64 = 0.01;
/// So viele fette Zeilen darf ein Produktname haben, bevor der Fließtext kommt.
const NAME_BLOCK_DEPTH: usize = 6;

/// Sternpreis, wie OCR ihn liest: „2,99°", „1.70*d)", „-.57", „5.790 **))"
/// (die Fußnote „*d)" als Ziffer gelesen).
/// Ohne Vorkommastelle nur mit Minus davor — „.66'6" ist ein auf dem Kopf
/// stehender Preis, kein 0,66.
static BIG_PRICE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:(\d{1,3})|-)[.,](\d{2})(?:$|[^\d]|\d(?:$|\s))").unwrap());
/// Dieselbe Zahl ohne Trennzeichen („199" für 1.99 — oder 199 €, siehe
/// [`WHOLE_EUROS_ASPECT`]). Nur dreistellig: Vier Ziffern in dieser Größe
/// sind Jahreszahlen („2026").
static BIG_PRICE_NO_SEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d)(\d{2})[*°\x22']?$").unwrap());
/// Ganze Euro: „549.-", „129-", „549.". Ein bloßer Punkt erst ab drei
/// Ziffern — „10." stammt aus der Überschrift „Zum 10. Mal" (19.10.2026).
static WHOLE_EUROS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:(\d{1,4})[.,]?-|(\d{3,4})[.,])\W{0,2}$").unwrap());
/// Breite durch Höhe (in Bildpunkten), ab der eine dreistellige Zahl ohne
/// Trennzeichen ganze Euro sind. „7⁹⁹" setzt die Cent klein und hochgestellt
/// und kommt auf 1,4–1,6, „149.-" in voller Größe auf 1,9–2,1 (gemessen
/// über beide Prospekte vom 19. und 26.10.2026).
const WHOLE_EUROS_ASPECT: f64 = 1.75;
/// „1.200 feine Tropfen" ist eine Überschrift, kein Preis.
static THOUSANDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,3}[.,]\d{3}\s+\p{L}").unwrap());
/// Ein Stern nach einer kleinen Zahl ist ein Lesefehler oder eine Fußnote,
/// kein Sternpreis — der Parser soll ihn nicht als Preis zählen.
static SMALL_STAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d[.,]\d{2})\s*\*").unwrap());
/// Plakette über dem Namen; sie stünde sonst vor jedem Titel.
static ONLINE_BADGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(auch|nur)\s*online$").unwrap());

/// Eine erkannte Textzeile. Koordinaten relativ zur Bildgröße (0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Strichstärke, Anteil der Bildhöhe.
    pub stroke: f64,
    /// Helle Schrift auf dunklem Grund.
    pub light: bool,
    pub text: String,
}

/// Ausgabe von `scripts/lidl_ocr.py` lesen: Bildindex, x, y, w, h,
/// Strichstärke, hell (0/1), Text.
pub fn parse_ocr_output(out: &str) -> Vec<(usize, OcrLine)> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.splitn(8, '\t');
            let idx = f.next()?.parse().ok()?;
            let x = f.next()?.parse().ok()?;
            let y = f.next()?.parse().ok()?;
            let w = f.next()?.parse().ok()?;
            let h = f.next()?.parse().ok()?;
            let stroke = f.next()?.parse().ok()?;
            let light = f.next()? == "1";
            let text = f.next()?.trim().to_string();
            Some((
                idx,
                OcrLine {
                    x,
                    y,
                    w,
                    h,
                    stroke,
                    light,
                    text,
                },
            ))
        })
        .collect()
}

/// Hängt unter dieser fetten Zeile ein linksbündiger Block, der in normaler
/// Schrift endet? So steht ein Produktname im Prospekt: Marke und Name fett,
/// darunter bündig „Je 500 g; 1 kg = 7.98". Schrift auf Verpackungsfotos
/// („XXL XXL", „Alesto", „PULL!") ist ebenso fett, hat diesen Block aber nie.
fn heads_a_text_block(lines: &[OcrLine], i: usize) -> bool {
    let mut cur = &lines[i];
    for _ in 0..NAME_BLOCK_DEPTH {
        let next = lines
            .iter()
            .filter(|o| o.h < PRICE_LINE_SHARE)
            .filter(|o| (o.x - cur.x).abs() < ALIGN_TOLERANCE)
            .filter(|o| o.y > cur.y + cur.h * 0.5 && o.y - (cur.y + cur.h) < cur.h * 1.2)
            .min_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
        match next {
            Some(n) if n.stroke < BOLD_STROKE => return true,
            Some(n) => cur = n,
            None => return false,
        }
    }
    false
}

/// Schriftgrad je Zeile, wie ihn die Textebene hätte. Große Zeilen (Preise,
/// Überschriften) behalten ihre Höhe in pt; 0 heißt: Zeile verwerfen.
fn font_sizes(lines: &[OcrLine], ph: f64) -> Vec<f64> {
    let small = |l: &OcrLine| l.h < PRICE_LINE_SHARE;
    let regular: Vec<&OcrLine> = lines
        .iter()
        .filter(|l| small(l) && l.stroke < BOLD_STROKE)
        .collect();
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if !small(l) {
                return l.h * ph;
            }
            if l.stroke < BOLD_STROKE {
                return BODY_PT;
            }
            // Fette Schrift ohne Textblock darunter steht auf einem Foto.
            if !heads_a_text_block(lines, i) {
                return 0.0;
            }
            let centre = |o: &OcrLine| (o.x + o.w / 2.0, o.y + o.h / 2.0);
            let (cx, cy) = centre(l);
            let mut near: Vec<(f64, bool)> = regular
                .iter()
                .map(|o| {
                    let (ox, oy) = centre(o);
                    ((ox - cx).powi(2) + (oy - cy).powi(2), o.light)
                })
                .collect();
            near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let near = &near[..near.len().min(POLARITY_NEIGHBOURS)];
            let light_share = near.iter().filter(|n| n.1).count() * 2;
            let body_light = light_share > near.len();
            if !near.is_empty() && light_share != near.len() && l.light != body_light {
                BANNER_PT
            } else {
                NAME_PT
            }
        })
        .collect()
}

/// Text einer Zeile so umschreiben, wie ihn die Textebene liefern würde.
/// `None` für Zeilen, die nicht in die Ausgabe gehören.
/// `aspect` ist Breite durch Höhe des Bildes.
pub fn normalise_line(line: &OcrLine, aspect: f64) -> Option<String> {
    let text = line.text.trim();
    if text.is_empty() || ONLINE_BADGE.is_match(text) {
        return None;
    }
    if line.h >= PRICE_LINE_SHARE {
        if THOUSANDS.is_match(text) {
            return Some(text.to_string());
        }
        if let Some(c) = BIG_PRICE.captures(text) {
            let euros = c.get(1).map_or("0", |m| m.as_str());
            return Some(format!("{euros}.{}*", &c[2]));
        }
        if let Some(c) = WHOLE_EUROS.captures(text) {
            let euros = c.get(1).or(c.get(2)).map_or("", |m| m.as_str());
            return Some(format!("{euros}.00*"));
        }
        if let Some(c) = BIG_PRICE_NO_SEP.captures(text) {
            if line.w * aspect / line.h >= WHOLE_EUROS_ASPECT {
                return Some(format!("{}{}.00*", &c[1], &c[2]));
            }
            return Some(format!("{}.{}*", &c[1], &c[2]));
        }
        return Some(text.to_string());
    }
    Some(SMALL_STAR.replace_all(text, "$1").into_owned())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Erkannte Zeilen als `<flow>`-Blöcke einer Seite der Größe `pw`×`ph` pt.
///
/// Jede Zeile wird ein eigener Flow: Der Parser fasst nahe Inseln ohnehin
/// selbst zu Kacheln zusammen. Die Wortgrenzen innerhalb der Zeile sind nach
/// Zeichenzahl verteilt — RapidOCR liefert nur Zeilenrahmen, und der Parser
/// braucht von den Wörtern nur Höhe und Text.
pub fn lines_to_flows(lines: &[OcrLine], pw: f64, ph: f64) -> String {
    let mut out = String::new();
    for (line, pt) in lines.iter().zip(font_sizes(lines, ph)) {
        if pt <= 0.0 {
            continue;
        }
        let Some(text) = normalise_line(line, pw / ph) else {
            continue;
        };
        let (x0, x1) = (line.x * pw, (line.x + line.w) * pw);
        let (y0, y1) = (line.y * ph, (line.y + line.h) * ph);
        // Nur die Wörter tragen den Schriftgrad. Der Block behält den echten
        // Rahmen: Größer gezogen liefen benachbarte Kacheln zusammen.
        let yc = (y0 + y1) / 2.0;
        let (wy0, wy1) = (yc - pt / 2.0, yc + pt / 2.0);
        let words: Vec<&str> = text.split_whitespace().collect();
        let chars: usize = words.iter().map(|w| w.chars().count()).sum::<usize>() + words.len() - 1;
        let step = (x1 - x0) / chars.max(1) as f64;
        let rect = format!(r#"xMin="{x0:.4}" yMin="{y0:.4}" xMax="{x1:.4}" yMax="{y1:.4}""#);
        out.push_str(&format!(
            "    <flow>\n      <block {rect}>\n        <line {rect}>\n"
        ));
        let mut pos = 0usize;
        for word in words {
            let n = word.chars().count();
            let (a, b) = (x0 + step * pos as f64, x0 + step * (pos + n) as f64);
            pos += n + 1;
            out.push_str(&format!(
                "          <word xMin=\"{a:.4}\" yMin=\"{wy0:.4}\" xMax=\"{b:.4}\" yMax=\"{wy1:.4}\">{}</word>\n",
                xml_escape(word)
            ));
        }
        out.push_str("        </line>\n      </block>\n    </flow>\n");
    }
    out
}

/// Seiten der `-bbox-layout`-Ausgabe: (Breite, Höhe, trägt Wörter).
fn page_scan(xml: &str) -> Vec<(f64, f64, bool)> {
    let mut pages = Vec::new();
    for line in xml.lines() {
        let line = line.trim_start();
        if line.starts_with("<page ") {
            let attr = |name: &str| -> f64 {
                line.split(&format!("{name}=\""))
                    .nth(1)
                    .and_then(|r| r.split('"').next())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0)
            };
            pages.push((attr("width"), attr("height"), false));
        } else if line.starts_with("<word ")
            && let Some(page) = pages.last_mut()
        {
            page.2 = true;
        }
    }
    pages
}

/// Welche Seiten der `-bbox-layout`-Ausgabe aus der Texterkennung stammen
/// (markiert mit `ocr="1"`).
pub fn ocr_pages(xml: &str) -> Vec<bool> {
    xml.lines()
        .map(str::trim_start)
        .filter(|l| l.starts_with("<page "))
        .map(|l| l.contains("ocr=\"1\""))
        .collect()
}

/// Die URL des Seitenbilds aus einem Eintrag von `flyer.pages`.
fn page_image(page: &serde_json::Value) -> Option<String> {
    ["zoom", "image"]
        .iter()
        .find_map(|k| page.get(*k).and_then(|v| v.as_str()))
        .map(str::to_string)
}

/// Texterkennung für eine Liste von Bild-URLs: je URL ihre Zeilen.
pub type Ocr<'a> = dyn Fn(&[String]) -> Result<Vec<Vec<OcrLine>>> + 'a;

/// Leere Seiten per OCR füllen. `ocr` bekommt die Bild-URLs der leeren Seiten
/// und liefert je URL ihre Zeilen.
pub fn fill_textless_pages_with(
    xml: &str,
    flyer_pages: &[serde_json::Value],
    ocr: &Ocr,
) -> Result<(String, usize)> {
    let scan = page_scan(xml);
    let empty: Vec<usize> = (0..scan.len()).filter(|&i| !scan[i].2).collect();
    if empty.is_empty() {
        return Ok((xml.to_string(), 0));
    }
    // Seite i der PDF ist Eintrag i im JSON. Passt die Zahl nicht, wäre jede
    // Zuordnung geraten.
    if scan.len() != flyer_pages.len() {
        bail!(
            "PDF hat {} Seiten, das Prospekt-JSON {} — Seitenbilder nicht zuordenbar",
            scan.len(),
            flyer_pages.len()
        );
    }
    let mut targets = Vec::new();
    let mut urls = Vec::new();
    for &i in &empty {
        if let Some(url) = page_image(&flyer_pages[i]) {
            targets.push(i);
            urls.push(url);
        }
    }
    let lines = ocr(&urls)?;
    let flows: HashMap<usize, String> = targets
        .iter()
        .zip(&lines)
        .map(|(&i, l)| (i, lines_to_flows(l, scan[i].0, scan[i].1)))
        .collect();

    let mut out = String::with_capacity(xml.len() + flows.values().map(String::len).sum::<usize>());
    let mut page = usize::MAX;
    for line in xml.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("<page ") {
            page = page.wrapping_add(1);
            if flows.contains_key(&page) {
                out.push_str(&line.replacen("<page ", "<page ocr=\"1\" ", 1));
                continue;
            }
        } else if trimmed.starts_with("</page>")
            && let Some(f) = flows.get(&page)
        {
            out.push_str(f);
        }
        out.push_str(line);
    }
    Ok((out, targets.len()))
}

/// Wie [`fill_textless_pages_with`] mit dem echten OCR. Scheitert die
/// Texterkennung, bleibt die Ausgabe unverändert — dann greift wie vorher
/// [`super::lidl_prospekt::NoTextLayer`].
pub fn fill_textless_pages(xml: &str, flyer_pages: &[serde_json::Value]) -> String {
    match fill_textless_pages_with(xml, flyer_pages, &run_ocr) {
        Ok((filled, 0)) => filled,
        Ok((filled, n)) => {
            println!("  {n} Seiten ohne Textebene per OCR gelesen");
            filled
        }
        Err(e) => {
            eprintln!("WARNUNG [Lidl] OCR der Seiten ohne Textebene fehlgeschlagen: {e:#}");
            xml.to_string()
        }
    }
}

/// Zwischenspeicher der OCR-Ergebnisse, eine Datei je Seitenbild. Die
/// Nightly hält ihn per `actions/cache` über die Woche: Jede Prospektvariante
/// wird einmal gelesen, nicht in jedem Lauf.
fn cache_dir() -> PathBuf {
    std::env::var_os("LIDL_OCR_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("lechariot-lidl-ocr"))
}

fn cache_path(dir: &Path, key: &[u8]) -> PathBuf {
    let hash = Sha256::digest(key);
    let hex: String = hash.iter().take(16).map(|b| format!("{b:02x}")).collect();
    dir.join(format!("{hex}.tsv"))
}

/// Befehl für die Texterkennung. `LIDL_OCR` überschreibt ihn, etwa mit einem
/// anderen Python.
fn ocr_command() -> Vec<String> {
    match std::env::var("LIDL_OCR") {
        Ok(cmd) if !cmd.trim().is_empty() => cmd.split_whitespace().map(str::to_string).collect(),
        _ => vec![
            "python3".into(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/lidl_ocr.py").into(),
        ],
    }
}

/// Zwei Schlüssel je Seite: die URL, damit ein späterer Lauf nichts mehr laden
/// muss, und der Bildinhalt. Die Prospektvarianten einer Woche teilen fast
/// alle Seiten byteweise (26.10.2026: 71 von 72 zwischen zwei Regionen),
/// unter verschiedenen URLs. Über den Inhalt wird jede Seite nur einmal
/// gelesen, eine weitere Variante kostet ein, zwei Seiten statt 72.
fn run_ocr(urls: &[String]) -> Result<Vec<Vec<OcrLine>>> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("{} anlegen", dir.display()))?;
    let missing: Vec<&String> = urls
        .iter()
        .filter(|u| !cache_path(&dir, u.as_bytes()).exists())
        .collect();

    if !missing.is_empty() {
        let client = util::blocking_client()?;
        // Seiten, deren Inhalt noch niemand gelesen hat: (URL, Inhaltsschlüssel, Bild).
        let mut unread: Vec<(&String, PathBuf, PathBuf)> = Vec::new();
        for url in &missing {
            let bytes = client
                .get(url.as_str())
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.bytes())
                .with_context(|| format!("Seitenbild {url}"))?;
            let by_content = cache_path(&dir, &bytes);
            if by_content.exists() {
                std::fs::copy(&by_content, cache_path(&dir, url.as_bytes()))?;
                continue;
            }
            if unread.iter().any(|u| u.1 == by_content) {
                // Dasselbe Bild zweimal in diesem Aufruf: einmal lesen.
                unread.push((url, by_content, PathBuf::new()));
                continue;
            }
            let image = by_content.with_extension("jpg");
            std::fs::write(&image, &bytes)?;
            unread.push((url, by_content, image));
        }
        let images: Vec<&PathBuf> = unread
            .iter()
            .map(|u| &u.2)
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if !images.is_empty() {
            let cmd = ocr_command();
            let output = Command::new(&cmd[0])
                .args(&cmd[1..])
                .args(&images)
                .output()
                .with_context(|| format!("{} nicht startbar", cmd.join(" ")));
            for image in &images {
                let _ = std::fs::remove_file(image);
            }
            let output = output?;
            if !output.status.success() {
                bail!(
                    "OCR fehlgeschlagen: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            let mut per_image: Vec<String> = vec![String::new(); images.len()];
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let Some((idx, rest)) = line.split_once('\t') else {
                    continue;
                };
                if let Some(slot) = idx.parse::<usize>().ok().and_then(|i| per_image.get_mut(i)) {
                    slot.push_str(&format!("0\t{rest}\n"));
                }
            }
            for (image, text) in images.iter().zip(per_image) {
                std::fs::write(image.with_extension("tsv"), text)?;
            }
        }
        for (url, by_content, _) in &unread {
            std::fs::copy(by_content, cache_path(&dir, url.as_bytes()))?;
        }
    }

    urls.iter()
        .map(|u| {
            let text = std::fs::read_to_string(cache_path(&dir, u.as_bytes()))?;
            Ok(parse_ocr_output(&text)
                .into_iter()
                .map(|(_, l)| l)
                .collect())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(h: f64, text: &str) -> OcrLine {
        OcrLine {
            x: 0.1,
            y: 0.1,
            w: 0.2,
            h,
            stroke: 0.001,
            light: false,
            text: text.into(),
        }
    }

    #[test]
    fn grosse_zahlen_werden_sternpreise() {
        let n = |h, t| normalise_line(&line(h, t), 0.59);
        assert_eq!(n(0.045, "2,99°").as_deref(), Some("2.99*"));
        assert_eq!(n(0.045, "1,70*0 25").as_deref(), Some("1.70*"));
        assert_eq!(n(0.045, "5.790 **))").as_deref(), Some("5.79*"));
        // „7⁹⁹" mit hochgestellten Cent ist schmal.
        let schmal = OcrLine {
            w: 0.045 * 1.5 / 0.59,
            ..line(0.045, "199")
        };
        assert_eq!(normalise_line(&schmal, 0.59).as_deref(), Some("1.99*"));
        assert_eq!(n(0.045, "-.57").as_deref(), Some("0.57*"));
        assert_eq!(n(0.045, "549.-").as_deref(), Some("549.00*"));
        assert_eq!(n(0.045, "129-").as_deref(), Some("129.00*"));
        assert_eq!(n(0.045, "549.").as_deref(), Some("549.00*"));
        assert_eq!(n(0.08, "10.").as_deref(), Some("10."));
        // „149.-" ohne Strich gelesen: so breit wie hoch mal zwei, keine 1.49.
        let breit = OcrLine {
            w: 0.045 * 2.0 / 0.59,
            ..line(0.045, "149")
        };
        assert_eq!(normalise_line(&breit, 0.59).as_deref(), Some("149.00*"));
    }

    #[test]
    fn was_kein_sternpreis_ist_bleibt_stehen() {
        let n = |h, t| normalise_line(&line(h, t), 0.59);
        // Auf dem Kopf stehender Preis, keine 0,66.
        assert_eq!(n(0.045, ".66'6").as_deref(), Some(".66'6"));
        assert_eq!(n(0.045, "1.200 feine").as_deref(), Some("1.200 feine"));
        assert_eq!(n(0.07, "2026").as_deref(), Some("2026"));
        // Kleine Zahl mit Stern: Fußnote, kein Preis.
        assert_eq!(n(0.011, "9.99* н50").as_deref(), Some("9.99 н50"));
        assert_eq!(
            n(0.011, "Normalpreis: 2.55").as_deref(),
            Some("Normalpreis: 2.55")
        );
        assert_eq!(n(0.011, "Auch online"), None);
    }

    #[test]
    fn leere_seiten_bekommen_die_ocr_zeilen() {
        let xml = "<doc>\n  <page width=\"100\" height=\"200\">\n  </page>\n  \
                   <page width=\"100\" height=\"200\">\n    <word xMin=\"1\" yMin=\"1\" xMax=\"2\" yMax=\"2\">A</word>\n  </page>\n</doc>\n";
        let pages = vec![
            serde_json::json!({"zoom": "https://example.org/1.jpg"}),
            serde_json::json!({"zoom": "https://example.org/2.jpg"}),
        ];
        let ocr = |urls: &[String]| -> Result<Vec<Vec<OcrLine>>> {
            assert_eq!(urls, ["https://example.org/1.jpg"]);
            Ok(vec![vec![
                line(0.045, "2,99°"),
                line(0.01, "HALLOWEEN Berliner"),
            ]])
        };
        let (filled, n) = fill_textless_pages_with(xml, &pages, &ocr).unwrap();
        assert_eq!(n, 1);
        let first = filled.split("</page>").next().unwrap();
        assert!(first.contains(">2.99*</word>"), "{first}");
        assert!(first.contains(">Berliner</word>"), "{first}");
        assert_eq!(filled.matches("<word ").count(), 4);
    }

    #[test]
    fn falsche_seitenzahl_wird_nicht_geraten() {
        let xml = "<page width=\"1\" height=\"1\">\n</page>\n";
        let ocr = |_: &[String]| -> Result<Vec<Vec<OcrLine>>> { unreachable!() };
        assert!(fill_textless_pages_with(xml, &[], &ocr).is_err());
    }
}
