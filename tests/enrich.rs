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

/// **Der Import sortierte Küchengeräte als Lebensmittel ein** (gefunden am
/// 2026-08-06 beim Entschlacken der Top-Deals, behoben am 08.08.).
///
/// Derselbe Fehler aus zwei Richtungen, und beide Richtungen haben dieselbe
/// Ursache: `enrich` führte sein eigenes Wissen darüber, was Essen ist,
/// neben dem des Wörterbuchs. RUSSELL HOBBS stand längst auf dessen
/// NONFOOD-Liste und der Import wusste nichts davon.
#[test]
fn das_woerterbuch_entscheidet_was_essen_ist() {
    let kat = |t: &str, s: &str, c: &str| {
        enrich::enrich(t, Some(s).filter(|x| !x.is_empty()), Some(c).filter(|x| !x.is_empty()))
            .category
    };

    // Richtung 1: Non-Food im Essensregal. Die Rohkategorie der Kette heißt
    // „kochen-und-grillen" — das ist ihr Gang, nicht die Beschaffenheit der
    // Ware. Vorher beide „Vorräte & Kochen".
    assert_eq!(kat("GOURMETMAXX Ölsprüher und -spender", "", "kochen-und-grillen"), "Sonstiges");
    assert_eq!(kat("RUSSELL HOBBS Food Processor 27111-56", "", "kochen-und-grillen"), "Sonstiges");
    assert_eq!(kat("MEDION Toaster MD 15709 FAMILY", "", "kochen-und-grillen"), "Sonstiges");
    // Und der Toaster darf nicht über sein „toast" in den Backwaren landen.
    assert_eq!(kat("Doppelschlitz-Toaster STK 870 D2", "1 Stk", ""), "Sonstiges");

    // Richtung 2: Essen im Sonstiges-Topf, weil kein Keyword passte. Der
    // Begriff des Wörterbuchs weiß, in welches Regal es gehört.
    assert_eq!(kat("MAGGI Fix", "für Spaghetti Bolognese", ""), "Vorräte & Kochen");
    assert_eq!(kat("Lätta", "je 500-g-Becher", ""), "Molkerei & Eier");
    assert_eq!(kat("BAUER Cheestrings", "je 4 Stück", ""), "Molkerei & Eier");
    assert_eq!(kat("SOMERSBY Cider", "je 0,33 l", ""), "Alkohol");

    // Gegenprobe: Wo der Import schon richtig lag, ändert sich nichts.
    assert_eq!(kat("Frische Vollmilch", "1 l", "Milch"), "Molkerei & Eier");
    assert_eq!(kat("MÜHLENHOF Frisches Hackfleisch", "800 g", "fleisch-und-wurst"), "Fleisch & Wurst");
    assert_eq!(kat("K-CLASSIC Sonnenblumenöl", "1 l", ""), "Vorräte & Kochen");
    assert_eq!(kat("Wilkinson Hydro 5 Rasierklingen", "4 Stück", "drogerie"), "Drogerie");
}

/// **Neun Zeilen Essen standen unter „Sonstiges"** (gemessen in der
/// Backend-Runde vom 08.08., behoben am selben Tag).
///
/// Seit [#64] holt `enrich` das Regal über den Wörterbuch-Begriff. Kennt die
/// `RULES`-Tabelle den Begriff nicht, bleibt die Zeile im Sonstiges-Topf —
/// dem Fach, das sich das Unsortierbare mit der Aktionsware teilt.
///
/// Der teuerste Fall war nicht ein *fehlender*, sondern ein *falscher*
/// Eintrag: `tofu` stand bei den Non-Food-Zeilen und zeigte auf „Sonstiges".
/// Das ist für die Suche nach dem Regal ein gültiges Ergebnis, kein
/// fehlendes — der Begriff verdeckte damit jeden feineren Begriff derselben
/// Zeile (`hummus`, `krautsalat`), weil die Suche beim ersten Treffer endet.
///
/// `cargo run --example regal_audit` misst es nach: vorher 9, jetzt 0.
#[test]
fn essen_faellt_nicht_in_den_sonstiges_topf() {
    let kat = |t: &str, s: &str, c: &str| {
        enrich::enrich(t, Some(s).filter(|x| !x.is_empty()), Some(c).filter(|x| !x.is_empty()))
            .category
    };

    // Die fünf `tofu`-Zeilen des Korpus — alle über das Wort „vegan", denn
    // ein Produkt namens Tofu stand in keiner der Wochen.
    assert_eq!(kat("Bougatsa  Vegan", "", "Veganes"), "Vorräte & Kochen");
    assert_eq!(kat("Peanuts vegan", "", "Veganes"), "Vorräte & Kochen");
    assert_eq!(kat("Krautsalat griechischer Art vegan", "", "Veganes"), "Vorräte & Kochen");
    assert_eq!(kat("REWE Bio + vegan Hummus Natur", "je 200-g-Schale", ""), "Vorräte & Kochen");
    assert_eq!(kat("Bio-Tempeh 200 g, Natur", "0,2 kg", "Bio-Produkte"), "Vorräte & Kochen");
    assert_eq!(kat("Feine Speisestärke", "0.4 kg", "Backzutaten & -mischungen"), "Vorräte & Kochen");
    assert_eq!(kat("NATURGUT Bio Kapern Surfines", "je 100 g", "naturgut"), "Vorräte & Kochen");
    assert_eq!(kat("NATURGUT Bio Edamame", "", "naturgut"), "Obst & Gemüse");

    // Gegenprobe 1: Non-Food bleibt im Sonstiges-Topf. Der Weg über den
    // Begriff darf nur für Essen laufen.
    assert_eq!(kat("PARKSIDE Akku-Bohrschrauber", "", "werkzeug"), "Sonstiges");
    assert_eq!(kat("ESMARA Damen-Shirt", "", "bekleidung"), "Sonstiges");
    // Gegenprobe 2: Wer schon ein Regal hatte, behält es — der Eingriff gilt
    // der Tabelle, nicht dem Vorrang.
    assert_eq!(kat("Vegane Frikadellen", "", "Veganes"), "Fleisch & Wurst");
    // Der Haferdrink trägt `milch` und stand schon vorher in der Molkerei —
    // nachgesehen, nicht erwartet: „vegan" und „drink" sind gleich lang, bei
    // Gleichstand entscheidet die Tabellenreihenfolge, und die Zeile kommt
    // deshalb über den Begriff und nicht über das Keyword.
    assert_eq!(kat("Veganer Bio Haferdrink", "", "Veganes"), "Molkerei & Eier");
    assert_eq!(kat("Soja-Sauce", "0.5 l", ""), "Vorräte & Kochen");
    // Gegenprobe 3: „schoten" ist der kürzere Eintrag und darf die
    // Vanilleschote nicht aus den Backzutaten ziehen.
    assert_eq!(kat("Bourbon-Vanilleschoten", "2 Stück", ""), "Vorräte & Kochen");
    assert_eq!(kat("Kaiserschoten", "je 200-g-Pckg.", ""), "Obst & Gemüse");
}

/// Welche Begriffe die `RULES`-Tabelle in den Sonstiges-Topf schickt — als
/// Liste, damit sie nicht still wächst.
///
/// Der Test ist aus dem `tofu`-Fund entstanden und hat ihn beim ersten Lauf
/// gleich um sieben Geschwister erweitert. Bei diesen sieben ist „Sonstiges"
/// **richtig**: Handschuhe und Blumenerde sind kein Essen. Falsch war nur
/// `tofu`, und die Kosten trug nicht er selbst, sondern die feineren
/// Begriffe, die er verdeckte.
///
/// Seit `enrich` „Sonstiges" nicht mehr als Antwort zählt, ist die Liste
/// folgenlos — sie steht hier, weil ein **Essens**begriff darin ein Fehler
/// wäre, den sonst wieder nur eine Messung findet.
#[test]
fn welche_begriffe_kein_regal_haben() {
    let dict: serde_json::Value =
        serde_json::from_str(include_str!("../docs/matching-woerterbuch.json")).unwrap();
    let mut ohne_regal: Vec<String> = dict["begriffe"]
        .as_object()
        .unwrap()
        .keys()
        // Non-Food ist kein Begriff, sondern das Gegenteil davon.
        .filter(|b| *b != "nonfood")
        .filter(|b| {
            // Genauso gefragt wie in `enrich`: über einen Titel, der nur aus
            // dem Begriff besteht. `emoji_is_fallback` trennt „die Tabelle
            // sagt Sonstiges" von „die Tabelle kennt das Wort nicht".
            let e = enrich::enrich(b, None, None);
            e.category == FALLBACK_CATEGORY && !e.emoji_is_fallback
        })
        .cloned()
        .collect();
    ohne_regal.sort();
    assert_eq!(
        ohne_regal,
        [
            "blumenerde",
            "blumentopf",
            "gartenwerkzeug",
            "handschuhe",
            "socken",
            "taschenlampe",
            "zimmerpflanze",
        ],
        "Neuer Begriff ohne Regal — ist es Essen, gehört er in die RULES-Tabelle"
    );
}
