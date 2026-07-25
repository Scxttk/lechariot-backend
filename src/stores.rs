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
}

/// None: die Kette hat laut Store-Finder keine Filiale im Umkreis der PLZ
/// (nur bei Lidl/ALDI möglich — die übrigen Finder scheitern dann mit Err).
pub fn scrape_store(
    store: Store,
    zip: &str,
    cert: &str,
    key: &str,
) -> Result<Option<(Market, Vec<Offer>)>> {
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
    println!("Lade Angebote...");
    let offers = match store {
        Store::Rewe => scrapers::rewe::fetch_offers(&market, cert, key)?,
        Store::Penny => scrapers::penny::fetch_offers(&market)?,
        Store::Kaufland => scrapers::kaufland::fetch_offers(&market)?,
        Store::Lidl => scrapers::lidl::fetch_offers(&market, zip)?,
        Store::Netto => scrapers::netto::fetch_offers(&market)?,
        Store::AldiNord => scrapers::aldi_nord::fetch_offers(&market)?,
        Store::AldiSued => scrapers::aldi_sued::fetch_offers(&market)?,
        Store::Edeka => scrapers::edeka::fetch_offers(&market)?,
    };
    Ok(Some((market, offers)))
}

pub fn save_offers(db: &str, market: &Market, offers: &[Offer]) -> Result<()> {
    let conn = db::open(db)?;
    db::upsert_market(&conn, market)?;
    for offer in offers {
        db::upsert_offer(&conn, offer)?;
    }
    Ok(())
}
