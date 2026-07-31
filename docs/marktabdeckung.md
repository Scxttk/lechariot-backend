# Marktabdeckung — welche Ketten fehlen und warum

Stand: 2026-07-31. Marktzahlen aus dem NIQ-Trade-Dimensions-Ranking 2025
(veröffentlicht bei lebensmittelpraxis.de); alle technischen Aussagen sind an
diesem Tag live gegen die jeweilige Seite gemessen, nicht geschätzt.

Diese Notiz beantwortet zwei Fragen, die immer wieder aufkommen: **Wie viel vom
deutschen Markt deckt Le Chariot ab?** Und: **Welche Kette lohnt als nächste?**
Zu drei prominenten Kandidaten (dm, Rossmann, Konsum) lautet die Antwort „gar
nicht" bzw. „nicht jetzt" — deshalb steht die Begründung samt Beleg hier, damit
die Frage nicht in drei Monaten von vorn gestellt wird.

## Die Rechenbasis

Edeka und Rewe halten zusammen 46,8 % bei 155,282 Mrd. € Umsatz. Daraus folgt
ein Gesamtuniversum von rund **331,8 Mrd. €**, und daran sind alle Prozentzahlen
unten gerechnet. Die Gruppenumsätze enthalten die Töchter: Netto
Marken-Discount steckt in Edeka, Penny in Rewe, Lidl und Kaufland in Schwarz.

## Abgedeckt: 77,8 %

| Gruppe | Mrd. € | Anteil | in der App |
|---|---:|---:|---|
| Edeka-Gruppe | 84,667 | 25,5 % | EDEKA · Netto Marken-Discount (19,833 / 6,0 %) |
| Rewe-Gruppe | 70,615 | 21,3 % | REWE · Penny (10,837 / 3,3 %) |
| Schwarz-Gruppe | 61,335 | 18,5 % | Lidl (34,810 / 10,5 %) · Kaufland (24,425 / 7,4 %) |
| Aldi-Gruppe | 36,900 | 11,1 % | ALDI Nord (16,310 / 4,9 %) · ALDI SÜD (20,520 / 6,2 %) |
| Norma | 4,775 | 1,4 % | NORMA (seit diesem Zweig, siehe unten) |
| **Summe** | **258,3** | **77,8 %** | |

Das sind **neun Ketten** in `Store::ALL` — acht waren es bis NORMA.

## Nicht abgedeckt

| Kette | Mrd. € | Anteil | Angebote holbar? |
|---|---:|---:|---|
| dm | 13,270 | 4,0 % | **nein — es gibt keine Angebote** |
| Rossmann | 10,500 | 3,2 % | **nein — Bot-Challenge auf jeder URL** |
| Bartels-Langness (famila Nordost, CITTI) | 7,265 | 2,2 % | erreichbar, Struktur ungeprüft |
| Globus | 6,652 | 2,0 % | erreichbar, Struktur ungeprüft |
| Müller (Drogerie) | 3,726 | 1,1 % | `/angebote/` → 404, Pfad unbekannt |
| Bünting (Combi, famila Nordwest) | 2,420 | 0,7 % | erreichbar, Struktur ungeprüft |
| Dohle (HIT) | 1,962 | 0,6 % | erreichbar, Struktur ungeprüft |
| Dennree / denn's Biomarkt | 1,660 | 0,5 % | erreichbar, Struktur ungeprüft |
| tegut | 1,404 | 0,4 % | erreichbar, Struktur ungeprüft |
| Alnatura | 1,247 | 0,4 % | ungeprüft |
| **Netto ApS („mit Hund")** | 1,225 | 0,4 % | **ja — offene JSON-API, Angebote *und* Filialen** |
| Kaes (V-Markt) | 1,128 | 0,3 % | ungeprüft |
| Wasgau · Mix Markt · Klaas+Kock · Budnikowsky | je 0,5–0,7 | je 0,2 % | budni erreichbar |
| Konsum Leipzig | ~0,201 | 0,06 % | Angebotsweg ungeklärt |
| Konsum Dresden | ~0,111 | 0,03 % | nur PDF, funktioniert (siehe unten) |

Nicht aufgeführt, weil Großhandel ohne Endkundenfilialen: Metro (5,870),
Transgourmet (5,062), Lüning (0,865), Stroetmann (0,626), Hamberger (0,440),
Weiling (0,295).

**Die wichtigste Aussage dieser Tabelle steht nicht drin:** Es gibt keine große
fehlende Kette mehr. Was hinter den abgedeckten 77,8 % liegt, sind zwei
Drogerien (7,2 %, davon eine ohne Angebotsmodell) und ein langer Schwanz
regionaler Ketten, von denen keine über 2,2 % kommt. Jede weitere Kette ist ein
Nachkommastellen-Geschäft — außer für die Nutzer, die genau dort einkaufen.

---

## Die zwei Nettos: gleicher Name, andere Angebote

Es gibt in Deutschland **zwei Ketten namens Netto**, und Le Chariot deckt genau
eine davon ab. Die Verwechslung ist so verbreitet, dass die Zuordnung hier
einmal festgehalten wird — mitsamt der Messung, die die naheliegende Frage
beantwortet.

| | Netto **Marken-Discount** | Netto **ApS & Co. KG** |
|---|---|---|
| Umgangssprachlich | „Netto **ohne** Hund" | „Netto **mit** Hund", der schwarz-gelbe |
| Logo | gelb-rot, kein Tier | gelb-schwarz, Scottish Terrier „Scottie" |
| Eigentümer | Edeka | Salling Group (Dänemark) |
| Filialen | ~4.400, bundesweit | ~340, acht nord- und ostdeutsche Länder |
| Domain | netto-online.de | netto.de |
| In Le Chariot | **ja** (`src/scrapers/netto.rs`) | nein |

Die beiden waren nie dasselbe Unternehmen; Edeka hielt von 2005 bis 2012 25 %
an der deutschen Netto-ApS-Tochter, diese Verbindung besteht nicht mehr.

### Haben sie dieselben Angebote? Nein — gemessen am 2026-07-31

Verglichen wurde **dieselbe Kalenderwoche** (KW 31): der Netto-ApS-Prospekt
gegen den Live-Lauf `fetch --store netto --zip 01219`.

| | Netto ApS | Netto Marken-Discount |
|---|---:|---:|
| Angebote der Woche | 327 | 287 |
| wörtlich identische Titel | **1** („Aprikosen") | |
| Wortschatz-Überschneidung (Jaccard) | **9 %** | |
| ApS-Angebote mit einem auch nur *ähnlichen* Gegenstück (≥ 2 gemeinsame Wörter) | **17 von 327 (5 %)** | |

Der eine identische Titel ist ausgerechnet loses Obst. Alles andere geht
auseinander, und zwar an der Wurzel: verschiedene Einkäufer, verschiedene
Eigenmarken, verschiedene Sortimente (Netto ApS wirbt mit „mehr als 1.700
Artikeln", ein harter Discounter). **Ein Netto-ApS-Angebot lässt sich nicht aus
den Marken-Discount-Daten ableiten** — wer beide will, braucht beide Scraper.

### Netto ApS wäre billig zu bauen

Die Angebote liegen bei **Tjek** (ehemals ShopGun), und deren API ist offen —
kein Schlüssel, kein Token (verifiziert 2026-07-31):

```sh
# Angebote eines Prospekts, paginiert
curl 'https://squid-api.tjek.com/v2/offers?catalog_id=Nmr1rXcs&limit=100&offset=0'
# Filialen im Umkreis
curl 'https://squid-api.tjek.com/v2/stores?dealer_id=90f2VL&r_lat=51.05&r_lng=13.74&r_radius=25000'
```

Je Angebot: `heading`, `description` (mit Menge, Grundpreis und UVP),
`pricing.price`, **`pricing.pre_price` als Streichpreis**, `quantity.unit`,
`catalog_page`, Laufzeit. Die Filial-Antwort trägt Name, Straße, PLZ, Ort und
Koordinaten. Beide Hälften also aus einer Quelle, sauber strukturiert.

Zwei Dinge sind trotzdem abzuwägen:

1. **Es wäre wieder ein Dritter.** Genau davon ist das Projekt bei Lidl
   weggegangen (`remove-marktguru`). Die Prospektbilder auf netto.de sind reine
   `.webp`-Seiten ohne Textebene — einen herstellereigenen Weg gibt es hier
   also nicht, Tjek ist die Quelle.
2. **Der Kettenname muss unterscheidbar sein.** `Store::chain()` liefert für
   die abgedeckte Kette „Netto". Käme Netto ApS als „Netto" dazu, liefen beide
   in derselben Sektion der App zusammen und `Store::from_chain` würde
   mehrdeutig. Vorschlag: „Netto" bleibt beim Marken-Discount (so stehen die
   Daten schon in Supabase), Netto ApS bekommt einen eigenen Namen.

**Für Dresden ist die Kette keine Randnotiz:** 11 Filialen im 25-km-Umkreis
(Lindengasse, Blasewitzer Straße, Prohliser Allee, Leuben, Bühlau …), Berlin
66, Hamburg 8, Leipzig 3. National sind es 0,4 % — im Testgebiet dieser Beta
deutlich mehr.

---

## dm: es gibt nichts zu holen

dm verzichtet seit 1994 **prinzipiell** auf Werbeprospekte und Rabattaktionen.
Das Preismodell heißt „Dauerpreis": ein Preis, der mindestens vier Monate hält.
Es ist keine technische Hürde und kein Bot-Schutz — es existiert schlicht kein
Wochenangebot, das ein Scraper einsammeln könnte.

Das trifft Le Chariot an der Wurzel, weil die App ausdrücklich **Angebote und
keine Normalpreise** vergleicht (so steht es in der Fußnote der Ranking-Karte:
„Die Summe zählt nur die gematchten Angebote — Artikel ohne Treffer und
Normalpreise sind nicht eingerechnet"). Ein dm-Scraper im heutigen Datenmodell
liefert null Zeilen.

### Was ein dm-Weg wirklich wäre

dm betreibt eine offene, unauthentifizierte JSON-API. Verifiziert am 2026-07-31:

```sh
curl 'https://product-search.services.dmtech.com/de/search/crawl?query=shampoo'
# HTTP 200, 115 KB, 30 Produkte
```

Je Produkt kommen GTIN, `dan` (dm-Artikelnummer), Marke, Titel, Bild,
`price.price.current.value` („1,75 €"), Grundpreis („0,25 l (7,00 € je 1 l)")
und `trackingData.price` als Zahl. Technisch ist das der einfachste Scraper, den
dieses Repo je hätte.

Fachlich ist es aber ein **neues Feature**, kein neunter Angebots-Scraper, und
es hängt an zwei Entscheidungen, die niemand nebenbei trifft:

1. **Normalpreise in einer Angebots-App.** Der ganze Vergleich, das Ranking und
   die ehrliche Fußnote setzen darauf, dass eine Zeile ein befristetes Angebot
   ist. Dauerpreise müssten sichtbar anders behandelt werden.
2. **Der Non-Food-Filter müsste weichen.** `src/matching.rs` verwirft Drogerie
   heute aktiv: `NONFOOD_CAT` enthält `drogerie`, `NONFOOD_TERMS` trifft
   Duschgel, Shampoo, Sonnencreme, Feuchttücher und Dutzende weitere Begriffe.
   Das war kein Versehen, sondern Absicht — diese Zeilen verschmutzen die
   Treffer. Wer dm aufnimmt, dreht diese Entscheidung für eine ganze
   Warengruppe um.

**Fazit: nicht bauen.** Wenn Drogerieartikel je dazukommen sollen, beginnt das
mit der Produktfrage, nicht mit einem Scraper.

## Rossmann: am Vordereingang zu

Rossmann hat Wochenangebote — anders als dm. Sie sind nur nicht erreichbar.

Jede angefragte URL liefert dieselben **3038 Bytes** mit dem Titel „Client
Challenge": Startseite, `/de/angebote`, `/de/filialen`, sogar `robots.txt` und
`sitemap.xml`. Die Asset-Pfade der Seite beginnen mit `/_fs-ch-…` — das ist F5
Distributed Cloud Bot Defense mit JavaScript-Challenge.

Getestet und **wirkungslos**: derselbe Header-Satz, mit dem `util::curl_get`
Netto, EDEKA und ALDI SÜD durch Akamai bringt (System-curl, voller
Browser-User-Agent, `Accept`, `Accept-Language`, `Sec-Fetch-*`,
`Upgrade-Insecure-Requests`). Antwort: wieder 3038 Bytes.

Der Unterschied zu Akamai ist grundsätzlich. Akamai fingerprintet den
TLS-Stack — dagegen hilft ein anderer Client. F5 verlangt hier, dass echtes
JavaScript im Browser ein Token rechnet. Das ginge nur mit einem
Headless-Browser im Nightly, und damit wären Laufzeit, Speicherbedarf und
Wartungsaufwand der Pipeline in einer anderen Größenordnung als für alle neun
bestehenden Ketten zusammen.

**Fazit: nicht bauen**, solange Rossmann die Challenge fährt. Der Befund ist
billig nachzuprüfen — wenn `curl -s https://www.rossmann.de/de/angebote | wc -c`
irgendwann etwas anderes als 3038 sagt, lohnt ein zweiter Blick.

## Konsum Dresden: geht, lohnt aber (noch) nicht

Konsum Dresden veröffentlicht die Wochenangebote **ausschließlich als PDF**. Die
HTML-Seite `konsum.de/angebote` enthält keinen einzigen Preis, nur den Link:

```
https://konsum-dev.de/content-website/wp-content/uploads/2026/07/
KONSUM_DRESDEN_Angebote-der-Woche-KW31_2026.pdf     # 8,1 MB
```

Die gute Nachricht: `pdftotext -layout` liefert eine **saubere Textebene** mit
allem, was gebraucht wird — Marke, Produkt, Sorte, Preis, Packungsgröße,
Grundpreis, Rabattprozent, Laufzeit im Seitenfuß („Gültig vom 27.07. bis
01.08.2026 / KW 31"). Sogar ein echter Referenzpreis steht drin: „n. G. = niedrigster
Gesamtpreis der letzten 30 Tage" — sauberer als jeder UVP.

Das ist genau der Weg, den `src/scrapers/lidl_prospekt.rs` schon geht, und der
Konsum-Prospekt ist um ein Vielfaches einfacher als Lidls 69-Seiten-Heft.

Was dagegen spricht, ist die Größe: 30 Märkte, rund 111 Mio. € Umsatz, **0,03 %**
des deutschen Marktes. Konsum Leipzig (61 Märkte, ~201 Mio. €, 0,06 %) ist eine
eigene Genossenschaft mit eigener Domain; deren Angebotsweg wurde nicht geklärt.
Beide zusammen bleiben unter 0,1 %.

**Fazit: vertagt, nicht verworfen.** Der teure Teil einer solchen Entscheidung
ist die Sondierung, und die steht jetzt hier. Wenn Konsum je gebaut wird, ist es
eine Heimat-Entscheidung für Dresden, keine Marktanteils-Entscheidung — und das
ist ein legitimer Grund, nur eben ein anderer.

Randnotiz: Konsum Dresden schließt sich dem EDEKA-Verbund an und wird ab Mitte
2026 überwiegend von Edeka Nordbayern-Sachsen-Thüringen beliefert. Das ändert
die Belieferung, nicht die Angebotsblätter — die Genossenschaft wirbt weiter
selbst.

## NORMA: gebaut

`src/scrapers/norma.rs`, seit diesem Zweig in `Store::ALL`. Endpunkte und
Fallstricke stehen in [docs/scrapers/README.md](scrapers/README.md); hier die
Messung vom 2026-07-31, die die Entscheidung getragen hat:

- **221 Angebote** über 22 Themenseiten, **215 davon (97 %)** mit einem Preis,
  der maschinenlesbar im Markup steht, **221 (100 %)** mit Bild und Rohkategorie
- **23 HTTP-Requests** pro Lauf, plain `reqwest` — kein Akamai, kein Cookie,
  kein API-Key, keine Anmeldung
- **Streichpreise inklusive** („statt 1,59" / „UVP 2,49") — 91 von 221; das
  trifft den offenen Roadmap-Punkt „Streichpreise Lidl → REWE → EDEKA"
- Grundpreis, Packungsgröße, Marke, Bild, Kategorie und Startdatum stehen dabei
- Angebotskatalog bundesweit → gehört zu `Store::stores_nationally()` wie ALDI

Zum Vergleich: EDEKA liefert ~200 Angebote pro Woche und braucht dafür
System-curl gegen Akamai plus einen Redirect je Filiale; ALDI SÜD liefert ~75.

**Die Kategorie ist ehrlich schwächer als die anderen Zahlen.** 137 der 221
Angebote (62 %) bekommen von `enrich` eine echte Kategorie, der Rest landet auf
„Sonstiges". Das liegt nicht am Parser, sondern an NORMAs Themen: 59 der 84
Sonstiges-Zeilen stehen unter „Profi-Heimwerker Ausrüstung" (19/19),
„Alles für den Garten" (11/11), „Sommer Must-haves" (12/15), „Ab aufs Rad"
(9/10) und „Der grüne Clou" (6/7) — Non-Food, das bei NORMA im selben Prospekt
steht. Die Lebensmittel-Themen laufen sauber durch: „Wochenend-Spezial" 34/36,
„XXL" 18/20, „Kunterbunte Küchentrends" 21/24, „Unser Obst und Gemüse",
„Mittwochs-Clou", „Genuss mit Nuss" und „NEU im Sortiment" komplett.

Bilder liegen auf `www.norma-online.de/ext/img/product/…`, demselben Host wie
die Angebotsseiten, und der gibt sie heraus: blankes `curl` und iPhone-UA
bekommen beide 200 (gemessen 2026-07-31). NORMA braucht also **kein Spiegeln** —
anders als Netto, dessen CDN jeden schlichten Client mit 403 abweist.

**Die Filialen sind die zweite Hälfte, und sie war zuerst nicht da.** Ein
bundesweiter Katalog nützt nichts, solange im Filial-Picker keine NORMA-Filiale
steht — die App liest ihre Auswahl aus `public.branches`, und dort stand NORMA
nicht. Der Filialfinder hängt jetzt in `branches::sync`; der Weg dorthin steht
in [docs/scrapers/README.md](scrapers/README.md), weil der naheliegende
Koordinaten-Endpunkt immer nur eine einzige Filiale liefert. Gemessen am
2026-07-31 im 25-km-Kreis: **01219 Dresden 9 Filialen, 90402 Nürnberg 60** —
alle mit Koordinaten und Straße.

**Ein Fund aus dem Bauen, der über NORMA hinausgeht:** Die App liest
`valid_until` als **nicht-optionales** `Date`. Eine Kette ohne Enddatum kippt
deshalb nicht nur ihre eigenen Zeilen, sondern das Decoding der ganzen
Angebotsantwort. Wer die nächste Kette baut, prüft das **vor** dem ersten Push
— NORMA nennt auf den Themenseiten kein Enddatum, und genau daran wäre es
beinahe gescheitert.

## Regionale Ketten, sondiert am 2026-07-31

Nur ein HTTP-Aufruf je Kette, mit gewöhnlichem Browser-User-Agent. Das sagt,
**ob die Tür offen ist** — nicht, ob die Angebote im HTML stehen. Die
Seitengröße ist ein Indiz, mehr nicht: server-gerenderte Angebotsseiten sind
groß, SPA-Hüllen klein.

| Kette | URL | Ergebnis |
|---|---|---|
| Globus | `globus.de/angebote` | 200, 310 KB |
| Bünting / Combi | `combi.de/angebote` | 200, 253 KB |
| Budnikowsky | `budni.de/angebote` | 200, 250 KB |
| Dohle / HIT | `hit.de/angebote` | 200, 180 KB |
| famila Nordost | `famila-nordost.de/angebote` | 200, 141 KB |
| tegut | `tegut.com/angebote.html` | 200, 70 KB |
| denn's Biomarkt | `denns-biomarkt.de/angebote` | 200, 29 KB |
| Müller | `mueller.de/angebote/` | **404** — anderer Pfad nötig |

Wer hier weitermacht, fängt mit **Globus** an (2,0 %, größter erreichbarer
Kandidat) und prüft als Erstes, ob Preise im gelieferten HTML stehen oder erst
per JavaScript nachgeladen werden. Achtung bei allen sieben: es sind
Regionalketten. Ein Scraper hilft nur den Testern in deren Gebiet — Globus
betreibt rund 64 Märkte, überwiegend im Saarland, in Hessen und Thüringen.

## Wie diese Zahlen nachzuprüfen sind

Marktanteile ändern sich jährlich, die Endpunkte häufiger. Beides ist billig
nachzumessen:

```sh
# Erreichbarkeit einer Kette (3038 Bytes bei Rossmann = Challenge)
curl -s -o /dev/null -w '%{http_code} %{size_download}\n' \
     -A 'Mozilla/5.0' https://www.rossmann.de/de/angebote

# dm hat weiterhin keine Angebote? Dann findet diese Seite keine Preise:
curl -s https://www.dm.de/angebote | grep -c 'aria-label.*Euro'
```

Die Umsatzzahlen stehen im jährlichen Top-30-Ranking von NIQ Trade Dimensions.
Wichtig beim Nachrechnen: Die Gruppenumsätze **enthalten** die Discount-Töchter;
wer Edeka und Netto addiert, zählt Netto doppelt.
