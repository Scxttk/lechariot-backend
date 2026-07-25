use anyhow::Result;
use clap::ValueEnum;

use crate::models::{Market, Offer};
use crate::{db, scrapers};

#[derive(Clone, Copy, ValueEnum)]
pub enum Store {
    Rewe,
    Penny,
    Kaufland,
    Lidl,
    Netto,
    AldiNord,
    AldiSued,
    Edeka,
}

impl Store {
    pub const ALL: [Store; 8] = [
        Store::Rewe,
        Store::Penny,
        Store::Kaufland,
        Store::Lidl,
        Store::Netto,
        Store::AldiNord,
        Store::AldiSued,
        Store::Edeka,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Store::Rewe => "Rewe",
            Store::Penny => "Penny",
            Store::Kaufland => "Kaufland",
            Store::Lidl => "Lidl",
            Store::Netto => "Netto",
            Store::AldiNord => "Aldi Nord",
            Store::AldiSued => "Aldi Süd",
            Store::Edeka => "Edeka",
        }
    }

    // Anzeigename der Kette im Supabase-Schema (Spalte `market`)
    pub fn chain(&self) -> &'static str {
        match self {
            Store::Rewe => "REWE",
            Store::Penny => "Penny",
            Store::Kaufland => "Kaufland",
            Store::Lidl => "Lidl",
            Store::Netto => "Netto",
            Store::AldiNord => "ALDI Nord",
            Store::AldiSued => "ALDI SÜD",
            Store::Edeka => "EDEKA",
        }
    }

    /// Gegenstück zu `chain()`: gespeicherten Kettennamen zurück auf den Store
    /// abbilden. Unbekannte Werte liefern None, damit der Push sie verwirft
    /// statt sie ungeprüft nach `offers.market` durchzureichen.
    pub fn from_chain(chain: &str) -> Option<Store> {
        Store::ALL.into_iter().find(|s| s.chain() == chain)
    }

    /// Ketten, deren Angebotskatalog bundesweit derselbe ist und die deshalb
    /// **genau einmal** gespeichert werden, unter einer synthetischen
    /// National-ID statt unter jeder einzelnen Filiale.
    ///
    /// Gemessen am 2026-07-25 gegen die Live-Datenbank: ALDI Nord stand mit
    /// 2.633 Zeilen in 11 Regionen, die sich in **genau einem Angebot**
    /// unterschieden — und der Trennstrich lag exakt am Sync-Zeitpunkt, nicht
    /// an der Region. Die Kopien haben also den Scrape-Zeitpunkt gespeichert,
    /// nicht regionale Wahrheit.
    ///
    /// **Lidl gehört ausdrücklich nicht dazu.** Die Kette galt früher als
    /// national (der marktguru-Weg lieferte bundesweit dasselbe), aber seit
    /// der Prospekt-Quelle hängt der Katalog an der Filiale — `fetch_offers`
    /// bekommt die PLZ übergeben und lädt den Prospekt dieses Marktes.
    pub fn stores_nationally(self) -> bool {
        matches!(self, Store::AldiNord | Store::AldiSued)
    }

    /// Die synthetische National-Filiale einer solchen Kette
    /// (`ALDI_NORD_DE` / `ALDI_SUED_DE`); None für alle übrigen.
    pub fn national_market(self) -> Option<Market> {
        match self {
            Store::AldiNord => Some(scrapers::aldi_nord::national()),
            Store::AldiSued => Some(scrapers::aldi_sued::national()),
            _ => None,
        }
        .map(|m| m.with_chain(self.chain()))
    }
}

/// Quelle der Lidl-Angebote.
///
/// Lidl ist die einzige Kette, die ihre Angebote über einen Dritten
/// (api.marktguru.de) bezieht, und mit rund 30 % aller Zeilen zugleich die
/// größte. Beide Alternativen lesen Lidls eigenen Wochenprospekt und
/// unterscheiden sich nur darin, wie sie Preis und Produktname zusammen-
/// bringen. `LIDL_SOURCE` entscheidet:
///
/// * `prospekt` — Kachelbildung über Wortkoordinaten
///   (`scrapers::lidl_prospekt`). Braucht `pdftotext`, sonst nichts.
/// * `prospekt-llm` — dieselbe Textebene, seitenweise durch ein Sprachmodell
///   (`scrapers::lidl_llm`). Braucht zusätzlich `ANTHROPIC_API_KEY`, liest
///   dafür auch Kacheln ohne saubere Marke-Name-Struktur (Obst, Gemüse,
///   Non-Food), an denen die Geometrie scheitert.
/// * alles andere oder nicht gesetzt — marktguru (Stand heute der Standard)
///
/// Der Schalter ist bewusst eine Umgebungsvariable und kein CLI-Flag: So
/// lässt sich ein einzelner nightly-Lauf umstellen, ohne die Aufrufe in
/// `sync_region` und `fetch_all` anzufassen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LidlSource {
    Marktguru,
    Prospekt,
    ProspektLlm,
}

pub fn lidl_source() -> LidlSource {
    match std::env::var("LIDL_SOURCE") {
        Ok(v) if v.eq_ignore_ascii_case("prospekt") => LidlSource::Prospekt,
        Ok(v) if v.eq_ignore_ascii_case("prospekt-llm") => LidlSource::ProspektLlm,
        _ => LidlSource::Marktguru,
    }
}

/// None: die Kette hat laut Store-Finder keine Filiale im Umkreis der PLZ
/// (nur bei Lidl/ALDI möglich — die übrigen Finder scheitern dann mit Err).
pub fn scrape_store(
    store: Store,
    zip: &str,
    cert: &str,
    key: &str,
) -> Result<Option<(Market, Vec<Offer>)>> {
    let Some(market) = find_market(store, zip, cert, key)? else {
        return Ok(None);
    };
    let offers = fetch_offers(store, &market, zip, cert, key)?;
    Ok(Some((market, offers)))
}

/// Nur die Filiale suchen, ohne Angebote zu laden — die erste Hälfte von
/// [`scrape_store`].
///
/// Eigener Einstieg für die national gespeicherten Ketten: Ob ALDI in einer
/// Region überhaupt vertreten ist, bleibt eine regionale Frage (der
/// Aldi-Äquator verläuft mitten durch Deutschland), die Angebote sind es
/// nicht. Der Regions-Sync fragt deshalb nur noch nach der Filiale.
pub fn find_market(store: Store, zip: &str, cert: &str, key: &str) -> Result<Option<Market>> {
    println!("Suche {}-Markt für PLZ {zip}...", store.label());
    let market = match store {
        Store::Rewe => scrapers::rewe::find_market(zip, cert, key)?,
        Store::Penny => scrapers::penny::find_market(zip)?,
        Store::Kaufland => scrapers::kaufland::find_market(zip)?,
        Store::Netto => scrapers::netto::find_market(zip)?,
        Store::Edeka => scrapers::edeka::find_market(zip)?,
        Store::Lidl => match scrapers::lidl::find_market(zip)? {
            Some(m) => m,
            None => return Ok(None),
        },
        Store::AldiNord => match scrapers::aldi_nord::find_market(zip)? {
            Some(m) => m,
            None => return Ok(None),
        },
        Store::AldiSued => match scrapers::aldi_sued::find_market(zip)? {
            Some(m) => m,
            None => return Ok(None),
        },
    };
    // Kette hier einmal zentral stempeln: der Aufrufer kennt sie exakt, der
    // Push muss sie später nicht mehr aus ID und Filialname erraten.
    let market = market.with_chain(store.chain());
    println!("Markt gefunden: {} (ID: {})", market.name, market.id);
    Ok(Some(market))
}

/// Angebote **dieser** Filiale holen, ohne vorher zu suchen.
///
/// Die zweite Hälfte von [`scrape_store`], als eigener Einstieg: Seit
/// migration_v13 gehören Angebote der Filiale, und die App wählt sie selbst
/// aus dem Verzeichnis (`public.branches`). Dann gibt es nichts mehr zu
/// finden — die ID steht schon fest.
///
/// `zip` braucht nur Lidl (die marktguru-Abfrage filtert über die PLZ, nicht
/// über die Filial-ID); alle übrigen Ketten binden die Filiale über
/// `market.id`: REWE als `-market`-Parameter, Kaufland über das Cookie
/// `x-aem-variant`, Netto über `netto_user_stores_id`, EDEKA über den
/// Marktpfad.
pub fn fetch_offers(
    store: Store,
    market: &Market,
    zip: &str,
    cert: &str,
    key: &str,
) -> Result<Vec<Offer>> {
    println!("Lade Angebote...");
    Ok(match store {
        Store::Rewe => scrapers::rewe::fetch_offers(market, cert, key)?,
        Store::Penny => scrapers::penny::fetch_offers(market)?,
        Store::Kaufland => scrapers::kaufland::fetch_offers(market)?,
        // `LIDL_SOURCE` wählt die Quelle — dieselbe Filiale, anderer Weg
        // (siehe lidl_source).
        Store::Lidl => match lidl_source() {
            LidlSource::Prospekt => scrapers::lidl_prospekt::fetch_offers(market, zip)?,
            LidlSource::ProspektLlm => scrapers::lidl_llm::fetch_offers(market, zip)?,
            LidlSource::Marktguru => scrapers::lidl::fetch_offers(market, zip)?,
        },
        Store::Netto => scrapers::netto::fetch_offers(market)?,
        Store::AldiNord => scrapers::aldi_nord::fetch_offers(market)?,
        Store::AldiSued => scrapers::aldi_sued::fetch_offers(market)?,
        Store::Edeka => scrapers::edeka::fetch_offers(market)?,
    })
}

pub fn save_offers(db: &str, market: &Market, offers: &[Offer]) -> Result<()> {
    let conn = db::open(db)?;
    db::upsert_market(&conn, market)?;
    for offer in offers {
        db::upsert_offer(&conn, offer)?;
    }
    Ok(())
}
