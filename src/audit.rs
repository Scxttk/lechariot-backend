//! Bild-Probe: fragt nach, ob die in `public.offers.image_url` stehenden Bilder
//! überhaupt abrufbar sind.
//!
//! Der Anlass steht im Klartext im Projektprotokoll vom 2026-07-31: „Nichts in
//! dieser Kette fragt je nach, ob eine gerade geschriebene `image_url`
//! überhaupt abrufbar ist." Genau daran hingen Nettos Bilder vier Tage lang.
//! In der Datenbank sah alles gesund aus — jede Zeile trug eine URL — während
//! das Telefon durchweg das Kategoriesymbol zeigte, weil Nettos CDN jedem
//! schlichten Client mit 403 antwortet. Eine Zeile mit toter URL und eine Zeile
//! ohne URL sehen in SQL verschieden aus und auf dem Bildschirm gleich.
//!
//! Deshalb misst diese Probe nicht die Datenbank, sondern den Abruf: je Kette
//! eine Stichprobe der wirklich gespeicherten URLs, geholt auf demselben Weg,
//! den der Spiegel nimmt (System-curl für die geblockten Hosts, sonst reqwest).
//!
//! Läuft in CI, nicht auf dem Entwicklungsrechner: `netto-online.de` antwortet
//! dieser IP auf **jede** Anfrage mit 403, eine Messung von hier wäre also eine
//! Aussage über den Mac und nicht über die App.
//!
//! Braucht `SUPABASE_URL` und `SUPABASE_SERVICE_KEY` — dieselben Variablen wie
//! `push`/`prune-images`. Rein lesend.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::push::{PushConfig, config_from_env};
use crate::storage::{self, BUCKET};

/// PostgREST-Seitengröße beim Einlesen der Angebote.
const OFFERS_PAGE: usize = 1000;

pub struct AuditOptions {
    /// Wie viele verschiedene Bild-URLs je Kette abgerufen werden.
    pub sample: usize,
    /// Nur diese Kette prüfen (Wert aus `offers.market`); None = alle.
    pub market: Option<String>,
    /// Mit Exit-Code 1 enden, wenn eine Kette mit Bildern keine einzige
    /// abrufbare URL hat. Für die Nightly gedacht — ein einzelnes totes Bild
    /// ist Alltag, eine komplett tote Kette ist der Netto-Fall.
    pub fail_on_dead_chain: bool,
}

#[derive(Deserialize)]
struct OfferRow {
    market: Option<String>,
    image_url: Option<String>,
}

/// Woher ein Bild kommt. Die Unterscheidung ist der Kern der Probe: Nur eine
/// Bucket-URL gehört uns, alles andere hängt am Wohlwollen eines fremden CDNs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageKind {
    /// Unser Spiegel (`…/offer-images/…`).
    Mirror,
    /// Direkt beim Händler (http/https).
    Merchant,
    /// `file://` — ein Kachelbild, das den Spiegel nicht erreicht hat. Auf dem
    /// Telefon immer tot; siehe die 441 Zeilen vom 2026-07-31.
    Local,
}

pub fn image_kind(url: &str) -> ImageKind {
    if url.contains(&format!("/{BUCKET}/")) {
        ImageKind::Mirror
    } else if url.starts_with("file://") {
        ImageKind::Local
    } else {
        ImageKind::Merchant
    }
}

/// Was die Probe je Kette zusammenträgt.
#[derive(Default)]
pub struct ChainReport {
    pub offers: usize,
    pub with_image: usize,
    pub mirror: usize,
    pub merchant: usize,
    pub local: usize,
    /// Abgerufene Stichprobe: (URL, Kurzfassung des Ergebnisses, Bild?).
    pub probes: Vec<(String, String, bool)>,
}

impl ChainReport {
    pub fn checked(&self) -> usize {
        self.probes.len()
    }
    pub fn alive(&self) -> usize {
        self.probes.iter().filter(|(_, _, ok)| *ok).count()
    }
    pub fn dead(&self) -> usize {
        self.checked() - self.alive()
    }

    pub fn verdict(&self) -> Verdict {
        if self.with_image == 0 {
            return Verdict::KeineBilder;
        }
        match (self.checked(), self.alive()) {
            (0, _) => Verdict::KeineBilder,
            (_, 0) => Verdict::Tot,
            (n, a) if a == n => Verdict::Gesund,
            _ => Verdict::Angeschlagen,
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Jede geprüfte URL liefert ein Bild.
    Gesund,
    /// Ein Teil der Stichprobe ist tot — kann ein einzelnes verschwundenes
    /// Produktfoto sein, muss beobachtet werden.
    Angeschlagen,
    /// Kein einziges Bild der Stichprobe ist abrufbar. Genau der Netto-Fall
    /// vom 31.07.: 1.722 Angebote, jede Zeile mit URL, keins sichtbar.
    Tot,
    /// Die Kette speichert gar keine Bilder — nichts zu prüfen, kein Fehler.
    KeineBilder,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Gesund => "gesund",
            Verdict::Angeschlagen => "angeschlagen",
            Verdict::Tot => "TOT",
            Verdict::KeineBilder => "keine Bilder",
        }
    }
}

/// Gleichmäßig verteilte Stichprobe aus einer sortierten Liste — ohne Zufall,
/// damit zwei Läufe dieselbe Auswahl prüfen und ein Unterschied wirklich einer
/// ist. Die ersten n zu nehmen wäre schlechter: content-adressierte Pfade
/// sortieren nach Hash, aber Händler-URLs sortieren nach Pfad und damit nach
/// Kategorie — die Stichprobe säße dann in einer Ecke des Prospekts.
pub fn sample_evenly<T>(items: &[T], n: usize) -> Vec<&T> {
    if n == 0 || items.is_empty() {
        return Vec::new();
    }
    if items.len() <= n {
        return items.iter().collect();
    }
    (0..n).map(|i| &items[i * items.len() / n]).collect()
}

pub fn run(opts: &AuditOptions, cfg: Option<&PushConfig>) -> Result<()> {
    let owned;
    let cfg = match cfg {
        Some(c) => c,
        None => {
            owned = config_from_env()?;
            &owned
        }
    };
    let client = reqwest::blocking::Client::new();
    // Wie beim Spiegeln: keinen Redirects folgen, damit ein 3xx nicht auf ein
    // internes Ziel zeigt (SSRF) — und damit ein Redirect als das gemessen
    // wird, was er für die App ist.
    let probe_client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("HTTP-Client für die Bild-Probe konnte nicht erstellt werden")?;

    let rows = read_offers(&client, cfg, opts.market.as_deref())?;
    println!("Bild-Probe über {} Angebote (Stichprobe {} je Kette).\n", rows.len(), opts.sample);

    // Je Kette zählen und die verschiedenen URLs sammeln. `BTreeSet`, weil sich
    // Angebote ein Bild teilen (content-adressiert) — geprüft wird das Bild,
    // nicht die Zeile.
    let mut reports: BTreeMap<String, ChainReport> = BTreeMap::new();
    let mut urls: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in &rows {
        let chain = row.market.clone().unwrap_or_else(|| "(ohne Kette)".to_string());
        let r = reports.entry(chain.clone()).or_default();
        r.offers += 1;
        let Some(url) = row.image_url.as_deref().filter(|u| !u.is_empty()) else { continue };
        r.with_image += 1;
        match image_kind(url) {
            ImageKind::Mirror => r.mirror += 1,
            ImageKind::Merchant => r.merchant += 1,
            ImageKind::Local => r.local += 1,
        }
        urls.entry(chain).or_default().insert(url.to_string());
    }

    for (chain, report) in reports.iter_mut() {
        let Some(set) = urls.get(chain) else { continue };
        let list: Vec<&String> = set.iter().collect();
        for url in sample_evenly(&list, opts.sample) {
            let p = storage::probe(&probe_client, url);
            report.probes.push((url.to_string(), p.describe(), p.is_image()));
        }
    }

    print_table(&reports);

    // Maschinenlesbar für die Zusammenfassung des Workflows — eine Zeile je
    // Kette, Tabulator getrennt.
    for (chain, r) in &reports {
        println!(
            "BILD-PROBE\t{chain}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            r.offers,
            r.with_image,
            r.mirror,
            r.merchant,
            r.local,
            r.checked(),
            r.alive(),
            r.verdict().label()
        );
    }

    for (chain, r) in &reports {
        for (url, note, ok) in &r.probes {
            if !ok {
                println!("WARNUNG [{chain}] Bild nicht abrufbar ({note}): {url}");
            }
        }
        if r.local > 0 {
            println!(
                "WARNUNG [{chain}] {} Angebot(e) tragen eine file://-URL — die zeigt auf einen Runner, den es nicht mehr gibt.",
                r.local
            );
        }
    }

    let tot: Vec<&String> = reports
        .iter()
        .filter(|(_, r)| r.verdict() == Verdict::Tot)
        .map(|(c, _)| c)
        .collect();
    if !tot.is_empty() {
        let namen = tot.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
        if opts.fail_on_dead_chain {
            bail!(
                "{namen}: keine einzige geprüfte Bild-URL ist abrufbar. In der Datenbank sieht die Kette gesund aus, in der App zeigt sie nur Emojis."
            );
        }
        println!("WARNUNG: {namen} — keine einzige geprüfte Bild-URL ist abrufbar.");
    }
    Ok(())
}

fn print_table(reports: &BTreeMap<String, ChainReport>) {
    println!(
        "{:<12} {:>8} {:>8} {:>8} {:>9} {:>7} {:>8} {:>7}  Urteil",
        "Kette", "Angebote", "mit Bild", "Spiegel", "Händler", "file://", "geprüft", "lebt"
    );
    for (chain, r) in reports {
        println!(
            "{:<12} {:>8} {:>8} {:>8} {:>9} {:>7} {:>8} {:>7}  {}",
            chain,
            r.offers,
            r.with_image,
            r.mirror,
            r.merchant,
            r.local,
            r.checked(),
            r.alive(),
            r.verdict().label()
        );
    }
    println!();
}

/// Alle Angebote (Kette + Bild-URL) einlesen, paginiert bis leer.
fn read_offers(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    market: Option<&str>,
) -> Result<Vec<OfferRow>> {
    let url = format!("{}/rest/v1/offers", cfg.base_url);
    let mut all = Vec::new();
    let mut offset = 0usize;
    loop {
        let mut query: Vec<(&str, String)> = vec![
            ("select", "market,image_url".to_string()),
            ("limit", OFFERS_PAGE.to_string()),
            ("offset", offset.to_string()),
            ("order", "id.asc".to_string()),
        ];
        if let Some(m) = market {
            query.push(("market", format!("eq.{m}")));
        }
        let resp = client
            .get(&url)
            .header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .query(&query)
            .send()
            .with_context(|| format!("offers lesen fehlgeschlagen ({})", cfg.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            let excerpt: String = body.chars().take(200).collect();
            bail!("offers lesen: HTTP {status}: {excerpt}");
        }
        let page: Vec<OfferRow> = resp.json().context("offers-Antwort nicht parsebar")?;
        let n = page.len();
        if n == 0 {
            break;
        }
        all.extend(page);
        offset += n;
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_url_counts_as_mirror_merchant_url_does_not() {
        assert_eq!(
            image_kind("https://x.supabase.co/storage/v1/object/public/offer-images/ab.webp"),
            ImageKind::Mirror
        );
        assert_eq!(
            image_kind("https://www.netto-online.de/media_nfs/1.jpg"),
            ImageKind::Merchant
        );
        assert_eq!(image_kind("file:///tmp/lidl/LIDL_1.png"), ImageKind::Local);
    }

    // Die Stichprobe soll über die ganze Liste laufen, nicht über ihren Anfang:
    // Händler-URLs sortieren nach Pfad und damit nach Kategorie, die ersten n
    // wären sonst alle aus derselben Prospektecke.
    #[test]
    fn sample_spreads_over_the_whole_list() {
        let items: Vec<usize> = (0..100).collect();
        let picked: Vec<usize> = sample_evenly(&items, 5).into_iter().copied().collect();
        assert_eq!(picked, vec![0, 20, 40, 60, 80]);
    }

    #[test]
    fn sample_never_asks_for_more_than_there_is() {
        let items = vec!["a", "b"];
        assert_eq!(sample_evenly(&items, 5).len(), 2);
        assert_eq!(sample_evenly(&items, 0).len(), 0);
        let leer: Vec<&str> = Vec::new();
        assert_eq!(sample_evenly(&leer, 3).len(), 0);
    }

    // Der Netto-Fall: jede Zeile trägt eine URL, keine ist abrufbar. Das ist
    // NICHT dasselbe wie eine Kette ohne Bilder, und die Probe muss die beiden
    // auseinanderhalten — sonst meldet sie den einen Zustand als den anderen.
    #[test]
    fn a_chain_whose_every_image_is_dead_is_not_a_chain_without_images() {
        let mut tot = ChainReport { offers: 1722, with_image: 1722, merchant: 1722, ..Default::default() };
        tot.probes = vec![("u".into(), "403".into(), false); 5];
        assert_eq!(tot.verdict(), Verdict::Tot);

        let ohne = ChainReport { offers: 300, ..Default::default() };
        assert_eq!(ohne.verdict(), Verdict::KeineBilder);
    }

    #[test]
    fn one_dead_image_is_not_a_dead_chain() {
        let mut r = ChainReport { offers: 100, with_image: 100, mirror: 100, ..Default::default() };
        r.probes = vec![
            ("a".into(), "200 image/webp".into(), true),
            ("b".into(), "404".into(), false),
        ];
        assert_eq!(r.verdict(), Verdict::Angeschlagen);
        assert_eq!(r.dead(), 1);

        r.probes[1].2 = true;
        assert_eq!(r.verdict(), Verdict::Gesund);
    }
}
