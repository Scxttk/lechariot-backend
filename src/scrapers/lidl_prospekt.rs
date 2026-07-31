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

use std::collections::{HashMap, HashSet};
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
/// Menge plus Einheit und sonst nichts — die Zeile unter dem Produkt, nicht
/// das Produkt. Gemessen am Lauf für 01219 vom 2026-07-31: „10 Paar" stand
/// dreimal als eigenes Angebot in der Liste.
static QUANTITY_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i)\d+\s*[-\s]?\s*(paar|stück|stk\.?|teilig|er[-\s]?pack)\.?$").unwrap()
});

/// Bindewörter, die am Ende eines Titels hängen bleiben, wenn die Kachel den
/// Rest der Zeile nicht mehr eingefangen hat: „ESMARA MEN Slips/Boxer
/// Baumwolle und", „TRONIC Knopfzellen Multipack mit". Das Produkt ist
/// erkennbar, nur der Satz ist abgeschnitten — also kürzen statt verwerfen.
static DANGLING_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[\s,]+(und|oder|mit|für|in|aus|von|zum|zur|sowie|inkl\.?)$").unwrap()
});

/// Banner, die der Prospekt VOR den Produktnamen klebt: „Erhältlich ab
/// Do. 30.7.", „Entspricht 3.33/Stk.", „Tiefpreis Garantie", „Weitere Farbe:
/// Weiß". Jedes davon erkennt der Extraktor für sich als Layouttext — vor
/// einem Produktnamen riss es bisher den ganzen Titel mit (gemessen am
/// Prospekt vom 27.07.: 14 echte Produkte, u. a. PARKSIDE Winkelschleifer,
/// LIVARNO Steppbett, WAGNER Steinofen Pizza, GELATELLI Eis).
///
/// Das Spiegelbild von [`DANGLING_TAIL`], nur am Anfang: kürzen statt
/// verwerfen, denn das Produkt steht da — nur das Banner klebt davor.
///
/// Jede Alternative endet vor einem Leerzeichen oder dem Titelende
/// (`\s+|$` im Aufruf), damit „Aktionszeitraum" nie als „Aktion" + Rest
/// gelesen wird.
static LEADING_BANNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)^(?:
            # Aktionsdatum, auch mit Bis-Teil: "Ab Do. 30.7. bis Sa. 1.8."
            (?:erhältlich\s+)?ab\s+(?:mo|di|mi|do|fr|sa|so)\.\s*\d{1,2}\.\d{1,2}\.?
                (?:\s*bis\s+(?:mo|di|mi|do|fr|sa|so)\.\s*\d{1,2}\.\d{1,2}\.?)?
          | # "Entspricht 3.33/Stk." - der Betrag kann schon von BADGE_TOKEN
            # gefressen sein, deshalb optional.
            entspricht\s*[-\d.,]*\s*/\s*stk\.?
          | # "Tiefpreis Garantie", auch auseinandergerissen: pdftotext
            # verschachtelt die Plakette mit der Rubrik dazwischen
            # ("Tiefpreis Wohnen & Einrichtung Garantie LIVARNO ...").
            tiefpreis(?:\s+garantie)? | garantie
          | wohnen\s*&\s*einrichtung
          | # Plaketten der Frischetheke und des Dauersortiments. Sie stehen
            # in `BOILERPLATE` — dort wirken sie aber auf den *ganzen* Titel,
            # und weil sie über dem Produktnamen gedruckt sind, rissen sie ihn
            # mit (gemessen am Prospekt vom 27.07.: METZGERFRISCH Rinder-
            # Minutensteaks, GRILLMEISTER Puten-Grillies, MILBONA Creme
            # Joghurt). Vorn abgeschnitten bleibt der Name stehen; steht sonst
            # nichts da, greift `BOILERPLATE` weiter wie bisher.
            frischluftstall | dauerhaft\s+im\s+sortiment
          | # Gebindeplakette über dem Namen: "42er- Pack … TEMPO Taschen-
            # tücher", "9er-/8er-Netz … BABYBEL". `PACK_ONLY` verwirft solche
            # Titel schon heute komplett, es geht hier also kein bestehendes
            # Angebot verloren — nur das Produkt dahinter kommt dazu. Bleibt
            # nach dem Schnitt nichts übrig ("5er-Pack" allein), greift
            # `PACK_ONLY` unverändert.
            \d+\s*er[-\s]*(?:/\s*\d+\s*er[-\s]*)*(?:pack|netz)?
          | weitere\s+farben?:?\s+\S+
          | für\s+(?:drinnen|draußen)(?:\s*/\s*(?:drinnen|draußen))?
          | mit\s+lidl\s+plus
          | gültig\s+vom\s+[\d.,\s\x{2013}-]+
          | frische\s+qualität\s+lohnt\s+sich\.?
          | (?:xxl\s+)?mehr\s+fürs\s+geld!?
          | # Einzelner Großbuchstabe ohne Punkt: Schriftsplitter der
            # Plaketten-Grafik ("R ESMARA MEN Sneaker"). Marken mit Punkt
            # (f.a.n., s.Oliver) tragen ihren Punkt und bleiben stehen.
            [A-ZÄÖÜ]
        )(?:\s+|$)"#,
    )
    .unwrap()
});

/// Dieselben Flächen-Banner am Ende des Titels: „Grünmix im Korb Für
/// drinnen Für drinnen". Bewusst nur die Drinnen/Draußen-Plakette — alles
/// andere am Titelende ist entweder schon `DANGLING_TAIL` oder gehört zum
/// Namen.
static TRAILING_BANNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|\s+)für\s+(?:drinnen|draußen)(?:\s*/\s*(?:drinnen|draußen))?$").unwrap()
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

// ------------------------------------------------------- Eingebettete Bilder
//
// Der Schnitt (`photo_rect`) rastert den Platz ÜBER einer Kachel. Wo dieser
// Platz zu hoch ist, um plausibel das eigene Foto zu sein, verweigert er —
// lieber kein Bild als ein fremdes. Genau dort hilft der zweite Weg: Die
// Produktfotos liegen im Prospekt-PDF als eigene XObjekte, nicht flach in ein
// Seitenbild gerechnet, und `pdftohtml -xml` legt sie als Dateien ab und
// nennt ihre Rechtecke.
//
// Beide Wege ersetzen einander nicht, sie ergänzen sich: Auf dem Prospekt vom
// 20.07. schneidet der eine 160 von 227 Kacheln, und die eingebetteten Bilder
// füllen 31 der 67 Lücken.

/// Ein eingebettetes Bild des PDFs, umgerechnet in PDF-Punkte.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedImage {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    /// Dateiname, den `pdftohtml` neben das XML geschrieben hat.
    pub file: String,
}

impl EmbeddedImage {
    fn width(&self) -> f64 {
        self.x1 - self.x0
    }
    fn height(&self) -> f64 {
        self.y1 - self.y0
    }
}

/// Seitenmaße in PDF-Punkten, aus der `pdftotext -bbox-layout`-Ausgabe.
///
/// Sie sind der Maßstab, mit dem die Bildrechtecke von `pdftohtml`
/// zurückgerechnet werden — siehe [`parse_embedded_images`].
fn page_sizes(bbox_xml: &str) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for line in bbox_xml.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("<page ") {
            let w = attr_f64(rest, "width");
            let h = attr_f64(rest, "height");
            out.push((w.unwrap_or(0.0), h.unwrap_or(0.0)));
        }
    }
    out
}

/// Ein Zahlen-Attribut aus einem Tag-Rest lesen (`width="467.72"`).
fn attr_f64(rest: &str, name: &str) -> Option<f64> {
    let needle = format!("{name}=\"");
    let start = rest.find(&needle)? + needle.len();
    let end = rest[start..].find('"')? + start;
    rest[start..end].parse().ok()
}

/// Die Bilder je Seite aus `pdftohtml -xml`, in PDF-Punkte umgerechnet.
///
/// **Der Maßstab ist das Seitenrechteck, nicht ein Textabgleich.** `pdftohtml`
/// rastert mit festem Zoom, der Faktor ist also schlicht das Verhältnis der
/// Seitenmaße und der Versatz null. Das ist nachgemessen, nicht angenommen:
/// Auf den 33 Seiten des Prospekts vom 20.07., auf denen sich zusätzlich eine
/// Ausgleichsgerade über eindeutige Textzeilen legen ließ, weichen beide Wege
/// um **höchstens 0,72 pt** voneinander ab — weniger als eine Zeilenhöhe, und
/// die Zuordnung unten arbeitet mit Toleranzen von Dutzenden Punkten. Der
/// Textabgleich scheiterte dafür auf 13 Seiten (zu wenige eindeutige Zeilen);
/// das Seitenrechteck gibt es immer.
///
/// Seiten ohne bekanntes PDF-Maß liefern nichts — ohne Maßstab wäre jedes
/// Rechteck geraten.
fn parse_embedded_images(xml: &str, pdf_pages: &[(f64, f64)]) -> Vec<Vec<EmbeddedImage>> {
    let mut pages: Vec<Vec<EmbeddedImage>> = Vec::new();
    let mut scale: Option<(f64, f64)> = None;

    for line in xml.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("<page number=") {
            let idx = pages.len();
            pages.push(Vec::new());
            let hw = attr_f64(rest, "width").unwrap_or(0.0);
            let hh = attr_f64(rest, "height").unwrap_or(0.0);
            scale = match pdf_pages.get(idx) {
                Some(&(pw, ph)) if hw > 0.0 && hh > 0.0 && pw > 0.0 && ph > 0.0 => {
                    Some((pw / hw, ph / hh))
                }
                _ => None,
            };
        } else if let Some(rest) = line.strip_prefix("<image ") {
            let (Some((sx, sy)), Some(page)) = (scale, pages.last_mut()) else { continue };
            let (Some(l), Some(t), Some(w), Some(h)) = (
                attr_f64(rest, "left"),
                attr_f64(rest, "top"),
                attr_f64(rest, "width"),
                attr_f64(rest, "height"),
            ) else {
                continue;
            };
            let needle = "src=\"";
            let Some(s) = rest.find(needle).map(|i| i + needle.len()) else { continue };
            let Some(e) = rest[s..].find('"').map(|i| i + s) else { continue };
            page.push(EmbeddedImage {
                x0: l * sx,
                y0: t * sy,
                x1: (l + w) * sx,
                y1: (t + h) * sy,
                file: rest[s..e].to_string(),
            });
        }
    }
    pages
}

/// Wie weit ein eingebettetes Bild breiter sein darf als seine Kachel.
///
/// Dieselbe Sorge wie bei [`MAX_SHOT_ASPECT`]: Ein Bild, das viel breiter ist
/// als die Kachel, gehört ihr nicht allein — es ist der Seitenhintergrund oder
/// das Foto einer ganzen Kachelgruppe. Auf dem Prospekt vom 20.07. kostet die
/// Schranke 13 Zuordnungen (44 statt 31) und verhindert dafür ebenso viele
/// falsche.
const MAX_EMBEDDED_WIDTH_RATIO: f64 = 1.6;

/// Größter Abstand zwischen Bildunterkante und Kacheloberkante (pt).
const MAX_EMBEDDED_GAP_PT: f64 = 60.0;

/// Das eingebettete Bild über dieser Kachel — oder keines.
///
/// Gesucht wird wie beim Schnitt: unmittelbar **über** dem Kacheltext, in
/// derselben Spalte. Von mehreren Kandidaten gewinnt der mit dem kleinsten
/// Abstand plus Mittenversatz.
///
/// `taken` sind die Dateien, die schon einer anderen Kachel gehören — ein Bild
/// steht genau einer Kachel zu, sonst trägt eine Reihe von Kacheln dasselbe
/// Foto.
fn embedded_photo_for<'a>(
    tile: &Island,
    images: &'a [EmbeddedImage],
    taken: &std::collections::HashSet<String>,
) -> Option<&'a EmbeddedImage> {
    let tile_width = tile.x1 - tile.x0;
    let tile_mid = (tile.x0 + tile.x1) / 2.0;
    images
        .iter()
        .filter(|img| !taken.contains(&img.file))
        .filter(|img| img.width() >= MIN_SHOT_PT && img.height() >= MIN_SHOT_PT)
        .filter(|img| img.width() <= MAX_EMBEDDED_WIDTH_RATIO * tile_width)
        // Dieselbe Spalte: die Rechtecke überlappen sich in x deutlich.
        .filter(|img| {
            let overlap = img.x1.min(tile.x1) - img.x0.max(tile.x0);
            overlap > 0.35 * tile_width.min(img.width())
        })
        // Über der Kachel. Etwas Überlappung ist erlaubt — Prospektfotos ragen
        // gern unter ihren eigenen Text.
        .filter(|img| {
            let gap = tile.y0 - img.y1;
            gap >= -0.5 * img.height() && gap <= MAX_EMBEDDED_GAP_PT
        })
        .min_by(|a, b| {
            let cost = |i: &EmbeddedImage| {
                (tile.y0 - i.y1).abs() + ((i.x0 + i.x1) / 2.0 - tile_mid).abs()
            };
            cost(a).total_cmp(&cost(b))
        })
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
    // Reihenfolge der Kacheln, wie sie zuerst auftauchen. **Ohne diese Liste
    // wäre das Ergebnis von Lauf zu Lauf verschieden:** `HashMap` iteriert in
    // zufälliger Reihenfolge (RandomState je Instanz), und `into_values()`
    // gäbe die Kacheln damit mal so, mal so heraus.
    //
    // Innerhalb einer Kachel war das nie ein Problem — dafür sorgt `order`
    // unten. Aber die Reihenfolge der Kacheln *untereinander* entscheidet,
    // welches Produkt zu welchem Preis gepaart wird: `pairs.sort_by` ist
    // stabil, gleich weite Paare behalten also die Eingabereihenfolge. War die
    // zufällig, war die Paarung es auch.
    //
    // Genau das war der Wackeltest, der drei PRs in dieser Nacht ein
    // unklares Signal gegeben hat: `lidl_prospekt_builds_tiles_from_word_
    // coordinates` fiel mit „weicher Trennstrich nicht zusammengezogen" um,
    // weil „CELEBRATIONS" mal in einem Titel landete und mal nicht — bei
    // byte-gleichem Commit.
    let mut roots_in_order: Vec<usize> = Vec::new();
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
                roots_in_order.push(root);
            }
        }
    }
    // Lesereihenfolge der Seite: die Kachel, deren erste Insel oben links
    // steht, kommt zuerst.
    roots_in_order
        .into_iter()
        .filter_map(|root| groups.remove(&root))
        .collect()
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
/// Beginnt der Titel mit einem gewöhnlichen kleingeschriebenen Wort — also
/// ohne Punkt und ohne Ziffer darin?
///
/// Der Punkt ist die Ausnahme, die `f.a.n.` durchlässt (siehe
/// `is_plausible_title`); Ziffern lassen Schreibweisen wie `2in1` stehen.
fn first_word_is_plain_lowercase(title: &str) -> bool {
    let Some(word) = title.split_whitespace().next() else {
        return false;
    };
    // Satzzeichen am Wortende gehören nicht zum Wort. Ohne diesen Schritt
    // rutscht „er:" durch: Das Wort ist mitten in „Anbieter:" abgeschnitten,
    // aber der Doppelpunkt macht es für den Test unten „nicht rein
    // alphabetisch". Genau so wurde das Kleingedruckte einer Vodafone-Anzeige
    // zum Angebot (Prospekt vom 20.07., Seite 31).
    //
    // Nur am ENDE geschnitten, nicht überall: Ein Punkt *im* Wort ist die
    // Ausnahme, die `f.a.n.` rettet, und `s.Oliver` hängt genauso daran.
    let word = word.trim_end_matches(|c: char| !c.is_alphanumeric());
    word.chars().next().is_some_and(char::is_lowercase)
        && word.chars().all(|c| c.is_alphabetic() || c == '-')
}

/// Ein am Ende hängendes Bindewort abschneiden — mehrfach, denn der Prospekt
/// bricht auch mal „… Baumwolle und mit" ab.
///
/// Bewusst eine Reparatur und kein Ausschluss: Das Produkt steht da
/// („ESMARA MEN Slips/Boxer Baumwolle"), abgeschnitten ist nur der
/// Beschreibungssatz dahinter. Es wegzuwerfen kostete ein echtes Angebot.
pub fn trim_dangling_tail(title: &str) -> String {
    let mut out = title.trim().to_string();
    // Der Prospekt hängt selten mehr als zwei solche Wörter aneinander; die
    // Schranke verhindert trotzdem, dass hier je eine Endlosschleife entsteht.
    for _ in 0..3 {
        let trimmed = DANGLING_TAIL.replace(&out, "").trim().to_string();
        if trimmed == out {
            break;
        }
        out = trimmed;
    }
    out
}

/// Titel einer Kachel, nachdem vorangeklebte Banner-**Zeilen** übersprungen
/// wurden.
///
/// [`trim_leading_banner`] allein reicht nicht: pdftotext zerlegt das Banner
/// in eigene Zeilen („Erhältlich" / „ab Do. 30.7." / „Für draußen"), und die
/// Drei-Zeilen-Schranke von [`title_of`] ist voll, bevor der Produktname
/// überhaupt drankommt — der Titel des Lavendels war das nackte Banner.
/// Deshalb: die längste Folge von Anfangszeilen finden, deren Zusammenschluss
/// restlos aus Bannern besteht, und den Titel aus dem Rest bauen.
///
/// **Nur für den fertigen Titel**, nicht für die Rollenzuteilung — dort würde
/// eine Aktionsüberschrift wie „Ab Do. 30.7. Deluxe-Woche" zum Produkt und
/// stähle einer echten Kachel den Preis (der gemessene Fehlschlag vom
/// 2026-07-30).
fn title_after_leading_banner(island: &Island) -> String {
    let mut skip = 0usize;
    let mut joined = String::new();
    // Der tiefste echte Stapel sind vier Banner-Zeilen (LIVARNO: Datum +
    // zerrissene Tiefpreis-Plakette); sechs ist Schranke, keine Messgröße.
    for (i, line) in island.lines.iter().take(6).enumerate() {
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(line);
        if trim_leading_banner(&joined).is_empty() {
            skip = i + 1;
        }
    }
    if skip == 0 {
        return title_of(island);
    }
    let after = Island {
        x0: island.x0,
        y0: island.y0,
        x1: island.x1,
        y1: island.y1,
        lines: island.lines[skip..].to_vec(),
    };
    title_of(&after)
}

/// Vorangeklebte Banner abschneiden — mehrfach, denn der Prospekt stapelt
/// sie („Erhältlich ab Do. 30.7. Für draußen Lavendel", „Ab Do. 30.7.
/// Tiefpreis Wohnen & Einrichtung Garantie LIVARNO …").
///
/// Das Spiegelbild von [`trim_dangling_tail`], aus demselben Grund eine
/// Reparatur und kein Ausschluss: Das Produkt steht da, nur das Banner
/// klebt davor. Es wegzuwerfen kostete am Prospekt vom 27.07. genau 14
/// Kacheln (PARKSIDE Winkelschleifer, WAGNER Steinofen Pizza, GELATELLI
/// Eis, LIVARNO Steppbett …) — gemessen mit `measure_dropped_tiles`:
/// 20 verworfene Kacheln vorher, 6 nachher, 254 statt 236 Angebote.
///
/// **Gefahrlos für die Paarung:** Der Schnitt läuft erst auf dem fertigen
/// Titel, wenn Produkt und Preis längst gepaart sind — anders als der am
/// 2026-07-30 verworfene Versuch, `LEAD_DATE` schon in `is_layout_text` zu
/// kürzen, der die Rollenzuteilung verschob und mehr kostete als er brachte.
/// Bleibt nach dem Schnitt kein plausibler Name übrig (reine Banner-Kacheln
/// wie „Für drinnen/draußen"), verwerfen die bestehenden Prüfungen die
/// Kachel wie bisher.
pub fn trim_leading_banner(title: &str) -> String {
    let mut out = title.trim().to_string();
    // Der tiefste echte Stapel sind vier Banner übereinander (Datum +
    // zerrissene Tiefpreis-Plakette); die Schranke verhindert trotzdem, dass
    // hier je eine Endlosschleife entsteht.
    for _ in 0..6 {
        let trimmed = LEADING_BANNER.replace(&out, "").trim().to_string();
        let trimmed = TRAILING_BANNER.replace(&trimmed, "").trim().to_string();
        if trimmed == out {
            break;
        }
        out = trimmed;
    }
    out
}

/// Der fertige Titel einer Kachel — genau der, der später im Angebot steht.
///
/// Bis 2026-07-31 stand diese Kette nur an einer Stelle, mitten in
/// [`extract_offers_shots_and_open`]. Die Auswahl der *selbsttragenden*
/// Preiskacheln — Kacheln, die ihren Namen selbst mitbringen — prüfte
/// dagegen den rohen [`title_of`], also den Titel **vor** dem Bannerschnitt
/// aus #31. Ergebnis: „Tiefpreis Garantie METZGERFRISCH Frisches
/// Rinder-Hackfleisch" fiel schon an der Auswahl durch, obwohl der Schnitt
/// daraus ein sauberes „METZGERFRISCH Frisches Rinder-Hackfleisch" gemacht
/// hätte.
///
/// Beide Stellen rufen jetzt dieselbe Funktion. Beurteilt wird damit
/// derselbe Text, der am Ende im Angebot steht.
fn tile_title(island: &Island) -> String {
    let title = title_after_leading_banner(island);
    // "Mit Lidl Plus" steht mitunter in derselben Zeile wie der Name.
    let title = title.trim_end_matches("Mit Lidl Plus").trim().to_string();
    // Vorangeklebte Banner abschneiden — sonst reißen sie in
    // `is_layout_text` und `is_boilerplate` den ganzen Titel mit.
    let title = trim_leading_banner(&title);
    // Abgeschnittene Sätze am Ende kürzen, statt sie mitzuschleppen.
    trim_dangling_tail(&title)
}

pub fn is_plausible_title(title: &str) -> bool {
    if title.len() < 3 || is_boilerplate(title) {
        return false;
    }
    // Reine Gebindeangaben: "5er-Pack", "8er", "3 Stk"
    if PACK_ONLY.is_match(title) {
        return false;
    }
    // Fängt der Titel mit einem gewöhnlichen kleingeschriebenen Wort an, hat
    // die Kachelbildung mitten in einem Wort oder Satz angesetzt: „aren
    // 4er-Pack ALPRO", „eben braucht Ales", „moderne Waffel struktur" —
    // allesamt aus einem echten Lauf für 01219 am 2026-07-31. Der Test auf ein
    // großgeschriebenes Wort unten fängt sie nicht, weil die Marke ja *im*
    // Fragment steht.
    //
    // **Punkte im ersten Wort retten die Kleinschreibung**, und zwar wegen
    // eines echten Falls: `f.a.n.` ist eine Matratzenmarke und schreibt sich
    // so. Eine Regel „klein = Fragment" verwürfe sie. Ein abgeschnittenes Wort
    // trägt dagegen nie einen Punkt.
    if first_word_is_plain_lowercase(title) {
        return false;
    }
    // Menge plus Einheit und sonst nichts: „10 Paar", „3 Stück". Das ist die
    // Zeile unter dem Produkt, nicht das Produkt. Mit Marke daneben bleibt sie
    // stehen — „62-teilig SILVERCREST Kombiservice" ist ein echter Artikel.
    if QUANTITY_ONLY.is_match(title.trim()) {
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
/// Eine Kachel, für die der Schnitt kein Bild gefunden hat.
///
/// Sie ist nicht bildlos, sondern *ungeschnitten*: `photo_rect` verweigert,
/// wenn der Platz über ihr zu hoch ist, um plausibel ihr eigenes Foto zu sein.
/// Genau da greift das eingebettete Bild.
#[derive(Debug, Clone)]
struct OpenTile {
    offer_id: String,
    /// 1-basierte Seitenzahl im PDF.
    page: usize,
    tile: Island,
}

pub fn extract_offers_with_shots(
    xml: &str,
    market_id: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
) -> (Vec<Offer>, Vec<TileShot>) {
    let (offers, shots, _) = extract_offers_shots_and_open(xml, market_id, valid_from, valid_until);
    (offers, shots)
}

/// Wie [`extract_offers_with_shots`], liefert zusätzlich die Kacheln, für die
/// der Schnitt nichts gefunden hat — die Kandidaten für ein eingebettetes Bild.
fn extract_offers_shots_and_open(
    xml: &str,
    market_id: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
) -> (Vec<Offer>, Vec<TileShot>, Vec<OpenTile>) {
    let mut offers = Vec::new();
    let mut shots = Vec::new();
    let mut open: Vec<OpenTile> = Vec::new();
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
        //
        // Gemessen wird am **fertigen** Titel ([`tile_title`]), nicht am
        // rohen: Über dem Namen klebt oft eine Plakette, und diese Prüfung
        // lief bisher vor dem Schnitt aus #31.
        //
        // Gefahrlos für die Paarung, anders als eine Änderung an der
        // Rollenzuteilung weiter oben: Diese Liste entsteht erst, wenn alle
        // Paare stehen, und enthält nur Preiskacheln, die ohnehin niemand
        // mehr bekommt. Sie kann keiner anderen Kachel den Preis wegnehmen.
        let self_contained: Vec<usize> = (0..prices.len())
            .filter(|j| !used_price[*j])
            .filter(|j| is_plausible_title(&tile_title(&prices[*j])))
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

            let title = tile_title(product);
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
                if let Ok(path) = std::env::var("LIDL_RECT_DUMP") {
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                    {
                        let _ = writeln!(
                            f,
                            "{}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{}",
                            offer_id,
                            page_index + 1,
                            tile_rect.x0,
                            tile_rect.y0,
                            tile_rect.x1,
                            tile_rect.y1,
                            title
                        );
                    }
                }
                if let Some((x, y, w, h)) = photo_rect(&tile_rect, &page_rects) {
                    shots.push(TileShot {
                        offer_id: offer_id.clone(),
                        page: page_index + 1,
                        x,
                        y,
                        w,
                        h,
                    });
                } else {
                    // Der Schnitt verweigert — der Platz über der Kachel ist zu
                    // hoch, um plausibel ihr eigenes Foto zu sein. Genau diese
                    // Kacheln bekommen die zweite Chance über das eingebettete
                    // Bild; die Kachel selbst wird dafür aufgehoben.
                    open.push(OpenTile {
                        offer_id: offer_id.clone(),
                        page: page_index + 1,
                        tile: tile_rect.clone(),
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
    // Dieselbe Buchführung für die offenen Kacheln — und zusätzlich: Was der
    // Schnitt auf einer anderen Kachel doch bekommen hat, braucht hier keine
    // zweite Quelle mehr.
    let mut seen_open = std::collections::HashSet::new();
    open.retain(|o| {
        kept.contains(o.offer_id.as_str())
            && !seen.contains(&o.offer_id)
            && seen_open.insert(o.offer_id.clone())
    });

    stats.report(offers.len());
    (offers, shots, open)
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

// ------------------------------------------------------- Prospekt je Lauf
//
// Gemessen am 2026-07-31: Die drei gewählten Lidl-Filialen liegen in
// derselben Absatzregion und bekommen deshalb denselben Prospekt
// (`aktionsprospekt-27-07-2026-01-08-2026-eee91d`). Ohne Cache lud und las
// jeder Markt dieselbe Datei neu — 85 MB Download und rund 40 s
// `pdftotext -bbox-layout`, dreimal für eine Datei.

/// Die PDF eines Prospekts und ihre Textebene, solange dieser Prozess läuft.
///
/// **Bewusst nur im Speicher und nur für diesen Lauf.** Der Prospekt wechselt
/// wöchentlich; ein Cache über Läufe hinweg lieferte irgendwann die Angebote
/// der Vorwoche, ohne dass jemand es merkt. Ein neuer Prozess fängt leer an
/// und lädt die PDF neu.
///
/// Es liegt immer höchstens **ein** Prospekt hier: Fragt ein Markt nach einer
/// anderen `pdf_url`, wird die vorige Datei gelöscht. Der Platzbedarf auf der
/// Platte bleibt damit derselbe wie vor dem Cache.
struct Leaflet {
    url: String,
    pdf: std::path::PathBuf,
    /// Textebene je `pdftotext`-Modus — `-bbox-layout` für den Extraktor,
    /// `-layout` für den LLM-Weg. Beide lesen dieselbe Datei.
    text: HashMap<String, std::sync::Arc<String>>,
}

static LEAFLET: LazyLock<std::sync::Mutex<Option<Leaflet>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

/// PDF und Textebene zu `pdf_url` — beim ersten Aufruf geladen und gelesen,
/// danach aus dem Cache.
///
/// `download` und `parse` stehen als Parameter da und nicht fest im Rumpf,
/// damit der Test nachweisen kann, dass der zweite Aufruf wirklich keine
/// Arbeit mehr macht: Er reicht zählende Attrappen herein.
fn cached_leaflet(
    pdf_url: &str,
    mode: &str,
    download: &dyn Fn(&str) -> Result<std::path::PathBuf>,
    parse: &dyn Fn(&std::path::Path, &str) -> Result<String>,
) -> Result<(std::path::PathBuf, std::sync::Arc<String>)> {
    // Ein vergifteter Mutex ist hier kein Grund aufzugeben: Im Slot steht
    // nur ein Pfad und Text, keine halb geänderte Datenstruktur.
    let mut slot = LEAFLET.lock().unwrap_or_else(|e| e.into_inner());

    // Anderer Prospekt: die alte Datei wird nicht mehr gebraucht.
    if slot.as_ref().is_some_and(|l| l.url != pdf_url) {
        if let Some(old) = slot.take() {
            let _ = std::fs::remove_file(&old.pdf);
        }
    }
    if slot.is_none() {
        *slot = Some(Leaflet {
            url: pdf_url.to_string(),
            pdf: download(pdf_url)?,
            text: HashMap::new(),
        });
    }

    let leaflet = slot.as_ref().expect("gerade gesetzt");
    if !leaflet.text.contains_key(mode) {
        let parsed = parse(&leaflet.pdf, mode);
        match parsed {
            Ok(text) => {
                slot.as_mut()
                    .expect("gerade gesetzt")
                    .text
                    .insert(mode.to_string(), std::sync::Arc::new(text));
            }
            Err(e) => {
                // Eine PDF, die sich nicht lesen lässt, darf nicht im Cache
                // hängen bleiben — sonst scheitert jeder folgende Markt an
                // derselben kaputten Datei, ohne es noch einmal zu versuchen.
                if let Some(old) = slot.take() {
                    let _ = std::fs::remove_file(&old.pdf);
                }
                return Err(e);
            }
        }
    }

    let leaflet = slot.as_ref().expect("gerade gesetzt");
    Ok((leaflet.pdf.clone(), leaflet.text[mode].clone()))
}

/// Den Prospekt dieses Laufs freigeben und seine PDF löschen.
///
/// Ruft der Sync am Ende der Filialschleife auf. Vor dem Cache löschte jeder
/// Markt seine eigene Kopie; jetzt teilen sie sich eine, und die letzte darf
/// nicht liegen bleiben.
pub fn release_leaflet() {
    let mut slot = LEAFLET.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(leaflet) = slot.take() {
        let _ = std::fs::remove_file(&leaflet.pdf);
    }
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
/// Was in einem gerasterten Streifen tatsächlich Bild ist.
#[derive(Debug, PartialEq)]
pub struct CropContent {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
    /// Fand sich über dem Produkt eine Hintergrundlücke? Ohne sie stößt der
    /// Streifen randlos an das Foto der Kachel darüber, und dann ist nicht
    /// entscheidbar, wo das eigene Produkt anfängt.
    pub separated: bool,
}

/// Farbabstand, ab dem ein Pixel nicht mehr Hintergrund ist.
const BG_TOLERANCE: i32 = 28;
/// Ab diesem Anteil gilt eine Bildzeile als „hat Inhalt".
const ROW_CONTENT: f64 = 0.06;
/// So viele fast leere Zeilen hintereinander sind die Lücke zwischen zwei
/// Kacheln.
const GAP_ROWS: u32 = 6;

/// Das Produkt im Streifen anhand seiner Hintergrundfläche eingrenzen.
///
/// Scotts Idee: Das Produkt hat andere Farben als die Fläche, also lässt sich
/// seine Ausdehnung schätzen, statt sie aus Kachelmaßen zu raten. Gelesen wird
/// von **unten** — das eigene Foto sitzt unmittelbar über dem Kacheltext — bis
/// eine Hintergrundlücke kommt. Was darüber liegt, gehört der Kachel darüber.
///
/// Damit darf die Geometrie großzügiger werden: Sie liefert ein weites Band,
/// und wo das Produkt darin endet, entscheiden die Farben.
///
/// **Grenze, ehrlich benannt:** Stoßen zwei randlose Fotos aneinander, gibt es
/// keine Lücke zu finden. Dann meldet das Ergebnis `separated == false`, und
/// die Antwort darauf bleibt „kein Bild" statt einer feineren Schätzung.
pub fn analyse_crop(img: &image::RgbImage) -> Option<CropContent> {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return None;
    }
    let mut bins: HashMap<(u8, u8, u8), u32> = HashMap::new();
    let mut border = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        *bins.entry((p[0] / 16, p[1] / 16, p[2] / 16)).or_default() += 1;
    };
    for y in 0..h {
        border(0, y);
        border(w - 1, y);
    }
    let (bin, _) = bins.into_iter().max_by_key(|&(bin, n)| (n, bin))?;
    let bg = [bin.0 as i32 * 16 + 8, bin.1 as i32 * 16 + 8, bin.2 as i32 * 16 + 8];
    let is_content = |p: &image::Rgb<u8>| {
        (p.0[0] as i32 - bg[0]).abs().max((p.0[1] as i32 - bg[1]).abs())
            .max((p.0[2] as i32 - bg[2]).abs())
            > BG_TOLERANCE
    };

    let rows: Vec<f64> = (0..h)
        .map(|y| (0..w).filter(|&x| is_content(img.get_pixel(x, y))).count() as f64 / w as f64)
        .collect();

    let bottom = (0..h).rev().find(|&y| rows[y as usize] >= ROW_CONTENT)?;
    let mut top = bottom;
    let mut gap = 0u32;
    let mut separated = false;
    for y in (0..=bottom).rev() {
        if rows[y as usize] < ROW_CONTENT {
            gap += 1;
            if gap >= GAP_ROWS {
                separated = true;
                break;
            }
        } else {
            gap = 0;
            top = y;
        }
    }

    let left = (0..w).find(|&x| (top..=bottom).any(|y| is_content(img.get_pixel(x, y))))?;
    let right = (0..w).rev().find(|&x| (top..=bottom).any(|y| is_content(img.get_pixel(x, y))))?;
    Some(CropContent { top, bottom, left, right, separated })
}

/// Den gerasterten Streifen auf das Produkt zuschneiden — oder verwerfen.
///
/// `Ok(false)` heißt: Hier ist nicht entscheidbar, was das eigene Produkt ist.
/// Dann bleibt das Angebot ohne Bild und behält sein Emoji.
fn refine_crop(path: &std::path::Path) -> Result<bool> {
    let img = image::open(path)
        .with_context(|| format!("Kachelbild nicht lesbar: {}", path.display()))?
        .to_rgb8();
    let Some(c) = analyse_crop(&img) else {
        return Ok(false); // reine Fläche, kein Produkt
    };
    // **Die Lücke entscheidet hier nichts.** Gemessen an einem echten Lauf für
    // 01219: Als Tor benutzt, verwirft sie 99 von 197 Streifen und drückt die
    // Abdeckung von 202 auf 98 — die Kacheln des Rasters stoßen meist ohne
    // sechs leere Zeilen aneinander, weil die Geometrie das Band schon an der
    // Nachbarkachel abgeschnitten hat. Das Tor bleibt deshalb die Geometrie
    // (`MAX_SHOT_ASPECT`), die nachweislich keine falschen Bilder durchlässt;
    // die Farben schneiden nur den leeren Rand weg.

    const PAD: u32 = 4;
    let (w, h) = img.dimensions();
    let x0 = c.left.saturating_sub(PAD);
    let y0 = c.top.saturating_sub(PAD);
    let x1 = (c.right + PAD).min(w - 1);
    let y1 = (c.bottom + PAD).min(h - 1);
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    if cw < 24 || ch < 24 {
        return Ok(false);
    }
    if cw != w || ch != h {
        image::imageops::crop_imm(&img, x0, y0, cw, ch)
            .to_image()
            .save(path)
            .with_context(|| format!("Kachelbild nicht schreibbar: {}", path.display()))?;
    }
    Ok(true)
}

/// Anteil grauer Pixel, ab dem ein Bild eine Alphamaske ist und kein Foto.
///
/// poppler gibt die Maske eines Bildes als **eigenes** `<image>` aus — eine
/// graustufige Silhouette. Auf dem Prospekt vom 20.07. waren das 2 032 der
/// 3 150 ausgeworfenen Dateien; ungefiltert war fast die Hälfte aller
/// Zuordnungen ein leeres weißes Rechteck.
const MASK_GREY_SHARE: u32 = 98;

/// Höchstzahl belegter Farbeimer (4 bit je Kanal), unter der ein Bild
/// Bedienelement ist und kein Produkt.
///
/// Gemessen an den 31 Zuordnungen des 20.07.-Prospekts: Der Knopf „Jetzt
/// entdecken" belegt **17** Eimer, das ärmste echte Produktfoto — eine
/// einfarbige Schürze vor Weiß — belegt **32**. Der Abstand ist der ganze
/// Spielraum, den es hier gibt, deshalb liegt die Schranke bei 24 und nicht
/// höher: Ein verworfenes Produkt kostet mehr als ein durchgelassener Knopf.
///
/// **Was diese Probe NICHT fängt**, gemessen und nicht vermutet: das Siegel
/// „Geschützte Geografische Angabe" belegt 382 Eimer. Es ist ein dichter Ring
/// aus Schrift und Sternen und sieht in jeder Farbkennzahl aus wie ein Foto.
/// Wer die Schranke so weit anhebt, verliert echte Produkte — die Schürze
/// liegt bei 32.
const MIN_PHOTO_COLOUR_BINS: usize = 24;

/// Ist dieses Bild Fläche statt Foto — Maske, Knopf, Logo?
fn is_flat_artwork(img: &image::RgbImage) -> bool {
    let total = img.pixels().len().max(1);
    let grey = img
        .pixels()
        .filter(|px| {
            let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
            (r - g).abs() <= 8 && (g - b).abs() <= 8 && (r - b).abs() <= 8
        })
        .count();
    if (grey * 100 / total) as u32 >= MASK_GREY_SHARE {
        return true;
    }
    let mut bins: HashSet<(u8, u8, u8)> = HashSet::new();
    for px in img.pixels() {
        bins.insert((px[0] / 16, px[1] / 16, px[2] / 16));
        if bins.len() > MIN_PHOTO_COLOUR_BINS {
            return false;
        }
    }
    true
}

/// Die zugeordneten eingebetteten Bilder in das Kachelbild-Verzeichnis legen
/// und je Angebot ihre `file://`-URL liefern.
///
/// `assigned` bildet Angebots-ID auf die von `pdftohtml` geschriebene Datei
/// ab. Was die Flächenprobe nicht besteht, fällt hier heraus — lieber kein
/// Bild als ein Knopf neben einem Preis.
fn place_embedded_photos(
    extracted_dir: &std::path::Path,
    assigned: &[(String, String)],
) -> HashMap<String, String> {
    if assigned.is_empty() {
        return HashMap::new();
    }
    let dir = crop_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("WARNUNG [Lidl] Kachelbilder nicht ablegbar ({e}) — keine eingebetteten Bilder.");
        return HashMap::new();
    }
    let (mut out, mut flat, mut failed) = (HashMap::new(), 0usize, 0usize);
    for (offer_id, file) in assigned {
        let src = extracted_dir.join(file);
        let Ok(img) = image::open(&src) else {
            failed += 1;
            continue;
        };
        let rgb = img.to_rgb8();
        if is_flat_artwork(&rgb) {
            flat += 1;
            continue;
        }
        let target = dir.join(format!("{offer_id}.png"));
        match rgb.save(&target) {
            Ok(()) => {
                out.insert(offer_id.clone(), format!("file://{}", target.display()));
            }
            Err(e) => {
                eprintln!("  Eingebettetes Bild nicht schreibbar ({offer_id}): {e}");
                failed += 1;
            }
        }
    }
    println!(
        "  {} eingebettete Bilder übernommen ({flat} als Fläche verworfen{})",
        out.len(),
        if failed > 0 { format!(", {failed} fehlgeschlagen") } else { String::new() }
    );
    out
}

/// Die eingebetteten Bilder für die Kacheln holen, die der Schnitt abgelehnt
/// hat.
///
/// `pdftohtml -xml` schreibt die Bilder des PDFs als Dateien in ein
/// Verzeichnis und nennt daneben ihre Rechtecke. Nur die Seiten, auf denen
/// überhaupt eine offene Kachel sitzt, werden gelesen — auf dem Prospekt vom
/// 20.07. sind das 46 von 69 Seiten, und der ganze Durchlauf kostete sonst
/// 3 min 39 s und 234 MB.
///
/// Jeder Fehlschlag ist hier folgenlos: Es gibt dann kein zweites Bild, und
/// die Kachel bleibt bei ihrem Emoji.
fn embedded_photos(
    pdf: &std::path::Path,
    bbox_xml: &str,
    open: &[OpenTile],
) -> HashMap<String, String> {
    if open.is_empty() {
        return HashMap::new();
    }
    let (Some(first), Some(last)) = (
        open.iter().map(|o| o.page).min(),
        open.iter().map(|o| o.page).max(),
    ) else {
        return HashMap::new();
    };

    let dir = std::env::temp_dir().join(format!("lechariot-lidl-xobj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("WARNUNG [Lidl] Bildauszug nicht ablegbar ({e}) — keine eingebetteten Bilder.");
        return HashMap::new();
    }

    let out = dir.join("doc");
    let status = Command::new("pdftohtml")
        .args(["-xml", "-f", &first.to_string(), "-l", &last.to_string()])
        .arg(pdf)
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() => {}
        other => {
            eprintln!("WARNUNG [Lidl] pdftohtml fehlgeschlagen ({other:?}) — keine eingebetteten Bilder.");
            let _ = std::fs::remove_dir_all(&dir);
            return HashMap::new();
        }
    }

    let xml = match std::fs::read_to_string(dir.join("doc.xml")) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("WARNUNG [Lidl] Bildauszug nicht lesbar ({e}) — keine eingebetteten Bilder.");
            let _ = std::fs::remove_dir_all(&dir);
            return HashMap::new();
        }
    };

    // `-f N` lässt pdftohtml bei Seite N zu zählen anfangen; die Seitenmaße
    // aus dem bbox-XML sind dagegen ab Seite 1 durchnummeriert.
    let sizes = page_sizes(bbox_xml);
    let window: Vec<(f64, f64)> =
        sizes.get(first - 1..last.min(sizes.len())).unwrap_or(&[]).to_vec();
    let images = parse_embedded_images(&xml, &window);

    let mut taken: HashSet<String> = HashSet::new();
    let mut assigned: Vec<(String, String)> = Vec::new();
    for tile in open {
        let Some(page) = images.get(tile.page - first) else { continue };
        if let Some(img) = embedded_photo_for(&tile.tile, page, &taken) {
            taken.insert(img.file.clone());
            assigned.push((tile.offer_id.clone(), img.file.clone()));
        }
    }

    let placed = place_embedded_photos(&dir, &assigned);
    let _ = std::fs::remove_dir_all(&dir);
    placed
}

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
    let mut rejected = 0usize;
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
            Ok(s) if s.success() && target.exists() => match refine_crop(&target) {
                Ok(true) => {
                    out.insert(shot.offer_id.clone(), format!("file://{}", target.display()));
                }
                // Der Streifen hat die Hintergrundprobe nicht bestanden: kein
                // Bild ist besser als das der Nachbarkachel.
                Ok(false) => {
                    let _ = std::fs::remove_file(&target);
                    rejected += 1;
                }
                Err(e) => {
                    eprintln!("  Kachelbild nicht auswertbar ({}): {e}", shot.offer_id);
                    let _ = std::fs::remove_file(&target);
                    failed += 1;
                }
            },
            _ => failed += 1,
        }
    }
    println!(
        "  {} Kachelbilder gerastert ({} an der Hintergrundprobe verworfen{})",
        out.len(),
        rejected,
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
    // Nach Produkt-ID sortiert, aus demselben Grund wie in `cluster`: Die
    // Onlineshop-Artikel stehen in einer `HashMap`, und die gibt sie in
    // zufälliger Reihenfolge heraus. Gemessen am 2026-07-31 an zwei Läufen
    // hintereinander: dieselben 393 Angebote, andere Reihenfolge — und alle
    // Abweichungen waren Onlineshop-Zeilen.
    let mut keys: Vec<&String> = flyer.products.keys().collect();
    keys.sort();
    for product in keys.into_iter().filter_map(|k| flyer.products.get(k)) {
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
    let (_, text) = cached_leaflet(&pdf_url, "-layout", &download_pdf, &run_pdftotext)?;
    // pdftotext trennt Seiten mit einem Seitenvorschub.
    let pages = text
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
    // Die PDF bleibt liegen, bis auch die Kachelbilder geschnitten sind — sie
    // ein zweites Mal zu laden wären 85 MB für nichts. Seit dem Cache gilt das
    // über die Filiale hinaus: Die drei gewählten Lidl-Märkte teilen sich
    // Prospekt und Textebene, geladen und gelesen wird beides einmal je Lauf.
    let (pdf_path, xml) = cached_leaflet(pdf_url, "-bbox-layout", &download_pdf, &run_pdftotext)?;
    let xml = xml.as_str();

    let (mut offers, shots, open) = extract_offers_shots_and_open(
        xml,
        &market.id,
        flyer.offer_start_date.as_deref(),
        flyer.offer_end_date.as_deref(),
    );
    if offers.is_empty() {
        bail!("Prospekt gelesen, aber keine Angebote extrahiert — Layout geändert?");
    }

    let crops = render_shots(&pdf_path, &shots);
    // Zweiter Weg für die Kacheln, die der Schnitt abgelehnt hat. Scheitert er,
    // ist das kein Fehler des Laufs — die Angebote stehen, nur einige tragen
    // weiter ihr Emoji.
    let embedded = embedded_photos(&pdf_path, xml, &open);
    for offer in &mut offers {
        if let Some(url) = crops.get(&offer.id).or_else(|| embedded.get(&offer.id)) {
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

    /// Die Rechenprobe liest **nur den ersten** Grundpreis einer Kachel, und
    /// das ist Absicht.
    ///
    /// Am 2026-07-31 wurde am Prospekt vom 27.07. nachgemessen, was ein
    /// „liest halt alle Grundpreise der Kachel" kosten würde: Auf Seite 61
    /// hängen JOHNNIE WALKER (0,7 l, 1 l = 15.70 -> 10.99 €), LENOR, WODKA
    /// GORBATSCHOW (1.827 l, 1 l = 2.55 -> 4.66 €) und ein vierter Artikel
    /// (0,7 l, 1 l = 9.27 -> 6.49 €) in **einer** Kachel. Der Titel ist
    /// „JOHNNIE WALKER Red Label Blended Scotch Whisky". Dürfte die Probe
    /// jeden Grundpreis der Kachel benutzen, gingen 4.65 € und 6.49 € als
    /// Whiskypreis durch — genau die Fehlpaarung, gegen die es sie gibt.
    ///
    /// Dieser Test ist die Bremse dafür. Er darf nur fallen, wenn jemand eine
    /// Regel mitbringt, die 4.65 € und 6.49 € weiterhin ablehnt.
    #[test]
    fn the_arithmetic_check_only_trusts_the_first_base_price() {
        let seite61 = "JOHNNIE WALKER Red Label Blended Scotch Whisky LENOR Weichspüler \
                       40 % vol Je 0,7 l 1 l = 15.70 Normalpreis: 15.99 1 l = 22.84 \
                       Mit Lidl Plus WODKA GORBATSCHOW Versch. Sorten. Je 1.827 ml \
                       1 l = 2.55 37,5 % vol Je 0,7 l 1 l = 9.27";
        // Der Whisky selbst geht durch.
        assert_eq!(arithmetic_check(seite61, 10.99), Some(true));
        // Die Preise der Nachbarn nicht — obwohl beide gegen *irgendeinen*
        // Grundpreis derselben Kachel aufgehen.
        assert_eq!(arithmetic_check(seite61, 4.65), Some(false));
        assert_eq!(arithmetic_check(seite61, 6.49), Some(false));
    }

    /// Der Prospekt-Cache ist ein Prozess-Global; `cargo test` lässt Tests
    /// nebenläufig laufen. Beide Cache-Tests nehmen deshalb dieselbe Sperre.
    fn leaflet_tests_one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        static REIHUM: std::sync::Mutex<()> = std::sync::Mutex::new(());
        REIHUM.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Alle drei Lidl-Filialen bekommen denselben Prospekt. Geladen und
    /// gelesen wird er trotzdem nur einmal.
    ///
    /// Der Nachweis läuft über Attrappen mit Zähler statt über echte Dateien:
    /// Ein Test, der 85 MB lädt und 40 s `pdftotext` startet, würde nie
    /// laufen. Gemessen wird genau das, was die Nacht kostet — Zahl der
    /// Downloads und Zahl der Parse-Läufe.
    #[test]
    fn the_leaflet_is_fetched_once_per_run() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let _reihum = leaflet_tests_one_at_a_time();

        let dir = std::env::temp_dir().join(format!("lechariot-test-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("Testverzeichnis");
        let downloads = AtomicUsize::new(0);
        let parses = AtomicUsize::new(0);
        let download = |_url: &str| -> Result<std::path::PathBuf> {
            let n = downloads.fetch_add(1, Ordering::SeqCst);
            let path = dir.join(format!("prospekt-{n}.pdf"));
            std::fs::write(&path, b"%PDF-Attrappe")?;
            Ok(path)
        };
        let parse = |_p: &std::path::Path, mode: &str| -> Result<String> {
            parses.fetch_add(1, Ordering::SeqCst);
            Ok(format!("<doc mode=\"{mode}\"/>"))
        };

        // Ein neuer Prozess fängt leer an; im Test stellen wir das her.
        release_leaflet();

        const A: &str = "https://assets.leaflets.schwarz/a.pdf";
        const B: &str = "https://assets.leaflets.schwarz/b.pdf";

        let erste = cached_leaflet(A, "-bbox-layout", &download, &parse).expect("erster Markt");
        for markt in 2..=3 {
            let weiterer =
                cached_leaflet(A, "-bbox-layout", &download, &parse).expect("weiterer Markt");
            assert_eq!(weiterer.0, erste.0, "Markt {markt} bekam eine andere Datei");
            // Byte-gleiche Textebene: Der Extraktor ist absichtlich
            // deterministisch, und derselbe Prospekt muss ihm denselben Text
            // vorlegen — sonst wären die Angebote je Filiale andere.
            assert_eq!(*weiterer.1, *erste.1, "Markt {markt} bekam anderen Text");
        }
        assert_eq!(downloads.load(Ordering::SeqCst), 1, "PDF mehrfach geladen");
        assert_eq!(parses.load(Ordering::SeqCst), 1, "PDF mehrfach gelesen");

        // Der LLM-Weg liest dieselbe Datei in einem anderen Modus: kein
        // zweiter Download, aber ein zweiter Lauf von pdftotext.
        let layout = cached_leaflet(A, "-layout", &download, &parse).expect("zweiter Modus");
        assert_eq!(downloads.load(Ordering::SeqCst), 1);
        assert_eq!(parses.load(Ordering::SeqCst), 2);
        assert!(layout.1.contains("-layout"));

        // Ein anderer Prospekt verdrängt den alten — und räumt ihn weg.
        let alt = erste.0.clone();
        let zweiter = cached_leaflet(B, "-bbox-layout", &download, &parse).expect("anderer Prospekt");
        assert_eq!(downloads.load(Ordering::SeqCst), 2);
        assert!(!alt.exists(), "die verdrängte PDF liegt noch da: {alt:?}");

        // Freigabe am Ende des Laufs: Datei weg, Cache leer. Der nächste Lauf
        // lädt neu — der Prospekt wechselt wöchentlich, ein Cache über Läufe
        // hinweg lieferte irgendwann die Angebote der Vorwoche.
        release_leaflet();
        assert!(!zweiter.0.exists(), "die PDF blieb nach der Freigabe liegen");
        cached_leaflet(B, "-bbox-layout", &download, &parse).expect("neuer Lauf");
        assert_eq!(downloads.load(Ordering::SeqCst), 3, "der neue Lauf nahm den alten Cache");

        release_leaflet();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Eine PDF, die sich nicht lesen lässt, darf nicht im Cache hängen
    /// bleiben — sonst scheitern die beiden folgenden Märkte an derselben
    /// Datei, ohne es noch einmal zu versuchen.
    #[test]
    fn a_leaflet_that_cannot_be_read_is_not_kept() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let _reihum = leaflet_tests_one_at_a_time();

        let downloads = AtomicUsize::new(0);
        let download = |_url: &str| -> Result<std::path::PathBuf> {
            downloads.fetch_add(1, Ordering::SeqCst);
            Ok(std::env::temp_dir().join("lechariot-test-kaputt.pdf"))
        };
        let parse =
            |_p: &std::path::Path, _mode: &str| -> Result<String> { bail!("pdftotext fehlgeschlagen") };

        release_leaflet();
        const URL: &str = "https://assets.leaflets.schwarz/kaputt.pdf";
        assert!(cached_leaflet(URL, "-bbox-layout", &download, &parse).is_err());
        assert!(cached_leaflet(URL, "-bbox-layout", &download, &parse).is_err());
        assert_eq!(
            downloads.load(Ordering::SeqCst),
            2,
            "der zweite Markt hat es nicht noch einmal versucht"
        );
        release_leaflet();
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


    /// Messgerät, kein Test: Abdeckung mit und ohne eingebettete Bilder,
    /// gegen dasselbe PDF. `LIDL_PDF` zeigt auf eine lokale Prospekt-PDF.
    ///
    /// Beide Zahlen müssen aus demselben Prospekt stammen — ein Vorher/Nachher
    /// aus zwei Wochen ist keins. Der Nenner sind die Kacheln, die überhaupt
    /// ein Bild brauchen (`shots` + `open`); Angebote ohne Kachelrechteck
    /// gehören nicht in eine Quote, die sie gar nicht verfehlen können.
    #[test]
    #[ignore = "Messgerät — braucht LIDL_PDF mit lokaler Prospekt-PDF"]
    fn measure_union_coverage() {
        let pdf = std::env::var("LIDL_PDF").expect("LIDL_PDF fehlt");
        let path = std::path::Path::new(&pdf);
        let xml = run_pdftotext(path, "-bbox-layout").expect("pdftotext");
        let (offers, shots, open) =
            extract_offers_shots_and_open(&xml, "MESSUNG", None, None);
        let embedded = embedded_photos(path, &xml, &open);

        let kacheln = shots.len() + open.len();
        let vereint = shots.len() + embedded.len();
        eprintln!("ANGEBOTE\t{}", offers.len());
        eprintln!("KACHELN\t{kacheln}");
        eprintln!(
            "NUR_SCHNITT\t{}\t{:.1}%",
            shots.len(),
            100.0 * shots.len() as f64 / kacheln as f64
        );
        eprintln!("OFFEN\t{}", open.len());
        eprintln!("EINGEBETTET_DAZU\t{}", embedded.len());
        eprintln!(
            "VEREINT\t{vereint}\t{:.1}%",
            100.0 * vereint as f64 / kacheln as f64
        );
        for (id, url) in embedded.iter().take(400) {
            println!("DAZU\t{id}\t{url}");
        }
    }

    /// Ein Bild-Rechteck von `pdftohtml` muss in PDF-Punkten landen, sonst
    /// vergleicht die Zuordnung Pixel mit Punkten.
    ///
    /// Die Maße sind die echten des Prospekts: 467 x 794 pt, gerastert auf
    /// 700 x 1191 px — Faktor 1,499, wie im Lauf gemessen.
    #[test]
    fn embedded_images_come_back_in_pdf_points() {
        let bbox = r#"<page width="467.72" height="794.00">"#;
        let html = concat!(
            r#"<page number="1" position="absolute" top="0" left="0" height="1191" width="700">"#,
            "\n",
            r#"<image top="150" left="100" width="200" height="240" src="doc-1_7.png"/>"#,
        );
        let pages = parse_embedded_images(html, &page_sizes(bbox));

        assert_eq!(pages.len(), 1);
        let img = &pages[0][0];
        assert_eq!(img.file, "doc-1_7.png");
        // 100 px * 467.72/700 = 66.8 pt
        assert!((img.x0 - 66.8).abs() < 0.1, "x0 = {}", img.x0);
        assert!((img.y0 - 100.0).abs() < 0.2, "y0 = {}", img.y0);
        assert!((img.width() - 133.6).abs() < 0.1, "Breite = {}", img.width());
    }

    /// Ohne bekanntes Seitenmaß gibt es keinen Maßstab — dann lieber kein Bild
    /// als eines an geratener Stelle.
    #[test]
    fn images_without_a_page_size_are_dropped() {
        let html = concat!(
            r#"<page number="1" height="1191" width="700">"#,
            "\n",
            r#"<image top="150" left="100" width="200" height="240" src="doc-1_7.png"/>"#,
        );
        assert!(parse_embedded_images(html, &[]).concat().is_empty());
    }

    fn tile_at(x0: f64, y0: f64, x1: f64, y1: f64) -> Island {
        Island { x0, y0, x1, y1, lines: vec!["Produkt".to_string()] }
    }

    fn img_at(x0: f64, y0: f64, x1: f64, y1: f64, file: &str) -> EmbeddedImage {
        EmbeddedImage { x0, y0, x1, y1, file: file.to_string() }
    }

    /// Das Bild unmittelbar über der Kachel gewinnt, nicht das weiter oben.
    #[test]
    fn the_image_directly_above_the_tile_wins() {
        let tile = tile_at(100.0, 300.0, 200.0, 340.0);
        let images = vec![
            img_at(100.0, 100.0, 200.0, 180.0, "weit-oben.png"),
            img_at(100.0, 200.0, 200.0, 295.0, "direkt-drueber.png"),
        ];
        let taken = std::collections::HashSet::new();
        assert_eq!(
            embedded_photo_for(&tile, &images, &taken).map(|i| i.file.as_str()),
            Some("direkt-drueber.png")
        );
    }

    /// Ein Bild, das viel breiter ist als die Kachel, gehört ihr nicht allein —
    /// dieselbe Regel wie `MAX_SHOT_ASPECT` beim Schnitt. Auf dem Prospekt vom
    /// 20.07. kostet sie 13 Zuordnungen und verhindert ebenso viele falsche.
    #[test]
    fn an_image_far_wider_than_its_tile_belongs_to_no_one() {
        let tile = tile_at(100.0, 300.0, 200.0, 340.0);
        // 300 pt breit gegen 100 pt Kachel: Seitenhintergrund, nicht Produkt.
        let images = vec![img_at(50.0, 200.0, 350.0, 295.0, "hintergrund.png")];
        let taken = std::collections::HashSet::new();
        assert_eq!(embedded_photo_for(&tile, &images, &taken), None);
    }

    /// Die Nachbarspalte zählt nicht, auch wenn ihr Bild näher liegt.
    #[test]
    fn images_in_the_neighbouring_column_are_ignored() {
        let tile = tile_at(100.0, 300.0, 200.0, 340.0);
        let images = vec![img_at(210.0, 250.0, 300.0, 298.0, "nachbar.png")];
        let taken = std::collections::HashSet::new();
        assert_eq!(embedded_photo_for(&tile, &images, &taken), None);
    }

    /// Ein Bild gehört genau einer Kachel. Ohne diese Buchführung trüge eine
    /// ganze Kachelreihe dasselbe Foto.
    #[test]
    fn an_image_is_only_given_away_once() {
        let tile = tile_at(100.0, 300.0, 200.0, 340.0);
        let images = vec![img_at(100.0, 200.0, 200.0, 295.0, "schon-vergeben.png")];
        let taken = std::collections::HashSet::from(["schon-vergeben.png".to_string()]);
        assert_eq!(embedded_photo_for(&tile, &images, &taken), None);
    }

    /// Zu weit über der Kachel heißt: gehört der Kachel darüber.
    #[test]
    fn an_image_too_far_above_is_not_this_tiles_photo() {
        let tile = tile_at(100.0, 300.0, 200.0, 340.0);
        let images = vec![img_at(100.0, 100.0, 200.0, 200.0, "zu-weit.png")];
        let taken = std::collections::HashSet::new();
        assert_eq!(embedded_photo_for(&tile, &images, &taken), None);
    }

    /// poppler gibt die Alphamaske eines Bildes als eigenes `<image>` aus.
    /// Ungefiltert war auf dem Prospekt vom 20.07. fast die Hälfte aller
    /// Zuordnungen so ein leeres Rechteck — und in der App sähe das aus wie
    /// ein kaputtes Bild.
    #[test]
    fn alpha_masks_are_not_product_photos() {
        let mut mask = image::RgbImage::new(40, 40);
        for (x, y, px) in mask.enumerate_pixels_mut() {
            // Graustufen-Silhouette: R, G und B liegen gleichauf.
            let v = if (x + y) % 7 == 0 { 20u8 } else { 240u8 };
            *px = image::Rgb([v, v, v]);
        }
        assert!(is_flat_artwork(&mask));
    }

    /// Ein Knopf ist nicht grau, er ist arm an Farben. „Jetzt entdecken"
    /// belegte 17 Farbeimer.
    #[test]
    fn interface_chrome_is_not_a_product_photo() {
        let mut knopf = image::RgbImage::new(60, 20);
        for (x, _y, px) in knopf.enumerate_pixels_mut() {
            *px = if x % 9 == 0 {
                image::Rgb([255, 255, 255]) // Schrift
            } else {
                image::Rgb([16, 82, 214]) // Knopffläche
            };
        }
        assert!(is_flat_artwork(&knopf));
    }

    /// Und die Gegenprobe, die den Preis dieser Schranke festhält: Ein Foto
    /// mit vielen Farbabstufungen muss durch. Das ärmste echte Produktfoto des
    /// Prospekts — eine einfarbige Schürze vor Weiß — belegte 32 Eimer, die
    /// Schranke liegt bei 24.
    #[test]
    fn a_photo_with_many_shades_survives() {
        let mut foto = image::RgbImage::new(40, 40);
        for (x, y, px) in foto.enumerate_pixels_mut() {
            *px = image::Rgb([(x * 6) as u8, (y * 6) as u8, ((x + y) * 3) as u8]);
        }
        assert!(!is_flat_artwork(&foto));
    }

    /// Die Kachelbildung setzt mitunter mitten im Wort an. Was so entsteht,
    /// trägt die Marke im Fragment und besteht deshalb jede Prüfung, die nur
    /// nach einem großgeschriebenen Wort sucht.
    #[test]
    fn titles_cut_mid_word_are_rejected() {
        for junk in [
            "aren 4er-Pack ALPRO (1 l · 1 kg = 18.69 €)",
            "eben braucht Ales",
            "moderne Waffel struktur",
        ] {
            assert!(!is_plausible_title(junk), "durchgelassen: {junk}");
        }
    }

    /// Und die Gegenprobe, an einem echten Fall: `f.a.n.` ist eine
    /// Matratzenmarke und schreibt sich klein. Eine Regel „klein = Fragment"
    /// verwürfe ein gültiges Angebot.
    #[test]
    fn lowercase_brands_with_dots_survive() {
        assert!(is_plausible_title(
            "f.a.n. 7-Zonen-Kaltschaummatratze »Sweet Dream XXL«"
        ));
        // Dieselbe Ausnahme trägt s.Oliver — der Punkt steht mitten im Wort,
        // nicht an seinem Ende.
        assert!(is_plausible_title("s.Oliver Herren-Poloshirt"));
    }

    /// Das Kleingedruckte einer Anzeige, das die Kachelbildung eingesammelt
    /// hat — Prospekt vom 20.07., Seite 31.
    ///
    /// Es besteht #20s Prüfung auf ein abgeschnittenes Wort nur um ein
    /// Satzzeichen: Das erste Wort ist „er:", die Hälfte von „Anbieter:", und
    /// der Doppelpunkt machte es „nicht rein alphabetisch". Der Titel trägt am
    /// Ende sogar ein echtes Angebot („Stapelturm"), aber ein Eintrag, der zu
    /// neun Zehnteln aus einer Adresse und einer Widerrufsbelehrung besteht,
    /// passt in einer Einkaufsliste auf nichts.
    ///
    /// Er ist auch der Grund, warum das Bild daneben falsch war: Auf die
    /// Kachel wurde ein Ravensburger-Buch geschnitten, weil sie gar keine
    /// Angebotskachel ist.
    #[test]
    fn advertisement_small_print_is_not_an_offer() {
        for junk in [
            "er: Vodafone GmbH („Vodafone“), Ferdinand-Braun-Platz 1, 40549 Düs \
             egistrierung und Legitimation über Ident-Verfahren erforderlich. gg) Neuk Stapelturm",
            "gg) Neukunden erhalten den Rabatt",
            "ttp://www.lidl.de/agb",
        ] {
            assert!(!is_plausible_title(junk), "durchgelassen: {junk}");
        }
    }

    /// „10 Paar" stand dreimal als eigenes Angebot in der Liste — das ist die
    /// Zeile unter dem Produkt, nicht das Produkt.
    #[test]
    fn bare_quantities_are_not_products() {
        assert!(!is_plausible_title("10 Paar"));
        assert!(!is_plausible_title("3 Stück"));
        // Mit Marke daneben ist es ein echter Artikel und bleibt stehen.
        assert!(is_plausible_title("62-teilig SILVERCREST Kombiservice"));
    }

    /// Abgeschnittene Sätze werden gekürzt, nicht verworfen: Das Produkt steht
    /// da, nur der Beschreibungssatz dahinter fehlt.
    #[test]
    fn dangling_conjunctions_are_trimmed_not_dropped() {
        assert_eq!(
            trim_dangling_tail("ESMARA MEN Slips/Boxer Baumwolle und"),
            "ESMARA MEN Slips/Boxer Baumwolle"
        );
        assert_eq!(
            trim_dangling_tail("TRONIC Knopfzellen Multipack mit"),
            "TRONIC Knopfzellen Multipack"
        );
        // Mehrfach hängende Wörter ebenfalls.
        assert_eq!(trim_dangling_tail("PARKSIDE Zwingen-Set zum Einspannen und"),
                   "PARKSIDE Zwingen-Set zum Einspannen");
        // Ein Bindewort mitten im Namen bleibt unangetastet.
        assert_eq!(
            trim_dangling_tail("Brot und Butter Aufstrich"),
            "Brot und Butter Aufstrich"
        );
    }


    /// Vorangeklebte Banner werden abgeschnitten, nicht mitgerissen — das
    /// Spiegelbild von `dangling_conjunctions_are_trimmed_not_dropped`.
    /// Alle Fälle stammen aus dem echten Lauf für den Prospekt vom 27.07.
    /// (20 verworfene Kacheln, 14 davon echte Produkte).
    #[test]
    fn leading_banners_are_trimmed_not_dropped() {
        for (glued, clean) in [
            (
                "Tiefpreis Garantie PARKSIDE Winkelschleifer",
                "PARKSIDE Winkelschleifer",
            ),
            (
                "Weitere Farbe: Weiß PARKSIDE Kabelbinder-Set",
                "PARKSIDE Kabelbinder-Set",
            ),
            (
                "Entspricht 3.33/Stk. GRANDIOL Herbst-Rasendünger",
                "GRANDIOL Herbst-Rasendünger",
            ),
            // Der Betrag kann schon von BADGE_TOKEN gefressen sein.
            (
                "Entspricht /Stk. GELATELLI Crisp ’N’ Cake Eis",
                "GELATELLI Crisp ’N’ Cake Eis",
            ),
            // Aktionsdatum, auch mit Bis-Teil.
            (
                "Ab Do. 30.7. bis Sa. 1.8. WAGNER Steinofen Pizza",
                "WAGNER Steinofen Pizza",
            ),
            (
                "Erhältlich ab Do. 30.7. Für draußen Lavendel angustifolia",
                "Lavendel angustifolia",
            ),
            // Die zerrissene Tiefpreis-Plakette: pdftotext verschachtelt sie
            // mit der Rubrik dazwischen.
            (
                "Ab Do. 30.7. Tiefpreis Wohnen & Einrichtung Garantie LIVARNO 4-Jahreszeiten-Steppbett",
                "LIVARNO 4-Jahreszeiten-Steppbett",
            ),
            // Schriftsplitter der Plakette vor der Marke.
            (
                "Mit Lidl Plus Gültig vom 27.7.–2.8. R ESMARA MEN Sneaker",
                "ESMARA MEN Sneaker",
            ),
            // Die Drinnen/Draußen-Plakette klebt auch HINTER dem Namen.
            ("Grünmix im Korb Für drinnen Für drinnen", "Grünmix im Korb"),
        ] {
            assert_eq!(trim_leading_banner(glued), clean, "aus: {glued}");
        }
    }

    /// Und die Gegenprobe: Banner-Wörter mitten im Namen bleiben stehen, und
    /// ein Titel ganz ohne Banner kommt unverändert zurück.
    #[test]
    fn titles_without_a_leading_banner_stay_untouched() {
        for title in [
            "DELUXE Olivenöl Italienisches Natives",
            "Gartenhortensie",
            // Punkt-Marken sind keine Schriftsplitter.
            "f.a.n. 7-Zonen-Kaltschaummatratze",
            "s.Oliver Herren-Poloshirt",
            // „Garantie" mitten im Namen ist kein Banner.
            "SILVERCREST Wasserkocher mit Garantie-Siegel",
        ] {
            assert_eq!(trim_leading_banner(title), title);
        }
    }

    /// Reine Banner-Kacheln bleiben verworfen: Nach dem Schnitt steht kein
    /// Name mehr da, und die bestehenden Prüfungen greifen wie bisher.
    #[test]
    fn pure_banner_tiles_are_not_rescued() {
        for banner in [
            "Für drinnen/ draußen",
            "Erhältlich ab Do. 30.7. Für draußen",
            "Tiefpreis Garantie",
        ] {
            let trimmed = trim_leading_banner(banner);
            assert!(
                !is_plausible_title(&trimmed),
                "Banner wurde zum Produkt: {banner:?} -> {trimmed:?}"
            );
        }
    }

    /// Messgerät, kein Test: nur der Textweg, damit die Verwurfsgründe je
    /// Kachel sichtbar werden. Ohne Netz und ohne Rastern, also in Sekunden.
    /// `LIDL_PDF` zeigt auf eine lokale Prospekt-PDF;
    /// `LIDL_PROSPEKT_DEBUG=1` druckt jede verworfene Kachel mit Grund.
    #[test]
    #[ignore = "Messgerät — braucht LIDL_PDF mit lokaler Prospekt-PDF"]
    fn measure_dropped_tiles() {
        let pdf = std::env::var("LIDL_PDF").expect("LIDL_PDF fehlt");
        let xml = run_pdftotext(std::path::Path::new(&pdf), "-bbox-layout").expect("pdftotext");
        let (offers, _) = extract_offers_with_shots(&xml, "MESSUNG", None, None);
        eprintln!("ANGEBOTE\t{}", offers.len());
    }

    /// Messgerät, kein Test: ein ganzer Lidl-Abend für drei Filialen
    /// derselben Absatzregion — genau die Nacht, um die es beim Cache geht.
    ///
    /// Läuft mit Netz und lädt den Prospekt (85 MB), deshalb `#[ignore]`.
    /// `LIDL_MESSUNG_ZIP` setzt die PLZ (Standard 01219, Dresden).
    ///
    /// Die erste Zeile ist der Preis ohne Cache — so lief bis 2026-07-31
    /// jeder der drei Märkte. Die beiden folgenden sind der Preis mit.
    #[test]
    #[ignore = "Messgerät — braucht Netz und lädt den Prospekt (85 MB)"]
    fn measure_full_run() {
        let zip = std::env::var("LIDL_MESSUNG_ZIP").unwrap_or_else(|_| "01219".to_string());
        release_leaflet();
        let gesamt = std::time::Instant::now();
        for id in ["LIDL_MESSUNG_1", "LIDL_MESSUNG_2", "LIDL_MESSUNG_3"] {
            let market = Market::new(id, "Messung");
            let start = std::time::Instant::now();
            let offers = fetch_offers(&market, &zip).expect("Lauf");
            eprintln!(
                "MARKT\t{id}\t{} Angebote\t{:.1} s",
                offers.len(),
                start.elapsed().as_secs_f64()
            );
        }
        eprintln!("GESAMT\t{:.1} s", gesamt.elapsed().as_secs_f64());
        release_leaflet();
    }

    fn canvas(w: u32, h: u32, bg: [u8; 3]) -> image::RgbImage {
        image::RgbImage::from_pixel(w, h, image::Rgb(bg))
    }

    fn fill(img: &mut image::RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                img.put_pixel(x, y, image::Rgb(c));
            }
        }
    }

    /// Eine reine Fläche ist kein Produktfoto. Gemessen an einem echten Lauf:
    /// 18 der 181 Streifen waren genau das — einfarbige Rechtecke von 189 bis
    /// 277 Byte, die in der App wie ein kaputtes Bild aussehen.
    #[test]
    fn a_flat_area_carries_no_product() {
        let img = canvas(80, 80, [255, 240, 0]);
        assert_eq!(analyse_crop(&img), None);
    }

    /// Das Produkt wird von unten gesucht und an der Hintergrundlücke
    /// abgeschnitten — was darüber liegt, gehört der Kachel darüber.
    #[test]
    fn the_product_is_bounded_by_the_gap_above_it() {
        let mut img = canvas(80, 100, [220, 0, 0]);
        // Fremdes Foto oben, dann Lücke, dann das eigene Produkt unten.
        fill(&mut img, 5, 0, 74, 20, [255, 255, 255]);
        fill(&mut img, 10, 50, 69, 95, [0, 0, 255]);

        let c = analyse_crop(&img).expect("Produkt nicht gefunden");
        assert!(c.separated, "die Luecke wurde nicht erkannt");
        assert!(c.top >= 45 && c.top <= 55, "Oberkante bei {}", c.top);
        assert_eq!(c.bottom, 95);
        assert_eq!((c.left, c.right), (10, 69));
    }

    /// Läuft der Inhalt ohne Lücke bis an die Oberkante, stößt der Streifen
    /// randlos an die Nachbarkachel — dann ist nicht entscheidbar, wo das
    /// eigene Produkt anfängt.
    #[test]
    fn content_running_into_the_top_edge_is_not_separated() {
        let mut img = canvas(80, 100, [220, 0, 0]);
        fill(&mut img, 5, 0, 74, 95, [0, 0, 255]);
        let c = analyse_crop(&img).expect("Inhalt nicht gefunden");
        assert!(!c.separated);
    }

    /// Messgerät, kein Test: baut aus einer Liste von Bildpfaden
    /// (`LIDL_SHEET_LIST`, ein Pfad je Zeile) eine Kontaktbogen-PNG, damit die
    /// Zuordnung mit dem Auge geprüft werden kann statt nur mit Zahlen.
    #[test]
    #[ignore]
    fn build_contact_sheet() {
        use image::{GenericImage, Rgba, RgbaImage};
        let list = std::env::var("LIDL_SHEET_LIST").expect("LIDL_SHEET_LIST fehlt");
        let out = std::env::var("LIDL_SHEET_OUT").expect("LIDL_SHEET_OUT fehlt");
        let paths: Vec<String> = std::fs::read_to_string(&list)
            .expect("liste")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        const CELL: u32 = 150;
        const COLS: u32 = 6;
        let rows = paths.len().div_ceil(COLS as usize) as u32;
        let mut sheet = RgbaImage::from_pixel(COLS * CELL, rows * CELL, Rgba([255, 255, 255, 255]));
        for (i, p) in paths.iter().enumerate() {
            let Ok(img) = image::open(p) else { continue };
            let thumb = img.resize(CELL - 8, CELL - 8, image::imageops::FilterType::Triangle);
            let thumb = thumb.to_rgba8();
            let ox = (i as u32 % COLS) * CELL + 4;
            let oy = (i as u32 / COLS) * CELL + 4;
            // Transparenz auf Weiss legen, sonst ist das Produkt nicht zu sehen.
            for (x, y, px) in thumb.enumerate_pixels() {
                let al = px[3] as f32 / 255.0;
                let mix = |c: u8| (c as f32 * al + 255.0 * (1.0 - al)) as u8;
                sheet.put_pixel(
                    ox + x,
                    oy + y,
                    Rgba([mix(px[0]), mix(px[1]), mix(px[2]), 255]),
                );
            }
        }
        sheet.save(&out).expect("speichern");
        eprintln!("KONTAKTBOGEN\t{}\t{} bilder", out, paths.len());
    }

    /// Messgerät, kein Test: zählt, wie viele der zugeordneten Bilder gar keine
    /// Fotos sind, sondern Masken — poppler gibt die Alphamaske eines Bildes als
    /// eigenes `<image>` aus, und die ist einfarbig schwarz/weiß.
    #[test]
    #[ignore]
    fn count_flat_images() {
        let list = std::env::var("LIDL_SHEET_LIST").expect("LIDL_SHEET_LIST fehlt");
        let paths: Vec<String> = std::fs::read_to_string(&list)
            .expect("liste")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        let (mut flat, mut ok, mut bad) = (0usize, 0usize, 0usize);
        for p in &paths {
            let Ok(img) = image::open(p) else {
                bad += 1;
                continue;
            };
            let rgb = img.to_rgb8();
            // Grau heißt hier: R, G und B liegen dicht beieinander. Eine Maske
            // besteht nur aus solchen Pixeln, ein Produktfoto nicht.
            let total = rgb.pixels().len().max(1);
            let grey = rgb
                .pixels()
                .filter(|px| {
                    let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
                    (r - g).abs() <= 8 && (g - b).abs() <= 8 && (r - b).abs() <= 8
                })
                .count();
            if grey * 100 / total >= 98 {
                flat += 1;
                println!("MASKE\t{p}");
            } else {
                ok += 1;
            }
        }
        eprintln!("FARBIG\t{ok}\tMASKE\t{flat}\tUNLESBAR\t{bad}\tGESAMT\t{}", paths.len());
    }

    /// Messgerät, kein Test: Kennzahlen je Bild, um Bedienelemente (Knöpfe,
    /// Siegel, Logos) von Produktfotos zu trennen.
    ///
    /// Die Maskenprobe oben reicht dafür nicht: Ein blauer Knopf mit weißer
    /// Schrift ist nicht grau, er ist nur **arm an Farben**. Gemessen werden
    /// deshalb die Zahl belegter Farbeimer (4 bit je Kanal) und der Anteil der
    /// zwei häufigsten — ein Foto verteilt sich, ein Knopf nicht.
    #[test]
    #[ignore]
    fn colour_metrics_per_image() {
        let list = std::env::var("LIDL_SHEET_LIST").expect("LIDL_SHEET_LIST fehlt");
        let paths: Vec<String> = std::fs::read_to_string(&list)
            .expect("liste")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        println!("datei\teimer\ttop2%\tkanten%\tbreite\thoehe");
        for p in &paths {
            let Ok(img) = image::open(p) else { continue };
            let rgb = img.to_rgb8();
            let (w, h) = rgb.dimensions();
            let mut bins: HashMap<(u8, u8, u8), u32> = HashMap::new();
            for px in rgb.pixels() {
                *bins.entry((px[0] / 16, px[1] / 16, px[2] / 16)).or_default() += 1;
            }
            let total = (w * h).max(1);
            let mut counts: Vec<u32> = bins.values().copied().collect();
            counts.sort_unstable_by(|a, b| b.cmp(a));
            let top2: u32 = counts.iter().take(2).sum();
            // Kantenanteil: Nachbarpixel, die sich deutlich unterscheiden. Ein
            // Foto ist überall leicht unruhig, eine Fläche nur an ihrem Rand.
            let mut edges = 0u32;
            for y in 0..h {
                for x in 1..w {
                    let a = rgb.get_pixel(x - 1, y).0;
                    let b = rgb.get_pixel(x, y).0;
                    let d = (0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).max().unwrap_or(0);
                    if d > 24 {
                        edges += 1;
                    }
                }
            }
            let name = p.rsplit('/').next().unwrap_or(p);
            println!(
                "{name}\t{}\t{}\t{}\t{w}\t{h}",
                counts.len(),
                top2 * 100 / total,
                edges * 100 / total,
            );
        }
    }

    /// Messgerät, kein Test: liest ein lokales Prospekt-PDF und meldet, wie
    /// viele Angebote es trägt und für wie viele der Schnitt einen Streifen
    /// findet. `#[ignore]`, weil es eine Datei von außen braucht.
    #[test]
    #[ignore]
    fn measure_local_pdf() {
        let pdf = std::env::var("LIDL_PDF").expect("LIDL_PDF fehlt");
        let xml = run_pdftotext(std::path::Path::new(&pdf), "-bbox-layout").expect("pdftotext");
        let (offers, shots) = extract_offers_with_shots(&xml, "MEASURE", None, None);
        if let Ok(path) = std::env::var("LIDL_SHOT_DUMP") {
            let body: String = shots
                .iter()
                .map(|s| format!("{}\t{}\n", s.offer_id, s.page))
                .collect();
            std::fs::write(path, body).expect("streifen schreiben");
        }
        eprintln!("ANGEBOTE\t{}\tSTREIFEN\t{}", offers.len(), shots.len());
    }
}
