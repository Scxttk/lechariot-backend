//! Die Einkaufszettel-Probe: ein von Hand gepflegter Prüfstand für das
//! Wörterbuch.
//!
//! **Wozu, wenn es schon eine Abdeckungsquote gibt?** Weil eine Quote nicht
//! sagt, ob die *richtigen* Zeilen herauskommen. Am 2026-08-01 stieg die
//! Abdeckung von 17,7 % auf 10,1 % ungetaggt — und in derselben Runde steckten
//! vierzehn Fehltreffer, die nur aufgefallen sind, weil jemand die geänderten
//! Zeilen einzeln angesehen hat. Dieser Test dreht die Richtung um: Nicht
//! „wie viel ist getaggt", sondern „findet mein Zettel, was ich meine".
//!
//! Gepflegt wird `tests/fixtures/einkaufszettel.tsv` von Hand — Regal und
//! Zettel sind erfunden. Das ist Absicht: Ein Regal aus echten Prospektzeilen
//! veraltet jede Woche und prüft dann die Angebotslage statt das Wörterbuch.
//!
//! Zwei Erwartungsarten, und der Unterschied ist der Punkt:
//!
//! * `ZETTEL` muss stimmen — fehlt ein Treffer oder kommt einer dazu, ist der
//!   Test rot.
//! * `WUNSCH` darf noch scheitern. Der Test zählt die offenen Wünsche und
//!   schreibt sie hin. So kann jemand aufschreiben, was das Wörterbuch können
//!   *soll*, ohne die Latte zu reißen, und beim nächsten Lauf sieht man, ob es
//!   besser geworden ist. Wird ein Wunsch erfüllt, sagt der Test es und will
//!   die Zeile als `ZETTEL` sehen — sonst verrottet der Fortschritt wieder.

use lechariot::matching::match_keys;

/// Ein erfundenes Angebot aus dem Regal.
struct Angebot {
    titel: String,
    untertitel: String,
    kategorie: String,
    tags: Vec<String>,
}

/// Eine Zeile des Zettels: ein Listenwort und die Titel, die es finden soll.
struct Erwartung {
    wort: String,
    titel: Vec<String>,
    pflicht: bool,
    verboten: bool,
    zeile: usize,
}

/// Wortzerlegung wie in der App (`OfferMatcher.tokens`): alles, was kein
/// Buchstabe ist, trennt; Einzelbuchstaben zählen nicht.
fn worte(s: &str) -> Vec<String> {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphabetic() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| w.chars().count() >= 2)
        .map(str::to_string)
        .collect()
}

/// Tippfehler-Toleranz wie in der App: erst ab fünf Zeichen und nur bei
/// **verschiedener** Länge. Gleich lang und ein Buchstabe anders wäre ein
/// anderes Wort — „Butter" ist nicht „Bitter" (echte Nutzermeldung, 21.07.).
fn wort_passt(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    if la < 5 || lb < 5 || la == lb {
        return false;
    }
    levenshtein(a, b) <= 1
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut zeile: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut vorher = zeile[0];
        zeile[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let kosten = if ca == cb { 0 } else { 1 };
            let neu = (zeile[j] + 1).min(zeile[j + 1] + 1).min(vorher + kosten);
            vorher = zeile[j + 1];
            zeile[j + 1] = neu;
        }
    }
    zeile[b.len()]
}

/// Die Suchregel der App, hier nachgebildet (`OfferMatcher.matches`): Jedes
/// Wort der Anfrage muss erfüllt sein — entweder steht es im Titel, oder ein
/// Tag des Angebots trägt es.
///
/// Die Begriffe eines Suchworts holt sich der Test über `match_keys` auf dem
/// Wort selbst. Das ist dieselbe Abbildung, die die App über
/// `MatchDictionary.terms(forToken:)` benutzt — beide lesen die exact-Listen
/// derselben Datei.
fn findet(wort: &str, angebot: &Angebot) -> bool {
    let anfrage = worte(wort);
    if anfrage.is_empty() {
        return false;
    }
    let titelworte = worte(&angebot.titel);
    anfrage.iter().all(|q| {
        if titelworte.iter().any(|t| wort_passt(q, t)) {
            return true;
        }
        // `nonfood` fliegt heraus, und das ist kein Detail: Es ist ein
        // Ausschluss-Vermerk, kein Begriff, den jemand tippt. Die App kennt
        // ihn auf der Anfrageseite gar nicht (`MatchDictionary` liest nur die
        // exact-Listen). Ohne diese Zeile fänden sich alle Non-Food-Zeilen
        // gegenseitig — „Rasierklingen" holte „Petersilie im Topf", weil
        // `topf` in der Non-Food-Liste steht.
        let begriffe: Vec<String> = match_keys(q, None, None)
            .into_iter()
            .filter(|b| b != lechariot::matching::NONFOOD_KEY)
            .collect();
        angebot
            .tags
            .iter()
            .any(|tag| wort_passt(q, tag) || begriffe.iter().any(|b| b == tag))
    })
}

fn lade() -> (Vec<Angebot>, Vec<Erwartung>) {
    let roh = include_str!("fixtures/einkaufszettel.tsv");
    let mut regal = Vec::new();
    let mut zettel = Vec::new();
    for (nr, zeile) in roh.lines().enumerate() {
        let zeile = zeile.trim_end();
        if zeile.trim().is_empty() || zeile.starts_with('#') {
            continue;
        }
        let feld: Vec<&str> = zeile.split('\t').collect();
        match feld[0] {
            "REGAL" => {
                let titel = feld.get(1).unwrap_or(&"").trim().to_string();
                let untertitel = feld.get(2).unwrap_or(&"").trim().to_string();
                let kategorie = feld.get(3).unwrap_or(&"").trim().to_string();
                let tags = match_keys(
                    &titel,
                    Some(untertitel.as_str()).filter(|s| !s.is_empty()),
                    Some(kategorie.as_str()).filter(|s| !s.is_empty()),
                );
                regal.push(Angebot { titel, untertitel, kategorie, tags });
            }
            art @ ("ZETTEL" | "WUNSCH" | "NICHT" | "NICHT-WUNSCH") => zettel.push(Erwartung {
                wort: feld.get(1).unwrap_or(&"").trim().to_string(),
                titel: feld
                    .get(2)
                    .unwrap_or(&"")
                    .split('|')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .collect(),
                pflicht: art == "ZETTEL" || art == "NICHT",
                verboten: art.starts_with("NICHT"),
                zeile: nr + 1,
            }),
            anderes => panic!("Zeile {}: unbekannte Art {anderes:?}", nr + 1),
        }
    }
    (regal, zettel)
}

#[test]
fn der_einkaufszettel_findet_was_er_meinen_soll() {
    let (regal, zettel) = lade();
    assert!(!regal.is_empty() && !zettel.is_empty(), "Prüfstand ist leer");

    // Ein Regal, in dem ein Titel zweimal steht, macht jede Erwartung
    // mehrdeutig — dann prüft der Test seine eigene Fixture, nicht das
    // Wörterbuch.
    for (i, a) in regal.iter().enumerate() {
        assert!(
            !regal[i + 1..].iter().any(|b| b.titel == a.titel),
            "Der Titel {:?} steht zweimal im Regal",
            a.titel
        );
    }
    for e in &zettel {
        for t in &e.titel {
            assert!(
                regal.iter().any(|a| &a.titel == t),
                "Zeile {}: {:?} erwartet {:?} — das steht nicht im Regal",
                e.zeile,
                e.wort,
                t
            );
        }
    }

    let mut fehler: Vec<String> = Vec::new();
    let mut offene_wuensche: Vec<String> = Vec::new();
    let mut erfuellte_wuensche: Vec<String> = Vec::new();

    for e in &zettel {
        let gefunden: Vec<&str> = regal
            .iter()
            .filter(|a| findet(&e.wort, a))
            .map(|a| a.titel.as_str())
            .collect();
        let fehlt: Vec<&String> = e
            .titel
            .iter()
            .filter(|t| !gefunden.contains(&t.as_str()))
            .collect();
        let zuviel: Vec<&&str> = gefunden
            .iter()
            .filter(|g| !e.titel.iter().any(|t| t == **g))
            .collect();

        // `NICHT`-Zeilen prüfen die Gegenrichtung: Diese Titel dürfen nicht
        // auftauchen. Alles andere darf — die App zeigt Tag-Treffer bewusst
        // als „Passt vielleicht" an, und die dürfen nicht jede Zeile hier
        // durchfallen lassen.
        if e.verboten {
            let da: Vec<&&str> = gefunden
                .iter()
                .filter(|g| e.titel.iter().any(|t| t == **g))
                .collect();
            if da.is_empty() {
                if !e.pflicht {
                    erfuellte_wuensche.push(format!("Zeile {}: „{}“", e.zeile, e.wort));
                }
            } else if e.pflicht {
                fehler.push(format!(
                    "Zeile {}: „{}“ darf {} nicht finden",
                    e.zeile,
                    e.wort,
                    strich(&da)
                ));
            } else {
                offene_wuensche.push(format!(
                    "Zeile {}: „{}“ findet weiterhin {}",
                    e.zeile,
                    e.wort,
                    strich(&da)
                ));
            }
            continue;
        }

        let stimmt = fehlt.is_empty();
        if e.pflicht {
            if !stimmt {
                let mut m = format!("Zeile {}: „{}“\n      fehlt:  {}", e.zeile, e.wort, strich(&fehlt));
                if !zuviel.is_empty() {
                    m += &format!("\n      (nebenbei gefunden: {})", strich(&zuviel));
                }
                fehler.push(m);
            }
        } else if stimmt {
            erfuellte_wuensche.push(format!("Zeile {}: „{}“", e.zeile, e.wort));
        } else {
            offene_wuensche.push(format!(
                "Zeile {}: „{}“ findet {} statt {}",
                e.zeile,
                e.wort,
                if gefunden.is_empty() { "nichts".to_string() } else { strich(&gefunden) },
                strich(&e.titel)
            ));
        }
    }

    let pflicht = zettel.iter().filter(|e| e.pflicht).count();
    println!(
        "\nEinkaufszettel-Probe: {} von {} Pflichtzeilen erfüllt, {} Wünsche offen, {} Angebote im Regal",
        pflicht - fehler.len(),
        pflicht,
        offene_wuensche.len(),
        regal.len()
    );
    for w in &offene_wuensche {
        println!("  offen: {w}");
    }

    // Erst die Pflicht, dann die Kür: Ein erfüllter Wunsch ist eine gute
    // Nachricht und darf die schlechte nicht verdecken.
    assert!(
        fehler.is_empty(),
        "Der Einkaufszettel findet nicht, was er meinen soll:\n    {}",
        fehler.join("\n    ")
    );
    assert!(
        erfuellte_wuensche.is_empty(),
        "Erfüllte Wünsche — bitte auf ZETTEL umstellen, sonst geht der Fortschritt \
         wieder verloren:\n    {}",
        erfuellte_wuensche.join("\n    ")
    );
}

fn strich<T: std::fmt::Display>(v: &[T]) -> String {
    v.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ")
}

/// Ein Angebot ohne jeden Tag ist für die Suche fast unsichtbar — es kommt
/// nur noch über ein Wort, das wörtlich in seinem Titel steht.
///
/// Zwei dürfen das, und zwar mit Grund: Beide sind Süß- bzw. Backware, die
/// sich nach etwas anderem anhört, und beide sind in ihrer jeweiligen
/// Sperrliste genau deshalb eingetragen. Sie stehen hier namentlich, damit
/// ein *drittes* stummes Angebot auffällt, statt in der Menge unterzugehen.
#[test]
fn stumme_angebote_sind_die_zwei_bekannten() {
    const BEKANNT: [&str; 2] = ["Speck-Käse-Twister", "KORO Bio-Nut-Butter-Cups"];
    let (regal, _) = lade();
    let stumm: Vec<&str> = regal
        .iter()
        .filter(|a| a.tags.is_empty())
        .map(|a| a.titel.as_str())
        .collect();
    let neu: Vec<&&str> = stumm.iter().filter(|t| !BEKANNT.contains(t)).collect();
    assert!(
        neu.is_empty(),
        "Neu ohne Begriff — nur über den eigenen Titel auffindbar:\n    {}",
        strich(&neu)
    );
    let verschwunden: Vec<&&str> = BEKANNT.iter().filter(|t| !stumm.contains(t)).collect();
    assert!(
        verschwunden.is_empty(),
        "Trägt wieder einen Begriff — die Sperre ist weg, oder der Titel hat sich \
         geändert:\n    {}",
        strich(&verschwunden)
    );
}

/// Damit die Fixture nicht unbemerkt zur Karteileiche wird: Untertitel und
/// Kategorie sind Teil der Eingabe des Taggers, also müssen sie auch belegt
/// sein.
#[test]
fn das_regal_ist_vollstaendig_beschrieben() {
    let (regal, _) = lade();
    for a in &regal {
        assert!(!a.titel.is_empty(), "Angebot ohne Titel");
        assert!(
            !a.untertitel.is_empty() && !a.kategorie.is_empty(),
            "{:?}: Untertitel und Kategorie gehören dazu — der Tagger liest beide",
            a.titel
        );
    }
}
