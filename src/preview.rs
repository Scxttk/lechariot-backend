//! Die Vorschau auf die Angebote der kommenden Woche — hinter einem Schalter.
//!
//! **Voreinstellung AUS.** Solange `LECHARIOT_PREVIEW` nicht gesetzt ist, lädt
//! und schreibt jeder Lauf byte-genau das, was er vorher geladen und
//! geschrieben hat; der Zweig lässt sich also mergen, ohne die Nightly zu
//! ändern. Angeschaltet wird über die Umgebung (`LECHARIOT_PREVIEW=1`) oder
//! über den Workflow-Eingang `preview` in `nightly.yml`.
//!
//! Warum ein Schalter und keine dritte Betriebsart: Die Vorschau verdoppelt
//! grob die Zeilen in `offers`, und diese Entscheidung gehört Scott. Die
//! Messung dazu steht im [[Le Chariot Backlog]] unter „Phase 0".
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
    pub const ALREADY_LIVE: &[&str] = &["Penny", "NORMA"];

    /// Ketten, für die dieser Zweig den Weg gebaut hat.
    pub const BUILT: &[&str] = &["Kaufland", "Lidl", "ALDI Nord"];

    /// Nachweislich vorhanden, aber hier nicht gebaut — mit dem Grund.
    pub const MEASURED_NOT_BUILT: &[(&str, &str)] = &[
        ("ALDI SÜD", "Seite /angebote/<datum> liegt hinter Akamai; der API-Weg \
                      kennt keinen Datumsparameter"),
        ("Netto", "Der Vorschau-Prospekt hängt an der Filialwahl per Cookie"),
        ("REWE", "Der Reiter rendert erst mit Marktwahl; braucht den \
                  zertifikatsgebundenen Abruf"),
    ];

    /// Veröffentlicht nachweislich nichts im Voraus. Die App sagt das,
    /// statt eine leere Zusage zu machen.
    pub const NO_PREVIEW: &[&str] = &["EDEKA"];
}
