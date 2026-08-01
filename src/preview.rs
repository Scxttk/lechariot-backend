//! Die Vorschau auf die Angebote der kommenden Woche — hinter einem Schalter.
//!
//! **Der Code bleibt AUS, der Zeitplan ist AN.** Ohne gesetztes
//! `LECHARIOT_PREVIEW` lädt und schreibt ein Lauf byte-genau das, was er
//! vorher geschrieben hat — ein Testlauf oder ein fremder Aufruf zieht die
//! Folgewoche also nicht versehentlich mit. Die Nightly setzt die Variable
//! seit dem 2026-08-01 selbst (`nightly.yml`, Scotts Freigabe); das Häkchen
//! am Hand-Start ist seither ein *Aus*-Schalter.
//!
//! Warum ein Schalter und keine dritte Betriebsart: Die Vorschau verdoppelt
//! grob die Zeilen in `offers`, und diese Entscheidung gehörte Scott. Die
//! Messung dazu steht im [[Le Chariot Backlog]] unter „Phase 0" (+3,3 MB
//! Heap bei 500 MB Grenze), das Ergebnis unter „Phase 2": 6 615 Zeilen der
//! Folgewoche in der Produktion, aus fünf Ketten statt zwei.
//!
//! Welche Ketten mitmachen, steht in [`sources`] — gemessen, nicht vermutet.

/// Ist die Vorschau eingeschaltet?
///
/// Alles außer `0`, `false`, `nein` und leer zählt als an; ein gesetzter, aber
/// unverständlicher Wert soll nicht still auf AUS zurückfallen.
pub fn enabled() -> bool {
    match std::env::var("LECHARIOT_PREVIEW") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "nein" || v == "off")
        }
        Err(_) => false,
    }
}

/// Was am 2026-08-01 nachgemessen wurde, Kette für Kette.
///
/// Die Tabelle ist Dokumentation mit Test dahinter (`preview_sources_match_the_code`)
/// — sie soll nicht auseinanderlaufen wie die Notiz, die behauptete, die App
/// filtere Zukunftszeilen weg.
pub mod sources {
    /// Liefert diese Kette schon heute Zeilen der Folgewoche, ohne Zutun?
    ///
    /// **Netto steht hier seit dem 2026-08-01, und das war eine Messung, keine
    /// Annahme.** Am Samstagabend lieferten die Seiten `/filialangebote/{1,2,4,5}`,
    /// die `fetch_offers` ohnehin liest, 255 Angebote — **alle 255 mit
    /// `valid_from` in der Zukunft** (03.08. 182 · 06.08. 59 · 07.08. 4 ·
    /// 08.08. 10) und keines aus der laufenden Woche. Netto schaltet seine
    /// Seiten selbst um; ein eigener Vorschau-Weg wäre Arbeit ohne Ertrag.
    ///
    /// Die frühere Notiz „der Vorschau-Prospekt hängt an der Filialwahl per
    /// Cookie" beschrieb den Prospekt-Betrachter — nicht die Seiten, die der
    /// Scraper liest.
    pub const ALREADY_LIVE: &[&str] = &["Penny", "NORMA", "Netto"];

    /// Ketten, für die dieser Zweig den Weg gebaut hat.
    pub const BUILT: &[&str] = &["Kaufland", "Lidl", "ALDI Nord", "REWE"];

    /// Nachweislich vorhanden, aber hier nicht gebaut — mit dem Grund.
    ///
    /// **Der Grund für ALDI SÜD war bis zum 2026-08-01 falsch.** „Liegt hinter
    /// Akamai" stimmte nicht: Mit dem Header-Satz des Scrapers antworten die
    /// datierten Seiten sehr wohl (1 142 438 B, 222 Preis-Treffer). Sie
    /// antworten nur **immer gleich** — `2026-08-03`, `-06` und `-07` liefern
    /// byte-gleich dieselbe Seite (`sha=80fb3a44b13d`). Der Datumsteil im Pfad
    /// ist Dekoration; die Seite nennt das Datum im Text und ändert sich nicht.
    /// Ohne `__NEXT_DATA__` greift auch der ALDI-Nord-Parser nicht.
    pub const MEASURED_NOT_BUILT: &[(&str, &str)] = &[
        ("ALDI SÜD", "Die datierten Seiten antworten, liefern aber für jedes \
                      Datum byte-gleich dieselbe Seite; der API-Weg kennt \
                      keinen Datumsparameter"),
    ];

    /// Veröffentlicht nachweislich nichts im Voraus. Die App sagt das,
    /// statt eine leere Zusage zu machen.
    pub const NO_PREVIEW: &[&str] = &["EDEKA"];
}
