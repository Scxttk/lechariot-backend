# Scraper-Referenz

Stand: 2026-07 (KW 29), verifiziert mit PLZ 01219 (Dresden). Typische
Angebotszahlen schwanken je Woche und Region.

## Endpoints pro Kette

| Kette | Quelle / URL-Muster | Auth / Eigenheiten | Markt-Bezug | Typisch/Woche | Fixture |
|---|---|---|---|---|---|
| REWE | `rewerse`-CLI (mobile API) | **mTLS**: Client-Zertifikat aus der REWE-App nötig (`cert.pem` + `private.key`, siehe [docs/rewe-cert.md](../rewe-cert.md)) | filialspezifisch (PLZ → marketId) | variiert je Filiale | `tests/fixtures/rewe/discounts.json` (handgebaut im rewerse-Format) |
| Penny | `penny.de/.rest/market`, Kategorien aus `/angebote`-HTML, dann `/.rest/offers/by-category/<JAHR-WOCHE>/<kategorie>?region=<sellingRegion>` | nur Browser-User-Agent; Aktionspreise als String mit Fußnoten-Sternchen (`"0.49*"`) | regional (`sellingRegion` des Markts) | ~550-600 (2 Wochen) | `tests/fixtures/penny/offers_kuehlregal.json` |
| Kaufland | `filiale.kaufland.de/.klstorefinder.json` + server-seitig gerendertes `/angebote/uebersicht.html` | Filiale über Cookie `x-aem-variant=<id>`; **Titel = Marke, Produkt im Untertitel** (Offer-ID enthält deshalb den Untertitel); **dasselbe Angebot erscheint in mehreren Kategorien** (Warengruppe + „Unsere Knüller" etc.) — Dedup erst beim DB-Upsert über die ID | filialspezifisch | ~650 (inkl. Kategorie-Duplikate) | `tests/fixtures/kaufland/uebersicht.html` |
| Lidl (Standard) | `api.marktguru.de/api/v1/offers/search` (paginiert) | **einzige Kette über einen Dritten**; API-/Client-Key aus dem marktguru-HTML gelesen, eingefrorene Keys als Fallback; `advertisers[].uniqueName == "lidl"` filtert Fremdhändler | regional (marktguru-Region zur PLZ) | ~750 (2-3 Wochen) | `tests/fixtures/lidl/marktguru_offers.json` |
| Lidl (Prospekt) | Store-Finder-Feld `AR` → `lidl.com/flyer/esi-overview` → `endpoints.leaflets.schwarz/v4/flyer?flyer_identifier=<slug>` → `pdfUrl` → `pdftotext -bbox-layout` | kein Key, keine Anmeldung; braucht **poppler-utils**; PDF ~83 MB; Angebotspreis am Stern erkennbar (`2.49*`) | Absatzregion der Filiale (18 Varianten/Woche, 40 Regionscodes) | ~195 (1 Woche) | `tests/fixtures/lidl/prospekt_bbox_layout.xml`, `tests/fixtures/lidl/prospekt_flyer.json` |
| EDEKA | `edeka.de/api/marketsearch/markets?searchstring=<PLZ>`, Markt-ID via 308-Redirect der Legacy-URL, Angebote aus `/maerkte/<id>/angebote/`-HTML | Akamai-Bot-Schutz → System-`curl` (util.rs); Preis maschinenlesbar im `sr-only`-Div („Festpreis von 3.99 €" / „App-Preis von …") | filialspezifisch | ~200 | `tests/fixtures/edeka/angebote.html` |
| Netto | Intershop-Filialsuche (JSON) + `/filialangebote/{1,2,4,5}`-HTML | Akamai → System-`curl`; Filiale über Cookie `netto_user_stores_id` | filialspezifisch | ~300 | `tests/fixtures/netto/filialangebote_1.html` |
| ALDI Nord | `aldi-nord.de/angebote.html`, Daten im `__NEXT_DATA__`-JSON (`OFFER_GET.res.algoliaDataMap`) | plain reqwest | bundesweit (`ALDI_NORD_DE`) | ~230 | `tests/fixtures/aldi_nord/angebote.html` |
| ALDI Süd | `api.aldi-sued.de/v3/product-search?categoryKey=1588161426582123` (paginiert) | Akamai → System-`curl`; Preise in **Cent**; keine Gültigkeitsdaten | Süd-Gebiet einheitlich (`ALDI_SUED_DE`) | ~75 | `tests/fixtures/aldi_sued/product_search.json` |

## Bekannte NULL-Preise (diagnostiziert 2026-07)

- **EDEKA (~20-25/Woche): echt.** „Tagespreis"-Kacheln und reine
  PAYBACK-Extra-Punkte-Kacheln tragen weder in der Kachel noch im
  zugehörigen Dialog einen Preis. Sie kommen bewusst mit `price = NULL` an.
- **Lidl: erledigt mit dem Quellenwechsel.** Die alten ~7 NULL-Preise/Woche
  stammten aus der `lidl.de/q/api/search`-Quelle, in der Lidl-Plus-Angebote
  den Preis in `lidlPlus[0].price` statt in `price` trugen. Diese Quelle gibt
  es nicht mehr (seit 2026-07 marktguru). Im Prospekt-Weg sind
  Lidl-Plus-Preise ganz normale Sternpreise und im Untertitel als „nur mit
  Lidl Plus" gekennzeichnet.

## Lidl: zwei Quellen, umschaltbar

Lidl ist die einzige Kette, die ihre Angebote über einen Dritten bezieht, und
mit rund 30 % aller Zeilen zugleich die größte. `LIDL_SOURCE=prospekt`
schaltet auf Lidls eigenen Wochenprospekt um (`src/scrapers/lidl_prospekt.rs`),
alles andere bleibt bei marktguru. Beides läuft nebeneinander, damit sich die
Quellen über ein paar Wochen vergleichen lassen.

```sh
LIDL_SOURCE=prospekt smartshop fetch --store lidl --zip 01219 --dry-run
```

Was der Prospekt besser kann:

- **regionsgenau** — der Prospekt gilt für die Absatzregion der Filiale,
  marktguru nur ungefähr regional;
- **Streichpreise** — `UVP` / `Normalpreis` stehen im Prospekt, marktguru
  liefert für Lidl gar kein `regular_price`;
- **seitengenaue Laufzeiten** — Donnerstag-Angebote tragen im Seitenkopf
  („Ab Do. 23.7. bis Sa. 25.7.") eine kürzere Gültigkeit als der Prospekt.

### Wie vollständig ist der Prospekt-Weg?

Gemessen gegen marktguru, weil das die Messlatte ist. marktguru bezieht seine
Lidl-Daten aus **demselben Prospekt** — die API gruppiert Angebote nach
`leafletFlightId`, und für die Woche 20.–25.07.2026 hängen alle 375 Zeilen an
genau einem Flight. Es gibt also keine geheime Zusatzquelle.

Stand 2026-07-25 für PLZ 01219:

| | marktguru | Prospekt-Weg |
|---|---:|---:|
| Angebote der Woche | 375 | **382** |
| Preis in der jeweils anderen Quelle vorhanden | — | **96,3 %** |
| Preis **und** passender Name | — | **76,5 %** |

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

### Dritter Weg: `LIDL_SOURCE=prospekt-llm`

Dieselbe Textebene, aber seitenweise durch ein Sprachmodell
(`src/scrapers/lidl_llm.rs`, braucht `ANTHROPIC_API_KEY`). Gedacht für die
Kacheln, an denen die Geometrie scheitert — Obst und Gemüse ohne Marke,
Non-Food ohne Marke-Name-Beschreibung-Struktur.

Die Halluzinationsfrage ist die einzige, die hier zählt, und sie ist
mechanisch beantwortet: Jeder Preis muss **wörtlich im Seitentext stehen**
(`price_is_grounded`), und wo Packungsgröße und Grundpreis vorliegen, muss
dieselbe Rechenprobe aufgehen wie im geometrischen Weg. Was das nicht besteht,
wird verworfen — das Modell kann Zeilen übersehen, aber keine erfinden.

Kosten: ein Wochenprospekt sind rund 35.000 Tokens; der Systemprompt ist über
alle Seiten identisch und wird per `cache_control` zwischengespeichert. Seiten
ohne Sternpreis gehen gar nicht erst ans Modell.

```sh
ANTHROPIC_API_KEY=... LIDL_SOURCE=prospekt-llm smartshop fetch --store lidl --zip 01219 --dry-run
```

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
