# Von der Rückmeldung zum Wörterbuch-Eintrag

Die App fragt nach, wenn jemand einen Treffer weglegt, und schreibt die Antwort
nach `match_feedback` (Tabelle: `supabase/migration_match_feedback.sql` im
App-Repo). Dieses Dokument beschreibt, wie daraus ein Wörterbuch-Eintrag wird.

Seit dem 25.10.2026 läuft das jede Woche selbst: `lechariot feedback-runde`
(`src/feedback.rs`, Workflow `feedback-runde.yml`, montags) macht aus neuen
Meldungen einen PR. Gemergt wird weiter von Hand. Der Weg von Hand weiter
unten bleibt für Fälle, die die Runde nicht lösen kann.

## Die Runde

1. Meldungen nach `letzte_id` aus `docs/matching-woerterbuch-feedback.json`
   holen, je (Suchwort, Titel) bündeln, `personal_taste` und `wrong_size`
   weglassen. Fälle, die das heutige Wörterbuch nicht mehr erzeugt, fallen
   ohne Modell heraus. Das Suchwort wird so aufgelöst wie in der App
   (`MatchDictionary.meaning`: der engste Begriff, nicht der Topf).
2. Haiku 5.5 schlägt je Fall eine Änderung vor: `sperrwort`, `synonym`,
   `streichen`, `nonfood`, `regal` oder `keine`. Nur an vorhandenen Begriffen;
   ein neuer Begriff braucht in der App ein Artikelzeichen.
3. Jede Änderung wird einzeln auf den bisher übernommenen Stand gelegt. Sie
   fällt weg, wenn sie
   - ihren gemeldeten Fall nicht behebt,
   - auf dem Goldsatz (Wörterbuch allein, gewichtet) ok senkt oder fehlt bzw.
     falsch hebt,
   - im Wochenbestand mehr als 10 Titel bewegt (Synonym: 25),
   - einer Sorte ihren Topf nimmt (#92) oder als einzelnes Wort den Begriff
     selbst trifft („bratwurst“ bei `wurst`).
4. Ein zweiter Haiku-Aufruf sieht jede übrig gebliebene Änderung mit den
   Titeln, die sie bewegt, und kann nur ablehnen. Danach wird ohne die
   abgelehnten neu gemessen.
5. Ergebnis: `matching-woerterbuch.json`, die Liste der übernommenen
   Änderungen samt neuem Stand in `matching-woerterbuch-feedback.json` (die
   Python-Referenz liest sie mit, `scripts/woerterbuch-paritaet.py` prüft das)
   und der PR-Text. Regal-Vorschläge stehen nur im PR: Das Regal gehört
   `enrich::kategorie_fuer_begriff` und damit auch dem Import.

**Warum der Stand im Repo liegt und nicht in einer Tabelle:** Er rückt erst
vor, wenn jemand den PR merged. Bleibt der PR liegen, rechnet die nächste Runde
alle Meldungen seit dem letzten Merge neu und ersetzt den Zweig; abgelehnte
Änderungen gehen nicht verloren, und in die Produktion wird nichts geschrieben.
Wer einzelne Änderungen nicht will, nimmt sie aus dem PR und merged den Rest
mit dem Stand.

**Was eine Änderung bewirkt:** Sperrlisten und Synonyme liest die App selbst
(Direkttreffer, Suchwort-Auflösung), dort wirken sie nach dem
Wörterbuch-Abgleich sofort. Die Tags der Angebote kommen seit #101 von Haiku
(`replace`); dessen Cache-Schlüssel enthält die Wörterbuch-Tags nicht, also
ändert eine Runde dort nichts außer dem Rückfall und dem Hinweis für neue
Produkte.

**Gemeldete Fehltreffer als Negativbeispiele im Tagging-Prompt: vorerst
nicht.** Gemessen am 25.10. an den 157 gemeldeten Kategorie-Treffern
(`wrong_product`/`wrong_variant`): Das Wörterbuch erzeugt heute noch 65 davon,
Haiku 26, alle 26 auch im Wörterbuch. Die meisten dieser 26 sind nach dem
Wörterbuch selbst richtig (Buttermilch ist `milch`, Oliven im Glas sind
`konserven`), echte Fehler bleiben eine Handvoll. Dafür jede Woche den Prompt zu
ändern hieße, jede Woche den ganzen Cache neu zu taggen. Außerdem gäbe es nichts
mehr, woran man den Effekt misst, solange die Beispiele nicht aus dem Goldsatz
kommen dürfen. Neu fragen, wenn eine Haiku-Stichprobe eine wiederkehrende
Fehlerklasse zeigt: dann eine Regel in den Prompt, keine Produktliste.

## Von Hand

### Warum kein Machine Learning

Die App hat einen Nutzer. Ein Modell bräuchte Tausende gelabelte Beispiele. Es
braucht auch keines: das Wörterbuch ist eine Regelstruktur
(`begriff -> (exact, suffix, block)`), und jede Ablehnung zeigt auf genau eine
Regel. Die Auswahloption im Sheet sagt bereits, welche Operation gemeint ist:

| Antwort im Sheet | `reason` | Wörterbuch-Operation |
|---|---|---|
| Passt gar nicht zum Artikel | `wrong_product` | Wort auf die `block`-Liste des Begriffs |
| Falsche Sorte oder Variante | `wrong_variant` | `block`, oder neuer feinerer Begriff mit eigenem `exact` |
| Falsche Menge oder Größe | `wrong_size` | **keine** — Mengenproblem, siehe Backlog |
| Mag ich einfach nicht | `personal_taste` | **keine** — Vorliebe. Zählen, nicht einarbeiten |
| Anderes | `other` | Freitext lesen |

Der Wert steckt in der Option, nicht im Freitext. `personal_taste` ins
Wörterbuch zu tragen würde die Daten für alle anderen vergiften — genau deshalb
fragt das Sheet danach.

### Ablauf

#### 1. Fälle holen

```sh
export SUPABASE_URL=... SUPABASE_SERVICE_KEY=...   # anon key reicht nicht:
                                                   # die Tabelle hat bewusst
                                                   # keine Select-Policy
python3 docs/feedback-auswertung.py --tage 7
```

Das Skript gruppiert nach (Suchbegriff, Produkttitel) — zehn Meldungen zum
selben Fehltreffer sind *ein* Fall, kein zehnfaches Gewicht —, wirft alles ohne
Wörterbuchbezug raus und bestimmt pro Fall den verantwortlichen Eintrag. Dafür
importiert es `V` und die Trefferregel aus `matching-woerterbuch-eval.py`, statt
sie nachzubauen: ein Vorschlag, der am falschen Eintrag ansetzt, ist schlimmer
als kein Vorschlag.

Ohne Zugang zur Datenbank:

```sh
python3 docs/feedback-auswertung.py --demo      # eingebaute Beispielzeilen
python3 docs/feedback-auswertung.py --datei dump.json
```

#### 2. Pro Fall genau eine Frage beantworten

Das Skript stellt sie schon formuliert, samt aktuellem Eintrag und den Tokens,
die noch auf keiner seiner Listen stehen:

> Suchbegriff „käse“, Produkt „Schinken-Käse-Croissant“ — welches Token gehört
> auf die Blockliste von „käse“, ohne echte käse-Treffer zu verlieren?

Das kann ein Mensch beantworten oder ein LLM. Wichtig ist der Zuschnitt: **eine**
Frage, **ein** Eintrag, Ausgabe ist ein Diff gegen `V`, kein fertiger Commit.

Drei Fälle bekommen keine Blocklisten-Frage:

- **Direkttreffer** — das Suchwort stand wörtlich im Titel, es gibt keinen
  Eintrag zu reparieren. Wenn überhaupt, braucht der Begriff eine feinere
  Aufteilung.
- **Kein Eintrag trifft mehr** — das heutige Wörterbuch erzeugt diesen Treffer
  gar nicht mehr. Schon behoben, oder er kam über die Markenliste.
- **Alle Titelwörter gehören zum Begriff** — das Produkt *ist* ein Vertreter,
  nur nicht der gewünschte. Blocken würde echte Treffer mitreißen; hier hilft
  nur ein feinerer Begriff mit eigenem `exact` — oder es war doch eine Vorliebe.

#### 3. Übernehmen

Der Vorschlag wandert von Hand in `V` in `docs/matching-woerterbuch-eval.py`.
Die Python-Referenz ist die Quelle; `matching-woerterbuch.json` wird daraus
erzeugt, nicht umgekehrt.

#### 4. Das Eval-Skript ist der Schiedsrichter

Ein Vorschlag wird nur übernommen, wenn die Abdeckung **nicht fällt** und die
Regressionsfälle weiter stimmen. Damit kann ein Fix für „Käse“ nicht heimlich
„Butter“ zerschießen.

```sh
python3 docs/matching-woerterbuch-eval.py    # Abdeckung + regeneriert die JSON
cargo test matching                          # Regressionsfälle (src/matching.rs)
cargo test parity_with_eval_db -- --ignored --nocapture   # Rust == Python
```

Vorher/nachher vergleichen: die Zeile `regelbasiert getaggt: N (X%)`. Fällt sie,
ist der Vorschlag zu grob — meist wurde ein Token geblockt, das auch in echten
Treffern vorkommt.

Danach neu bauen: die JSON ist per `include_str!` einkompiliert (siehe
`docs/tagging.md`).

#### 5. Gegenprobe

Der konkret gemeldete Fehltreffer muss weg sein. Am schnellsten über das
Eval-Skript selbst, das jeden Treffer pro Begriff auflistet, oder über einen
Regressionsfall in `src/matching.rs` — für einen Fall, der einmal echt gemeldet
wurde, lohnt sich die Zeile.
