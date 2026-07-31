//! Lidl-Angebote aus Lidls eigenem Wochenprospekt — ohne marktguru.
//!
//! Lidl war die einzige der acht Ketten, deren Angebote über einen Dritten
//! (api.marktguru.de) kamen, und mit ~30 % aller Zeilen zugleich die größte
//! Quelle. Dieser Weg holt dieselben Angebote beim Händler selbst:
//!
//! ```text
//! PLZ -> Bing-Store-Finder (Feld AR = Absatzregion)
//!     -> lidl.com/flyer/esi-overview  (~18 Prospektvarianten je Woche)
//!     -> endpoints.leaflets.schwarz/v4/flyer?flyer_identifier=<slug>
//!     -> JSON: offerStartDate/offerEndDate, regions[].code, pdfUrl
//!     -> PDF (~83 MB, mit echter Textebene) -> pdftotext -bbox-layout
//!     -> Kacheln -> Offers
//! ```
//!
//! Kein Schlüssel, keine Anmeldung, kein LLM.
//!
//! **Warum der Umweg über die PDF?** Das Prospekt-JSON hat zwar ein
//! `products`-Feld, darin stehen aber ausschließlich Onlineshop-Artikel
//! (Porzellan, Kaffeevollautomat, Wein) — 2026-07-25 nachgezählt: 138
//! Einträge, null Lebensmittel. Dasselbe gilt für `pages[].links`. Ein
//! erster Anlauf im Juli (Branch `feature/lidl-prospekt-llm-pipeline`, jetzt
//! Tag `archiv/…`) hat deshalb die Seitenbilder per Vision-LLM ausgelesen —
//! teuer, langsam und mit Halluzinationsrisiko genau beim Preis. Dass unter
//! den Bildern eine Textebene liegt, hat er nie geprüft. Sie liegt dort:
//! `pdftotext` zieht aus einem Wochenprospekt rund 315 Preise, 215
//! Rabattangaben und 214 Grundpreise im Klartext.
//!
//! **Warum Geometrie und nicht Regex?** Preis und Produktname stehen nicht in
//! einer gemeinsamen Textzeile, sondern nebeneinander auf der Seite. Deshalb
//! `-bbox-layout` (Wortkoordinaten), Kachelbildung über Abstände und eine
//! Zuordnung Produkt <-> Preis. Details bei [`extract_offers`].

use std::collections::HashMap;
use std::process::Command;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use regex::Regex;

use crate::models::{Market, Offer};
use crate::scrapers::{store_finder, util};

/// Übersicht aller Prospektvarianten. `mode=iframe` liefert das nackte
/// Fragment statt der kompletten Shop-Seite.
const OVERVIEW_URL: &str =
    "https://www.lidl.com/flyer/esi-overview/overview?client_locale=lidl%2Fde-DE&mode=iframe";
const FLYER_URL: &str = "https://endpoints.leaflets.schwarz/v4/flyer?flyer_identifier=";

/// Abstand, bis zu dem zwei Textinseln zur selben Kachel verschmelzen (pt).
/// Bei 10 pt tragen 91 % der preisführenden Kacheln genau einen Sternpreis;
/// ab ~20 pt laufen benachbarte Kacheln zusammen (gemessen 2026-07-25).
const CLUSTER_GAP_PT: f64 = 10.0;
/// Maximaler Abstand, über den ein Preis noch seinem Produkttext zugeordnet
/// wird. Großzügig, weil zwischen beiden das Produktbild liegt.
const PAIR_GAP_PT: f64 = 120.0;
/// Abstand, in dem Badges (Rabatt, UVP, „Mit Lidl Plus") zur Kachel zählen.
const BADGE_GAP_PT: f64 = 45.0;
/// Toleranz der Rechenprobe (siehe [`arithmetic_check`]).
const PRICE_TOLERANCE: f64 = 0.06;
/// Preise außerhalb dieser Spanne sind keine Lebensmittelangebote.
const MIN_PRICE: f64 = 0.10;
const MAX_PRICE: f64 = 100.0;

// ------------------------------------------------------------------ Muster

/// Der Angebotspreis trägt im Prospekt konsequent einen Stern („2.49*"),
/// Streich- und Grundpreise nicht. Das ist der verlässlichste Anker.
static PRICE_STAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,3}[.,]\d{2})\s*\*").unwrap());
/// „1 kg = 6.23", „1 l = -.68" (Lidl schreibt Beträge unter 1 € mit
/// Bindestrich) und „1 kg = 13.29/11.50" für Artikel in mehreren Größen.
static BASE_PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"1\s*(kg|l|Stk)\s*=\s*((?:-?[.,]?\d{0,3}[.,]\d{2})(?:\s*/\s*-?[.,]?\d{0,3}[.,]\d{2})*)",
    )
    .unwrap()
});
static REGULAR_PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:UVP|Normalpreis|Letzter Preis)[:\s]*(\d{1,3}[.,]\d{2})").unwrap()
});
/// Packungsgröße: „Je 400 g", „Je 2x 75 ml", „Je 6x 0,25 l", „Je 200/250 g"
static QUANTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:(\d+)\s*x\s*)?((?:\d+(?:[.,]\d+)?)(?:\s*/\s*\d+(?:[.,]\d+)?)*)\s*(g|kg|ml|l|Stk)\b",
    )
    .unwrap()
});
/// Seitenkopf mit eigener Laufzeit: „Ab Mo. 20.7. bis Sa. 25.7."
///
/// Ein Wochenprospekt läuft Montag bis Samstag, einzelne Seiten aber kürzer
/// (Donnerstag-Angebote). Ohne diese Zeile bekämen sie die Laufzeit des
/// Gesamtprospekts und stünden drei Tage zu früh in der App.
static PAGE_VALIDITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Ab\s+\w{2}\.\s*(\d{1,2})\.(\d{1,2})\.\s*bis\s+\w{2}\.\s*(\d{1,2})\.(\d{1,2})\.")
        .unwrap()
});
/// Rabatt- und Streichpreisreste, die beim Clustern aus einer Nachbarkachel
/// in den Titel geraten („MILKA -42% 3.49 Pralinés"). Bewusst eng gefasst:
/// Der Rabatt braucht sein Minus und der Preis seine zwei Nachkommastellen,
/// damit „Type 405" oder „37,5 % vol" im Namen stehen bleiben.
static BADGE_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*(?:-\s?\d{1,2}\s*%|\b\d{1,3}[.,]\d{2}\*?|\bUVP\b|\bNormalpreis:?)").unwrap()
});
/// Titel, die nur das Gebinde beschreiben („5er-Pack", „8er", „3 Stk").
static PACK_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+\s*(er|Stk|x)\b").unwrap());
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-zÄÖÜäöüß][A-Za-zÄÖÜäöüß’'.\-]{2,}").unwrap());
/// Zeilen, ab denen die Beschreibung beginnt — davor steht Marke + Name.
static DESCRIPTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(Versch\.|Je\s|1 (kg|l|Stk) =|zzgl\.)").unwrap());
/// Aktionszeile mit Datumsvorspann: „Ab Do. 30.7. Deluxe-Woche",
/// „Erhältlich ab Do. 30.7. Für draußen". Das ist die Überschrift eines
/// Prospektblocks, kein Artikel.
///
/// Der Titel wird **verworfen, nicht gekürzt**: Schneidet man den Vorspann ab,
/// fällt der Rest mit einer bereits vorhandenen Zeile desselben Produkts
/// zusammen (die Angebots-ID ist Filiale + Titel + Preis) und überschreibt
/// sie. Am 2026-07-30 gemessen kostete das mehr echte Angebote, als die
/// Rettung einbrachte.
static LEAD_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i)(erhältlich\s+)?ab\s+(mo|di|mi|do|fr|sa|so)\.\s*\d{1,2}\.\d{1,2}\.?\s*")
        .unwrap()
});
/// Zeilen, die eine Eigenschaft beschreiben statt ein Produkt zu benennen.
/// Ein Produktname im Prospekt fängt nicht mit „Für" oder „Inkl." an.
static DESCRIPTIVE_LEAD: LazyLock<Regex> = LazyLock::new(|| {
    // Die Wortgrenze steht in jeder Alternative einzeln: nach „inkl." folgt ein
    // Punkt, und zwischen Punkt und Leerzeichen gibt es kein \b.
    Regex::new(
        r"^(?i)(für\b|inkl\.|passende[rs]?\b|bis zu\b|geeignet\b|helligkeit\b|multifunktional\b)",
    )
    .unwrap()
});
/// Reine Layout-Zeilen aus dem Prospekt: Aktionsdaten, Farbhinweise,
/// Eigenschaftstexte. Sie sehen wie ein Produktname aus und bestehen die
/// Geometrie-Prüfung, benennen aber kein Angebot.
///
/// **Bewusst getrennt von `is_plausible_title`:** Jene Funktion entscheidet
/// beim Clustern auch, welche Kachel Produkt und welche Preis ist. Wird sie
/// strenger, verschiebt sich die Zuordnung, und es fallen echte Angebote
/// heraus — am 2026-07-30 gemessen: 15 Stück, darunter Weizenmehl,
/// Plattpfirsiche und Cordon bleu. Diese Prüfung läuft deshalb erst auf dem
/// fertigen Titel, wenn die Paarung längst steht.
const LAYOUT_TEXT: &[&str] = &[
    "aktionszeitraum",
    "artikel mit",
    "entspricht",
    "qualität die schmeckt",
    "weitere farbe",
    "mit backindikator",
    "passende co2",
    "für glasstärke",
    "für drinnen",
    "für draußen",
    "für 6 personen",
    "sparen beim preis",
    "testsieger",
    // pdftotext zerlegt die Herkunfts-Grafik der Fleischtheke; „AUS UT HER
    // LAN" ist der Rest von „AUS DEUTSCHER LANDWIRTSCHAFT".
    "aus ut her lan",
];

/// Ist der Titel nur Prospekt-Layout?
///
/// Wird **vor** der Paarung angewandt, nicht danach: Die Zuordnung vergibt
/// jede Preis-Kachel nur einmal. Verwirft man die Layout-Kachel erst am
/// Ende, hat sie den Preis schon belegt, und das echte Produkt daneben geht
/// leer aus — am 2026-07-30 kostete das vier Lebensmittel (u. a. MÖVENPICK
/// Eis und PRIMADONNA Olivenöl). Vorher aussortiert, wird der Preis frei.
fn is_layout_text(title: &str) -> bool {
    let lower = title.to_lowercase();
    LAYOUT_TEXT.iter().any(|n| lower.contains(n))
        || DESCRIPTIVE_LEAD.is_match(title)
        || LEAD_DATE.is_match(title)
}

static SLUG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"aktionsprospekt-[A-Za-z0-9-]+").unwrap());
static SLUG_RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^aktionsprospekt-(\d{2})-(\d{2})-(\d{4})-(\d{2})-(\d{2})-(\d{4})-").unwrap()
});
/// Steuerzeichen, die pdftotext ungefiltert in sein XHTML schreibt und die
/// jeden XML-Parser zerlegen (2026-07-25: 7 Stück in einem Wochenprospekt).
static CONTROL_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x08\x0b\x0c\x0e-\x1f]").unwrap());

/// Vokabular der Preisauszeichnung. Ein Textblock, in dem nach Abzug dieser
/// Wörter nichts übrig bleibt, ist ein Badge und niemals ein Produktname.
const BADGE_WORDS: &[&str] = &[
    "uvp",
    "normalpreis",
    "letzter",
    "preis",
    "aktion",
    "mit",
    "lidl",
    "plus",
    "im",
    "aufsteller",
    "je",
    "ab",
    "neu",
    "versch",
    "sorten",
    "gekühlt",
    "tiefgefroren",
    "pfand",
    "zzgl",
    "kg",
    "g",
    "l",
    "ml",
    "stk",
    "pack",
    "liter",
    "und",
    "der",
    "die",
    "das",
    "in",
    "bis",
    "mo",
    "sa",
    "nur",
    "statt",
    "pro",
    "inkl",
    "ca",
    "x",
    "d",
    "vol",
    "stück",
];

/// Fließtext, der im Prospekt neben Angeboten steht und sonst als
/// Produktname durchginge (Fußnoten, Werbeclaims, Redaktionskürzel).
///
/// `lhz`/`dhz` sind Lidls interne Druckkennungen („LHZ – 30/2026 – BE/BY"),
/// die auf jeder Seite stehen und ohne diese Liste als Produkt in der App
/// landen würden.
const BOILERPLATE: &[&str] = &[
    "aus 110 branchen",
    "genereller hinweis",
    "dieser artikel kann",
    "shoppe auf lidl",
    "bewertungen",
    "stand ",
    "dhz",
    "lhz",
    "tiefpreis garantie",
    "abbildungen ähnlich",
    "solange der vorrat",
    "www.",
    "lidl.de",
    "herkunft-deutschland",
    "gilt für alle",
    "gilt für jeden",
    "dauerhaft im sortiment",
    "haltungsform",
    "standardpackung",
    "mehr fürs geld",
    "frischluftstall",
    "frische qualität",
    "unser einsatz",
    "jetzt neu",
    "nur in der",
    "auf alle",
    "weitere variante",
    "gültig vom",
    "angebot ausschließlich",
    "lidlplus",
    "coupon",
    "es gelten die",
    "personalisierten",
];

// -------------------------------------------------------------- Prospekt-JSON

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Flyer {
    #[serde(default, rename = "offerStartDate")]
    pub offer_start_date: Option<String>,
    #[serde(default, rename = "offerEndDate")]
    pub offer_end_date: Option<String>,
    /// Absatzregionen, für die diese Variante gilt. `"0"` ist der nationale
    /// Platzhalter und deckt keine echte Region ab.
    #[serde(default)]
    pub regions: Vec<FlyerRegion>,
    #[serde(default, rename = "pdfUrl")]
    pub pdf_url: Option<String>,
    #[serde(default)]
    pub pages: Vec<serde_json::Value>,
    /// Artikel, die zusätzlich im Onlineshop verkauft werden — Möbel,
    /// Großgeräte, Textilien. **Keine Lebensmittel** (2026-07-25: 138
    /// Einträge, null Food), deshalb taugt das Feld nicht als Hauptquelle.
    /// Es schließt aber genau die Lücke zu marktguru, die im PDF-Text fehlt:
    /// Von zehn marktguru-Angeboten ohne Treffer im Prospekttext waren zehn
    /// Onlineshop-Möbel. Als Bonus liefert es Bild und Kategorie, die der
    /// PDF-Weg nicht hat.
    #[serde(default)]
    pub products: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct FlyerRegion {
    #[serde(default)]
    pub code: String,
}

/// Prospekt-JSON auspacken. Die Nutzlast liegt unter `flyer`.
pub fn parse_flyer(raw: &serde_json::Value) -> Result<Flyer> {
    let node = raw.get("flyer").unwrap_or(raw);
    let flyer: Flyer =
        serde_json::from_value(node.clone()).context("Prospekt-JSON hat unerwartete Struktur")?;
    if flyer.pages.is_empty() {
        bail!("Prospekt ohne Seiten — Struktur geändert?");
    }
    Ok(flyer)
}

// ------------------------------------------------------------------- Slugs

/// Alle Prospekt-Slugs aus der Übersicht, in Reihenfolge des Auftretens.
///
/// Ein vollständiger Slug trägt beide Datumsangaben und einen Hash
/// (`aktionsprospekt-20-07-2026-25-07-2026-00d2c5`). Kürzere Treffer sind
/// Navigationsreste und fliegen über die Datumsprüfung raus.
pub fn parse_overview_slugs(html: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for m in SLUG.find_iter(html) {
        let slug = m.as_str().trim_end_matches('-').to_string();
        if slug_range(&slug).is_some() && !seen.contains(&slug) {
            seen.push(slug);
        }
    }
    seen
}

/// Gültigkeitszeitraum aus dem Slug (`DD-MM-YYYY-DD-MM-YYYY`).
pub fn slug_range(slug: &str) -> Option<(NaiveDate, NaiveDate)> {
    let c = SLUG_RANGE.captures(slug)?;
    let get = |i: usize| c.get(i).unwrap().as_str().parse::<u32>().ok();
    let from = NaiveDate::from_ymd_opt(get(3)? as i32, get(2)?, get(1)?)?;
    let to = NaiveDate::from_ymd_opt(get(6)? as i32, get(5)?, get(4)?)?;
    Some((from, to))
}

/// Alle Varianten der Woche, die heute gilt.
///
/// Die Übersicht führt mehrere Wochen gleichzeitig. Gewählt wird der
/// Zeitraum, der `today` enthält; sonst der nächste beginnende; sonst der
/// erste. Zurück kommen alle Varianten mit genau diesem Zeitraum — aus denen
/// sucht [`pick_region_variant`] anschließend die passende Absatzregion.
pub fn week_slugs(slugs: &[String], today: NaiveDate) -> Vec<String> {
    let ranges: Vec<_> = slugs
        .iter()
        .filter_map(|s| slug_range(s).map(|r| (s, r)))
        .collect();
    let chosen = ranges
        .iter()
        .find(|(_, (f, t))| *f <= today && today <= *t)
        .or_else(|| {
            ranges
                .iter()
                .filter(|(_, (f, _))| *f > today)
                .min_by_key(|(_, (f, _))| *f)
        })
        .or_else(|| ranges.first())
        .map(|(_, r)| *r);
    let Some(range) = chosen else {
        return Vec::new();
    };
    ranges
        .into_iter()
        .filter(|(_, r)| *r == range)
        .map(|(s, _)| s.clone())
        .collect()
}

/// Variante zur Absatzregion wählen.
///
/// Trifft keine Variante die AR, wird die erste ohne reinen `"0"`-Platzhalter
/// genommen — das ist immer noch ein echter Prospekt, nur nicht der regional
/// exakte. Das ist bewusst: lieber leicht unpassende Angebote als keine.
pub fn pick_region_variant(variants: &[(String, Vec<String>)], ar: Option<&str>) -> Option<String> {
    if let Some(ar) = ar
        && let Some((slug, _)) = variants
            .iter()
            .find(|(_, codes)| codes.iter().any(|c| c == ar))
    {
        return Some(slug.clone());
    }
    variants
        .iter()
        .find(|(_, codes)| !codes.is_empty() && codes.iter().any(|c| c != "0"))
        .or_else(|| variants.first())
        .map(|(slug, _)| slug.clone())
}

// --------------------------------------------------------- PDF -> Textinseln

/// Eine von poppler gruppierte Textinsel mit ihrem umschließenden Rechteck.
#[derive(Debug, Clone)]
struct Island {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    lines: Vec<String>,
}

impl Island {
    fn text(&self) -> String {
        self.lines.join(" ")
    }

    /// Abstand zweier Rechtecke; 0, wenn sie sich überlappen.
    fn gap(&self, other: &Island) -> f64 {
        let dx = (self.x0 - other.x1).max(other.x0 - self.x1).max(0.0);
        let dy = (self.y0 - other.y1).max(other.y0 - self.y1).max(0.0);
        (dx * dx + dy * dy).sqrt()
    }

    fn merge(&mut self, other: &Island) {
        self.x0 = self.x0.min(other.x0);
        self.y0 = self.y0.min(other.y0);
        self.x1 = self.x1.max(other.x1);
        self.y1 = self.y1.max(other.y1);
        self.lines.extend(other.lines.iter().cloned());
    }
}

/// Der Bildstreifen einer Kachel: das Rechteck über ihrem Text, in dem im
/// Prospekt das Produktfoto steht. Koordinaten in PDF-Punkten, Ursprung oben
/// links — dieselbe Ecke, aus der `pdftotext -bbox-layout` und `pdftoppm`
/// rechnen.
///
/// **Warum überhaupt gerechnet und nicht gelesen?** Der Prospekt trägt seine
/// Fotos nicht als benannte Objekte; die Textebene weiß nur, wo Wörter stehen.
/// Das Foto ist der Platz *darüber* — begrenzt nach oben durch die nächste
/// Kachel derselben Spalte. Nachgemessen an Seite 15 des Prospekts vom
/// 2026-07-20: Für 7 der 13 Sternpreis-Kacheln bleibt so ein Streifen von
/// 85-111 pt, und der enthält genau das Produktfoto (geprüft an Red Bull,
/// Ben's Original, Leerdammer).
#[derive(Debug, Clone, PartialEq)]
pub struct TileShot {
    /// Die `Offer`-ID, zu der dieser Streifen gehört.
    pub offer_id: String,
    /// 1-basierte Seitenzahl im PDF.
    pub page: usize,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Unter dieser Höhe ist der Streifen kein Foto, sondern der Zeilenabstand zur
/// Kachel darüber. 25 pt trennt an der Messung von Seite 15 sauber: die
/// verworfenen Streifen liegen bei 8-24 pt, die echten ab 30 pt.
const MIN_SHOT_PT: f64 = 25.0;

/// Höchstverhältnis Höhe zu Breite eines Bildstreifens. Siehe `photo_rect` —
/// der Deckel verhindert, dass ein Streifen die Kachel darüber verschluckt.
const MAX_SHOT_ASPECT: f64 = 1.3;

/// Auflösung, mit der die Streifen aus dem PDF gerastert werden. Der Prospekt
/// ist 467 x 794 pt groß und der Viewer zeigt ihn mit 1415 x 2400 px, also
/// rund 218 dpi — 150 dpi liegt darunter und reicht: `storage::downscale`
/// kappt ohnehin auf `MAX_IMAGE_EDGE` und kodiert nach WebP.
const RENDER_DPI: u32 = 150;

/// Der Streifen über `tile`, begrenzt durch die nächste Kachel darüber, die
/// sich mit ihr in der Waagerechten überschneidet. None, wenn zu flach.
///
/// `others` sind die umschließenden Rechtecke **aller** Kacheln der Seite,
/// inklusive `tile` selbst — die eigene fällt über den `y1 <= tile.y0`-Test
/// heraus.
fn photo_rect(tile: &Island, others: &[(f64, f64, f64, f64)]) -> Option<(f64, f64, f64, f64)> {
    let width = tile.x1 - tile.x0;
    let top = others
        .iter()
        .filter(|(_, _, _, y1)| *y1 <= tile.y0 + 1.0)
        // „Dieselbe Spalte" heißt: die Rechtecke überlappen sich in x deutlich.
        // Ohne diesen Test begrenzt die Nachbarspalte den Streifen.
        .filter(|(x0, _, x1, _)| {
            let overlap = x1.min(tile.x1) - x0.max(tile.x0);
            overlap > 0.2 * width.min(x1 - x0)
        })
        .map(|(_, _, _, y1)| *y1)
        .fold(0.0_f64, f64::max);

    // Ist der Platz über der Kachel **zu groß**, um ihr eigenes Foto zu sein,
    // wird gar nichts geschnitten.
    //
    // Der Streifen reicht bis zur nächsten Textkachel darüber. Sitzt dazwischen
    // eine bildfüllende Fläche ohne eigenen Text, gehört sie dem Nachbarn —
    // und der Schnitt zeigt dessen Produkt. Nachgemessen an einem echten Lauf
    // für 01219: „MILBONA Saure Sahne" bekam so das Zaziki-Foto der Kachel
    // darüber. Ein FALSCHES Bild ist schlimmer als keines; es behauptet etwas
    // über den Preis, der daneben steht.
    //
    // Deckeln statt verwerfen hilft hier nicht — der gekappte Streifen liegt
    // dann immer noch im fremden Foto. Also: nur schneiden, wenn der Platz
    // plausibel der eigene ist. 1,3 × Kachelbreite ist das Maß; Prospektfotos
    // sind ungefähr quadratisch bis leicht hochkant, und das Foto einer Kachel
    // sitzt unmittelbar über ihrem Text. Was das nicht besteht, bleibt ohne
    // Bild und zeigt in der App weiter sein Emoji.
    let height = tile.y0 - top;
    if height > MAX_SHOT_ASPECT * width {
        return None;
    }
    (height >= MIN_SHOT_PT).then_some((tile.x0, top, width, height))
}

/// `pdftotext -bbox-layout`-Ausgabe in Seiten aus Textinseln zerlegen.
///
/// Bewusst ein Zeilenparser statt eines XML-Parsers: Das Format ist streng
/// zeilenweise, und die Ausgabe ist regelmäßig *nicht* wohlgeformtes XML
/// (siehe [`CONTROL_CHARS`]) — ein echter Parser bräuchte die Bereinigung
/// trotzdem und brächte nur eine weitere Abhängigkeit mit.
fn parse_bbox_layout(xml: &str) -> Vec<Vec<Island>> {
    let cleaned = CONTROL_CHARS.replace_all(xml, "");
    let mut pages: Vec<Vec<Island>> = Vec::new();
    let mut island: Option<Island> = None;
    let mut line_words: Vec<String> = Vec::new();

    let flush_line = |island: &mut Option<Island>, words: &mut Vec<String>| {
        if !words.is_empty()
            && let Some(isl) = island.as_mut()
        {
            // Weiche Trennstriche trennen im Prospekt gesetzte Wörter, die
            // pdftotext als zwei Wörter ausgibt ("CELE\u{ad}" + "BRATIONS").
            // Ein Trennstrich am Zeilenende bleibt stehen: dort setzt
            // `title_of` beim Zusammenfügen der Zeilen wieder an.
            isl.lines.push(words.join(" ").replace("\u{00ad} ", ""));
        }
        words.clear();
    };

    for raw_line in cleaned.lines() {
        let line = raw_line.trim();
        if line.starts_with("<page ") {
            pages.push(Vec::new());
        } else if line.starts_with("<flow>") {
            island = Some(Island {
                x0: f64::MAX,
                y0: f64::MAX,
                x1: 0.0,
                y1: 0.0,
                lines: vec![],
            });
        } else if line.starts_with("</flow>") {
            flush_line(&mut island, &mut line_words);
            if let Some(isl) = island.take()
                && !isl.lines.is_empty()
                && isl.x0 <= isl.x1
                && let Some(page) = pages.last_mut()
            {
                page.push(isl);
            }
        } else if line.starts_with("<block ") {
            // Das Rechteck der Insel wächst über ihre Blöcke.
            if let (Some(isl), Some(b)) = (island.as_mut(), bbox_of(line)) {
                isl.x0 = isl.x0.min(b.0);
                isl.y0 = isl.y0.min(b.1);
                isl.x1 = isl.x1.max(b.2);
                isl.y1 = isl.y1.max(b.3);
            }
        } else if line.starts_with("<line ") {
            flush_line(&mut island, &mut line_words);
        } else if line.starts_with("<word ")
            && let Some(text) = word_text(line)
            && !text.is_empty()
        {
            line_words.push(text);
        }
    }
    pages
}

fn attr(line: &str, name: &str) -> Option<f64> {
    let key = format!("{name}=\"");
    let rest = line.split(&key).nth(1)?;
    rest.split('"').next()?.parse().ok()
}

fn bbox_of(line: &str) -> Option<(f64, f64, f64, f64)> {
    Some((
        attr(line, "xMin")?,
        attr(line, "yMin")?,
        attr(line, "xMax")?,
        attr(line, "yMax")?,
    ))
}

fn word_text(line: &str) -> Option<String> {
    let inner = line.split_once('>')?.1.split("</word>").next()?;
    Some(
        inner
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .trim()
            .to_string(),
    )
}

/// Textinseln zu Kacheln verschmelzen: alles, was näher als
/// [`CLUSTER_GAP_PT`] beieinander liegt, gehört zusammen (Union-Find).
fn cluster(islands: Vec<Island>) -> Vec<Island> {
    let n = islands.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..n {
        for j in (i + 1)..n {
            if islands[i].gap(&islands[j]) <= CLUSTER_GAP_PT {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    let mut groups: HashMap<usize, Island> = HashMap::new();
    // Nach Position sortiert zusammenfassen, damit Marke vor Beschreibung steht.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        (islands[a].y0, islands[a].x0)
            .partial_cmp(&(islands[b].y0, islands[b].x0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for i in order {
        let root = find(&mut parent, i);
        match groups.get_mut(&root) {
            Some(g) => g.merge(&islands[i]),
            None => {
                groups.insert(root, islands[i].clone());
            }
        }
    }
    groups.into_values().collect()
}

// ------------------------------------------------------------ Rollenerkennung

fn normalise(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// Wörter, die weder zum Preisvokabular gehören noch reine Zahlen sind.
fn name_words(text: &str) -> Vec<String> {
    WORD.find_iter(text)
        .map(|m| m.as_str().to_string())
        .filter(|w| !BADGE_WORDS.contains(&normalise(w).as_str()))
        .collect()
}

fn is_boilerplate(text: &str) -> bool {
    let lower = text.to_lowercase();
    BOILERPLATE.iter().any(|needle| lower.contains(needle))
}

/// Marke + Produktname: die Zeilen vor der ersten beschreibenden Zeile.
fn title_of(island: &Island) -> String {
    let mut head: Vec<String> = Vec::new();
    for line in &island.lines {
        if name_words(line).is_empty() {
            if head.is_empty() {
                continue;
            }
            break;
        }
        if DESCRIPTIVE.is_match(line) && !head.is_empty() {
            break;
        }
        head.push(line.split_whitespace().collect::<Vec<_>>().join(" "));
        if head.len() >= 3 {
            break;
        }
    }
    // Am Zeilenumbruch getrennte Wörter wieder zusammenziehen
    // ("Zahnfleisch- schutz" -> "Zahnfleisch-schutz").
    let joined = head
        .join(" ")
        .replace("\u{00ad} ", "")
        .replace('\u{00ad}', "")
        .replace("- ", "-");
    BADGE_TOKEN
        .replace_all(&joined, "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == '-')
        .to_string()
}

/// Taugt der Text als Produktname?
///
/// Der Prospekt ist voller Textinseln, die formal wie ein Name aussehen —
/// Mengenangaben („5er-Pack"), abgeschnittene Dekoschrift („aren"),
/// Werbezeilen. In einer Preisvergleichs-App ist so ein Eintrag schlimmer
/// als gar keiner, weil er in der Einkaufsliste auf nichts passt.
pub fn is_plausible_title(title: &str) -> bool {
    if title.len() < 3 || is_boilerplate(title) {
        return false;
    }
    // Reine Gebindeangaben: "5er-Pack", "8er", "3 Stk"
    if PACK_ONLY.is_match(title) {
        return false;
    }
    // Mindestens ein großgeschriebenes Wort — Marken und Produktnamen im
    // Prospekt sind immer so gesetzt, Dekofragmente meist nicht.
    name_words(title)
        .iter()
        .any(|w| w.chars().count() >= 3 && w.chars().next().is_some_and(char::is_uppercase))
}

pub fn parse_price(raw: &str) -> Option<f64> {
    let cleaned = raw.trim().replace(',', ".");
    // Lidl schreibt Beträge unter einem Euro als "-.68".
    let cleaned = if let Some(rest) = cleaned.strip_prefix('-') {
        format!("0{rest}")
    } else {
        cleaned
    };
    cleaned
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

/// Rechenprobe: Der Prospekt nennt Packungsgröße **und** Grundpreis, also
/// muss `Menge × Grundpreis ≈ Preis` gelten. Stimmt das nicht, ist die Kachel
/// falsch zusammengesetzt — und ein falscher Preis ist in einer
/// Preisvergleichs-App schlimmer als ein fehlendes Produkt.
///
/// `None`, wenn sich nichts nachrechnen lässt (dann wird die Kachel
/// übernommen — die Probe ist ein Filter gegen Fehlpaarungen, kein
/// Pflichtnachweis).
pub fn arithmetic_check(text: &str, price: f64) -> Option<bool> {
    let base = BASE_PRICE.captures(text)?;
    let base_unit = base.get(1)?.as_str();
    // "13.29/11.50": derselbe Artikel in zwei Größen, zwei gültige Grundpreise.
    let base_values: Vec<f64> = base
        .get(2)?
        .as_str()
        .split('/')
        .filter_map(parse_price)
        .collect();

    // Der Grundpreis selbst sieht aus wie eine Packungsgröße: In „1 kg = 2.76"
    // steckt „1 kg". Bliebe er stehen, ginge die Probe für *jeden* Preis auf,
    // der zufällig dem Grundpreis entspricht — die Kontrolle liefe leer.
    let without_base = BASE_PRICE.replace_all(text, " ");

    // "Je 200/250 g" — jede Kombination aus Größe und Grundpreis zählt.
    let mut quantities: Vec<(f64, &str)> = Vec::new();
    for c in QUANTITY.captures_iter(&without_base) {
        let mult: f64 = c
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1.0);
        let Some(unit) = c.get(3).map(|m| m.as_str()) else {
            continue;
        };
        for part in c.get(2).map(|m| m.as_str()).unwrap_or_default().split('/') {
            if let Ok(value) = part.trim().replace(',', ".").parse::<f64>() {
                quantities.push((mult * value, unit));
            }
        }
    }
    if quantities.is_empty() || base_values.is_empty() {
        return None;
    }

    let mut checked = false;
    for (amount, unit) in quantities {
        let (normalised, unit_name) = match unit {
            "g" => (amount / 1000.0, "kg"),
            "kg" => (amount, "kg"),
            "ml" => (amount / 1000.0, "l"),
            "l" => (amount, "l"),
            "Stk" => (amount, "Stk"),
            _ => continue,
        };
        if unit_name != base_unit {
            continue;
        }
        for base_value in &base_values {
            checked = true;
            let expected = normalised * base_value;
            if expected > 0.0 && ((expected - price).abs() / price) <= PRICE_TOLERANCE {
                return Some(true);
            }
        }
    }
    if checked { Some(false) } else { None }
}

/// Wo Sternpreise auf dem Weg zum Angebot verloren gehen.
///
/// Der Prospekt ist die Messlatte: Jeder Sternpreis ist ein Angebot, das in
/// der App landen sollte. Ohne diese Zahlen lässt sich nicht sagen, ob eine
/// Änderung am Extraktor etwas bringt — deshalb stehen sie in jedem Lauf.
/// `LIDL_PROSPEKT_DEBUG=1` zeigt jede Kachel, die kein Angebot geworden ist.
/// Ohne diese Ausgabe lässt sich nicht beurteilen, ob eine Regeländerung
/// wirklich hilft oder nur die Zahlen verschiebt.
fn debug_enabled() -> bool {
    std::env::var("LIDL_PROSPEKT_DEBUG").is_ok_and(|v| v == "1")
}

#[derive(Default)]
struct Stats {
    anchors: usize,
    no_partner: usize,
    implausible_price: usize,
    failed_arithmetic: usize,
    bad_title: usize,
}

impl Stats {
    fn report(&self, kept: usize) {
        println!(
            "  {kept} von {} Sternpreisen übernommen (ohne Partner {}, Preis unplausibel {}, \
             Rechenprobe {}, kein Titel {})",
            self.anchors,
            self.no_partner,
            self.implausible_price,
            self.failed_arithmetic,
            self.bad_title
        );
    }
}

/// Kacheln einer `pdftotext -bbox-layout`-Ausgabe in Angebote übersetzen.
///
/// Ablauf je Seite:
/// 1. Textinseln zu Kacheln clustern ([`cluster`]).
/// 2. Rollen verteilen: Preis-Kachel (Sternpreis), Produkt-Kachel (echte
///    Namenswörter), sonst Badge.
/// 3. Produkt und Preis global nach Abstand paaren — jede Kachel höchstens
///    einmal, kürzeste Abstände zuerst.
/// 4. Badges im Umkreis liefern Streichpreis und Lidl-Plus-Hinweis. Der
///    Rabatt („-42 %") wird bewusst nicht übernommen: `Offer` führt kein
///    Rabattfeld, die Auswertung rechnet ihn aus Preis und Streichpreis.
/// 5. Rechenprobe ([`arithmetic_check`]) verwirft Fehlpaarungen.
pub fn extract_offers(
    xml: &str,
    market_id: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
) -> Vec<Offer> {
    extract_offers_with_shots(xml, market_id, valid_from, valid_until).0
}

/// Wie [`extract_offers`], liefert aber zusätzlich die Bildstreifen der
/// Kacheln. Getrennt gehalten, weil das Rastern das PDF braucht und diese
/// Funktion rein auf der Textebene arbeitet — so bleibt sie gegen die Fixture
/// prüfbar.
pub fn extract_offers_with_shots(
    xml: &str,
    market_id: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
) -> (Vec<Offer>, Vec<TileShot>) {
    let mut offers = Vec::new();
    let mut shots = Vec::new();
    let mut stats = Stats::default();

    for (page_index, islands) in parse_bbox_layout(xml).into_iter().enumerate() {
        // Seiten mit eigenem Laufzeit-Kopf schlagen die Prospektdaten.
        let page_text: String = islands
            .iter()
            .map(Island::text)
            .collect::<Vec<_>>()
            .join(" ");
        let page_dates = page_validity(&page_text, valid_from);
        let (valid_from, valid_until) = match &page_dates {
            Some((from, until)) => (Some(from.as_str()), Some(until.as_str())),
            None => (valid_from, valid_until),
        };

        let tiles = cluster(islands);
        // Die Rechtecke aller Kacheln, bevor sie in Preise und Produkte
        // aufgeteilt werden — sie begrenzen die Bildstreifen nach oben.
        let page_rects: Vec<(f64, f64, f64, f64)> =
            tiles.iter().map(|t| (t.x0, t.y0, t.x1, t.y1)).collect();
        let mut prices = Vec::new();
        let mut products = Vec::new();
        let mut badges = Vec::new();
        for tile in tiles {
            if PRICE_STAR.is_match(&tile.text()) {
                prices.push(tile);
            } else if is_plausible_title(&title_of(&tile)) && !is_layout_text(&title_of(&tile)) {
                // Bewusst am *Titel* gemessen, nicht am ganzen Kacheltext:
                // Auf einer Fleisch-Kachel steht neben dem Namen auch
                // „Frischluftstall", und danach zu verwerfen hätte
                // „METZGERFRISCH Frisches Rinder-Hackfleisch" mitgerissen.
                products.push(tile);
            } else {
                badges.push(tile);
            }
        }

        // Kürzeste Abstände zuerst, jede Kachel nur einmal vergeben.
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for (i, product) in products.iter().enumerate() {
            for (j, price) in prices.iter().enumerate() {
                let gap = product.gap(price);
                if gap <= PAIR_GAP_PT {
                    pairs.push((gap, i, j));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut used_product = vec![false; products.len()];
        let mut used_price = vec![false; prices.len()];
        let mut matched: Vec<(usize, usize)> = Vec::new();

        for (_, i, j) in pairs {
            if used_product[i] || used_price[j] {
                continue;
            }
            used_product[i] = true;
            used_price[j] = true;
            matched.push((i, j));
        }

        // Manche Kacheln sind so eng gesetzt, dass Preis und Produkttext im
        // selben Cluster landen. Die haben oben keinen Partner bekommen,
        // tragen ihren Namen aber selbst — sie sind ihre eigene Kachel.
        let self_contained: Vec<usize> = (0..prices.len())
            .filter(|j| !used_price[*j])
            .filter(|j| is_plausible_title(&title_of(&prices[*j])))
            .collect();

        stats.anchors += prices
            .iter()
            .map(|t| PRICE_STAR.find_iter(&t.text()).count())
            .sum::<usize>();
        for j in (0..prices.len()).filter(|j| !used_price[*j] && !self_contained.contains(j)) {
            stats.no_partner += PRICE_STAR.find_iter(&prices[j].text()).count();
            if debug_enabled() {
                eprintln!(
                    "  [S{}] ohne Partner: {:?}",
                    page_index + 1,
                    prices[j].text().chars().take(110).collect::<String>()
                );
            }
        }

        for (i, j) in matched
            .into_iter()
            .chain(self_contained.iter().map(|j| (*j, *j)))
        {
            let price_tile = &prices[j];
            let product = if i == j { price_tile } else { &products[i] };

            // Eine Kachel kann mehrere Sternpreise tragen — „29.99* 2.99*"
            // ist ein Artikel mit zwei Größen, nicht ein Artikel. Jeder Stern
            // ist ein eigenes Angebot; der Untertitel unterscheidet sie.
            let tile_prices: Vec<f64> = PRICE_STAR
                .captures_iter(&price_tile.text())
                .filter_map(|c| parse_price(c.get(1)?.as_str()))
                .collect();
            let usable: Vec<f64> = tile_prices
                .iter()
                .copied()
                .filter(|p| (MIN_PRICE..=MAX_PRICE).contains(p))
                .collect();
            stats.implausible_price += tile_prices.len() - usable.len();

            let near: String = badges
                .iter()
                .filter(|b| b.gap(product) < BADGE_GAP_PT || b.gap(price_tile) < BADGE_GAP_PT)
                .map(|b| b.text())
                .collect::<Vec<_>>()
                .join(" ");
            let context = format!("{} {} {near}", product.text(), price_tile.text());

            let title = title_of(product);
            // "Mit Lidl Plus" steht mitunter in derselben Zeile wie der Name.
            let title = title.trim_end_matches("Mit Lidl Plus").trim().to_string();
            if !is_plausible_title(&title) || is_layout_text(&title) {
                stats.bad_title += usable.len();
                if debug_enabled() {
                    eprintln!(
                        "  [S{}] kein Titel aus: {:?}",
                        page_index + 1,
                        product.text().chars().take(110).collect::<String>()
                    );
                }
                continue;
            }

            // „Mit Lidl Plus" heißt: den Preis gibt es nur mit der Kundenkarte.
            // Er wandert mit, aber sichtbar gekennzeichnet — sonst rechnet die
            // Einkaufsplan-Karte mit einem Preis, den man an der Kasse ohne
            // App nicht bekommt.
            let lidl_plus = context.contains("Mit Lidl Plus");
            let subtitle = build_subtitle(product, &context, lidl_plus);

            // Die Rechenprobe wirkt als *Auswahl*, nicht als Veto.
            //
            // Trägt eine Kachel mehrere Sternpreise, sind zwei Produkte
            // zusammengeclustert und die Probe trennt sie zuverlässig (bei
            // ARLA Kaergarden bleibt 2.49 € und 3.99 € fliegt raus). Trägt
            // sie nur einen, ist der Preis über die Geometrie am Produkt
            // verankert — dann heißt eine gescheiterte Probe meistens, dass
            // der *Grundpreis* vom Nachbarn stammt, und ein Veto würde ein
            // korrektes Angebot wegwerfen (BELBAKE Speisestärke 0.59 €).
            let veto = usable.len() > 1;
            for price in usable.iter().copied() {
                if veto && arithmetic_check(&context, price) == Some(false) {
                    stats.failed_arithmetic += 1;
                    if debug_enabled() {
                        eprintln!(
                            "  [S{}] Rechenprobe {price:.2} €{}: {:?}",
                            page_index + 1,
                            if lidl_plus { " (Lidl Plus)" } else { "" },
                            context.chars().take(120).collect::<String>()
                        );
                    }
                    continue;
                }

                // Streichpreis nur übernehmen, wenn er über dem Angebotspreis
                // liegt — sonst hat die Kachel den Grundpreis eingefangen.
                let regular = REGULAR_PRICE
                    .captures(&context)
                    .and_then(|c| parse_price(c.get(1)?.as_str()))
                    .filter(|r| *r > price);

                let offer_id =
                    Offer::build_id(market_id, &format!("{title}_{price:.2}"), valid_from);

                // Der Bildstreifen der ganzen Kachel, also über Produkttext
                // UND Preis — die beiden stehen im Prospekt nebeneinander, das
                // Foto sitzt über beiden.
                let mut tile_rect = product.clone();
                tile_rect.merge(price_tile);
                if let Some((x, y, w, h)) = photo_rect(&tile_rect, &page_rects) {
                    shots.push(TileShot {
                        offer_id: offer_id.clone(),
                        page: page_index + 1,
                        x,
                        y,
                        w,
                        h,
                    });
                }

                offers.push(Offer {
                    // Preis gehört in die ID: Zwei Kacheln derselben Seite
                    // tragen oft denselben Namen ("Versch. Sorten") bei
                    // verschiedenen Preisen — ohne ihn verschluckt das Dedup
                    // das zweite Angebot.
                    id: offer_id,
                    market_id: market_id.to_string(),
                    title: title.clone(),
                    subtitle: subtitle.clone(),
                    overline: None,
                    price: Some(price),
                    regular_price: regular,
                    category: None,
                    nutri_score: None,
                    valid_from: valid_from.map(str::to_string),
                    valid_until: valid_until.map(str::to_string),
                    images: Vec::new(),
                    biozid: false,
                    flyer_page: Some(page_index as i64 + 1),
                });
            }
        }
    }

    let offers = dedup(offers);
    // Dedup wirft Kacheln weg; ihre Streifen dürfen nicht übrig bleiben, sonst
    // rastert der Lauf Bilder für Angebote, die es nicht mehr gibt.
    // Und je Angebot genau einer: Dieselbe ID entsteht mehrfach, wenn zwei
    // Kacheln denselben Namen zum selben Preis tragen. `dedup` führt sie zu
    // einem Angebot zusammen, ihre Streifen blieben sonst beide stehen und
    // würden dasselbe Bild zweimal rastern und hochladen.
    let kept: std::collections::HashSet<&str> = offers.iter().map(|o| o.id.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    shots.retain(|s| kept.contains(s.offer_id.as_str()) && seen.insert(s.offer_id.clone()));

    stats.report(offers.len());
    (offers, shots)
}

/// Untertitel aus Packungsgröße, Grundpreis und Lidl-Plus-Hinweis.
fn build_subtitle(product: &Island, context: &str, lidl_plus: bool) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = QUANTITY.captures(&product.text()) {
        let mult = c
            .get(1)
            .map(|m| format!("{} x ", m.as_str()))
            .unwrap_or_default();
        parts.push(format!(
            "{mult}{} {}",
            c.get(2)?.as_str(),
            c.get(3)?.as_str()
        ));
    }
    if let Some(c) = BASE_PRICE.captures(context) {
        parts.push(format!(
            "1 {} = {} €",
            c.get(1)?.as_str(),
            c.get(2)?.as_str()
        ));
    }
    if lidl_plus {
        parts.push("nur mit Lidl Plus".to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// Laufzeit einer einzelnen Seite, falls ihr Kopf eine eigene nennt.
///
/// Das Jahr steht nur in den Prospektdaten, nicht im Seitenkopf — deshalb
/// wird es von dort übernommen. Läuft der Prospekt über den Jahreswechsel,
/// gehört ein Enddatum vor dem Startdatum ins Folgejahr.
fn page_validity(page_text: &str, flyer_from: Option<&str>) -> Option<(String, String)> {
    let caps = PAGE_VALIDITY.captures(page_text)?;
    let num = |i: usize| caps.get(i)?.as_str().parse::<u32>().ok();
    let start = flyer_from.and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())?;
    let from = NaiveDate::from_ymd_opt(
        start.format("%Y").to_string().parse().ok()?,
        num(2)?,
        num(1)?,
    )?;
    let mut until = NaiveDate::from_ymd_opt(
        from.format("%Y").to_string().parse().ok()?,
        num(4)?,
        num(3)?,
    )?;
    if until < from {
        until = NaiveDate::from_ymd_opt(
            until.format("%Y").to_string().parse::<i32>().ok()? + 1,
            num(4)?,
            num(3)?,
        )?;
    }
    Some((
        from.format("%Y-%m-%d").to_string(),
        until.format("%Y-%m-%d").to_string(),
    ))
}

fn dedup(offers: Vec<Offer>) -> Vec<Offer> {
    let mut seen = std::collections::HashSet::new();
    offers
        .into_iter()
        .filter(|o| seen.insert(o.id.clone()))
        .collect()
}

// ------------------------------------------------------------------ Netzwerk

pub fn find_market(zip: &str) -> Result<Option<Market>> {
    Ok(store_finder::resolve(
        "Lidl",
        store_finder::lidl_branch(zip),
        Market::new("LIDL_DE", "Lidl Deutschland"),
    ))
}

fn fetch_text(url: &str, step: &str) -> Result<String> {
    util::polite_pause(url);
    util::blocking_client()?
        .get(url)
        .send()
        .with_context(|| util::ctx("Lidl", step, url))?
        .error_for_status()
        .with_context(|| util::ctx("Lidl", &format!("{step} (HTTP-Status)"), url))?
        .text()
        .with_context(|| util::ctx("Lidl", &format!("{step} lesen"), url))
}

fn fetch_flyer(slug: &str) -> Result<Flyer> {
    let url = format!("{FLYER_URL}{slug}");
    let raw: serde_json::Value = serde_json::from_str(&fetch_text(&url, "Prospekt-JSON")?)
        .with_context(|| util::ctx("Lidl", "Prospekt-JSON parsen", &url))?;
    parse_flyer(&raw)
}

/// Prospekt der Woche für die Absatzregion der PLZ.
fn resolve_flyer(zip: &str) -> Result<Flyer> {
    // Absatzregion zuerst — schlägt sie fehl, geht es ohne sie weiter und
    // wir nehmen eine überregionale Variante.
    let ar = match store_finder::lidl_region_code(zip) {
        Ok(code) => code,
        Err(e) => {
            eprintln!(
                "WARNUNG [Lidl] Absatzregion nicht ermittelbar ({e:#}) — nutze Standardvariante."
            );
            None
        }
    };

    let slugs = parse_overview_slugs(&fetch_text(OVERVIEW_URL, "Prospekt-Übersicht")?);
    if slugs.is_empty() {
        bail!("Keine Prospektvarianten in der Übersicht gefunden — Struktur geändert?");
    }
    let today = chrono::Utc::now()
        .with_timezone(&chrono_tz::Europe::Berlin)
        .date_naive();
    let week = week_slugs(&slugs, today);
    println!(
        "  {} Prospektvarianten diese Woche, Absatzregion {}",
        week.len(),
        ar.as_deref().unwrap_or("unbekannt")
    );

    // Die Varianten-JSONs sind klein; alle zu laden ist billiger als zu raten.
    let mut variants = Vec::new();
    let mut flyers = HashMap::new();
    for slug in &week {
        match fetch_flyer(slug) {
            Ok(flyer) => {
                let codes = flyer.regions.iter().map(|r| r.code.clone()).collect();
                variants.push((slug.clone(), codes));
                flyers.insert(slug.clone(), flyer);
            }
            Err(e) => eprintln!("WARNUNG [Lidl] Variante {slug} übersprungen: {e:#}"),
        }
    }
    let slug = pick_region_variant(&variants, ar.as_deref())
        .context("Keine brauchbare Prospektvariante gefunden")?;
    println!("  Variante {slug}");
    flyers
        .remove(&slug)
        .context("Gewählte Variante nicht geladen")
}

/// Prospekt-PDF in eine temporäre Datei laden.
///
/// pdftotext braucht eine Datei; bei ~83 MB ist der Umweg über stdin-Pipes
/// unnötig fragil. Der Aufrufer löscht die Datei wieder.
fn download_pdf(pdf_url: &str) -> Result<std::path::PathBuf> {
    util::polite_pause(pdf_url);
    let bytes = util::blocking_client()?
        .get(pdf_url)
        .send()
        .with_context(|| util::ctx("Lidl", "Prospekt-PDF", pdf_url))?
        .error_for_status()
        .with_context(|| util::ctx("Lidl", "Prospekt-PDF (HTTP-Status)", pdf_url))?
        .bytes()
        .with_context(|| util::ctx("Lidl", "Prospekt-PDF lesen", pdf_url))?;
    println!("  PDF geladen ({:.0} MB)", bytes.len() as f64 / 1_048_576.0);

    let path = std::env::temp_dir().join(format!("lechariot-lidl-{}.pdf", std::process::id()));
    std::fs::write(&path, &bytes)
        .context("Prospekt-PDF konnte nicht zwischengespeichert werden")?;
    Ok(path)
}

/// `pdftotext` mit dem gewünschten Modus laufen lassen.
fn run_pdftotext(path: &std::path::Path, mode: &str) -> Result<String> {
    let output = Command::new("pdftotext")
        .arg(mode)
        .arg(path)
        .arg("-")
        .output()
        .context("pdftotext nicht gefunden — poppler-utils wird für den Lidl-Prospekt benötigt")?;
    if !output.status.success() {
        bail!(
            "pdftotext fehlgeschlagen: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// PDF laden und durch `pdftotext -bbox-layout` schicken (Wortkoordinaten für
/// die Kachelbildung).
/// Verzeichnis der gerasterten Kachelbilder.
///
/// Bewusst **ohne** Prozess-ID im Namen, anders als beim PDF: Der Dateiname
/// wird zur Quell-URL des Bildes, und aus ihr leitet `storage::object_path`
/// den Objektpfad im Bucket ab. Wäre er je Lauf verschieden, lüde jede Nacht
/// dieselben Bilder unter neuen Namen hoch und der DB-Cache träfe nie.
pub fn crop_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("lechariot-lidl-crops")
}

/// Die Bildstreifen aus dem PDF rastern und als PNG ablegen; liefert je
/// Angebots-ID die `file://`-URL der Datei.
///
/// `pdftoppm` schneidet selbst zu (`-x -y -W -H`), es braucht also keine
/// Bildbibliothek in diesem Modul. Die Umwandlung nach WebP macht später
/// `storage::downscale` auf demselben Weg wie bei jedem Händlerbild.
///
/// Ein Fehlschlag einzelner Streifen ist kein Abbruchgrund: Dann bleibt das
/// Angebot ohne Bild und die App zeigt ihr Emoji — genau wie heute.
fn render_shots(pdf: &std::path::Path, shots: &[TileShot]) -> HashMap<String, String> {
    if shots.is_empty() {
        return HashMap::new();
    }
    let dir = crop_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("WARNUNG [Lidl] Kachelbilder nicht ablegbar ({e}) — Prospekt ohne Bilder.");
        return HashMap::new();
    }

    let scale = RENDER_DPI as f64 / 72.0;
    let mut out = HashMap::new();
    let mut failed = 0usize;
    for shot in shots {
        let target = dir.join(format!("{}.png", shot.offer_id));
        // `-singlefile` schreibt genau `<prefix>.png` statt `<prefix>-<seite>.png`.
        let prefix = dir.join(&shot.offer_id);
        let status = Command::new("pdftoppm")
            .args(["-png", "-singlefile", "-r"])
            .arg(RENDER_DPI.to_string())
            .args(["-f", &shot.page.to_string(), "-l", &shot.page.to_string()])
            .args(["-x", &((shot.x * scale) as i64).max(0).to_string()])
            .args(["-y", &((shot.y * scale) as i64).max(0).to_string()])
            .args(["-W", &((shot.w * scale) as i64).max(1).to_string()])
            .args(["-H", &((shot.h * scale) as i64).max(1).to_string()])
            .arg(pdf)
            .arg(&prefix)
            .status();
        match status {
            Ok(s) if s.success() && target.exists() => {
                out.insert(shot.offer_id.clone(), format!("file://{}", target.display()));
            }
            _ => failed += 1,
        }
    }
    println!(
        "  {} Kachelbilder gerastert{}",
        out.len(),
        if failed > 0 {
            format!(", {failed} fehlgeschlagen")
        } else {
            String::new()
        }
    );
    out
}

/// Onlineshop-Artikel des Prospekts als Angebote.
///
/// Diese Einträge stehen sauber strukturiert im Prospekt-JSON und müssen
/// nicht aus der PDF gelesen werden — Titel, Preis, Bild und Kategorie
/// kommen fertig. Beide Extraktoren hängen sie an ihr Ergebnis an.
pub fn products_as_offers(
    flyer: &Flyer,
    market_id: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
) -> Vec<Offer> {
    let mut offers = Vec::new();
    for product in flyer.products.values() {
        let Some(title) = product
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| t.len() >= 3)
        else {
            continue;
        };
        // `price` kommt als String ("9.99").
        let Some(price) = product
            .get("price")
            .and_then(|v| v.as_str())
            .and_then(parse_price)
        else {
            continue;
        };
        let images = product
            .get("image")
            .and_then(|v| v.as_str())
            .filter(|u| u.starts_with("https://"))
            .map(|u| vec![u.to_string()])
            .unwrap_or_default();
        // "Kategorien/Wohnen & Einrichten/Heimtextilien/..." — das letzte
        // Glied ist das aussagekräftigste.
        let category = product
            .get("categoryPrimary")
            .and_then(|v| v.as_str())
            .and_then(|path| path.rsplit('/').next())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string);

        offers.push(Offer {
            id: Offer::build_id(market_id, &format!("{title}_{price:.2}"), valid_from),
            market_id: market_id.to_string(),
            title: title.to_string(),
            subtitle: Some("Onlineshop".to_string()),
            overline: None,
            price: Some(price),
            regular_price: None,
            category,
            nutri_score: None,
            valid_from: valid_from.map(str::to_string),
            valid_until: valid_until.map(str::to_string),
            images,
            biozid: false,
            flyer_page: None,
        });
    }
    offers
}

/// Prospekt der Woche plus seinen Text, seitenweise in Lesereihenfolge.
///
/// `-layout` statt `-bbox-layout`, weil hier
/// kein Programm Koordinaten braucht, sondern ein Sprachmodell die Seite
/// lesen soll — die Spaltenausrichtung bleibt erhalten, die Wortkoordinaten
/// entfallen. Genutzt von [`crate::scrapers::lidl_llm`].
pub fn fetch_layout_pages(zip: &str) -> Result<(Flyer, Vec<String>)> {
    let flyer = resolve_flyer(zip)?;
    let pdf_url = flyer.pdf_url.clone().context("Prospekt ohne pdfUrl")?;
    let path = download_pdf(&pdf_url)?;
    let result = run_pdftotext(&path, "-layout");
    let _ = std::fs::remove_file(&path);
    // pdftotext trennt Seiten mit einem Seitenvorschub.
    let pages = result?
        .split('\u{000c}')
        .map(|page| page.trim_end().to_string())
        .filter(|page| !page.trim().is_empty())
        .collect();
    Ok((flyer, pages))
}

pub fn fetch_offers(market: &Market, zip: &str) -> Result<Vec<Offer>> {
    let flyer = resolve_flyer(zip)?;
    let pdf_url = flyer.pdf_url.as_deref().context("Prospekt ohne pdfUrl")?;
    println!(
        "  Gültig {} bis {}",
        flyer.offer_start_date.as_deref().unwrap_or("?"),
        flyer.offer_end_date.as_deref().unwrap_or("?")
    );
    // Das PDF bleibt hier liegen, bis auch die Kachelbilder geschnitten sind —
    // es ein zweites Mal zu laden wären 83 MB für nichts.
    let pdf_path = download_pdf(pdf_url)?;
    let xml = run_pdftotext(&pdf_path, "-bbox-layout");
    let xml = match xml {
        Ok(xml) => xml,
        Err(e) => {
            let _ = std::fs::remove_file(&pdf_path);
            return Err(e);
        }
    };

    let (mut offers, shots) = extract_offers_with_shots(
        &xml,
        &market.id,
        flyer.offer_start_date.as_deref(),
        flyer.offer_end_date.as_deref(),
    );
    if offers.is_empty() {
        let _ = std::fs::remove_file(&pdf_path);
        bail!("Prospekt gelesen, aber keine Angebote extrahiert — Layout geändert?");
    }

    let crops = render_shots(&pdf_path, &shots);
    let _ = std::fs::remove_file(&pdf_path);
    for offer in &mut offers {
        if let Some(url) = crops.get(&offer.id) {
            offer.images = vec![url.clone()];
        }
    }
    offers.extend(merge_products(&flyer, &market.id, &offers));
    Ok(offers)
}

/// Onlineshop-Artikel ergänzen, ohne bereits gelesene doppelt zu zählen.
pub fn merge_products(flyer: &Flyer, market_id: &str, existing: &[Offer]) -> Vec<Offer> {
    let seen: std::collections::HashSet<&str> = existing.iter().map(|o| o.id.as_str()).collect();
    let extra: Vec<Offer> = products_as_offers(
        flyer,
        market_id,
        flyer.offer_start_date.as_deref(),
        flyer.offer_end_date.as_deref(),
    )
    .into_iter()
    .filter(|o| !seen.contains(o.id.as_str()))
    .collect();
    if !extra.is_empty() {
        println!(
            "  {} Onlineshop-Artikel aus dem Prospekt-JSON ergänzt",
            extra.len()
        );
    }
    extra
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aus dem 11-Regionen-Audit (2026-07-30): 538 Zeilen eines
    /// Wochenprospekt-Satzes waren keine Angebote, sondern Layout-Text —
    /// Aktionsdaten, Farbhinweise, Eigenschaftszeilen. In einer
    /// Preisvergleichs-App ist so ein Eintrag schlimmer als gar keiner.
    #[test]
    fn layout_zeilen_sind_keine_produkte() {
        for zeile in [
            "Aktionszeitraum",
            "Artikel mit",
            "Entspricht",
            "Qualität die schmeckt lohnt sich",
            "Weitere Farbe: Schwarz",
            "Weitere Farbe: Weiß PARKSIDE",
            "Erhältlich ab Do. 30.7. Für draußen",
            "Für Glasstärke von 4–6 mm",
            "Für 6 Personen 62-teilig",
            "Für drinnen/ draußen",
            "Inkl. 2-in-1-Bürste und Fugendüse",
            "Mit Backindikator",
            "Helligkeit per Touchdimmer in 4 Stufen regelbar",
            "Multifunktional – bis zu 19 Aufnahmemöglichkeiten",
            "Passende CO2-Zylinder dauerhaft in der Filiale",
            "AUS UT HER LAN",
        ] {
            assert!(is_layout_text(zeile), "sollte Layout sein: {zeile}");
        }
    }

    /// Aktionsüberschriften mit Datum sind keine Artikel. Sie werden
    /// verworfen und **nicht** gekürzt — sonst fiele der Rest mit einer
    /// vorhandenen Zeile desselben Produkts zusammen und überschriebe sie.
    #[test]
    fn datumszeilen_sind_keine_produkte() {
        for zeile in [
            "Ab Do. 30.7",
            "Ab Do. 30.7. Deluxe-Woche",
            "Ab Mi. 29.7. DELUXE Olivenöl",
            "Erhältlich ab Do. 30.7. Für draußen",
        ] {
            assert!(is_layout_text(zeile), "sollte Layout sein: {zeile}");
        }
        assert!(!is_layout_text("BARILLA Pasta"));
    }

    /// Gegenprobe: echte Produkte aus demselben Prospekt bleiben gültig.
    #[test]
    fn echte_produkte_ueberstehen_die_neuen_filter() {
        for zeile in [
            "BARILLA Pasta",
            "ALESTO Pistazien XXL",
            "OCEAN SEA Lachsfilet-portionen XXL",
            "Gartenhortensie",
            "MÜHLENHOF Frisches Hähnchen-Innenbrustfilet",
            "Couronne Feigen-Walnuss",
        ] {
            assert!(is_plausible_title(zeile) && !is_layout_text(zeile), "sollte ein Produkt sein: {zeile}");
        }
    }

    #[test]
    fn parse_price_reads_the_lidl_notation_for_amounts_below_one_euro() {
        assert_eq!(parse_price("2,49"), Some(2.49));
        assert_eq!(parse_price("2.49"), Some(2.49));
        assert_eq!(parse_price("-.68"), Some(0.68));
        assert_eq!(parse_price("0.00"), None);
    }

    #[test]
    fn the_arithmetic_check_rejects_a_price_that_contradicts_the_base_price() {
        // ARLA Kaergarden: 400 g zu 6.23 €/kg sind 2.49 €, nicht 3.99 €.
        let text = "ARLA Kaergarden XXL Versch. Sorten. Je 400 g 1 kg = 6.23";
        assert_eq!(arithmetic_check(text, 2.49), Some(true));
        assert_eq!(arithmetic_check(text, 3.99), Some(false));
    }

    #[test]
    fn the_arithmetic_check_accepts_any_of_several_pack_sizes() {
        // "Je 200/250 g" mit zwei Grundpreisen: beide Lesarten sind gültig.
        let text = "MULINO BIANCO Gebäck Versch. Sorten. Je 200/250 g 1 kg = 11.10";
        assert_eq!(arithmetic_check(text, 2.22), Some(true));
    }

    #[test]
    fn the_arithmetic_check_stays_silent_when_nothing_is_computable() {
        assert_eq!(
            arithmetic_check("GRANT'S Blended Scotch Whisky", 13.99),
            None
        );
    }

    #[test]
    fn week_slugs_groups_every_variant_of_the_current_week() {
        let slugs: Vec<String> = [
            "aktionsprospekt-20-07-2026-25-07-2026-00d2c5",
            "aktionsprospekt-20-07-2026-25-07-2026-1a7366",
            "aktionsprospekt-27-07-2026-01-08-2026-0125df",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let today = NaiveDate::from_ymd_opt(2026, 7, 22).unwrap();
        assert_eq!(week_slugs(&slugs, today).len(), 2);
    }

    #[test]
    fn week_slugs_falls_forward_to_the_next_flyer_when_none_is_running() {
        let slugs = vec!["aktionsprospekt-27-07-2026-01-08-2026-0125df".to_string()];
        let today = NaiveDate::from_ymd_opt(2026, 7, 22).unwrap();
        assert_eq!(week_slugs(&slugs, today).len(), 1);
    }

    #[test]
    fn the_region_variant_matches_the_absatzregion_and_never_the_placeholder() {
        let variants = vec![
            ("national".to_string(), vec!["0".to_string()]),
            (
                "dresden".to_string(),
                vec!["19".to_string(), "20".to_string()],
            ),
            (
                "koeln".to_string(),
                vec!["13".to_string(), "42".to_string()],
            ),
        ];
        assert_eq!(
            pick_region_variant(&variants, Some("20")).as_deref(),
            Some("dresden")
        );
        assert_eq!(
            pick_region_variant(&variants, Some("42")).as_deref(),
            Some("koeln")
        );
        // Unbekannte AR: irgendeine echte Variante, aber nicht der Platzhalter.
        assert_eq!(
            pick_region_variant(&variants, Some("999")).as_deref(),
            Some("dresden")
        );
    }

    #[test]
    fn overview_slugs_are_deduped_and_incomplete_ones_dropped() {
        let html = r#"<a href="/c/aktionsprospekt-20-07-2026-25-07-2026-00d2c5">x</a>
                      <a href="/c/aktionsprospekt-20-07-2026-25-07-2026-00d2c5">y</a>
                      <a href="/c/aktionsprospekt-teaser">z</a>"#;
        assert_eq!(
            parse_overview_slugs(html),
            vec!["aktionsprospekt-20-07-2026-25-07-2026-00d2c5"]
        );
    }

    /// Live-Test gegen lidl.com und endpoints.leaflets.schwarz.
    #[test]
    #[ignore = "Live-Test gegen lidl.com (lädt ~83 MB PDF)"]
    fn lidl_prospekt_live() {
        let market = Market::new("LIDL_TEST", "Lidl Test");
        let offers = fetch_offers(&market, "01219").unwrap();
        assert!(offers.len() >= 50, "nur {} Angebote", offers.len());
        assert!(offers.iter().all(|o| o.price.is_some()));
        assert!(offers.iter().any(|o| o.regular_price.is_some()));
    }

    fn island(x0: f64, y0: f64, x1: f64, y1: f64) -> Island {
        Island { x0, y0, x1, y1, lines: vec!["x".into()] }
    }

    /// Der Schnitt darf nur dort entstehen, wo der Platz über der Kachel
    /// plausibel ihr eigenes Foto ist.
    #[test]
    fn photo_rect_skips_bands_too_tall_to_be_this_tiles_own_photo() {
        let tile = island(100.0, 200.0, 200.0, 230.0); // 100 pt breit
        let above = |y1: f64| vec![(100.0, 0.0, 200.0, y1), (100.0, 200.0, 200.0, 230.0)];

        // Streifen von 90 pt bei 100 pt Breite: plausibel, wird geschnitten.
        let (x, y, w, h) = photo_rect(&tile, &above(110.0)).expect("sollte schneiden");
        assert_eq!((x, y, w), (100.0, 110.0, 100.0));
        assert!((h - 90.0).abs() < 0.001);

        // 180 pt bei 100 pt Breite: das ist die Nachbarkachel, kein eigenes
        // Foto — lieber gar kein Bild als das falsche.
        assert!(photo_rect(&tile, &above(20.0)).is_none());

        // Und nichts Flaches: 10 pt ist der Zeilenabstand, kein Foto.
        assert!(photo_rect(&tile, &above(190.0)).is_none());
    }

    /// Die Nachbarspalte begrenzt den Streifen nicht — sonst bekäme jede
    /// Kachel die Höhe ihres Nachbarn.
    #[test]
    fn photo_rect_ignores_tiles_in_other_columns() {
        let tile = island(100.0, 200.0, 200.0, 230.0);
        // Kachel weit rechts, ohne nennenswerte Überlappung in x.
        let others = vec![(400.0, 150.0, 500.0, 190.0), (100.0, 0.0, 200.0, 120.0)];
        let (_, y, _, h) = photo_rect(&tile, &others).expect("sollte schneiden");
        assert_eq!(y, 120.0, "die fremde Spalte hat den Streifen begrenzt");
        assert!((h - 80.0).abs() < 0.001);
    }

}
