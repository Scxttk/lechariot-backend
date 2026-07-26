//! Tests für die deterministische Produkt-Anreicherung (Kategorie + Emoji).

use lechariot::enrich::{self, CATEGORIES, FALLBACK_CATEGORY};

#[test]
fn kategorien_sind_stabil() {
    // Die App hardcodet diese Liste — Änderungen müssen bewusst zusammen
    // mit einem App-Update passieren, nicht nebenbei.
    assert_eq!(
        CATEGORIES,
        [
            "Obst & Gemüse",
            "Molkerei & Eier",
            "Fleisch & Wurst",
            "Fisch",
            "Backwaren",
            "Tiefkühl",
            "Süßes & Snacks",
            "Getränke",
            "Alkohol",
            "Vorräte & Kochen",
            "Drogerie",
            "Haushalt",
            "Tierbedarf",
            "Kinder",
            "Sonstiges",
        ]
    );
}

#[test]
fn jede_kategorie_hat_default_emoji() {
    for cat in CATEGORIES {
        assert!(!enrich::default_emoji(cat).is_empty(), "kein Default-Emoji für {cat}");
    }
}

/// Tabelle: (Titel, Untertitel, Rohkategorie) → (Kategorie, Emoji).
#[test]
fn knifflige_faelle() {
    let cases: &[(&str, Option<&str>, Option<&str>, &str, &str)] = &[
        // Compound-Wörter: das längste Keyword gewinnt
        ("Apfelschorle", None, None, "Getränke", "🧃"),
        ("Katzenmilch", None, None, "Tierbedarf", "🐱"),
        ("Kinderschokolade", None, None, "Süßes & Snacks", "🍫"),
        // Wortgrenzen: kurze Keywords nur am Wortanfang
        ("Preisknüller Deckenleuchte", None, None, "Sonstiges", "💡"),
        ("Milram Eiskaffee", None, None, "Getränke", "☕"),
        ("Eis am Stiel", None, None, "Tiefkühl", "🍦"),
        // Rohkategorie gewinnt bei der Kategorie, wenn eindeutig
        ("Gartenerbsen", None, Some("Tiefkühlkost"), "Tiefkühl", "🪴"),
        ("Grappa Cristallo", None, Some("Obstbrand"), "Alkohol", "🥃"),
        ("Alkoholfreies Pils", None, Some("Alkoholfreie Weine, Biere & Spirituosen"), "Getränke", "🍺"),
        // Weiches Trennzeichen im Titel
        ("Fertig\u{ad}gericht", None, None, "Vorräte & Kochen", "🍲"),
        // Rohkategorie als zweite Keyword-Quelle fürs Emoji
        ("Lavendel", None, Some("Pflanzen"), "Sonstiges", "🪴"),
        // Basisfälle
        ("Gut&Günstig H-Milch", None, None, "Molkerei & Eier", "🥛"),
        ("Rispentomaten", Some("500-g-Schale"), None, "Obst & Gemüse", "🍅"),
        ("Frisches Hähnchenbrustfilet", None, None, "Fleisch & Wurst", "🍗"),
        ("Persil Waschmittel", None, None, "Haushalt", "🧴"),
    ];
    for (title, subtitle, raw, want_cat, want_emoji) in cases {
        let e = enrich::enrich(title, *subtitle, *raw);
        assert_eq!(e.category, *want_cat, "Kategorie für {title:?} (raw {raw:?})");
        assert_eq!(e.emoji, *want_emoji, "Emoji für {title:?} (raw {raw:?})");
    }
}

#[test]
fn fallback_verhalten() {
    // Völlig unbekanntes Produkt: Sonstiges + Fallback-Emoji, nie null.
    let e = enrich::enrich("Xyzzy Frobnitz", None, None);
    assert_eq!(e.category, FALLBACK_CATEGORY);
    assert_eq!(e.emoji, "🛒");
    assert!(e.emoji_is_fallback);

    // Bekannte Rohkategorie, unbekannter Titel: Kategorie + deren Default-Emoji.
    let e = enrich::enrich("Xyzzy Frobnitz", None, Some("Drogerie, Tiernahrung"));
    assert_eq!(e.category, "Drogerie");
    assert_eq!(e.emoji, enrich::default_emoji("Drogerie"));
    assert!(e.emoji_is_fallback);
}

#[test]
fn enrich_liefert_nur_bekannte_kategorien() {
    // Auch quer durch die Regel-Tabelle darf nichts außerhalb des Enums landen.
    for title in ["Milch", "Bohrhammer", "Weißwein trocken", "Hundefutter", "irgendwas"] {
        let e = lechariot::enrich::enrich(title, None, None);
        assert!(CATEGORIES.contains(&e.category), "{title} → {}", e.category);
    }
}
