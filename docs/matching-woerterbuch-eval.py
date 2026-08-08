#!/usr/bin/env python3
"""Wörterbuch-Entwurf: taggt aktuelle Angebote regelbasiert mit Alltagsbegriffen."""
import sqlite3, re, os, json, sys
from collections import Counter, defaultdict

DB = os.path.expanduser("~/.local/share/lechariot/lechariot.db")

# Kategorien, die klar Non-Food sind (Ketten-Marketing-Kategorien)
NONFOOD_CAT = re.compile(r"mode|style|heim|haus|garten|haustier|tierbedarf|tiernahrung|pflanzen|angeln|elektro|medien|kinderzimmer|wäschepflege|schulstart|alles für die schule|kochen-und-grillen|drogerie|spielzeug|alltagshelfer|technik|spielwaren|baumarkt|multimedia|bekleidung|schuhe|camping|auto|buero|non.?food|onlineshop|e-bikes?|fahrrad|trolley|koffer|unterhemd|\btops\b|\bteller\b|sch[üu]sseln?|staubsauger|wischroboter|k[üu]chenmasch|n[äa]hmasch|batterien|\bfarben\b|kleber|zahngesundheit|glasartikel|aufbewahr|essgeschirr|reinigungsger|k[üu]chenger|k[üu]chengro|k[üu]chenzubeh|grillzubeh|backzubeh|damen|herren|w[äa]sche\b|leuchten|m[öo]bel|werkzeug|haartrockner|rasierer|zahnpflege|fernseh|led ?& ?lcd|kapsel|padmasch|\bk[üu]che\b|blumen|strauß|kochen-und-backen|reinigen|waschmittel|büro", re.I)
FOOD_CAT = re.compile(r"obst|gemüse|fleisch|geflügel|wurst|molkerei|fette|getränke|feinkost|konserven|kaffee\b|tee|süßwaren|knabber|grundnahrung|fisch|bäckerei|backwaren|tiefkühl", re.I)

# Kategorie → Begriff, als LETZTER Ausweg: greift nur, wenn Titel und
# Untertitel nichts hergeben. Die Ketten schreiben dorthin, was das Produkt
# IST — „Die Extrazarte" ist nichtssagend, die Kategorie daneben sagt Butter.
#
# Gepflegte Zuordnung statt freier Wörterbuchsuche über die Kategorie, und der
# Unterschied ist gemessen (2026-07-31, 423 Food-Angebote): Die freie Variante
# holt 33 Zeilen, davon sind VIER falsch — „Not Milk" unter *Veganes* würde
# `tofu`, „Spitzkohl" unter *Brokkoli und Kohl* würde `brokkoli`, und beide
# Waschmittel würden `windeln/hygiene`. Ein fehlendes Tag kostet einen
# Treffer; ein falsches legt jemandem das falsche Produkt in den Einkauf.
# Deshalb steht hier nur, was jede Zeile für sich begründet.
#
# Verglichen wird die NORMALISIERTE Kategorie auf Gleichheit — kein Substring,
# kein Regex. „Geflügel" fehlt bewusst (das ganze Kaninchen stand darunter),
# „Veganes" und „Brokkoli und Kohl" ebenso.
KAT_ROH = {
    "Brot": "brot",
    "Brötchen": "brot",
    "Kuchen & Feinbackwaren": "backwaren",
    "Kekse": "kekse",
    "Joghurt": "joghurt",
    "Butter": "butter",
    "Käse": "käse",
    "Sahne, Schmand und Crème fraîche": "sahne",
    "Schokoladen": "schokolade",
    "Nüsse": "nüsse",
    "Eis": "eis",
    "Wasser": "wasser",
    "Softdrinks": "limonade",
    "Weissweine": "wein",
    "Rotweine": "wein",
    "Champagner": "wein",
    "Brauner Rum": "spirituosen",
    "Kräuter": "gewürze",
    "Gewürzmischungen": "gewürze",
    "Öle und Fette": "öl",
    "Meeresfrüchte": "fisch",
    "Tiefkühlfisch und Meeresfrüchte": "fisch",
    "Tiefkühlgerichte": "fertiggericht",
    "Instantgerichte": "fertiggericht",
    # Lidls Aufstrich-/Dip-Regale (2026-07-31): jede Zeile darunter ist ein
    # Dip oder herzhafter Aufstrich (Zaziki, Ajvar, Antipasti Creme) —
    # `soßen` führt Dips schon im exact. „Schokoaufstrich" fehlt bewusst:
    # die eine Zeile darunter (Nutella) trifft ihr Tag über den Titel.
    "herzhafte Aufstriche": "soßen",
    "Dips": "soßen",
    "Windeln": "windeln/hygiene",
    # Runde 2026-08-01, Op 2: zwölf weitere Kategorien aus dem 11-Regionen-
    # Korpus. Aufnahmekriterium war nicht „hilft viel", sondern: JEDES Produkt,
    # das im Korpus unter dieser Kategorie steht, gehört zu dem Begriff —
    # nachgesehen, nicht geschätzt (Produkte gesamt / davon vorher ungetaggt).
    "Bier": "bier",                       # 26/3  — alle 26 sind Bier, 23 tragen `bier` schon über den Titel
    "Fisch": "fisch",                     # 6/2   — alle 6 sind Fisch (die 2: Flusskrebssalat, Norweg. Lachsfiletseite)
    "Schnittkäse": "käse",                # 7/4   — die 4 sind „Plus Pack 156 g, Bärlauch/Cremig/Mild Nussig/Würzig"
    "Kaffee": "kaffee",                   # 2/1   — „Crema e Aroma 1 kg"
    "Schinken": "wurst",                  # 4/1   — „Bacon 150 g"; die anderen 3 tragen `wurst` schon
    "Bäckerei": "backwaren",              # 7/3   — Dinkel-Bürli, Vollkorn-Saaten-Rusti, Speck-Käse-Twister
    "Sportnahrung": "protein/fitness",    # 9/4   — Kreatinpulver, Magnesium-Sticks, Flavor Powder, Getränkesirup
    "Chips & Knabbereien": "chips",       # 4/2   — Grissotti (Sesam, Sesam-Mohn-Leinsamen); `chips` führt Cracker und Salzstangen
    "Lakritz & Fruchtgummi": "schokolade",# 4/2   — `schokolade` ist der Süßwaren-Topf des Wörterbuchs (Haribo, Katjes, Lakritz)
    "Nudeln & Pasta": "nudeln",           # 1/1   — „Lasagneblätter 500 g"
    "Nüsse & Trockenfrüchte": "nüsse",    # 1/1   — „Getrocknete Cranberries 200 g"
    "Rindfleisch": "rind",                # 1/1   — „Rind Hamburger 400 g"
    # Bewusst NICHT aufgenommen, obwohl sie ungetaggte Zeilen tragen: die
    # Kategorie nennt zwei Familien oder eine ganze Abteilung, und ein Begriff
    # daraus wäre für den Rest falsch. „Molkereiprodukte, Fette" (5 ungetaggt,
    # aber Käse, Joghurt, Pudding, Margarine nebeneinander), „Alkoholfreie
    # Getränke" (Wasser/Limo/Saft gemischt), „Obst und Gemüse", „Frühstück",
    # „Grundnahrungsmittel", „Kochen und Backen", „Feinkost, Konserven",
    # „Frische-Aktion: Fleisch & Fisch", „Wein und Spirituosen", „Kühlung"
    # (14 ungetaggt, aber Wurst, Käse, Milch, Rote Grütze in einem Regal).
}

# Wörterbuch: begriff -> (exakte tokens, komposita-suffixe, blockliste)
V = {
 # „toast" und „baguette" sind hier heraus und haben eigene Begriffe: Vier
 # Meldungen der Runde vom 05.08. sind `wrong_variant` — wer „Brot" schreibt,
 # meint einen Laib und bekam Toast, Baguette und Knäckebrot. Der Beschluss
 # dazu steht seit dem 31.07. in der Roadmap: ein feinerer Begriff, kein
 # Sperreintrag. Das Suffix „brot" fängt „Toastbrot" weiterhin — ein Brot,
 # das so heißt, ist eines.
 "brot":(["brot","broetchen","brötchen","ciabatta"],["brot","broetchen","brötchen"],["brotaufstrich","aufbackbrötchen?","russisch brot","knäckebrot","knäckebrote"]),
 "toast":(["toast","toasties"],[],[]),
 "baguette":(["baguette","baguettes"],["baguette"],[]),
 "knäckebrot":(["knäckebrot","knäckebrote","knäcke","reiswaffeln"],[],[]),
 "milch":(["milch","frischmilch","vollmilch","buttermilch","mandeldrink","haferdrink","sojadrink"],["milch"],["milchreis","milchschnitte","milchbrötchen","kokosmilch","milcheis","milchschokolade","kondensmilch","sonnenmilch","kokosnussmilch","milka","knoppers","milch schnitte","bergkäse","käsescheiben","quark","camembert"]),
 "butter":(["butter","süßrahmbutter","weidebutter","markenbutter","kærgården","kaergarden"],[],["butterkäse","buttergemüse","erdnussbutter","buttermilch","butterkeks","nut butter"]),
 "käse":(["käse","kaese","käsescheiben","käsesnack","cheestrings","cottage","gouda","emmentaler","edamer","maasdamer","bergkäse","butterkäse","cheddar","parmesan","grana","halloumi","finello","obazda"],["käse","kaese"],["käsekuchen","frischkäse","croissant","leberkäse","laugenstange","laugengebäck","brezel","käsebrötchen","käsestange","käsegebäck","fleischkäse","twister"]),
 "frischkäse":(["frischkäse","frischkaese"],[],[]),
 "mozzarella":(["mozzarella"],["mozzarella"],[]),
 "feta":(["feta","hirtenkäse","schafskäse"],[],[]),
 "quark":(["quark","speisequark","skyr"],["quark"],["quarkbällchen"]),
 "joghurt":(["joghurt","jogurt"],["joghurt","jogurt","ghurt"],[]),
 "sahne":(["sahne","schlagsahne","schmand","creme fraiche","crème fraîche"],["sahne"],["sahnetorte","sahnebonbon"]),
 "eier":(["eier","ei","freilandeier","bio-eier"],["eier"],["eierlikör","eiernudeln","eierkuchen"]),
 "tomaten":(["tomate","tomaten","rispentomaten","cherrytomaten","kirschtomaten","strauchtomaten","romatomaten","cocktailtomaten"],["tomaten"],["tomatenmark","tomatensoße","tomatensauce","tomatenketchup","tomatensaft","tomatensuppe"]),
 "gurke":(["gurke","gurken","salatgurke","salatgurken","minigurken"],["gurke","gurken"],["gewürzgurken","essiggurken","gurkensticks"]),
 "paprika":(["paprika","spitzpaprika"],["paprika"],["paprikachips","paprikasauce"]),
 "salat":(["salat","eisbergsalat","kopfsalat","feldsalat","rucola","blattsalat","salatherzen"],["salat"],["salatdressing","salatsoße","nudelsalat","kartoffelsalat","krautsalat","fleischsalat","wurstsalat","krebssalat","flusskrebssalat","matjessalat","thunfisch-salat","salatcreme","salatmayonnaise"]),
 "zwiebeln":(["zwiebel","zwiebeln","speisezwiebeln","gemüsezwiebeln","rote zwiebeln"],["zwiebeln"],["röstzwiebeln","zwiebelringe","zwiebelkuchen","zwiebelmettwurst"]),
 "knoblauch":(["knoblauch"],[],["knoblauchbaguette","knoblauchsauce"]),
 "kartoffeln":(["kartoffel","kartoffeln","speisekartoffeln","frühkartoffeln"],["kartoffeln"],["kartoffelsalat","kartoffelchips","kartoffelknödel","kartoffelpuffer","süßkartoffeln","kartoffelecken"]),
 "möhren":(["möhre","möhren","karotten","moehren","bundmöhren"],["möhren"],[]),
 "äpfel":(["apfel","äpfel","aepfel"],["äpfel"],["apfelsaft","apfelmus","apfelschorle","apfelkuchen","apfelessig","apfelringe"]),
 "bananen":(["banane","bananen"],[],["bananenmilch"]),
 "zitronen":(["zitrone","zitronen","limetten"],[],["zitronensaft","zitronenlimonade"]),
 "orangen":(["orange","orangen","mandarinen","clementinen"],[],["orangensaft","orangenlimonade"]),
 # `beeren` ist der Schirm und bleibt einer: Das Suffix „beeren" fängt jede
 # Sorte, deshalb braucht das exact die Sortennamen nicht. Sie standen dort
 # aber, und daran hing der Fehler der Runde vom 05.08. — die App bildet ein
 # Suchwort über die exact-Listen auf Begriffe ab, „heidelbeeren" landete so
 # auf `beeren`, und `beeren` trägt jede Erdbeere. Sechs Meldungen.
 "beeren":(["beerenmix","beerenmischung"],["beeren"],["erdbeermarmelade","erdbeerjoghurt"]),
 # Die Sorten. Jede trägt zusätzlich `beeren` über das Suffix — wer „Beeren"
 # schreibt, bekommt weiter alle.
 # Nur der Plural, und das ist gemessen: Die Einzahl fängt über die
 # Plural-Regel („Erdbeere" wird auch als „erdbeer" geprüft) den Geschmack
 # statt die Frucht — „Tafelschokolade, Erdbeere" und „Grießpudding,
 # Himbeere" bekamen so die Frucht-Begriffe. Wer die Frucht sucht, tippt
 # den Plural; das Aroma bleibt draußen.
 "erdbeeren":(["erdbeeren"],[],["erdbeermarmelade","erdbeerjoghurt","erdbeerkonfitüre"]),
 "himbeeren":(["himbeeren"],[],[]),
 "heidelbeeren":(["heidelbeeren","blaubeeren"],[],[]),
 "brombeeren":(["brombeeren"],[],[]),
 "johannisbeeren":(["johannisbeeren"],[],[]),
 "trauben":(["trauben","tafeltrauben","weintrauben"],["trauben"],["traubensaft","traubenzucker"]),
 "melone":(["melone","wassermelone","honigmelone","galiamelone","cantaloupe"],["melone"],[]),
 # `pfirsich` fasst Steinobst bewusst zusammen (Pflaumen, Zwetschgen, …). Die
 # Blockeinträge lösen nur die Kollision mit Pflaumentomaten — ein eigener
 # Begriff `pflaumen` fehlt NICHT (geklärt 2026-07-31).
 "pfirsich":(["pfirsich","pfirsiche","nektarinen","aprikosen","flachpfirsiche","kirschen","pflaumen","plattnektarinen","zwetschgen","mirabellen","sauerkirschen"],["pfirsiche","aprikosen","nektarinen","pflaumen"],["pflaumentomaten","minipflaumen"]),
 "avocado":(["avocado","avocados"],[],[]),
 "zucchini":(["zucchini"],[],[]),
 "aubergine":(["aubergine","auberginen"],[],[]),
 "brokkoli":(["brokkoli","broccoli","blumenkohl","kohlrabi","chicorée","chicoree"],[],[]),
 "pilze":(["champignon","champignons","pilze","pfifferlinge"],["pilze","champignons"],["pilzpfanne","pilzsauce"]),
 "hackfleisch":(["hackfleisch","hack","gehacktes","rinderhack","gemischtes hack"],["hackfleisch","hack"],["hacksteaks"]),
 "hähnchen":(["hähnchen","haehnchen","hähnchenbrust","hähnchenbrustfilet","hähnchenschenkel","hähnchenflügel","poulet","chicken","wings"],["hähnchen","medaillons"],["schweinemedaillons","rindfleisch","rindermedaillons"]),
 "pute":(["pute","putenbrust","putenbrustfilet","putenschnitzel","putensteaks"],["pute"],["putenwurst"]),
 "kondensmilch":(["kondensmilch"],[],[]),
 "kokosmilch":(["kokosmilch","kokosnussmilch"],[],[]),
 "lamm":(["lamm","lammfilets","lammlachs","lammkeule"],[],[]),
 # Suffix `nuggets` war für 100 % seiner Treffer falsch (sieben Hähnchen, ein
 # veganes, null Schwein im 11-Regionen-Korpus) — Chicken Nuggets kommen über
 # `chicken` an, vegane über `vegane`.
 "schwein":(["schwein","schweine","schweinefleisch","schweinemedaillons","kasseler","schweinefilet","schweineschnitzel","schweinebraten","schweinesteaks","nackensteaks","schweinelachs","kotelett","krustenbauch"],["kotelett"],[]),
 "rind":(["rindersteak","rinderfilet","rinderbraten","rumpsteak","entrecote","rinderrouladen","rinder","beinscheiben","roastbeef","gulasch","corned beef","hüftsteaks","patties"],["steak","steaks"],["nackensteaks","schweinesteaks","hacksteaks","putensteaks"]),
 "bratwurst":(["bratwurst","rostbratwurst","grillwurst","bratwürste"],["bratwurst","bratwürste"],[]),
 "wurst":(["wurst","salami","schinken","mortadella","lyoner","leberwurst","mettwurst","wiener","würstchen","aufschnitt","mett","edelsalami","cabanossi","chipolata","sülze","serrano","schinkenwürfel","currywurst","currykrakauer","leberkäse","hackepeter","knacker","landjäger","markenspeck","räucherlendchen","schinkenspeck","schinkenkrakauer"],["wurst","würstchen","schinken","salami","aufschnitt"],[]),
 "fisch":(["lachs","lachsfilet","forelle","kabeljau","seelachs","garnelen","garnele","shrimps","fischstäbchen","matjes","hering","thunfisch","räucherlachs","lachsseite","dorade","doraden","kabeljauloin","wildlachs"],["fisch","filet"],["fischsauce","schwein","schweine","lamm","lammfilets","kasseler","hähnchen","brustfilet","innenfilet","putenbrustfilet"]),
 "nudeln":(["nudeln","spaghetti","penne","fusilli","tagliatelle","tortellini","cappelletti","gnocchi","pasta","lasagne","ramen","ramyun","teigwaren","eierspätzle","kritharaki"],["nudeln"],["nudelsalat","nudelsuppe","pasta sauce","pastasauce","pizza pasta"]),
 "reis":(["reis","basmati","basmatireis","langkornreis","jasminreis","risottoreis"],["reis"],["milchreis","reiswaffeln","reisdrink","puffreis","wassereis"]),
 "mehl":(["mehl","weizenmehl","dinkelmehl","panko","tempura","paniermehl"],["mehl"],[]),
 "zucker":(["zucker","rohrzucker","puderzucker"],["zucker"],["traubenzucker","vanillezucker","zuckerrübensirup"]),
 "salz":(["salz","meersalz","speisesalz"],[],["salzstangen","salzbrezeln"]),
 "öl":(["öl","olivenöl","rapsöl","sonnenblumenöl","speiseöl","erdnussöl","sesamöl","kokosöl"],["öl","oel"],[]),
 "essig":(["essig","balsamico"],["essig"],["essiggurken"]),
 "müsli":(["müsli","muesli","haferflocken","granola","cornflakes","cerealien"],["müsli","flocken"],["müsliriegel"]),
 "marmelade":(["marmelade","konfitüre","fruchtaufstrich","brotaufstrich","honig","nutella","nussnougatcreme"],["marmelade","konfitüre"],["eisbecher","biscuits"]),
 "kaffee":(["kaffee","espresso","coffee","kaffeebohnen","filterkaffee","kaffeepads","kaffeekapseln"],["kaffee"],["kaffeesahne","eiskaffee","kaffeeweißer","haferdrink"]),
 "tee":(["tee","kräutertee","früchtetee","grüner tee","schwarztee","matcha","ländertee"],["tee"],["eistee"]),
 "wasser":(["wasser","mineralwasser","sprudel"],["wasser"],[]),
 "saft":(["saft","orangensaft","apfelsaft","multivitaminsaft","nektar","schorle"],["saft","schorle"],[]),
 "limonade":(["limonade","cola","coca-cola","fanta","sprite","mezzo mix","limo","eistee","energy drink","energydrink"],["limonade"],[]),
 # `weizen` stand im exact, aber in 19.629 Korpus-Zeilen gibt es kein einziges
 # Weizenbier-Angebot — beide Treffer waren falsch (Weizen-Brötchen, Weizen
 # Mehl). Weißbier bleibt; das Suffix `bier` fängt „Weizenbier", falls es kommt.
 "bier":(["bier","pils","pilsener","radler","weißbier","helles","dunkel","schwarzbier","biermischgetränk"],["bier"],["bierschinken","trauben","tafeltrauben"]),
 "wein":(["wein","rotwein","weißwein","rosé","sekt","prosecco","secco","fruchtsecco","chardonnay","merlot","riesling","grauburgunder","sauvignon","blanc","champagner","jahrgangssekt"],["wein"],["weinsauerkraut","weintrauben","weinessig"]),
 "schokolade":(["schokolade","tafelschokolade","pralinen","schokoriegel"],["schokolade"],["schokoladenpudding","trinkschokolade"]),
 "kekse":(["kekse","butterkeks","cookies","gebäck","waffeln"],["kekse","keks"],[]),
 # „Kartoffelchips" fällt über das Suffix `chips` hierher und nicht unter
 # `kartoffeln` — dort steht es auf der Blockliste. Das stand bis 2026-07-31
 # als Fließtext IN der Blockliste und war damit ein toter Eintrag: eine
 # Blockliste vergleicht Wörter, keine Sätze.
 "chips":(["chips","tortilla","nachos","erdnussflips","flips","cracker","salzstangen","kartoffelringe"],["chips"],[]),
 "eis":(["eis","eiscreme","speiseeis","eistafel","eiskonfekt","waffelhörnchen","eisbecher"],["eis"],["eistee","eiswürfel","eiskaffee"]),
 "pizza":(["pizza","steinofenpizza"],["pizza"],["pizzabrötchen","pizzakäse"]),
 # „Buttergemüse" darf `tiefkühlgemüse` bekommen (nur `butter` blockt es) —
 # ebenfalls eine Notiz, die als Fließtext in der Blockliste stand und dort
 # nie etwas tun konnte.
 "tiefkühlgemüse":(["tiefkühlgemüse","rahmspinat","spinat","erbsen","gemüsemix","kaidergemüse"],["gemüse"],[]),
 "pommes":(["pommes","pommes frites","wedges","kroketten","rösti"],[],[]),
 "tofu":(["tofu","vegane","vegan","veggie","fleischersatz","falafel","gemüsebällchen"],[],[]),
 "eintopf":(["eintopf","suppe","brühe","bouillon"],["eintopf","suppe"],[]),
 "konserven":(["mais","kidneybohnen","kichererbsen","linsen","bohnen","tomatenmark","passierte tomaten","gehackte tomaten","sauerkraut","rotkohl","oliven","pfefferoni","brechbohnen","datteln"],[],[]),
 "soßen":(["ketchup","mayonnaise","mayo","senf","grillsauce","sriracha","sojasauce","dressing","pesto","tomatenketchup","tzatziki","zaziki"],["sauce","soße","sosse","ketchup"],[]),
 "gewürze":(["pfeffer","paprikapulver","curry","gewürz","gewürze","gewürzmischung","kräuter","koriander","ingwer"],["gewürz"],["gewürzgurken"]),
 "backwaren":(["croissant","kuchen","torte","berliner","muffins","brezel","laugengebäck","hefezopf","stollen","backmischung","weckli","flammkuchenböden","törtchen"],["kuchen","backmischung","törtchen"],[]),
 # Der Sammeltopf ist eingedampft: zahnpasta, duschgel, shampoo, deo,
 # waschmittel, spülmittel, toilettenpapier und küchenrolle haben seit den
 # Artikelzeichen-Tranchen eigene Begriffe. Wer sie hier stehen ließ, gab
 # jeder Zahncreme zwei Tags, von denen eines nichts erklärt.
 "windeln/hygiene":(["windeln","vanish","lenor","zewa","wasserenthärter","calgon"],["papier","spülmittel"],["stofftaschentücher"]),
 "spirituosen":(["vodka","wodka","whisky","whiskey","gin","rum","likör","likoer","korn","tequila","aperol","batida","asti","spirituose","jack daniels","jim beam","bittergetränke","doppelkorn","edelbrand","wermut","grappa"],["likör","limes"],[]),
 "pudding":(["pudding","dessert","götterspeise","grießpudding","mousse","milchreis"],["pudding"],[]),
 "nüsse":(["nüsse","erdnüsse","cashewkerne","cashew","erdnuss","mandeln","pistazien","pistazienkerne","walnüsse","studentenfutter","trockenfrüchte"],["kerne","nüsse"],[]),
 "margarine":(["margarine","rama","cremefine","pflanzencreme"],["margarine"],[]),
 "fertiggericht":(["fertiggericht","fertiggerichte","tortelloni","maultaschen","bowl","mikrowellengericht","instant","gyoza","onigiri","wrap","wraps"],["gericht"],["tortilla"]),
 "knäckebrot":(["knäckebrot","knusperbrot","zwieback","wasa","reiswaffeln"],[],[]),
 "schoten/hülsen":(["kaiserschoten","zuckerschoten","edamame","bohnen grün"],["schoten"],[]),
 "protein/fitness":(["proteinriegel","high protein","proteindrink","proteinpulver","whey","trinkmahlzeiten","trinkmahlzeit"],[],[]),

 # --- Tranche 11: die letzten Lücken (2026-08-07) --------------------------
 "lippenpflege":(["lippenpflege","lippenbalsam","labello","lippenstift pflege"],[],[]),
 "rindfleisch":(["rindfleisch","kalbfleisch","wildfleisch","gulaschfleisch","grillfleisch"],[],[]),
 "naturtofu":(["naturtofu","seidentofu","räuchertofu"],[],[]),
 "speisemöhren":(["speisemöhren","bundmöhren","karottensaft"],[],[]),
 "fruchtsaft":(["fruchtsaft","mehrfruchtsaft","direktsaft","nektar"],[],[]),
 "cola":(["cola","cola light","cola zero","spezi","koffeingetränk","energydrink"],[],[]),

 # --- Tranche 10: der Rest (2026-08-07) ------------------------------------
 "haargel":(["haargel","haarspray","haaröl","haarwachs","haarschaum","bartöl"],[],[]),
 "gesichtsmaske":(["gesichtsmaske","peeling","scrub","gesichtswasser","serum"],[],[]),
 "nagelfeile":(["nagelfeile","nagelknipser","nagelschere","nagelzange"],[],[]),
 "parfüm":(["parfüm","eau de toilette","duftwasser","bodyspray"],[],[]),
 "mückenschutz":(["mückenschutz","insektenschutz","mückenspray","zeckenschutz"],[],[]),
 "muskelcreme":(["muskelcreme","kühlgel","wärmesalbe","sportsalbe","nasensalbe"],[],[]),
 "kohletabletten":(["kohletabletten","kohlekompretten","elektrolytpulver"],[],[]),
 "kondome":(["kondom","kondome","gleitgel"],[],[]),
 "kontaktlinsen":(["kontaktlinsen","linsenmittel","kontaktlinsenflüssigkeit"],[],[]),
 "klobürste":(["klobürste","toilettenbürste","wc-bürste","wc-stein"],[],[]),
 "möbelpolitur":(["möbelpolitur","holzpflege","lederpflege","politur"],[],[]),
 "aufbackbrötchen":(["aufbackbrötchen","aufbackbaguette","kräuterbaguette","knoblauchbaguette"],[],[]),
 "schokobrötchen":(["schokobrötchen","milchbrötchen","rosinenbrötchen"],[],[]),
 "knoblauchpulver":(["knoblauchpulver","zwiebelpulver","knoblauchgranulat"],[],[]),
 "mandelaroma":(["mandelaroma","rumaroma","backaroma","zitronenaroma"],[],[]),
 "vanillesoße":(["vanillesoße","vanillesauce","vanillepudding"],[],[]),
 "krabbenchips":(["krabbenchips","reischips","gemüsechips"],[],[]),
 "geschenk":(["geschenk","geschenkkarte","gutschein","tischbombe deko"],[],[]),
 "kostüme":(["kostüm","kostüme","kinderschminke","faschingsartikel"],[],[]),
 "reflektoren":(["reflektor","reflektoren","warnweste","fahrradlicht"],[],[]),
 "pestizide":(["pestizide","schneckenkorn","unkrautvernichter","insektizid"],[],[]),

 # --- Tranche 9: Baumarkt, Garten, Tierbedarf (2026-08-07) -----------------
 #
 # Der Rest von Bring!s Katalog. Hier steht der NONFOOD-Riegel besonders dicht
 # davor (Rasenmäher, Heckenschere und Schrauben sind in `NONFOOD_TERMS`) —
 # ein Begriff macht sie **auf der Liste** auffindbar, holt sie aber nicht in
 # den Angebotsvergleich zurück.
 "toilettenpapier":(["toilettenpapier","klopapier","küchenrolle","küchentücher","taschentücher","feuchttücher"],[],[]),
 "waschmittel":(["waschmittel","weichspüler","fleckenentferner","colorwaschmittel","vollwaschmittel"],[],[]),
 "spülmittel":(["spülmittel","handspülmittel","geschirrspülmittel"],[],[]),
 "besteck":(["besteck","gabel","messer set","löffel set","einweggeschirr"],[],[]),
 "pfanne":(["pfanne","bratpfanne","topf","kochtopf","schüssel"],[],[]),
 "backform":(["backform","kuchenform","ausstechformen","muffinform","springform"],[],[]),
 "küchenhelfer":(["schneebesen","schöpfkelle","pfannenwender","saftpresse","backpinsel","backpalette"],[],[]),
 "schere":(["schere","küchenschere","pinzette","nagelschere"],[],[]),
 "handschuhe":(["handschuhe","putzhandschuhe","gartenhandschuhe","einweghandschuhe"],[],[]),
 "luftballon":(["luftballon","luftballons","ballon","partydeko","lametta","girlande"],[],[]),
 "taschenlampe":(["taschenlampe","stirnlampe","handleuchte"],[],[]),
 "büroklammern":(["büroklammern","locher","tacker","tintenpatronen","klebeband"],[],[]),
 "hundefutter":(["hundefutter","hundesnack","hundeleckerli","nassfutter hund"],[],[]),
 "katzenfutter":(["katzenfutter","katzensnack","katzenstreu","nassfutter katze"],[],[]),
 "vogelfutter":(["vogelfutter","fischfutter","meisenknödel","nagerfutter"],[],[]),
 "blumenerde":(["blumenerde","pflanzerde","substrat","setzholz","setzlinge","sämereien","saatgut"],[],[]),
 "blumentopf":(["blumentopf","übertöpfe","übertopf","pflanzkübel","gießkanne"],[],[]),
 "zimmerpflanze":(["zimmerpflanze","zimmerpflanzen","grünpflanze","topfpflanze","schnittblumen"],[],[]),
 "gartenwerkzeug":(["schaufel","hacke","harke","rechen","gartenschere","heckenschere","rasenmäher"],[],[]),
 "schrauben":(["schrauben","nägel","dübel","muttern","unterlegscheiben"],[],[]),
 "pinsel":(["pinsel","malerpinsel","farbrolle","malerrolle"],[],[]),
 "streusalz":(["streusalz","auftausalz","schneeketten","splitt"],[],[]),
 "holzkohle":(["holzkohle","grillkohle","briketts","propangas","grillanzünder holz"],[],[]),
 "grillzubehör":(["grillzange","grillrost","grillspieße","alugrillschale"],[],[]),
 "sonnenschirm":(["sonnenschirm","sonnensegel","schirmständer"],[],[]),
 # `schal` wird über die Plural-Regel aus „Schale" gebildet (Tokens über vier
 # Zeichen verlieren ihr End-e), und Obst wird in Schalen verkauft: 28 Zeilen
 # des Korpus trugen `socken`, darunter Heidelbeeren, Nektarinen und
 # Obstsalat. Gefunden im Audit vom 08.08.
 "socken":(["socken","wollsocken","strümpfe","mütze","schal","handschuhe wolle"],[],["schale","schalen"]),

 # --- Tranche 8: Haushalt und Pflege (2026-08-07) --------------------------
 #
 # **Der Riegel, an dem ich Non-Food aufgehalten hatte, hält von selbst.**
 # `windeln/hygiene` ist längst ein Non-Food-Begriff — Zahnpasta, Küchenrolle
 # und Waschmittel sind darüber auffindbar —, und `NONFOOD_CAT`/`NONFOOD_TERMS`
 # sortieren echte Nicht-Lebensmittel weiter vorher aus (Staubsauger bleibt
 # nonfood). Ein Begriff auf der Liste heißt also nicht automatisch ein Posten
 # im Angebotsvergleich. Nachgemessen, nicht geglaubt: siehe den Diff-Lauf im
 # Commit.
 "müllbeutel":(["müllbeutel","müllsäcke","müllsack","gefrierbeutel","abfallbeutel"],[],[]),
 "alufolie":(["alufolie","frischhaltefolie","backpapier","butterbrotpapier"],[],[]),
 "geschirrtabs":(["geschirrtabs","spülmaschinentabs","klarspüler","geschirrsalz"],[],[]),
 "putzlappen":(["putzlappen","schwamm","topfschwamm","mikrofasertuch","staubwedel"],[],[]),
 "allzweckreiniger":(["allzweckreiniger","glasreiniger","badreiniger","abflussreiniger","wc-reiniger","toilettenreiniger","entkalker","essigessenz"],[],[]),
 "servietten":(["serviette","servietten","papierservietten"],[],[]),
 "kerzen":(["kerze","kerzen","teelichter","christbaumkerzen"],[],[]),
 "streichhölzer":(["streichhölzer","feuerzeug","grillanzünder"],[],[]),
 "batterien":(["batterie","batterien","knopfzelle","akkus"],[],[]),
 "glühbirne":(["glühbirne","glühlampe","leuchtmittel","led-lampe"],[],[]),
 "strohhalme":(["strohhalm","strohhalme","trinkhalme","spieße","zahnstocher"],[],[]),
 "geschenkpapier":(["geschenkpapier","geschenkband","geschenkschleife","tischbombe"],[],[]),
 "stifte":(["kugelschreiber","bleistift","filzstift","marker","textmarker","radiergummi","spitzer"],[],[]),
 "notizblock":(["notizblock","klebezettel","briefumschläge","schreibblock"],[],[]),
 "seife":(["seife","handseife","flüssigseife","kernseife"],[],[]),
 "duschgel":(["duschgel","badezusatz","badesalz","schaumbad"],[],[]),
 "shampoo":(["shampoo","haarspülung","conditioner","haarkur"],[],[]),
 "deo":(["deo","deodorant","rolldeo","deoroller","deostick"],[],[]),
 "zahnpasta":(["zahnpasta","zahncreme","zahnbürste","zahnseide","mundspülung"],[],[]),
 "handcreme":(["handcreme","bodylotion","gesichtscreme","körperlotion","peeling"],[],[]),
 "sonnencreme":(["sonnencreme","sonnenmilch","sonnenspray","aftersun"],[],[]),
 "rasierer":(["rasierer","rasierklingen","rasierschaum","rasiergel","rasierwasser"],[],[]),
 "wattepads":(["wattepads","wattestäbchen","kosmetiktücher","abschminktücher"],[],[]),
 "pflaster":(["pflaster","blasenpflaster","verband","kompressen","mullbinde"],[],[]),
 "schmerzmittel":(["schmerzmittel","kopfschmerztabletten","hustenbonbons","nasenspray","erkältungsmittel"],[],[]),
 "vitamine":(["vitamine","vitamintabletten","magnesium","nahrungsergänzung"],[],[]),
 "tampons":(["tampon","tampons","binden","slipeinlagen","monatshygiene"],[],[]),
 "makeup":(["makeup","lippenstift","nagellack","wimperntusche","mascara","puder"],[],[]),

 # --- Tranche 7: die letzten Lebensmittel (2026-08-07) ---------------------
 "ketchup":(["ketchup","tomatenketchup","curryketchup"],[],[]),
 "mayonnaise":(["mayonnaise","mayo","remoulade","aioli"],[],["salatmayonnaise"]),
 "senf":(["senf","dijonsenf","mittelscharfer senf","honigsenf"],[],["senfkörner","feigensenf"]),
 "sojasauce":(["sojasauce","sojasoße","teriyaki","kikkoman"],[],[]),
 "pesto":(["pesto","pesto verde","pesto rosso"],[],[]),
 "hummus":(["hummus","humus","kichererbsencreme"],[],[]),
 "grillsauce":(["bbq sauce","barbecuesauce","grillsauce","steaksauce"],[],[]),
 "ingwer":(["ingwer","ingwerknolle","galgant"],[],["ingwertee","ingwershot"]),
 "koriander":(["koriander","korianderkraut","kerbel","liebstöckel"],[],[]),
 "pfeffer":(["pfeffer","schwarzer pfeffer","cayennepfeffer"],[],["pfefferminze","pfefferminztee","paprikapulver","peperoni","pfefferoni"]),
 "paprikapulver":(["paprikapulver","paprikagewürz","rosenpaprika"],[],[]),
 "chili":(["chilipulver","chiliflocken","sambal","harissa"],[],["chili paprika"]),
 "babynahrung":(["babynahrung","babybrei","säuglingsnahrung","folgemilch","beikost"],[],[]),
 "maultaschen":(["maultaschen","ravioli","tortellini","tortelloni"],[],[]),
 "wraps":(["wrap","wraps","tortillafladen","fajita"],[],[]),
 "hamburger":(["hamburger","burgerpatty","frikadellen","buletten"],[],["hamburgerbrötchen"]),
 "hartkäse":(["hartkäse","käseecken","käsestück","manchego","pecorino"],[],[]),
 "kräuterfrischkäse":(["kräuterfrischkäse","frischkäse","doppelrahmstufe"],[],[]),
 "panettone":(["panettone","christstollen","stollen"],[],[]),
 "getreideriegel":(["getreideriegel","haferriegel","fruchtriegel","energieriegel"],[],[]),
 "trüffel":(["trüffel","trüffelöl","trüffelbutter"],[],["trüffelpralinen"]),
 "erdnussbutter":(["erdnussbutter","erdnussmus"],[],[]),
 "kokoswasser":(["kokoswasser","kokosnusswasser"],[],[]),
 "pfefferminztee":(["pfefferminztee","kräutertee","früchtetee","fencheltee","kamillentee"],[],[]),
 "weißwein":(["weißwein","weisswein","riesling","chardonnay","grauburgunder"],[],[]),

 # --- Tranche 6: der Rest aus Obst & Gemüse (2026-08-07) -------------------
 "blutorangen":(["blutorange","blutorangen","moro"],[],[]),
 "chicorée":(["chicorée","chicoree","radicchio","endivien"],[],[]),
 "drachenfrucht":(["drachenfrucht","pitaya","pitahaya"],[],[]),
 "kiwi":(["kiwi","kiwis","goldkiwi","kiwibeeren"],[],[]),
 "guave":(["guave","guaven","maracuja","passionsfrucht"],[],[]),
 "haselnüsse":(["haselnuss","haselnüsse","haselnusskerne"],[],["haselnusscreme"]),
 "kastanien":(["kastanie","kastanien","maronen","marroni","esskastanien"],[],[]),
 "kokosnuss":(["kokosnuss","kokosnüsse","kokos"],[],["kokosmilch","kokosflocken","kokoswasser","kokosjoghurt","kokosöl"]),
 "kresse":(["kresse","gartenkresse","brunnenkresse"],[],[]),
 "maiskolben":(["maiskolben","zuckermais","mais am stück"],[],[]),
 "mangold":(["mangold","stielmangold"],[],[]),
 "preiselbeeren":(["preiselbeere","preiselbeeren","cranberries","cranberry"],[],["preiselbeersauce"]),
 "quinoa":(["quinoa","amaranth","hirse","buchweizen"],[],[]),
 "römersalat":(["römersalat","romasalat","romana","kopfsalat","eisbergsalat","feldsalat","rucola"],[],[]),
 "schwarzwurzel":(["schwarzwurzel","schwarzwurzeln"],[],[]),
 "snacktomaten":(["snacktomaten","cocktailtomaten","datteltomaten"],[],[]),
 "weizengras":(["weizengras","spirulina","chlorella","gerstengras"],[],[]),
 "zitronengras":(["zitronengras","lemongras"],[],[]),
 "portulak":(["portulak","postelein"],[],[]),
 "artischocken":(["artischocke","artischocken","artischockenherzen"],[],[]),
 "spargel":(["spargel","spargeln","grüner spargel","bleichspargel"],[],[]),
 "lauch":(["lauch","porree","stangenlauch"],[],["lauchzwiebel","lauchzwiebeln"]),
 "kohlrabi":(["kohlrabi"],[],[]),

 # --- Tranche 5: Zutaten, Gewürze, Tiefkühl (2026-08-07) -------------------
 "ahornsirup":(["ahornsirup","agavendicksaft","zuckerrübensirup"],[],[]),
 "backpulver":(["backpulver","natron","weinstein backpulver"],[],[]),
 "hefe":(["hefe","frischhefe","trockenhefe","hefeflocken"],[],["hefeteig","hefezopf"]),
 "speisestärke":(["speisestärke","stärke","maisstärke","soßenbinder","sossenbinder"],[],[]),
 "semmelbrösel":(["semmelbrösel","paniermehl","semmelmehl"],[],[]),
 "vanille":(["vanille","vanilleschote","vanilleextrakt","bourbon vanille","vanillearoma"],[],["vanillezucker","vanillesoße","vanillepudding"]),
 "zimt":(["zimt","zimtstange","zimtpulver"],[],["zimtschnecke","zimtschnecken","zimtsterne"]),
 "muskatnuss":(["muskatnuss","muskat","nelken","lorbeer","lorbeerblätter","piment"],[],[]),
 "kurkuma":(["kurkuma","ingwerpulver","currypulver","curry paste","currypaste"],[],[]),
 "safran":(["safran","anis","sternanis","kardamom"],[],[]),
 "oregano":(["oregano","rosmarin","thymian","majoran","salbei","estragon","kräutermischung"],[],[]),
 "pfefferkörner":(["pfefferkörner","pfeffermühle","bunter pfeffer"],[],[]),
 "kapern":(["kapern","kapernäpfel"],[],[]),
 "pinienkerne":(["pinienkerne","kürbiskerne","sonnenblumenkerne","hanfsamen"],[],[]),
 "kokosflocken":(["kokosflocken","kokosraspeln","kokoschips"],[],[]),
 "mandelmus":(["mandelmus","tahini","sesammus","nussmus"],[],[]),
 "marzipan":(["marzipan","marzipanrohmasse","persipan"],[],[]),
 "schokodrops":(["schokodrops","schokotropfen","backschokolade","streusel","zuckerstreusel"],[],[]),
 "lebensmittelfarbe":(["lebensmittelfarbe","speisefarbe","zuckerguss"],[],[]),
 "dosentomaten":(["dosentomaten","gehackte tomaten","pizzatomaten","dosenobst"],[],[]),
 "bratensauce":(["bratensauce","bratensoße","rahmsoße","rahmsauce","preiselbeersauce"],[],[]),
 "fischsauce":(["fischsauce","austernsauce","tamarindenpaste","sojasauce"],[],[]),
 "marinade":(["marinade","grillmarinade","fleischgewürz","gewürzmischung"],[],[]),
 "tütensuppe":(["tütensuppe","instantsuppe","suppenpulver"],[],[]),
 "kartoffelpüree":(["kartoffelpüree","kartoffelstock","püreepulver"],[],[]),
 "knödel":(["knödel","klöße","kloßteig","semmelknödel"],[],["kartoffelknödel"]),
 "kohlrouladen":(["kohlrouladen","krautwickel"],[],[]),
 "hühnerfrikassee":(["hühnerfrikassee","frikassee","geschnetzeltes"],[],[]),

 # --- Tranche 4: Getränke, Süßwaren, Getreide (2026-08-07) -----------------
 "cornflakes":(["cornflakes","corn flakes","frühstücksflocken","knusperflakes"],[],[]),
 "couscous":(["couscous","bulgur"],[],[]),
 "grieß":(["grieß","griess","hartweizengrieß","polenta"],[],["grießbrei"]),
 "glasnudeln":(["glasnudeln","reisnudeln","mie nudeln","udon"],[],[]),
 "spätzle":(["spätzle","spaetzle","knöpfli","schupfnudeln"],[],[]),
 "lasagneblätter":(["lasagneblätter","lasagnenblätter","lasagneplatten"],[],[]),
 "chiasamen":(["chiasamen","chia","leinsamen","flohsamen"],[],[]),
 "reispapier":(["reispapier"],[],[]),
 "tempeh":(["tempeh","seitan"],[],[]),
 "bonbons":(["bonbon","bonbons","lutschbonbons","karamellen"],[],["sahnebonbon"]),
 "kaugummi":(["kaugummi","kaugummis"],[],[]),
 "lollis":(["lolli","lollis","lutscher"],[],[]),
 "plätzchen":(["plätzchen","lebkuchen","spekulatius","printen"],[],[]),
 "popcorn":(["popcorn","pop corn","puffmais"],[],[]),
 "nougatcreme":(["nougatcreme","nussnougatcreme","schokoaufstrich"],[],[]),
 "dörrobst":(["dörrobst","trockenobst","backpflaumen","rosinen","datteln getrocknet"],[],[]),
 "gelee":(["gelee","fruchtgelee","gelierzucker"],[],[]),
 "kuvertüre":(["kuvertüre","kovertüre","schokoladenglasur"],[],[]),
 "glühwein":(["glühwein","punsch","feuerzangenbowle"],[],[]),
 "sirup":(["sirup","fruchtsirup","holunderblütensirup","grenadine"],[],["zuckerrübensirup","ahornsirup"]),
 "smoothie":(["smoothie","smoothies"],[],[]),
 "tonicwater":(["tonic water","tonicwater","tonic","bitter lemon"],[],[]),
 "kaffeepads":(["kaffeepads","senseo pads","kaffeekapseln","padmaschine kapseln"],[],[]),
 "schnaps":(["schnaps","obstler","korn","wodka","gin","rum","whisky","likör"],[],[]),
 "sportgetränk":(["sportgetränk","isodrink","isotonisch","elektrolytgetränk"],[],[]),

 # --- Tranche 3: Brot, Milchprodukte, Fleisch & Fisch (2026-08-07) ---------
 #
 # Konkrete Gegenstände — die zeichnen sich am sichersten. Aus Bring!s
 # Kategorien mit 14, 25 und 24 unbekannten Artikeln die dreiundzwanzig, die
 # weder OCR-Rauschen noch Marke sind und sich als Bild trennen lassen.
 "bagel":(["bagel","bagels"],[],[]),
 "burgerbrötchen":(["burgerbrötchen","burgerbuns","hamburgerbrötchen","brioche buns"],[],[]),
 "croissant":(["croissant","croissants","buttercroissant","buttercroissants"],[],[]),
 "pizzateig":(["pizzateig","blätterteig","hefeteig","kuchenteig","mürbeteig"],[],[]),
 "roggenbrot":(["roggenbrot","vollkornbrot","körnerbrot","schwarzbrot","pumpernickel"],[],[]),
 "zimtschnecken":(["zimtschnecke","zimtschnecken","franzbrötchen","zimtrolle"],[],[]),
 "pflanzendrink":(["hafermilch","mandelmilch","sojamilch","reismilch","kokosdrink","erbsendrink"],[],[]),
 "hüttenkäse":(["hüttenkäse","huettenkaese","körniger frischkäse","cottage cheese"],[],[]),
 "magerquark":(["magerquark","speisequark"],[],[]),
 "raclettekäse":(["raclettekäse","raclette"],[],[]),
 "reibekäse":(["reibekäse","reibkäse","geriebener käse","pizzakäse","gratinkäse"],[],[]),
 "ricotta":(["ricotta","mascarpone"],[],["mascarpone joghurt"]),
 "grillkäse":(["grillkäse","ofenkäse","backcamembert"],[],[]),
 "sojajoghurt":(["sojajoghurt","pflanzenjoghurt","kokosjoghurt"],[],[]),
 "kaffeerahm":(["kaffeerahm","kaffeesahne","kondensmilch"],[],[]),
 "bacon":(["bacon","frühstücksspeck","speck","pancetta"],[],["twister"]),
 "fleischwurst":(["fleischwurst","lyoner","bierschinken","mortadella","jagdwurst"],[],[]),
 "kassler":(["kassler","kasseler","kasslernacken"],[],[]),
 "schinken":(["schinken","kochschinken","rohschinken","serranoschinken","parmaschinken","lachsschinken"],[],["schinkenwurst","bierschinken"]),
 "muscheln":(["muschel","muscheln","miesmuscheln","jakobsmuscheln"],[],["nudeln muscheln"]),
 "sardellen":(["sardelle","sardellen","anchovis","sardinen"],[],[]),
 "schnitzel":(["schnitzel","wiener schnitzel","putenschnitzel","schweineschnitzel"],[],["vegan schnitzel","veganes schnitzel"]),
 "steak":(["steak","steaks","rumpsteak","hüftsteak","entrecote","filetsteak"],[],["steakhouse","hacksteaks"]),

 # --- Tranche 2: Obst, Gemüse, Kräuter (2026-08-07) ------------------------
 #
 # Aus Bring!s Kategorie „Obst & Gemüse", die von 153 Artikeln 68 trug, die
 # unser Wörterbuch nicht kannte. Hier die vierundzwanzig, die sich als
 # Zeichnung **unterscheiden** lassen — bei sechs weiteren Kräutern wäre
 # jedes Bild dasselbe Büschel, und fünf gleiche Büschel sind der Fehler,
 # gegen den das ganze Vorhaben läuft.
 #
 # `suffix` bleibt leer, `block` nur wo gemessen nötig.
 "birnen":(["birne","birnen","williams christ"],[],["birnenkompott"]),
 # Gemessen: „Couronne Feigen-Walnuss" ist ein Brot mit Feigen darin und
 # muss „brot" bleiben.
 "feigen":(["feige","feigen"],[],["feigen walnuss","feigen-walnuss","feigensenf"]),
 "granatapfel":(["granatapfel","granatäpfel","granatapfelkerne"],[],[]),
 "kaki":(["kaki","kakis","sharonfrucht","persimone"],[],[]),
 "litschi":(["litschi","litschis","lychee"],[],[]),
 "papaya":(["papaya","papayas"],[],[]),
 "rhabarber":(["rhabarber"],[],[]),
 "stachelbeeren":(["stachelbeere","stachelbeeren"],[],[]),
 "quitten":(["quitte","quitten"],[],[]),
 "fenchel":(["fenchel","fenchelknolle"],[],["fencheltee"]),
 "kohl":(["kohl","weißkohl","weisskohl","rotkohl","rotkraut","spitzkohl","wirsing","wirz","chinakohl"],[],["kohlrabi","grünkohl","rosenkohl","blumenkohl"]),
 "kürbis":(["kürbis","kuerbis","hokkaido","butternut","muskatkürbis"],[],["kürbiskerne","kürbissuppe"]),
 "lauchzwiebeln":(["lauchzwiebel","lauchzwiebeln","frühlingszwiebel","frühlingszwiebeln","schnittzwiebeln"],[],[]),
 "pastinaken":(["pastinake","pastinaken","petersilienwurzel"],[],[]),
 # „chili" gehört bewusst **nicht** dazu: „Plus Pack 156 g, Chili Paprika"
 # ist ein Käse, und mit dem Wort im Begriff trug er plötzlich zwei Tags.
 # Chili bekommt später einen eigenen Begriff mit eigenen Sperren.
 "peperoni":(["peperoni","pfefferoni","pepperoni"],[],[]),
 "rettich":(["rettich","radi","daikon"],[],[]),
 "rosenkohl":(["rosenkohl"],[],[]),
 "sellerie":(["sellerie","staudensellerie","stangensellerie","knollensellerie"],[],["selleriesalz"]),
 "grünkohl":(["grünkohl","gruenkohl","kale"],[],[]),
 "basilikum":(["basilikum","basil"],[],[]),
 "minze":(["minze","pfefferminze","pfefferminz"],[],["minztee","pfefferminztee"]),
 "schnittlauch":(["schnittlauch"],[],[]),
 "dill":(["dill"],[],[]),
 "petersilie":(["petersilie"],[],["petersilienwurzel"]),

 # --- Tranche 1 zum Artikelzeichen-Vorhaben (2026-08-07) -------------------
 #
 # **Alle Wörter hier standen bisher auf einer Blockliste und gehörten keinem
 # Begriff.** Sie trafen also nichts: Wer „Kartoffelsalat" tippte, bekam weder
 # Treffer noch Kategorie noch Zeichen. Ihnen einen eigenen Begriff zu geben
 # ist **additiv** — es kann keinem bestehenden Begriff Treffer wegnehmen,
 # weil die Sperre genau das schon verhindert hat. Deshalb ist das die erste
 # und sicherste Tranche; siehe `lechariot-app/docs/ARTIKELZEICHEN.md`.
 #
 # Die Sperren bleiben stehen: „Kartoffelsalat" ist weiter keine Kartoffel.
 #
 # `suffix` bleibt überall leer. Suffixe grasen Produkttitel ab und greifen
 # bei so spezifischen Wörtern zu weit — dieselbe Zurückhaltung wie bei
 # `margarine` oder `knäckebrot`.
 # Die Sperre ist gemessen, nicht vorsorglich: Ohne sie holte sich
 # „NATURGUT Bio Süßkartoffel Chips" den Begriff, und wer Süßkartoffeln
 # aufschreibt, bekäme eine Tüte Chips vorgeschlagen.
 "süßkartoffeln":(["süßkartoffel","süßkartoffeln","suesskartoffeln","süsskartoffeln"],[],
                  ["süßkartoffel chips","süsskartoffel chips","suesskartoffel chips","süßkartoffelchips"]),
 "essiggurken":(["essiggurken","essiggurke","gewürzgurken","gewürzgurke","cornichons","silberzwiebeln"],[],[]),
 "kartoffelsalat":(["kartoffelsalat"],[],[]),
 "nudelsalat":(["nudelsalat"],[],[]),
 "krautsalat":(["krautsalat","coleslaw","weinsauerkraut","sauerkraut"],[],[]),
 "fleischsalat":(["fleischsalat","wurstsalat"],[],[]),
 "röstzwiebeln":(["röstzwiebeln","roestzwiebeln"],[],[]),
 "tomatensauce":(["tomatensauce","tomatensoße","tomatensosse","tomatensuppe","passata","pizzasauce","pizzasoße"],[],[]),
 "traubensaft":(["traubensaft"],[],[]),
 "zitronensaft":(["zitronensaft","limettensaft"],[],[]),
 "tomatensaft":(["tomatensaft"],[],[]),
 "vanillezucker":(["vanillezucker","vanillinzucker"],[],[]),
 "traubenzucker":(["traubenzucker","dextrose"],[],[]),
 "brühe":(["gemüsebrühe","gemuesebruehe","hühnerbrühe","rinderbrühe","brühwürfel","bouillon"],[],[]),
 "salatdressing":(["salatdressing","salatsoße","salatsauce","salatcreme","salatmayonnaise","dressing"],[],[]),
 "eiswürfel":(["eiswürfel","eiswuerfel"],[],[]),
 "kartoffelchips":(["kartoffelchips","paprikachips","kartoffelsnacks"],[],[]),
 "kartoffelknödel":(["kartoffelknödel","kartoffelkloß","kartoffelklöße","semmelknödel","kartoffelpuffer","kartoffelecken"],[],[]),
 "müsliriegel":(["müsliriegel","muesliriegel","haferriegel"],[],[]),
 "salzbrezeln":(["salzbrezeln","salzstangen","laugenbrezel"],[],[]),
 "maiswaffeln":(["maiswaffeln"],[],[]),
 "milcheis":(["milcheis","wassereis","speiseeis"],[],[]),
 "kuchen":(["käsekuchen","apfelkuchen","zwiebelkuchen","sahnetorte","quarktasche","kuchen","torte"],[],[]),
}

# Marke → Kategorie (Fallback, wenn Wörterbuch nichts trifft). "NONFOOD" = aussortieren.
MARKEN = {
 # Bier
 "bitburger":"bier","beck's":"bier","becks":"bier","radeberger":"bier","corona":"bier","peroni":"bier",
 "krombacher":"bier","sternburg":"bier","schöfferhofer":"bier","warsteiner":"bier","paulaner":"bier",
 "erdinger":"bier","franziskaner":"bier","eibauer":"bier","ur-krostitzer":"bier","wernesgrüner":"bier",
 "freiberger":"bier","heineken":"bier","desperados":"bier","astra":"bier","lausitzer":"bier",
 # Getränke
 "red bull":"limonade","monster":"limonade","capri-sun":"limonade","adelholzener":"wasser","volvic":"wasser",
 "gerolsteiner":"wasser","vio ":"wasser","fritz-kola":"limonade","valensina":"saft","pfanner":"saft",
 "granini":"saft","hohes c":"saft","marathon":"limonade","yfood":"limonade",
 # Kaffee
 "nescafé":"kaffee","nescaf":"kaffee","jacobs":"kaffee","dallmayr":"kaffee","melitta":"kaffee",
 "l'or":"kaffee","lavazza":"kaffee","tchibo":"kaffee","magico":"kaffee",
 # Süßes & Snacks
 "milka":"schokolade","ferrero":"schokolade","katjes":"schokolade","haribo":"schokolade","lindt":"schokolade",
 "ritter sport":"schokolade","kitkat":"schokolade","nesquik":"schokolade","smarties":"schokolade","lion":"schokolade",
 "merci":"schokolade","toffifee":"schokolade","wrigley":"schokolade","bahlsen":"kekse","leibniz":"kekse",
 "brandt":"knäckebrot","coppenrath":"kekse","lambertz":"kekse","oreo":"kekse","lorenz":"chips",
 "funny-frisch":"chips","pringles":"chips","chio":"chips","pombär":"chips",
 # Molkerei
 "ehrmann":"joghurt","müller":"joghurt","danone":"joghurt","fruchtzwerge":"joghurt","landliebe":"joghurt",
 "weihenstephan":"milch","bauer":"joghurt","meggle":"butter","hochland":"käse","st. mang":"käse",
 "patros":"käse","grünländer":"käse","loose":"käse","cheestrings":"käse","lindenhof":"käse","ergüllü":"frischkäse","miree":"frischkäse","kærgården":"butter","kaergarden":"butter","kerrygold":"butter","milprima":"joghurt","kids world":"joghurt","fruchtigurt":"joghurt","kuchenmeister":"kekse","borggreve":"kekse","oma hartmanns":"kekse","st. michel":"kekse","dickmann":"schokolade","storck":"schokolade","mentos":"schokolade","chupa chups":"schokolade","nimm2":"schokolade","halloren":"schokolade","milchmäuse":"schokolade","milka":"schokolade","knoppers":"schokolade","milch schnitte":"schokolade","suchard":"kakao","fuze tea":"limonade","active o2":"limonade","orangina":"limonade","vitamalz":"limonade","capri sun":"limonade","voelkel":"saft","lübzer":"bier","spaten":"bier","benediktiner":"bier","carlsberg":"bier","anheuser":"bier","bud ":"bier","kloster scheyern":"bier","gerstacker":"wein","frizzade":"wein","secconade":"wein","cavino":"wein","cecchi":"wein","lenz moser":"wein","doppio passo":"wein","calvet":"wein","rothschild":"wein","grand sud":"wein","vin de france":"wein","sandeman":"spirituosen","osborne":"spirituosen","nordbrand":"spirituosen","teekanne":"tee","oryza":"reis","leimer":"brot","miracel whip":"soßen","apostels":"soßen","mc cain":"pommes","mccain":"pommes","namdong":"fertiggericht","dovgan":"fertiggericht","satori":"fertiggericht","tönnies":"schwein","axel schulz":"schwein","wilhelm brandenburg":"wurst","golßener":"soßen","nordsee":"fisch","alfrio":"fisch","wurzener":"chips","pom-bär":"chips","bravo":"nüsse","corny":"müsli","little moons":"eis","snickers":"schokolade","dr. oetker":"backwaren","uncle sam":"NONFOOD","purina":"NONFOOD","vitakraft":"NONFOOD","spareribs":"schwein","fuet":"wurst","café latino":"kaffee","cafe latino":"kaffee","kinder joy":"schokolade","nicnac":"nüsse","tillman":"hähnchen","toasty":"hähnchen","rowenta":"NONFOOD","tassimo":"NONFOOD","tcl ":"NONFOOD","sodastream":"NONFOOD","ayran":"milch","prosciutto":"wurst","hot dog":"wurst","hot dogs":"wurst","geflügelbrust":"hähnchen","kapern":"konserven","sonnenmais":"konserven","gemüsebrühe":"eintopf","vollkornbaguette":"brot","sorbet":"eis","cashewbruch":"nüsse","linsenflips":"chips","knusper-ecken":"chips","pick up":"kekse","gifflar":"backwaren","knack & back":"backwaren","frischteig":"backwaren","aperitivo":"wein","tanqueray":"spirituosen","sonnenweizen":"reis","birnen":"obst","superfoodpulver":"müsli","eisglasur":"backwaren","köttbullar":"fleisch","colgate":"NONFOOD","head & shoulders":"NONFOOD","switch on":"NONFOOD","creatable":"NONFOOD","ecovacs":"NONFOOD","esmara":"NONFOOD","nicer dicer":"NONFOOD","livarno":"NONFOOD","ernesto":"NONFOOD","sensiplast":"NONFOOD","cien":"NONFOOD","pyrex":"NONFOOD","kitchenaid":"NONFOOD","curver":"NONFOOD","denver":"NONFOOD","kesper":"NONFOOD","russell hobbs":"NONFOOD","stabilo":"NONFOOD","vileda":"NONFOOD","geschenkkarte":"NONFOOD","eshop card":"NONFOOD","zalando":"NONFOOD","pepsi":"limonade","schwip schwap":"limonade","sinalco":"limonade","brause":"limonade","havana club":"spirituosen","underberg":"spirituosen","cachaca":"spirituosen","magenbitter":"spirituosen","weinfreunde":"wein","lugana":"wein","gaffels":"limonade","somersby":"bier","krombacher":"bier","warsteiner":"bier","holsten":"bier","köstritzer":"bier","hasseröder":"bier","weihenstephaner":"bier","wernesgrüner":"bier","weltenburger":"bier","saint albray":"käse","bärenmarke":"milch","hafercreme":"milch","sojacreme":"milch","streichfett":"margarine","sanella":"margarine","stremellachs":"fisch","rollmops":"fisch","pulpo":"fisch","deutsche see":"fisch","kabanos":"wurst","sucuk":"wurst","pancetta":"wurst","tyrolini":"wurst","köttbullar":"fleisch","tafelspitz":"rind","spanferkel":"schwein","hendl":"hähnchen","vosskko":"fleisch","almtaler":"hähnchen","smoothie":"saft","porridge":"müsli","cini minis":"müsli","cappuccino":"kaffee","quarktasche":"backwaren","apfeldreieck":"backwaren","bäckerkrönung":"backwaren","aioli":"soßen","true fruits":"saft","buko":"frischkäse","kiri":"frischkäse","magnum":"eis","ben&jerry":"eis","ben jerry":"eis","mikado":"kekse","prinzenrolle":"kekse","de beukelaer":"kekse","raffaello":"schokolade","maxi king":"schokolade","goldbären":"schokolade","pico-balla":"schokolade","lipton":"limonade","starbucks":"kaffee","karlsberg":"bier","löwenbräu":"bier","loewenbraeu":"bier","mixery":"bier","landskron":"bier","pülleken":"bier","büble":"bier","wilthener":"spirituosen","bacardi":"spirituosen","nordhäuser":"spirituosen","pircher":"spirituosen","martini":"spirituosen","fernet":"spirituosen","captain morgan":"spirituosen","lillet":"spirituosen","kinder bueno":"schokolade","kinder riegel":"schokolade","kinder schoko":"schokolade","kinder cards":"schokolade","kinder delice":"schokolade","kinder milchschnitte":"schokolade","mr. tom":"nüsse","novantaceppi":"wein","amédée":"wein","nudossi":"marmelade","gutfried":"wurst","dreistern":"wurst","steinhaus":"fleisch","cevapcici":"fleisch","bifteki":"fleisch","souvlaki":"fleisch","tante fanny":"backwaren","chovi":"soßen","delphi":"konserven","nong shim":"fertiggericht","garden gourmet":"tofu","popp":"soßen","schlichting":"soßen","hipp":"obst","gillette":"NONFOOD","bevola":"NONFOOD","biff":"NONFOOD","finish":"NONFOOD","kitekat":"NONFOOD","medion":"NONFOOD","tefal":"NONFOOD","philips":"NONFOOD","berndes":"NONFOOD","newcential":"NONFOOD","countryside":"NONFOOD","collectino":"NONFOOD","dick & durstig":"NONFOOD","miraball":"NONFOOD","rauch":"saft","happy day":"saft","meica":"bratwurst","becel":"margarine","brunch":"margarine","yogurette":"schokolade","mars":"schokolade","berggold":"schokolade","kathi":"backwaren","keunecke":"fleisch","mühlenhof":"fleisch","windau":"wurst","züger":"frischkäse","zespri":"obst","gösser":"bier","blanchet":"wein","grillo":"wein","tilly":"kekse","the bitery":"kekse","milram":"käse","actimel":"joghurt","vöslauer":"wasser","tulip":"fleisch",
 "zott":"joghurt","bresso":"frischkäse","géramont":"käse","leerdammer":"käse","milkana":"käse",
 "alpro":"milch","oatly":"milch","exquisa":"frischkäse","almette":"frischkäse","gazi":"käse","rama":"margarine","cremefine":"margarine",
 # Fleisch/Wurst/Fisch
 "reinert":"wurst","rügenwalder":"wurst","herta":"wurst","wiesenhof":"hähnchen","bifi":"wurst",
 "butcher":"rind","k-purland":"fleisch","nadler":"fisch","iglo":"tiefkühlgemüse","frosta":"fertiggericht",
 # Eis
 "mövenpick":"eis","schöller":"eis","nuii":"eis","langnese":"eis","fruity ice":"eis",
 # Soßen/Fertig
 "knorr":"soßen","kühne":"soßen","hellmann":"soßen","homann":"soßen","maggi":"soßen","develey":"soßen",
 "orto mio":"soßen","penny ready":"fertiggericht","bürger":"fertiggericht","san fabio":"pizza",
 "greenland":"tiefkühlgemüse","vitalis":"müsli","kellogg":"müsli","ben's original":"fertiggericht",
 # Spirituosen/Sekt
 "gorbatschow":"spirituosen","cinzano":"spirituosen","baileys":"spirituosen","jägermeister":"spirituosen",
 "mangaroca":"spirituosen","rotkäppchen":"wein","freixenet":"wein",
 # Drogerie
 "nivea":"windeln/hygiene","l'oréal":"windeln/hygiene","garnier":"windeln/hygiene","schwarzkopf":"windeln/hygiene",
 "palmolive":"windeln/hygiene","always":"windeln/hygiene","carefree":"windeln/hygiene","sagrotan":"windeln/hygiene",
 "softlan":"windeln/hygiene","persil":"windeln/hygiene","ariel":"windeln/hygiene","pampers":"windeln/hygiene",
 # Non-Food-Marken
 "crivit":"NONFOOD","silvercrest":"NONFOOD","grundig":"NONFOOD","hammersmith":"NONFOOD","livington":"NONFOOD",
 "kingshill":"NONFOOD","spice&soul":"NONFOOD","wenger":"NONFOOD","tronic":"NONFOOD","brita":"NONFOOD",
 "sodastream":"NONFOOD","trendhaus":"NONFOOD","parkside":"NONFOOD",
}
V["fleisch"] = ([],[],[])
V["obst"] = (["fruchtmix","sommerfrucht","obst","pak choi"],[],[])
V["kakao"] = (["kakao","kakaohaltiges","trinkschokolade"],["kakao"],[])
V["ente"] = (["ente","knusperente","entenbrust"],[],[])

# Erweiterungsrunde 2: Sorten & Begriffe in bestehende Einträge mergen
_ADD = {
 "käse":(["tilsiter","camembert","käsestangen","schmelzkäse"],[]),
 "schwein":(["spare ribs","schälrippchen","jägerschnitzel","cordon"],["rücken","rippchen"]),
 "fleisch":([],["frikadellen"]),
 "fisch":(["surimi","calamares","heringsspezialitäten"],["garnelen"]),
 "backwaren":(["laugenbrezel","kirschtasche","spritzring","donut","madeleines","blätterteig","quarkbällchen","börekstick","eisgebäck"],["croissant","brezel","ciabatta"]),
 "kaffee":(["caffe","barista","kaffeegetränk"],[]),
 "schokolade":(["hanuta","amicelli","lakritz","kaubonbons","lollipops","konfekties","tiramisu"],[]),
 "bier":(["klostergold","lager"],[]),
 "wein":(["bordeaux","chianti","primitivo","zweigelt","cremant","imiglykos","rosato","weinhaltiges"],[]),
 "spirituosen":(["ouzo","metaxa","campari","sherry","veterano","cocktails","bittergetränk"],[]),
 "limonade":(["kombucha","malztrunk","erfrischungsgetränk"],[]),
 "obst":(["kiwi","ananas","mango","sungold"],[]),
 "brokkoli":(["radieschen","porree","chinakohl","pak-choi","zuckermais","rote bete","ingwerstücke"],[]),
 "fertiggericht":(["frühlingsrollen","frühlingsrolle","gua bao","jjigae"],["teigtaschen"]),
 "eis":(["mochi","icesticks","raketeneis","stracciatella","eisfrüchte"],[]),
 "butter":(["kräuterbutter"],[]),
 "müsli":(["haferpops","cerealienmix"],[]),
 "soßen":(["ajvar","zaziki","tsatsiki","dips"],[]),
 "brot":(["croutons"],[]),
 "chips":(["krupuk","cheese balls"],[]),
 "pudding":(["puddingpulver"],[]),
}
for _t,(_ex,_sf) in _ADD.items():
    V[_t] = (V[_t][0]+_ex, V[_t][1]+_sf, V[_t][2])  # nur über Markenliste erreichbar (K-Purland etc.)

# Erweiterungsrunde 2026-08-01, Op 3: Alltagswörter. Jedes Wort steht so im
# 11-Regionen-Korpus und deckt eine Produktfamilie, die jede Woche wiederkommt
# — keine Marken (das war die Warnung der ersten Runde), keine Wörter ohne
# Beleg. Format wie `_ADD`: (exacts, suffixe).
_ADD3 = {
 # Fisch: „Makrele" fehlte schlicht; Prawns/Octopus stehen englisch im Prospekt.
 "fisch":(["makrele","makrelen","prawns","octopus","oktopus"],[]),
 # Käse: die Sorten, die als eigener Name auftreten und ohne das Wort „Käse"
 # auskommen. „Harzer" ist der Harzer Roller (Harzer Minis 115 g).
 "käse":(["burrata","mascarpone","pecorino","appenzeller","harzer"],[]),
 "mozzarella":(["mozzarelline"],[]),
 # Rahm: die Süddeutsch/Alpen-Schreibweise von Sahne. Das Wort `rahm` allein
 # stand hier zwischendurch und war gemessen falsch — „Rahm-Spinat",
 # „Rahm Soße", „Allgäuer Rahm-Torte" holten sich damit `sahne`. Deshalb nur
 # die zusammengesetzten Formen und die Phrase „schlag rahm" (aus
 # „Bio-Schlag-Rahm", das normalisiert auseinanderfällt).
 "sahne":(["schlagrahm","schlag rahm","sauerrahm"],[]),
 # Kefir gehört zu den gesäuerten Milchprodukten, nicht zur Trinkmilch.
 "joghurt":(["kefir"],[]),
 # Wurstsorten, die im Prospekt ohne das Wort „Wurst" stehen. „Fleischkäse"
 # und „Leberkäs" sind bewusst hier und nicht bei `käse` — dort stehen sie
 # seit dem Angebots-Audit vom 22.07. auf der Blockliste.
 "wurst":(["bockwurst","bockwürste","frankfurter","weißwurst","weißwürste",
           "leberpastete","pastete","krakauer","pfefferbeißer","bierbeißer",
           "salametti","speck","salsiccia","krainer","käsekrainer",
           "fleischkäse","leberkäs"],[]),
 "schwein":(["krustenbraten","bauchrippe","wammerl","ribs"],[]),
 # `rind` kannte bisher nur Komposita („Rinderfilet"), nicht das Wort selbst:
 # „Rouladen vom Rind" fiel durch. „Jungbullen" ist die Theken-Schreibweise.
 "rind":(["rind","jungbullen"],[]),
 # `brokkoli` ist der Sammeltopf für Kohl- und Stangengemüse (Porree, Chinakohl,
 # Rote Bete stehen schon drin) — Lauch und Sellerie gehören dorthin.
 "brokkoli":(["lauch","sellerie","staudensellerie"],[]),
 "möhren":(["möhrchen"],[]),
 "obst":(["grapefruit","grapefruits","passionsfrucht"],[]),
 "konserven":(["hülsenfrüchte","artischocke","artischocken","antipasti","buschbohnen"],[]),
 "soßen":(["hummus","meerrettich"],[]),
 # Apfel- und Pflaumenmus sind kein frisches Obst (`äpfel` blockt „apfelmus"
 # seit der ersten Runde) — sie gehören zum Fruchtaufstrich.
 "marmelade":(["apfelmus","pflaumenmus"],[]),
 "pudding":(["grütze","rote grütze"],[]),
 "eintopf":(["eintöpfe","fond"],[]),
 # „Apfelringe" steht auf der Blockliste von `äpfel` (kein frisches Obst) und
 # ist genau das, was `nüsse` unter „trockenfrüchte" schon führt.
 "nüsse":(["walnusskerne","apfelringe"],[]),
 "chips":(["popcorn"],[]),
 "kekse":(["biskuit","biskuits"],[]),
 "schokolade":(["fruchtgummi","fruchtkaramellen","karamellen"],[]),
 "backwaren":(["baklava","mohnhappen","schweinsöhrchen","laugenstange"],[]),
 "brot":(["buns","brioche"],[]),
 # Biersorten ohne das Wort „Bier". „hell"/„Helles" ist die häufigste
 # Sortenangabe im Korpus (Chiemseer Hell, Kosmonaut Hell, Münchner Hell).
 "bier":(["kölsch","märzen","hefeweizen","bock","pilsner","hell"],[]),
 "wein":(["sangria","lambrusco","burgunder","trollinger","portugieser",
          "weißherbst","frizzante","spritz","sprizz"],[]),
 "spirituosen":(["aquavit","weizenkorn"],[]),
 "limonade":(["bionade","tonic","ginger ale","mate","softdrink","spezi"],[]),
 # „Stieleis" endet auf „eis", und `eis` steht in SUFFIX_STOP („Preis") —
 # das Suffix kann es nie fangen, das Wort selbst schon.
 "eis":(["stieleis"],[]),
 "pommes":(["fritte","fritten"],[]),
 # Das Suffix `öl` ist wirkungslos (unter vier Zeichen, siehe `term_hits`),
 # deshalb stehen die Öl-Komposita als Wort da.
 "öl":(["keimöl","würzöl"],[]),
 "tee":(["chai"],[]),
 "kaffee":(["entkoffeiniert"],[]),
 "fertiggericht":(["dumpling","dumplings"],[]),
 "gewürze":(["petersilie"],[]),
}
for _t,(_ex,_sf) in _ADD3.items():
    V[_t] = (V[_t][0]+_ex, V[_t][1]+_sf, V[_t][2])

# Der Preis der neuen Wörter, gemessen und nicht geraten: Diese sechs Sperren
# stellen die Fehltreffer ab, die die Wörter oben im Korpus erzeugt haben.
# Jede einzelne stammt aus dem Vorher/Nachher-Lauf über die 3.474 Produkte.
_BLOCK3 = {
 # „Grapefruit" ist ein Obst — außer im Biermischgetränk. Beide Sperren sind
 # Wörter aus dem Getränkeregal, keine Obstwörter: Schöfferhofer Grapefruit
 # verlor sonst sein `bier`, der Lübzer Naturradler Grapefruit bekam `obst`.
 "obst":["schöfferhofer","radler","naturradler"],
 # „Antipasti Creme" ist ein Aufstrich (Lidls Kategorie *herzhafte Aufstriche*
 # führt ihn zu `soßen`), keine eingelegte Konserve.
 "konserven":["antipasti creme"],
 # Mascarpone-Joghurt ist Joghurt, nicht Käse.
 "käse":["mascarpone joghurt"],
 # „hell" ist die häufigste Biersorten-Angabe im Korpus (Chiemseer, Kosmonaut,
 # Kiliansbräu, Münchner Hell) — und steht daneben auf hellen Trauben und auf
 # Brötchen. Die Trauben schützt `bier` schon länger (block „trauben"), das
 # Brötchen ist der eine Fehltreffer, den die Messung neu fand. Als Phrase,
 # damit auch „Vollkornbrötchen hell" darunter fällt.
 "bier":["brötchen hell"],
 # „Speck-Käse-Twister" ist Backwerk; `käse` sperrt „twister" aus demselben
 # Grund schon seit dem Angebots-Audit.
 "wurst":["twister"],
}
for _t,_bl in _BLOCK3.items():
    V[_t] = (V[_t][0], V[_t][1], V[_t][2]+_bl)

# Erweiterungsrunde 2026-08-01, Op 4: Komposita, in denen ein Wort steckt, das
# das Wörterbuch längst kennt — „Steinofenbaguette", „Käsewiener",
# „Kartoffeltaschen", „Lachsfiletseite", „Schinkenplatte". Das ist keine Zeile
# je Produkt, sondern eine Sache am Kompositum-Mechanismus je Begriff.
#
# Der Suffix-Weg (bekanntes Wort am ENDE) gibt es seit der ersten Runde; neu
# ist PRAEFIX (bekanntes Wort am ANFANG), denn das Deutsche schreibt das
# Grundwort mal hinten und mal vorne. Beide laufen unter derselben Regel:
# mindestens vier Zeichen, SUFFIX_STOP gilt, und die Blockliste des Begriffs
# sticht — genau deshalb steht hier PRO BEGRIFF etwas und nicht pauschal.
# Der Preis ist bekannt und dokumentiert: „Reis"/„Preis", „Wein"/„Schwein".
_ADD4_SUFFIX = {
 # Sieben Baguettes im Korpus, keines mit dem Wort „Brot": Laugen-, Mehrkorn-,
 # Weizen-, Dinkel-, Steinofenbaguette.
 # Linzergebäck, Vitalgebäck. Bewusst bei `backwaren` und nicht bei `kekse` —
 # „Laugengebäck" wäre dort falsch einsortiert.
 "backwaren":["gebäck"],
 # Apfelrotkohl (2x). „rotkohl" steht schon im exact von `konserven`.
 "konserven":["rotkohl"],
 # Herzwaffeln, Karamellwaffeln. „Reiswaffeln" sind Knäckebrot und stehen
 # deshalb neu auf der Blockliste von `kekse`.
 "kekse":["waffeln"],
 "fisch":["hering","forelle"],
 "brokkoli":["brokkoli"],   # Stangenbrokkoli
 "butter":["butter"],       # Bio-Alpenbutter; Butterkäse/-milch/-keks blocken schon
 "wurst":["wiener"],        # Käsewiener
 "pudding":["dessert"],     # Puddingdessert
 "wein":["burgunder"],      # Spätburgunder, Weißburgunder
 "marmelade":["brotaufstrich"],  # Bio-Abendbrotaufstrich
}
for _t,_sf in _ADD4_SUFFIX.items():
    V[_t] = (V[_t][0], V[_t][1]+_sf, V[_t][2])

# Begriff → Komposita-PRÄFIXE. Gleiche Regeln wie die Suffixe.
PRAEFIX = {
 # Lachsfiletseite, Lachsfiletportionen, Lachsfleisch, Lachsartikel,
 # Garnelenspieße. „Lachsschinken" ist Schwein und steht auf der Blockliste.
 "fisch":["lachs","garnelen"],
 # Kartoffeltaschen, Kartoffelpüree. Kartoffelsalat/-chips/-puffer/-ecken
 # stehen seit jeher auf der Blockliste und bleiben draußen.
 "kartoffeln":["kartoffel"],
 "hähnchen":["hähnchen","chicken"],
 "pute":["pute"],
 # Wurstsalat, Wurstspezialität, Schinkenplatte, Schinkengulasch, Wienerle.
 "wurst":["wurst","schinken","wiener"],
 # „schweine", nicht „schwein": „Schweinsöhrchen" ist Backwerk und fängt mit
 # „schwein" an.
 "schwein":["schweine"],
 "rind":["steak"],           # Steakhüfte
 "hackfleisch":["hackfleisch"],  # NICHT „hack" — das fängt HACKER-PSCHORR
 "beeren":["beeren"],        # Beerenmischung
 "frischkäse":["frischkäse"],
 "käse":["schmelzkäse"],
 "mozzarella":["mozzarella"],
 "nudeln":["teigwaren"],
 "bratwurst":["bratwurst"],
 "nüsse":["cashewkerne","walnusskerne"],
 "joghurt":["joghurt"],
 "essig":["essig"],          # Essigessenz; „Essiggurken" blockt schon
 "lamm":["lammkeule"],
 "obst":["obst"],            # Obstsortiment
 "quark":["quark"],          # „Cremig und Quarkig"; Quarkbällchen blockt schon
 "wein":["wein"],            # Weingenuss; Weintrauben/-essig/-sauerkraut blocken
 "butter":["butter"],        # Butterschmalz
 "salat":["salat"],          # Salatrio, Salatmischung; Dressings blocken schon
 "tiefkühlgemüse":["gemüse"],  # Gemüsekonserven
 "pudding":["pudding"],
 # Bewusst NICHT als Präfix, gemessen und verworfen: „fisch" (fängt die
 # Fischer-E-Bikes, siehe Feedback-Auswertung), „hell" (Hella Near Water),
 # „wasser" (Wasserenthärter, Wasserfilterkartuschen), „gewürz" („fertig
 # gewürzt"), „apfel" (Apfeltaschen sind Backwerk), „spezi" (Hemelinger
 # Spezial ist Bier), „fritte" (Heißluft-Fritteuse), „kaffee" (Kaffeebecher),
 # „hack" (HACKER-PSCHORR), „schwein" (Schweinsöhrchen).
}

# Der Preis des Kompositum-Wegs, gemessen im selben Lauf über 3.474 Produkte.
# Neun Sperren; ohne sie erzeugt Op 4 genau diese neun Fehltreffer. Das ist die
# Sperrliste, von der die Analyse spricht — pro Begriff, nie pauschal.
_BLOCK4 = {
 # „Lachsschinken" und „Lachsrolle vom Schweinerücken" sind Schwein. Der
 # Präfix `lachs` holte beiden ein `fisch`.
 "fisch":["lachsschinken","lachsrolle"],
 # Gemüsebrühe ist Brühe. Der Treffer im Titel hätte den Marken-Fallback
 # („gemüsebrühe" → `eintopf`) überstimmt, der es richtig hatte.
 "tiefkühlgemüse":["gemüsebrühe"],
 # Ehrmann Obstgarten ist Joghurt.
 "obst":["obstgarten"],
 # Pom-Bär Kartoffelsnacks sind Chips, McCains Kartoffelprodukt ist Pommes —
 # beide standen über die Marke richtig da.
 "kartoffeln":["kartoffelsnacks","kartoffelprodukt"],
 # Quarktasche ist Backwerk (steht schon in der Markenliste).
 "quark":["quarktasche"],
 # Reis- und Maiswaffeln sind Knäckebrot, keine Kekse.
 "kekse":["reiswaffeln","maiswaffeln"],
 # „Steakhouse Pommes" sind Pommes.
 "rind":["steakhouse"],
 # Die Salatgurke ist eine Gurke.
 "salat":["salatgurke","salatgurken"],
}
for _t,_bl in _BLOCK4.items():
    V[_t] = (V[_t][0], V[_t][1], V[_t][2]+_bl)

# Non-Food-Begriffe im Titel (fängt Non-Food in Food-Kategorien wie „Wochenangebote")
# Vier Teile trafen mitten im Wort und warfen Essen aus dem Katalog. Jede
# Klammer ist am ganzen Korpus gemessen (Audit 2026-08-08):
#   …topf|\btopf\b    Erasco Eintopf (3 Zeilen) fällt raus, Kochtopf,
#                     Schnellkochtopf, Kräutertopf, Pflanztopf bleiben drin.
#   …blumen|\bblumen\b Sonnenblumenöl, Sonnenblumenbrötchen, Heublumen-Käse,
#                     Blumenkohl (4) fallen raus, Schnittblumen bleiben.
#   \baxe             Meica Bratmaxe (1) — Axe Bodyspray bleibt.
#   pflanzen?\b       Solvel Pflanzenmargarine (1) — Salatpflanzen und
#                     Pflanzenkörbe bleiben.
#   …grill           Der bare Riegel „grill\b" nahm den Grill-Käse und das
#                     Grill-Rosenbrötchen mit und ließ die Grillzange durch
#                     (kein \b zwischen „grill" und „zange"). Jetzt stehen
#                     die Geräte einzeln da: 2 Zeilen Essen zurück, 1 Gerät
#                     zusätzlich erwischt.
#
# Ohne Lookaround, und das ist keine Stilfrage: Die JSON ist die Quelle für
# die regex-Kiste von Rust, und die kennt weder (?<!) noch (?!). Ein Muster
# mit Lookaround kompiliert hier, lässt aber jeden Test in matching.rs beim
# Laden des Wörterbuchs sterben.
NONFOOD_TERMS = re.compile(r"lichterkette|lampion|wäschest|wäscheklammer|wäschekorb|kettensäge|akku|werkzeug|kinderbuch|spielzeug|\blego\b|rosen\b|blumenstrauß|blumenerde|blumenvase|schnittblumen|\bblumen\b|pflanzenkörbe|pflanzenkorb|pflanzen?\b|socken|shorts|shirt|cap\b|hose|schuhe|handtuch|bettwäsche|pfannen?\b|kochtopf|kräutertopf|energiespartopf|pflanztopf|\btopf\b|löffel|messer|kontaktgrill|elektrogrill|gasgrill|holzkohlegrill|tischgrill|kugelgrill|standgrill|schwenkgrill|grillrost|grillzange|grillbesteck|grill.und.abtropf|kohle|batterie|lampe|leuchte|katzen|hunde|tiernahrung|nassfutter|trockenfutter|snack für|rasenkanten|solar|deko|kissen|matratze|drucker|kopfhörer|wc-|reiniger|megaperls|oxi action|waschpulver|schreibwaren|mikrofon|duschregal|sonnensegel|wäscheparf|karaoke|trinkzubehör|wäschetrockner|weißer riese|sonnenspray|duftspüler|sonnencreme|feuchttücher|servietten|haushaltstücher|klumpstreu|geschirrtücher|platzset|schlafsack|fusselrolle|bügeleisen|glasschüssel|lautsprecher|geräusche-box|fliegengitter|kajak|husarenknöpfchen|lavendel|bilderbuch|wecker|hairstyler|bastelkoffer|kochgeschirr|grillplatte|boombox|fliegenfalle|mottenabwehr|badvorleger|schrubber|kosmetikspiegel|shorty|plaid|fototafel|komfort-bh|pantoletten|spannbetttuch|küchentücher|sneaker|hoodie|bodyspray|deospray|haarspray|rasierkling|sonnenschutz|dutch oven|gläsersortiment|sonnenschirm|tischdecke|fleece|wellnessbürste|maniküre|pediküre|teppich|taillenslip|haftcreme|wasserballon|doppelwandig|kollagenpulver|pokémon|pokemon|plüsch|spielfigur|sammelkarten|tiptoi|autorennbahn|gesellschaftsspiel|kreuzworträtsel|rätselbuch|pixi|bastel|schüleretui|sticker|puzzle|holzperlen|magnet-bausatz|wasserbahn|kinderbesteck|steckdose|usb|ladegerät|smart-tv|wasserkocher|toaster|standmixer|espressomaschine|kaffeemaschine|kaffeevollautomat|kapselmaschine|waffeleisen|reiskocher|luftkühler|ventilator|wetterstation|vakuumiergerät|hamburger-maker|hamburger maker|inspektionskamera|range extender|mini-led-tv|qled|e-bike|faltrad|mountainbike|fahrradträger|mähroboter|heckenschere|bohrhammer|abbruchhammer|bohrer|winkelschleifer|meißel|werkstatt|rohrzange|bolzenschneider|kabelbinder|elektrohobel|feinbohrschleifer|spannzwingen|zwingen-set|rasendünger|gartenspritze|gartenhocker|sanitär|montageschlüssel|sekundenkleber|buntlack|abdeckplane|duschtürdichtung|badewannenmatte|duschhocker|steppbett|spannbettlaken|tagesdecke|daunendecke|luftbett|matratze|kleiderschrank|drehtürenschrank|büroschrank|bürostuhl|beistelltisch|wohnzimmertisch|tischgruppe|schuhregal|metallregal|kunststoffregal|regalwürfel|polsterbank|schlafsessel|schminktisch|nischenwagen|akustikpaneel|bilderrahmen|sofa |brotkasten|kartoffelstampfer|schneebesen|kleid|tunika|slips|pyjama|leggings|unterhemden|retroboxer|sandalen|bademantel|freizeitanzug|loungewear|trikot-set|tops |ripptops|jersey|boardcase|reisetasche|rucksack|einkaufstrolley|packbänder|kuppelzelt|autodachzelt|zelt |trampolin|nestschaukel|rutsche|sandkasten|whirlpool|sup |sup-|campingstuhl|spieltipi|matschküche|super soaker|großfahrzeug|mini-fahrzeug|rennboot|inkontinenz|rollator|blutdruckmess|pulsoximeter|lesehilfe|spezialbrille|erste-hilfe|massagematte|haltungstrainer|beintrainer|rückenstütz|körperanalyse|waschhilfe|slipeinlagen|mighty patch|orchidee|phalaenopsis|chrysanthem|alpenveilchen|hortensie|glockenblume|dahlie|aster|eustoma|feigenkaktus|bogenhanf|celosia|zauberglöckchen|prärieenzian|rosenstrauß|bunter strauß|alufolie|frischhaltefolie|netflix|wertkarte|löschdecke|trinkflasche|zitronensäure|insektenschutz|corega|\baxe |all-in-1-pods|allzwecktücher|badebombe|beschriftungsgerät|beschäftigungsbuch|gaming|haushaltsartikel|hipster|kaffeebecher|kaltwachsstreifen|klebestift|nachtwäsche|nutri mixer|shaping-short|silikonform|sprühflasche|treteimer|vorlesebuch|wäschesammler|zitruspresse", re.I)

# Tokens, bei denen Suffix-Matching generell verboten ist (falsche Komposita)
SUFFIX_STOP = {"reis","preis","schwein","schweine","kreis","eis","wein",
               "hackfleisch","gehacktes","abwaschbecken"}

def norm(s):
    s = s.lower()
    # Der Apostroph FÄLLT WEG, er wird nicht zum Leerzeichen. Sonst wird aus
    # dem Markenschlüssel „l'oréal" ein „l oreal", das im Prospekt niemals
    # steht — der schreibt „Loreal Men Expert". Dasselbe bei „beck's" und
    # „ben's original". Typografische Varianten (’ ‘ ´) zählen mit, die
    # Prospekte mischen sie („Tesori d‘Oriente").
    s = re.sub(r"[®*™'’‘´`]", "", s)
    s = s.replace("-", " ")
    s = s.translate(str.maketrans("éèêáàâíìóòúù", "eeeaaaiioouu"))
    s = re.sub(r"[^a-zäöüß\- ]", " ", s)
    return re.sub(r"\s+", " ", s).strip()

# Die Schlüssel werden mit derselben Normalisierung verglichen wie alles
# andere — von Hand normalisiert stünde hier „sahne schmand und creme fra che"
# (das `î` fällt aus der Akzent-Tabelle und wird zum Leerzeichen), und der
# nächste Tippfehler fiele niemandem auf.
KAT = {norm(k): v for k, v in KAT_ROH.items()}


def tokens(s):
    base = [t for t in re.split(r"[ \-]", norm(s)) if len(t) > 2]
    extra = [t[:-1] for t in base if len(t) > 4 and t[-1] in "sne"]
    return base + extra


def enthaelt_als_wort(ntext, nadel):
    """Steht `nadel` als ganzes Wort in `ntext`? — Zwilling von
    `src/matching.rs::enthaelt_als_wort`, die Begründung steht dort.

    `ntext` ist normalisiert (nur a-zäöüß und Leerzeichen), „Grenze" heißt
    also: davor und dahinter steht kein Buchstabe.
    """
    if not nadel:
        return False
    return re.search(r"(?<![^\W\d_])" + re.escape(nadel) + r"(?![^\W\d_])",
                     ntext, re.UNICODE) is not None


def term_hits(text):
    """Begriffe des Wörterbuchs, die auf einen Angebotstext passen.

    Eigene Funktion, weil docs/feedback-auswertung.py dieselbe Regel braucht,
    um zu bestimmen, welcher Eintrag einen gemeldeten Fehltreffer verursacht
    hat. Zwei Kopien dieser Regel wären genau die Sorte Abweichung, die man
    erst merkt, wenn ein Vorschlag am falschen Eintrag ansetzt.
    """
    toks = tokens(text)
    ntext = norm(text)
    hits = []
    for term,(exact,suffixes,block) in V.items():
        # Ob ein Eintrag als Phrase (Teilstring in ntext) oder als Wort
        # (Token-Gleichheit) geprüft wird, entscheidet die NORMALISIERTE Form,
        # nicht die rohe. Bis 2026-07-31 stand das Leerzeichen-Kriterium auf
        # dem Rohstring, und `norm` macht aus dem Bindestrich ein Leerzeichen:
        # „thunfisch-salat" galt hier als Wort, konnte als Wort nie treffen
        # (Tokens enthalten keine Leerzeichen) und blockte damit nichts —
        # während Rust dieselbe Zeile längst als Phrase las und blockte.
        # „Thunfisch-Salat" bekam so in Python `salat` und in Rust nicht.
        # Gleiches galt für `kærgården`, `pak-choi`, `bio-eier`, `coca-cola`.
        nblock = [norm(b) for b in block]
        if any(b in ntext for b in nblock if " " in b) or any(b in toks for b in nblock):
            continue
        # Präfix und Suffix laufen unter derselben Regel: mindestens vier
        # Zeichen, echtes Kompositum (das Token ist LÄNGER als der Teil, sonst
        # wäre es ein exact), SUFFIX_STOP gilt, Blockliste sticht.
        hit = any(e in toks or (" " in e and e in ntext) for e in map(norm, exact)) \
           or any(any(t.endswith(sfx) and t not in SUFFIX_STOP and t not in nblock
                      for t in toks)
                  for sfx in map(norm, suffixes) if len(sfx) >= 4) \
           or any(any(t.startswith(pfx) and len(t) > len(pfx)
                      and t not in SUFFIX_STOP and t not in nblock
                      for t in toks)
                  for pfx in map(norm, PRAEFIX.get(term, [])) if len(pfx) >= 4)
        if hit: hits.append(term)
    return hits


# Nachgetragen aus docs/matching-woerterbuch.json: Die Runde vom 06.08.
# („Die letzten elf Lücken" und „Die letzten drei") hat diese Wörter direkt
# in der generierten Datei ergänzt, nie hier. Damit taggte die Nightly nach
# einem Wörterbuch, das der Schiedsrichter nicht kannte — sichtbar erst am
# ignorierten Paritätstest. Die Wörter stehen jetzt in der Quelle.
_ADD_BRING = {
 "tiefkühlgemüse":["kaisergemüse","buttergemüse","gemüse gefroren","tiefkühl brokoli","tk gemüse"],
 "grillsauce":["dip","dips"],
 "kräuterfrischkäse":["kochcreme"],
 "bratensauce":["preiselbeer sauce","pasta sauce","vanille sauce"],
 "pizzateig":["mini pizzen","mini pizza"],
 "sellerie":["stangenselerie"],
}
for _t,_ex in _ADD_BRING.items():
    V[_t] = (V[_t][0]+_ex, V[_t][1], V[_t][2])

NONFOOD_KEY = "nonfood"


def match_keys(title, sub="", cat=""):
    """Tags eines Angebots — die Python-Seite von `src/matching.rs::match_keys`.

    Gibt `(tags, weg)` zurück; `weg` ist "titel", "marke" oder "kategorie" und
    zählt nur für die Statistik. `[NONFOOD_KEY]` heißt erkanntes Non-Food, die
    leere Liste heißt ungetaggt (Review-Liste).

    Die Pipeline steht hier und nicht mehr ausgeschrieben in `main`, weil der
    zeilenweise Paritäts-Test in src/matching.rs genau diese Funktion aufruft:
    Eine zweite Kopie der Pipeline wäre die Sorte Abweichung, die der Test
    finden soll — und würde sie stattdessen verstecken.
    """
    text = f"{title} {sub}" if sub else title
    ntext = norm(text)
    if (NONFOOD_CAT.search(cat or "") and not FOOD_CAT.search(cat or "")) or NONFOOD_TERMS.search(text):
        return [NONFOOD_KEY], "titel"
    hits = term_hits(text)
    if hits:
        return hits, "titel"
    # Marken-Fallback: die erste passende Marke in Wörterbuch-Reihenfolge.
    for marke, term in MARKEN.items():
        nmarke = norm(marke)
        if nmarke and enthaelt_als_wort(ntext, nmarke):
            return [NONFOOD_KEY if term == "NONFOOD" else term], "marke"
    # Letzter Ausweg: die gepflegte Kategorie-Zuordnung. Nur exakte Gleichheit
    # der normalisierten Kategorie, nur wenn Titel und Untertitel nichts
    # hergeben — ein Titel-Treffer wird nie überstimmt.
    kat = KAT.get(norm(cat))
    if kat and kat in V:
        # Die Blockliste des Begriffs gilt auch hier — sonst holt sich
        # „Erdnussbutter" über die Kategorie „Butter" genau den Tag zurück,
        # den die Blockliste ihm nimmt.
        _, _, block = V[kat]
        toks = tokens(text)
        nblock = [norm(b) for b in block]
        if not (any(b in ntext for b in nblock if " " in b) or any(b in toks for b in nblock)):
            return [kat], "kategorie"
    return [], "titel"


# Ab hier: Messlauf gegen die lokale Nightly-DB. In eine Funktion gefasst,
# damit `V`, `MARKEN`, `norm` und `tokens` importierbar sind, ohne dass ein
# Import die Datenbank anfasst — docs/feedback-auswertung.py braucht genau
# diese Definitionen und darf keine zweite Kopie davon führen.
def schreibe_json():
    """Wörterbuch für die Rust-Seite ausgeben.

    Steht **vor** dem Messlauf und nicht mehr an dessen Ende, und das ist
    keine Kosmetik: Am 2026-08-08 lag die letzte Woche der Eval-DB vier Tage
    zurück, `main` fand null Zeilen und starb in der Abdeckungs-Rechnung an
    einer Division durch null — vor der Ausgabe. Wer danach die Datei ansah,
    fand die alte. Das Wörterbuch schreiben und das Wörterbuch messen sind
    zwei Dinge; das erste darf nicht am zweiten hängen.
    """
    json.dump({"begriffe":{t:{"exact":e,"suffix":s,"prefix":PRAEFIX.get(t,[]),"block":b} for t,(e,s,b) in V.items()},"marken":MARKEN,"kategorien":KAT,"nonfood_cat":NONFOOD_CAT.pattern,"nonfood_terms":NONFOOD_TERMS.pattern,"food_cat":FOOD_CAT.pattern},
              open(os.path.join(os.path.dirname(__file__),"matching-woerterbuch.json"),"w"), ensure_ascii=False, indent=1)


def main():
    schreibe_json()
    # `--alle` misst den ganzen Korpus statt der heute gültigen Woche. Nötig,
    # sobald die Eval-DB älter ist als die laufende Woche — ein Sync wäre der
    # andere Weg, aber der schreibt nach Supabase, und die Pflegerunde liest
    # nur (siehe .claude/commands/pflegerunde.md).
    wo = "" if "--alle" in sys.argv else "where o.valid_until >= date('now')"
    con = sqlite3.connect(DB)
    rows = con.execute(f"""select o.title, coalesce(o.subtitle,''), coalesce(o.category,''), m.name
                          from offers o join markets m on m.id=o.market_id {wo}""").fetchall()
    if not rows:
        print(f"Wörterbuch geschrieben. Keine Angebote mit valid_until >= heute in {DB} —\n"
              f"für einen Messlauf gegen den ganzen Korpus: matching-woerterbuch-eval.py --alle")
        return

    stats = Counter(); tagged = defaultdict(list); untagged = []
    for title, sub, cat, market in rows:
        hits, weg = match_keys(title, sub, cat)
        if hits == [NONFOOD_KEY]:
            stats["nonfood"] += 1; continue
        if hits:
            stats["tagged"] += 1
            if weg != "titel": stats["via_" + weg] += 1
            for h in hits: tagged[h].append((market, title))
        else:
            stats["untagged"] += 1
            untagged.append((market, title, sub, cat))

    total = len(rows)
    print(f"Angebote gültig heute: {total}")
    print(f"Non-Food (per Kategorie erkannt): {stats['nonfood']} ({stats['nonfood']/total:.0%})")
    food = total - stats["nonfood"]
    print(f"Food-Angebote: {food}")
    print(f"  regelbasiert getaggt: {stats['tagged']} ({stats['tagged']/food:.0%})")
    print(f"  ungetaggt:            {stats['untagged']} ({stats['untagged']/food:.0%})")
    print(f"  davon über Kategorie: {stats['via_kategorie']}   (über Marke: {stats['via_marke']})")
    print("\n== Treffer pro Begriff (Top 25) ==")
    for term, lst in sorted(tagged.items(), key=lambda x:-len(x[1]))[:25]:
        print(f"  {term:16s} {len(lst):3d}  z.B. {lst[0][1][:60]}")
    print("\n== Ungetaggte Beispiele (50 zufällig) ==")
    import random; random.seed(1)
    for market, title, sub, cat in random.sample(untagged, min(120, len(untagged))):
        print(f"  [{market[:12]:12s}] {title[:55]:55s} | {sub[:25]:25s} | {cat[:25]}")


if __name__ == "__main__":
    main()
