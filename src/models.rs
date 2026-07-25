use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    pub id: String,
    pub name: String,
    /// Filial-Koordinaten, wenn der Store-Finder sie liefert; nationale
    /// Platzhalter und ältere Datenbestände tragen None.
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lon: Option<f64>,
}

impl Market {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Market { id: id.into(), name: name.into(), lat: None, lon: None }
    }

    pub fn with_geo(mut self, lat: Option<f64>, lon: Option<f64>) -> Self {
        self.lat = lat;
        self.lon = lon;
        self
    }
}

/// Eine Filiale im Verzeichnis (`public.branches`).
///
/// Unterschied zu [`Market`]: `Market` ist „die Filiale, aus der dieser Lauf
/// gerade Angebote holt" und trägt nur, was der Angebots-Scraper braucht.
/// `Branch` ist ein Verzeichniseintrag, den ein Nutzer auf einer Karte
/// wiedererkennen soll — deshalb Adresse und Ort dazu.
///
/// Alle Adressfelder sind optional: Die acht Finder liefern
/// unterschiedlich viel, und eine Filiale ohne Straße ist immer noch besser
/// als keine. `market_id` und `chain` sind es nicht — ohne sie lässt sich die
/// Zeile weder scrapen noch anzeigen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub market_id: String,
    pub chain: String,
    pub name: String,
    pub street: Option<String>,
    pub plz: Option<String>,
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub source: String,
}

impl Branch {
    pub fn new(
        market_id: impl Into<String>,
        chain: impl Into<String>,
        name: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Branch {
            market_id: market_id.into(),
            chain: chain.into(),
            name: name.into(),
            street: None,
            plz: None,
            city: None,
            lat: None,
            lon: None,
            source: source.into(),
        }
    }

    pub fn with_address(
        mut self,
        street: Option<String>,
        plz: Option<String>,
        city: Option<String>,
    ) -> Self {
        self.street = street.filter(|s| !s.trim().is_empty());
        self.plz = plz.filter(|s| !s.trim().is_empty());
        self.city = city.filter(|s| !s.trim().is_empty());
        self
    }

    pub fn with_geo(mut self, lat: Option<f64>, lon: Option<f64>) -> Self {
        self.lat = lat;
        self.lon = lon;
        self
    }

    /// Verzeichniseintrag als der Markt, aus dem gescrapt wird.
    pub fn as_market(&self) -> Market {
        Market::new(&self.market_id, &self.name).with_geo(self.lat, self.lon)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub market_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub overline: Option<String>,
    pub price: Option<f64>,
    pub regular_price: Option<f64>,
    pub category: Option<String>,
    pub nutri_score: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub images: Vec<String>,
    pub biozid: bool,
    pub flyer_page: Option<i64>,
}

impl Offer {
    pub fn build_id(market_id: &str, title: &str, valid_from: Option<&str>) -> String {
        let date = valid_from.unwrap_or("unknown");
        format!("{market_id}_{title}_{date}")
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect()
    }
}
