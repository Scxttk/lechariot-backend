# Scraper-Referenz

Stand: 2026-07 (KW 29), verifiziert mit PLZ 01219 (Dresden). Typische
Angebotszahlen schwanken je Woche und Region.

## Endpoints pro Kette

| Kette | Quelle / URL-Muster | Auth / Eigenheiten | Markt-Bezug | Typisch/Woche | Fixture |
|---|---|---|---|---|---|
| REWE | `rewerse`-CLI (mobile API) | **mTLS**: Client-Zertifikat aus der REWE-App nötig (`cert.pem` + `private.key`, siehe [docs/rewe-cert.md](../rewe-cert.md)) | filialspezifisch (PLZ → marketId) | variiert je Filiale | `tests/fixtures/rewe/discounts.json` (handgebaut im rewerse-Format) |
| Penny | `penny.de/.rest/market`, Kategorien aus `/angebote`-HTML, dann `/.rest/offers/by-category/<JAHR-WOCHE>/<kategorie>?region=<sellingRegion>` | nur Browser-User-Agent; Aktionspreise als String mit Fußnoten-Sternchen (`"0.49*"`) | regional (`sellingRegion` des Markts) | ~550-600 (2 Wochen) | `tests/fixtures/penny/offers_kuehlregal.json` |
| Kaufland | `filiale.kaufland.de/.klstorefinder.json` + server-seitig gerendertes `/angebote/uebersicht.html` | Filiale über Cookie `x-aem-variant=<id>`; **Titel = Marke, Produkt im Untertitel** (Offer-ID enthält deshalb den Untertitel); **dasselbe Angebot erscheint in mehreren Kategorien** (Warengruppe + „Unsere Knüller" etc.) — Dedup erst beim DB-Upsert über die ID | filialspezifisch | ~650 (inkl. Kategorie-Duplikate) | `tests/fixtures/kaufland/uebersicht.html` |
| Lidl | Store-Finder-Feld `AR` → `lidl.com/flyer/esi-overview` → `endpoints.leaflets.schwarz/v4/flyer?flyer_identifier=<slug>` → `pdfUrl` → `pdftotext -bbox-layout` | kein Key, keine Anmeldung; braucht **poppler-utils**; PDF ~83 MB; Angebotspreis am Stern erkennbar (`2.49*`) | Absatzregion der Filiale (18 Varianten/Woche, 40 Regionscodes) | ~195 (1 Woche) | `tests/fixtures/lidl/prospekt_bbox_layout.xml`, `tests/fixtures/lidl/prospekt_flyer.json` |
| EDEKA | `edeka.de/api/marketsearch/markets?searchstring=<PLZ>`, Markt-ID via 308-Redirect der Legacy-URL, Angebote aus `/maerkte/<id>/angebote/`-HTML | Akamai-Bot-Schutz → System-`curl` (util.rs); Preis maschinenlesbar im `sr-only`-Div („Festpreis von 3.99 €" / „App-Preis von …") | filialspezifisch | ~200 | `tests/fixtures/edeka/angebote.html` |
| Netto | Intershop-Filialsuche (JSON) + `/filialangebote/{1,2,4,5}`-HTML | Akamai → System-`curl`; Filiale über Cookie `netto_user_stores_id` | filialspezifisch | ~300 | `tests/fixtures/netto/filialangebote_1.html` |
| ALDI Nord | `aldi-nord.de/angebote.html`, Daten im `__NEXT_DATA__`-JSON (`OFFER_GET.res.algoliaDataMap`) | plain reqwest; **Aktionstage und Produkt-Snapshot laufen auseinander** (siehe unten) | bundesweit (`ALDI_NORD_DE`) | ~230 | `tests/fixtures/aldi_nord/angebote.html`, `…/angebote_luecke.html` |
| ALDI Süd | `api.aldi-sued.de/v3/product-search?categoryKey=1588161426582123` (paginiert) | Akamai → System-`curl`; Preise in **Cent**; keine Gültigkeitsdaten | Süd-Gebiet einheitlich (`ALDI_SUED_DE`) | ~75 | `tests/fixtures/aldi_sued/product_search.json` |
| NORMA | `norma-online.de/de/angebote/` (Index) → je Themenseite `…/ab-<tag>,-<dd.mm.jj>/<thema>-t-<id>/` | plain reqwest, kein Key, kein Cookie; Preis maschinenlesbar im `aria-label` („0,99 Euro"), sichtbar steht dort `–,99`; **Marke und Produkt getrennt** (Offer-ID enthält deshalb die Marke); Laufzeit abgeleitet (siehe unten) | bundesweit (`NORMA_DE`) | ~220 (3 Termine) | `tests/fixtures/norma/angebote_index.html`, `…/thema_mehr_fuers_geld.html`, `…/filialfinder_01219.html` |

## NORMA: drei Termine pro Woche, ein abgeleitetes Enddatum

NORMA startet **drei Angebotszyklen je Woche** (Montag, Mittwoch, Freitag), und
der Index zeigt die laufenden *und* die kommenden. Am 2026-07-31 waren das vier
Termine mit 22 Themenseiten und 216 Angeboten, 157 davon am Montagstermin. Ein
Lauf kostet damit rund 23 Requests: den Index plus eine Seite je Thema.

Gemessen am 2026-07-31, 21:20 Uhr: **221 Angebote, 215 mit Preis (97 %), 91 mit
Streichpreis, 221 mit Bild, 221 mit Rohkategorie.** Davon bekommen 137 (62 %)
von `enrich` eine echte Kategorie; die übrigen sind fast alle Non-Food
(Heimwerker, Garten, Fahrrad), siehe [docs/marktabdeckung.md](../marktabdeckung.md).

Die PLZ steht hier bewusst nicht dabei: Der Katalog ist bundesweit. Nachgeprüft
am 2026-07-31, indem dieselbe Themenseite dreimal geholt wurde — ohne
Filialcookie, mit `NORMA_clid=2131` (Dresden) und mit einer anderen Filiale. Die
drei Antworten unterscheiden sich in **einem Akamai-Beacon-Token** und sonst in
keinem Byte; Preise und Kacheln sind identisch. Die Filialwahl auf der Website
schaltet nur einen „Meine Filiale"-Kasten frei, keinen anderen Preis.

**Das Enddatum ist abgeleitet, und das ist eine bewusste Ausnahme.** Auf der
Themenseite steht nur der Start („ab Montag, 03.08."). Leer lassen ging nicht:
Die App liest `valid_until` als **nicht-optionales** `Date`
(`ios/LeChariot/Models/Offer.swift`) — ein `null` bricht nicht nur die
NORMA-Zeile, sondern das Decoding der **ganzen** Angebotsantwort. Also sieben
Tage ab Start (`norma::TERM_DAYS`).

Die Zahl ist nicht geraten: Die **Artikel**-Detailseiten nennen den Zeitraum
wörtlich („Aktionszeitraum: 03.08. bis 09.08.2026"). Sie an jedem Angebot
abzufragen wäre 218 statt 23 Requests, deshalb steht die Laufzeit als Konstante
im Code — und der Live-Test `valid_until_is_the_published_window` rechnet sie
gegen zwei echte Detailseiten nach, damit ein geänderter Rhythmus auffällt:

```sh
cargo test --lib valid_until_is_the_published_window -- --ignored --nocapture
```

Zwei weitere Fallen, beide beim ersten Livelauf aufgefallen:

1. **Marke und Produkt stehen getrennt** (`strong.supplier` = „Sheba",
   `h3` = „Katzennassnahrung"). Ohne die Marke im Titel fallen zwei Marken
   desselben Produkts auf dieselbe Offer-ID — Sheba und Whiskas hatten am
   2026-07-31 beide eine „Katzennassnahrung" im selben Prospekt. Dieselbe
   Falle wie bei Kaufland.
2. **Textknoten ohne Leerraum dazwischen.** `…Tipo` + `je 520 g` ergab beim
   bloßen Aneinanderhängen „Tipoje 520 g"; die Knoten werden deshalb mit
   Leerzeichen verbunden. Ebenso schreibt NORMA das Pfand mal mit und mal ohne
   Leerzeichen vors Komma (`je 6 x 0,5 l,zzgl. …`) — es fliegt aus der
   Mengenangabe, damit `push::is_pure_quantity` sie als reine Menge erkennt.

## NORMA-Filialfinder: ein POST-Formular mit Sitzungspflicht

Die Filialliste kommt aus einem anderen Loch als die Angebote, und der
naheliegende Weg ist der falsche.

| Weg | Requests | Ergebnis |
|---|---|---|
| `GET /de/filialfinder/suchergebnis?lat=…&lng=…&r=…` | 1 (+1 Nominatim) | **immer genau eine** Filiale, `r` wirkungslos |
| `POST /de/filialfinder/` mit `filialfinder[suche][plz]` | 2 | alle Filialen im Radius, keine Geokodierung nötig |

Der GET-Weg sieht bequemer aus und ist es nicht: An sieben Orten geprüft
(Dresden, Nürnberg, Berlin, München, Hamburg, Köln, Fürth) kam jedes Mal genau
eine Filiale zurück, egal welcher Radius gefragt war. Für „ist die Kette hier
vertreten" reicht das, für den Filial-Picker der App nicht.

Der POST-Weg braucht **zwei** Requests, und der erste ist kein Versehen: Die
Suche landet in einer PHP-Sitzung, die Trefferseite hinter dem 302 liest sie von
dort, und ohne `PHPSESSID` antwortet der Server mit `?info=nosearch` — also mit
einer leeren Seite und HTTP 200, nicht mit einem Fehler. Ein Sitzungscookie gibt
es auf norma-online.de an genau einer Stelle: `/ext/ajax/get_wishlist.php`.
Weder `/de/filialfinder/`, noch `/de/angebote/`, noch die Trefferseite selbst
setzen eines (alle vier am 2026-07-31 geprüft). Deshalb steht dieser Handschlag
in `store_finder::norma_search` vor der Suche.

Gemessen am 2026-07-31, Radius 25 km (`branches::AREA_RADIUS_KM`):

| PLZ | Filialen | mit Koordinaten | mit Straße |
|---|---:|---:|---:|
| 01219 Dresden | 9 | 9 | 9 |
| 90402 Nürnberg | 60 | 60 | 60 |

Die 60 sind der Deckel, nicht die Wahrheit: `branches::MAX_PER_CHAIN` steht auf
60, und NORMAs Server kappt bei derselben Zahl (bei 15 km lieferte 90402 bereits
59, bei 50 km 60). In Nürnberg — NORMA sitzt in Fürth — ist die Liste also
abgeschnitten, wie bei jeder anderen Kette in einer Großstadt auch.

## Bekannte NULL-Preise (diagnostiziert 2026-07)

- **EDEKA (~20-25/Woche): echt.** „Tagespreis"-Kacheln und reine
  PAYBACK-Extra-Punkte-Kacheln tragen weder in der Kachel noch im
  zugehörigen Dialog einen Preis. Sie kommen bewusst mit `price = NULL` an.
- **Lidl: erledigt mit dem Quellenwechsel.** Die alten ~7 NULL-Preise/Woche
  stammten aus der `lidl.de/q/api/search`-Quelle, in der Lidl-Plus-Angebote
  den Preis in `lidlPlus[0].price` statt in `price` trugen. Diese Quelle gibt
  es nicht mehr. Im Prospekt-Weg sind
  Lidl-Plus-Preise ganz normale Sternpreise und im Untertitel als „nur mit
  Lidl Plus" gekennzeichnet.

## ALDI Nord: Aktionstage und Produkt-Snapshot laufen auseinander

Die Angebotsseite trägt zwei getrennte Quellen in einem statisch gebauten
`__NEXT_DATA__` (`"gsp": true`):

- `OFFER_GET.res.categories` — die Aktionstage aus dem Magnolia-CMS, je Sektion
  eine Liste von `productIds`.
- `OFFER_GET.res.algoliaDataMap` — der Produkt-Snapshot mit Name, Preis,
  Verkaufseinheit, Bildern.

Beide können auseinanderlaufen, und dann verspricht `categories` ein Produkt,
das `algoliaDataMap` nicht kennt. **Gemessen am 06.08.2026:** Die
„Osteuropa-Aktion" der KW 32 nannte elf `productIds`, der Snapshot kannte zehn.
Es fehlte `1032980` — „OSTEUROPA Original polnische Pierogi", 400-g-Packung,
2,49 €, im gedruckten Prospekt derselben Woche abgebildet. Ein zweites Produkt
(`4369`, Sektion „Haltbare Produkte" ab Do 6.8.) fehlte genauso.

Die Lücke sitzt in der Quelle, nicht im Parser: Auch die Produktseite
`/produkt/…-1032980.html` antwortete mit HTTP 404, und die gerenderte
Angebotsseite ließ die Kachel selbst weg. Nachbauen lässt sich so ein Angebot
nicht — ohne Name und Preis zeigt die App keine Zeile, und `push::map_offer`
verwirft preislose Angebote ohnehin.

Was der Scraper deshalb tut: Er **meldet** die Lücke.
`aldi_nord::parse_offers_reporting` gibt die verlorenen Produkte als
`MissingProduct` zurück, `parse_offers` schreibt je eine `WARNUNG`-Zeile mit der
`productId` auf stderr — dieselbe Klasse stiller Datenverlust, die `sync.rs`
schon für übersprungene Filialen laut macht. Ohne die ID ließe sich das fehlende
Produkt bei ALDI gar nicht wiederfinden.

Nachmessen: `cargo run --example aldi_nord_luecke`.

## Streichpreise: welche Kette den alten Preis überhaupt druckt

Der durchgestrichene Preis („statt 2.99") macht aus einem behaupteten Rabatt
einen nachprüfbaren. Er steht in `offers.regular_price` — die Spalte gibt es
seit Schema v2, gefüllt wird sie je Kette so:

| Kette | Streichpreis in der Quelle | Stand |
|---|---|---|
| Lidl | **ja**, im Prospekt gedruckt | 78 von 260 Angeboten (Prospekt 27.07.2026) |
| Penny | ja, `tile`-Feld | seit jeher |
| Kaufland | ja, eigener Selektor | seit jeher |
| Netto | ja, `strike`-Element | seit jeher |
| ALDI Nord / Süd | ja, im Preis-Objekt | seit jeher |
| NORMA | ja | 91 von 221 Angeboten (31.07.2026) |
| **REWE** | **nein** | siehe unten |
| **EDEKA** | **nein** | siehe unten |

### REWE veröffentlicht ihn nicht

Nachgesehen am 2026-07-31 an Markt 565005 (Dresden-Leuben), in beiden
Endpunkten, die das Zertifikat erreicht:

- **`rewerse discounts`** — die Wochenangebote, 335 Stück. Jedes Angebot trägt
  genau: `title`, `subtitle`, `images`, `priceRaw`, `price`, `priceParseFail`,
  `manufacturer`, `articleNo`, `nutriScore`, `productCategory` und bei 61
  Angeboten `loyaltyBonus`. Kein Feld für einen alten Preis. In den 335
  Untertiteln steht **kein einziges** „statt", „UVP", „Normalpreis" oder
  „vorher"; die 17 Prozentzeichen sind Fett- und Alkoholgehalt.
- **`rewerse products search`** — die Online-Listung desselben Markts, 173
  Produkte, 56 davon mit Rabatt. Das Rabatt-Objekt ist
  `{"__typename": "RegularProductDiscount", "validTo": "01.08."}` — Laufzeit
  und sonst nichts.

Der Kommentar in `rewe.rs` („kein fromDate/regularPrice/overline mehr") ist
damit bestätigt: `regular_price: None` ist für REWE die richtige Antwort.

**Grenze der Messung, ehrlich benannt:** Beobachtet wurde die Ausgabe des
`rewerse`-CLI, nicht die rohe API-Antwort — die URL steht nur als zerlegter
String im Go-Binary, und die vier geratenen Pfade unter `mobile-api.rewe.de`
antworten alle mit 404. Dass `rewerse` ein vorhandenes Feld verschweigt, ist
damit nicht restlos ausgeschlossen; dagegen spricht, dass sein zweiter
Endpunkt das Rabatt-Objekt roh durchreicht, mitsamt `__typename` — ein Feld,
das nur dort steht, weil niemand die Antwort aufgeräumt hat. Die
Gegenprobe über `www.rewe.de/angebote/` ist nicht gelaufen: Cloudflare
antwortet dort mit 403.

Was REWE stattdessen liefert und wir wegwerfen: `loyaltyBonus`, die
PAYBACK-Cents je Angebot (61 von 335). Das ist ein echter Vorteil, aber kein
Streichpreis.

### EDEKA veröffentlicht ihn auch nicht — druckt aber den Rabatt

Gemessen am 2026-07-31 an zwei Märkten (030567 und 035482, Passau), je 213
Angebotskacheln: **null** Vorkommen von „statt", „UVP" oder `line-through` im
HTML. Die Preisauszeichnung steht maschinenlesbar in `div.sr-only` und kennt
drei Formen:

```
Festpreis von 3.99 €                                  (410 Vorkommen)
App-Preis von 0.77 €                                   (44)
Rabattierter Preis von 0.88 € (Insgesamt -56 % Rabatt)  (3 Angebote)
```

Die dritte Form nennt den **Rabatt**, nicht den alten Preis. Daraus einen
Streichpreis zu rechnen hieße, eine Zahl zu erfinden: Der Prozentwert ist auf
ganze Punkte gerundet, aus „0.88 € bei -56 %" folgt nur ein Bereich von 1.98
bis 2.02 €, aus „9.49 € bei -32 %" einer von 13.96 bis 14.16 €. Ein
Streichpreis, den die Kette so nie gedruckt hat, ist genau die Behauptung, die
er widerlegen soll. Bleibt draußen.

Betroffen wären ohnehin 3 von 213 Kacheln.

## Lidl: der eigene Prospekt, und nur noch der

Lidl ist mit rund 30 % aller Zeilen die größte Kette. Die Angebote kommen aus
Lidls eigenem Wochenprospekt (`src/scrapers/lidl_prospekt.rs`);
`LIDL_SOURCE=prospekt-llm` wählt den zweiten Leseweg über ein Sprachmodell,
alles andere ist der Standard `prospekt`.

**marktguru wurde am 2026-07-31 entfernt.** Bis dahin war der Dritte
(`api.marktguru.de`) der Standard und der Rückfall bei unbekanntem
`LIDL_SOURCE`; genau dieser stille Rückfall war der Grund für die Entfernung —
`branches.yml` setzte die Variable nicht und holte deshalb aus einer anderen
Quelle als die Nightly. Was der Vergleich über zwei Wochen ergeben hat, steht
unten; was die Entfernung kostet, ebenfalls: marktguru war die einzige
Lidl-Quelle **mit Bildern**.

```sh
LIDL_SOURCE=prospekt lechariot fetch --store lidl --zip 01219 --dry-run
```

Was der Prospekt besser kann:

- **regionsgenau** — der Prospekt gilt für die Absatzregion der Filiale,
  marktguru war nur ungefähr regional;
- **Streichpreise** — `UVP` / `Normalpreis` stehen im Prospekt, marktguru
  lieferte für Lidl gar kein `regular_price` (Details unten);
- **seitengenaue Laufzeiten** — Donnerstag-Angebote tragen im Seitenkopf
  („Ab Do. 23.7. bis Sa. 25.7.") eine kürzere Gültigkeit als der Prospekt.

### Ein Prospekt je Lauf, nicht je Filiale

Alle gewählten Lidl-Filialen einer Absatzregion bekommen denselben Prospekt.
Seit 2026-07-31 lädt und liest ihn ein Lauf deshalb nur einmal
(`cached_leaflet`): Schlüssel ist die `pdf_url`, gehalten werden die PDF und
ihre Textebene je `pdftotext`-Modus. Der Cache liegt **nur im Speicher** —
der Prospekt wechselt wöchentlich, und ein Cache über Läufe hinweg lieferte
irgendwann die Angebote der Vorwoche. `release_leaflet()` löscht die Datei am
Ende des Laufs (`sync.rs` nach der Filialschleife, `main.rs` nach `fetch`).

Im Cache liegen drei Dinge: die PDF, ihre Textebene je `pdftotext`-Modus und
der Bildauszug (`pdftohtml -xml`) je Seitenfenster. Der Auszug ist der
teuerste Schritt des ganzen Abends — 2026-07-31 gemessen: 112 s und 163 MB
für die 64 Seiten mit offenen Kacheln, bei einem Marktdurchlauf von 145 s.

Was der Cache **nicht** einspart: das Rastern der Kachelstreifen
(`pdftoppm`, ein Aufruf je Kachel, rund 0,12 s). Das läuft weiter je Filiale,
weil der Dateiname eines Bildes die Angebots-ID ist und die den Markt enthält.

### Wie vollständig ist der Prospekt-Weg?

Gemessen gegen marktguru, weil das die Messlatte ist. marktguru bezieht seine
Lidl-Daten aus **demselben Prospekt** — die API gruppiert Angebote nach
`leafletFlightId`, und für die Woche 20.–25.07.2026 hängen alle 375 Zeilen an
genau einem Flight. Es gibt also keine geheime Zusatzquelle.

Stand 2026-07-25 für PLZ 01219:

| | marktguru | `prospekt` | `prospekt-llm` |
|---|---:|---:|---:|
| Angebote der Woche | 375 | **382** | **420** |
| marktguru-Preis abgedeckt | — | 96,3 % | **97,9 %** |
| Preis **und** passender Name | — | 76,5 % | **89,6 %** |

Beide Wege decken die Preise ab; der Unterschied liegt bei den **Namen**. Genau
das ist der Grund, warum es den LLM-Weg gibt — für den Warenkorb-Abgleich nützt
ein Preis nichts, dessen Produktname nicht zum Listeneintrag passt.

Die zweite Zeile ist die eigentliche Aussage: 361 der 375 marktguru-Preise
kommen auch hier heraus. Die 14 Fehlenden sind Randfälle (Artikel, deren Preis
im Prospekt nur im Fließtext steht).

Zwei Funde haben den Weg dorthin gebracht:

1. **Die Daten sind vollständig im PDF.** 365 der 375 marktguru-Angebote
   stehen mit ihrem Preis in der Textebene, 357 davon als Sternpreis. Es fehlte
   nie eine Quelle, nur die Extraktion.
2. **Die restlichen zehn stehen im `products`-Feld des Prospekt-JSON** — alles
   Onlineshop-Möbel und -Großgeräte. Die kommen jetzt aus dem JSON dazu
   (`products_as_offers`), sauber strukturiert und mit Bild und Kategorie, die
   der PDF-Weg gar nicht liefern kann.

Drei Fallen, die beim Bauen Zeit gekostet haben:

1. **Das `products`-Feld des Prospekt-JSON ist eine Sackgasse.** Es enthält
   ausschließlich Onlineshop-Artikel (138 Einträge, null Lebensmittel);
   dasselbe gilt für `pages[].links`. Die Lebensmittel stehen nur in der
   Textebene der PDF. Ein erster Anlauf (Tag
   `archiv/lidl-prospekt-llm-pipeline`) hat daraus geschlossen, der Weg
   brauche ein Vision-LLM — er hat die Textebene nie geprüft.
2. **`pdftotext -bbox-layout` liefert kein wohlgeformtes XML.** In einem
   Wochenprospekt stecken einzelne C0-Steuerzeichen mitten in `<word>`; jeder
   echte XML-Parser bricht daran ab. Deshalb der Zeilenparser samt
   Vorab-Filter.
3. **Preis und Produktname stehen nicht in derselben Textzeile.** Sie hängen
   an ihrer Position auf der Seite, also werden Kacheln über Abstände gebildet
   und Produkt und Preis einander zugeordnet.

**Rechenprobe als Wächter.** Der Prospekt nennt Packungsgröße *und*
Grundpreis, also muss `Menge × Grundpreis ≈ Preis` gelten. Kacheln, bei denen
das nicht aufgeht, sind falsch zusammengesetzt und werden verworfen (typisch
~15 je Prospekt). In einer Preisvergleichs-App ist ein falscher Preis
schlimmer als ein fehlendes Produkt.

Bekannte Grenze: Vereinzelt landet eine Werbezeile als Produktname in den
Daten („Woche", „Kernarm") — rund 2 % der Zeilen. Die Preise dieser Zeilen
sind korrekt, nur der Name taugt nicht zum Matchen.

Am 2026-07-31 gegen die Produktion nachgezählt: Von **371 verschiedenen
Lidl-Produkten** des Laufs vom 31.07. benennen dreizehn keine Ware. Elf davon
fängt `is_layout_remnant` ab („Woche", „XXL", „DELUXE", „Stall + Platz",
„Bester Backshop lohnt sich", der Druckvermerk in beiden Schreibweisen,
„Zubereitung von 190 g Crushed Ice", „Mit Bio-Baumwolle Sneaker", „Spare € n
Akku", „2 in 1: Manuell/Maschinell").

**Die Prüfung läuft erst auf dem fertigen Titel**, nicht schon bei der
Rollenzuteilung. Dieselben Wörter in `is_layout_text` zu setzen verschiebt die
Paarung und kostete am Prospekt vom 27.07. vier echte Artikel (CROWN-FIELD
Cerealien XXL, beide FLORALYS-Zeilen, LUPILU Bio Quetschbeutel), während
„Gültig am 1.8. R" neu hereinkam.

Was **nicht** abgefangen wird, und warum: Beschriftungen, die auf ein Teil des
Produktfotos zeigen — „Rohrschneider", „Sechskant-aufnahme", „Fugendüse und
Polsterdüse", „Keramikbeschichtete Bügelsohle Bodendüse", „Farbdisplay
Auflösung Speicher", „Flexibler Schwanen-hals zur genauen Ausrichtung". Sie
sind vom Text her nicht von einem Produktnamen zu unterscheiden: Auf denselben
Seiten stehen „Unterlegscheiben-Sortiment", „Fugenmesser-Set" und
„Trolley-Boardcase" als echte Artikel. Eine Regel, die die einen trifft,
trifft die anderen mit.

### Streichpreise: der gedruckte Rabatt ist der Beweis

Der Prospekt druckt den alten Preis in vier Schreibweisen (gezählt am
Prospekt vom 27.07.2026): `-42% UVP 3.49` (80-mal), `UVP 3.59` ohne
Rabattzahl (44), `Normalpreis: 3.39` mitten in der Beschreibungszeile, und
ganz ohne Stichwort nur `-20% 1.39` über dem Angebotspreis (42).

Ein Streichpreis am falschen Produkt ist schlimmer als keiner — er behauptet
einen Rabatt, den es nicht gibt, und die App zeigt ihn ohne Vorbehalt.
Deshalb wird ein Fund nur übernommen, wenn er festgenagelt ist:

1. **Rechnet der gedruckte Rabatt auf?** Lidl schneidet die Nachkommastellen
   ab (3.49 → 1.99 sind 42,98 % und stehen als „-42 %" im Heft). Passt die
   Zahl nicht, gehört die Plakette einem anderen Produkt.
2. **Ohne Rabattzahl zählt nur die eindeutige Kachel** — ein Preis, ein
   Betrag in Reichweite. Trägt die Kachel mehrere Sternpreise, gehört die
   Plakette einem davon, und welchem, sagt der Prospekt nicht.
3. **Eine Plakette gehört genau einem Preis.** Passt sie rechnerisch auf zwei
   Preise derselben Kachel, bekommt sie keiner.

Was das kostet und bringt, am Prospekt vom 27.07. gemessen: **61 → 78**
Angebote mit Streichpreis von 260. Der Zuwachs kommt aus den Plaketten ohne
Stichwort; gleichzeitig fallen die Fehlpaarungen weg, die der alte Weg
lieferte (BARILLA Pasta Sauce „1.79 statt 13.99", ROWENTA Staubsauger „8.99
statt 119.99", WAGNER Steinofen Pizza mit demselben `UVP 4.98` an allen drei
Preisen seiner Kachel).

### Dritter Weg: `LIDL_SOURCE=prospekt-llm` (Zusatz, nicht Ersatz)

**Der Standard bleibt der Weg ohne Modell.** `LIDL_SOURCE=prospekt` erreicht
96 % Preisabdeckung ohne Token, ohne Netzabhängigkeit zu einem weiteren
Anbieter und ohne Ratenlimit — es gibt keinen Grund, dafür ein Modell zu
bemühen. Der LLM-Weg existiert nur für den Rest: Kacheln ohne saubere
Marke-Name-Struktur (Obst, Gemüse, Non-Food) und die rund 2 % Zeilen, deren
Titel eine Werbezeile ist.

Läuft über **GitHub Models** (`src/scrapers/lidl_llm.rs`), weil das im
GitHub-Student-Paket enthalten ist — ein Zusatz darf keine laufenden Kosten
verursachen. Der Token kommt aus `GITHUB_MODELS_TOKEN`, sonst `GITHUB_TOKEN`,
sonst aus `gh auth token`; lokal ist damit nichts einzurichten, wenn `gh`
angemeldet ist.

Die Halluzinationsfrage ist die einzige, die hier zählt, und sie ist
mechanisch beantwortet, nicht durch Zureden im Prompt:

1. Jeder Preis muss **wörtlich im Seitentext stehen** (`price_is_grounded`).
2. Wo Packungsgröße und Grundpreis vorliegen, muss dieselbe **Rechenprobe**
   aufgehen wie im geometrischen Weg.

Das Modell kann also Zeilen übersehen, aber keine erfinden. Beim ersten
Livelauf hat genau das gegriffen: Für ARLA Kaergarden kamen 2,49 € *und* der
Nachbarpreis 3,99 € zurück — 400 g zu 6,23 €/kg sind 2,49 €, also flog der
zweite raus. Über den ganzen Prospekt: **293 Vorschläge, 9 verworfen** (3 %).

Was der Weg konkret dazugewinnt, sieht man an den Zeilen, die der
geometrische Weg gar nicht erst als Produkt erkennt — Obst und Gemüse ohne
Marke: „Galiamelone", „Heidelbeeren", „Rote Paprika", „Rote Äpfel". Dort gibt
es keine Marke-Name-Beschreibung-Struktur, an der sich eine Kachelbildung
festhalten könnte.

Ratenlimit und Dauer: Die Freistufe erlaubt rund 15 Anfragen pro Minute,
deshalb laufen die Seiten **nacheinander** mit 4 Sekunden Mindestabstand statt
parallel. Der Abstand ist aber nicht das Nadelöhr — eine Prospektseite braucht
beim Modell selbst rund 30 Sekunden, sodass ein Wochenprospekt (46 von 69
Seiten tragen Sternpreise) etwa **20 Minuten** läuft. Für einen wöchentlichen
Lauf unerheblich, für interaktives Ausprobieren zu langsam. Seiten ohne
Sternpreis gehen gar nicht erst ans Modell.

Erwartbare Aussetzer im Livebetrieb, beide mit einem zweiten Versuch
abgefangen: Ratenlimit-Antworten (429) und abgerissene Verbindungen
(„Connection reset by peer", beim ersten 46-Seiten-Lauf einmal aufgetreten).
Eine Seite, die auch dann scheitert, wird übersprungen — sie darf nicht den
ganzen Prospekt kippen.

```sh
LIDL_SOURCE=prospekt-llm lechariot fetch --store lidl --zip 01219 --dry-run
```

Modell über `LIDL_LLM_MODEL` austauschbar (Standard `openai/gpt-4.1-mini`).

Falls der Weg je im nightly laufen soll: In GitHub Actions gibt es `GITHUB_TOKEN`
von Haus aus, der Job braucht dafür aber `permissions: models: read` — ohne das
antwortet GitHub Models mit 401, obwohl ein Token gesetzt ist.

## Gemeinsame Infrastruktur (`src/scrapers/util.rs`)

- `curl_get` / `curl_redirect_url`: System-`curl` mit vollem
  Browser-Header-Satz für Akamai-geschützte Hosts (Netto, ALDI Süd, EDEKA) —
  reqwest/rustls wird dort per TLS-Fingerprint mit 403 geblockt. 3 Versuche
  mit 3 s Abstand.
- `async_client` / `blocking_client`: reqwest-Clients mit gemeinsamem
  Browser-User-Agent (Penny, Lidl, Kaufland, ALDI Nord).
- `polite_pause(url)`: höfliches Rate-Limiting — vor aufeinanderfolgenden
  Requests an denselben Host eine zufällig gestreute Pause (300-800 ms).
- `ctx(kette, schritt, url)`: einheitlicher Fehlerkontext
  (`[Kette] Schritt fehlgeschlagen: URL`).

## Tests

- Offline: `cargo test` — Parser-Tests gegen die Fixtures in
  `tests/fixtures/<kette>/` (`tests/scrapers.rs` + Modul-Unit-Tests).
- Live: `cargo test --lib -- --ignored --nocapture --test-threads=1` —
  ein Live-Test pro Kette (außer REWE, braucht das Zertifikat), PLZ 01219.

Fixtures sind auf wenige repräsentative Angebote gekürzte Live-Antworten
vom 2026-07-17; das REWE-Fixture ist mangels Zertifikat handgebaut.
