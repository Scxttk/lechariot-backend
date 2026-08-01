#!/usr/bin/env python3
"""Wörterbuch-Entwurf: taggt aktuelle Angebote regelbasiert mit Alltagsbegriffen."""
import sqlite3, re, os, json
from collections import Counter, defaultdict

DB = os.path.expanduser("~/.local/share/lechariot/lechariot.db")

# Kategorien, die klar Non-Food sind (Ketten-Marketing-Kategorien)
NONFOOD_CAT = re.compile(r"mode|style|heim|haus|garten|haustier|tierbedarf|tiernahrung|pflanzen|angeln|elektro|medien|kinderzimmer|wäschepflege|schulstart|alles für die schule|kochen-und-grillen|drogerie|spielzeug|alltagshelfer|technik|spielwaren|baumarkt|multimedia|bekleidung|schuhe|camping|auto|buero|non.?food|onlineshop|e-bikes?|fahrrad|trolley|koffer|unterhemd|\btops\b|\bteller\b|sch[üu]sseln?|staubsauger|wischroboter|k[üu]chenmasch|n[äa]hmasch|batterien|\bfarben\b|kleber|zahngesundheit|glasartikel|aufbewahr|essgeschirr|reinigungsger|k[üu]chenger|k[üu]chengro|k[üu]chenzubeh|grillzubeh|backzubeh|damen|herren|w[äa]sche\b|leuchten|m[öo]bel|werkzeug|haartrockner|rasierer|zahnpflege|fernseh|led ?& ?lcd|kapsel|padmasch|\bk[üu]che\b|blumen|strauß|kochen-und-backen|reinigen|waschmittel", re.I)
FOOD_CAT = re.compile(r"obst|gemüse|fleisch|geflügel|wurst|molkerei|fette|getränke|feinkost|konserven|kaffee|tee|süßwaren|knabber|grundnahrung|fisch|bäckerei|backwaren|tiefkühl", re.I)

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
 "brot":(["brot","broetchen","brötchen","toast","baguette","ciabatta"],["brot","broetchen","brötchen","toast"],["brotaufstrich","aufbackbrötchen?","russisch brot"]),
 "milch":(["milch","frischmilch","vollmilch","buttermilch","mandeldrink","haferdrink","sojadrink"],["milch"],["milchreis","milchschnitte","milchbrötchen","kokosmilch","milcheis","milchschokolade","kondensmilch","sonnenmilch","kokosnussmilch","milka","knoppers","milch schnitte","bergkäse","käsescheiben"]),
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
 "beeren":(["erdbeeren","himbeeren","blaubeeren","heidelbeeren","brombeeren","johannisbeeren","beerenmix"],["beeren"],["erdbeermarmelade","erdbeerjoghurt"]),
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
 "nudeln":(["nudeln","spaghetti","penne","fusilli","tagliatelle","tortellini","cappelletti","gnocchi","pasta","lasagne","ramen","ramyun","teigwaren","eierspätzle","kritharaki"],["nudeln"],["nudelsalat","nudelsuppe"]),
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
 "soßen":(["ketchup","mayonnaise","mayo","senf","grillsauce","bbq sauce","sriracha","sojasauce","dressing","pesto","tomatenketchup","tzatziki","zaziki"],["sauce","soße","sosse","ketchup"],[]),
 "gewürze":(["pfeffer","paprikapulver","curry","gewürz","gewürze","gewürzmischung","kräuter","koriander","ingwer"],["gewürz"],["gewürzgurken"]),
 "backwaren":(["croissant","kuchen","torte","berliner","muffins","brezel","laugengebäck","hefezopf","stollen","backmischung","weckli","flammkuchenböden","törtchen"],["kuchen","backmischung","törtchen"],[]),
 "windeln/hygiene":(["windeln","toilettenpapier","küchenrolle","taschentücher","zahnpasta","duschgel","shampoo","deo","deodorant","waschmittel","spülmittel","vanish","lenor","zewa"],["papier","waschmittel"],["stofftaschentücher"]),
 "spirituosen":(["vodka","wodka","whisky","whiskey","gin","rum","likör","likoer","korn","tequila","aperol","batida","asti","spirituose","jack daniels","jim beam","bittergetränke","doppelkorn","edelbrand","wermut","grappa"],["likör","limes"],[]),
 "pudding":(["pudding","dessert","götterspeise","grießpudding","mousse","milchreis"],["pudding"],[]),
 "nüsse":(["nüsse","erdnüsse","cashewkerne","cashew","erdnuss","mandeln","pistazien","pistazienkerne","walnüsse","studentenfutter","trockenfrüchte"],["kerne","nüsse"],[]),
 "margarine":(["margarine","rama","cremefine","pflanzencreme"],["margarine"],[]),
 "fertiggericht":(["fertiggericht","fertiggerichte","tortelloni","maultaschen","bowl","ravioli","mikrowellengericht","instant","gyoza","onigiri","wrap","wraps"],["gericht"],["tortilla"]),
 "knäckebrot":(["knäckebrot","knusperbrot","zwieback","wasa","reiswaffeln"],[],[]),
 "schoten/hülsen":(["kaiserschoten","zuckerschoten","edamame","bohnen grün"],["schoten"],[]),
 "protein/fitness":(["proteinriegel","high protein","proteindrink","proteinpulver","whey","trinkmahlzeiten","trinkmahlzeit"],[],[]),
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
 "soßen":(["ajvar","zaziki","tsatsiki","dip","dips"],[]),
 "brot":(["croutons"],[]),
 "chips":(["krupuk","cheese balls"],[]),
 "pudding":(["puddingpulver"],[]),
}
for _t,(_ex,_sf) in _ADD.items():
    V[_t] = (V[_t][0]+_ex, V[_t][1]+_sf, V[_t][2])  # nur über Markenliste erreichbar (K-Purland etc.)

# Non-Food-Begriffe im Titel (fängt Non-Food in Food-Kategorien wie „Wochenangebote")
NONFOOD_TERMS = re.compile(r"lichterkette|lampion|wäschest|wäscheklammer|wäschekorb|kettensäge|akku|werkzeug|kinderbuch|spielzeug|\blego\b|rosen\b|blumen|pflanze|socken|shorts|shirt|cap\b|hose|schuhe|handtuch|bettwäsche|pfannen?\b|topf\b|löffel|messer|grill\b|kohle|batterie|lampe|leuchte|katzen|hunde|tiernahrung|nassfutter|trockenfutter|snack für|rasenkanten|solar|deko|kissen|matratze|drucker|kopfhörer|wc-|reiniger|megaperls|oxi action|waschpulver|schreibwaren|mikrofon|duschregal|sonnensegel|wäscheparf|karaoke|trinkzubehör|wäschetrockner|weißer riese|sonnenspray|duftspüler|sonnencreme|feuchttücher|servietten|haushaltstücher|klumpstreu|geschirrtücher|platzset|schlafsack|fusselrolle|bügeleisen|glasschüssel|lautsprecher|geräusche-box|fliegengitter|kajak|husarenknöpfchen|lavendel|bilderbuch|wecker|hairstyler|bastelkoffer|kochgeschirr|grillplatte|boombox|fliegenfalle|mottenabwehr|badvorleger|schrubber|kosmetikspiegel|shorty|plaid|fototafel|komfort-bh|pantoletten|spannbetttuch|küchentücher|sneaker|hoodie|bodyspray|deospray|haarspray|rasierkling|sonnenschutz|dutch oven|gläsersortiment|sonnenschirm|tischdecke|fleece|wellnessbürste|maniküre|pediküre|teppich|taillenslip|haftcreme|wasserballon|doppelwandig|kollagenpulver|pokémon|pokemon|plüsch|spielfigur|sammelkarten|tiptoi|autorennbahn|gesellschaftsspiel|kreuzworträtsel|rätselbuch|pixi|bastel|schüleretui|sticker|puzzle|holzperlen|magnet-bausatz|wasserbahn|kinderbesteck|steckdose|usb|ladegerät|smart-tv|wasserkocher|toaster|standmixer|espressomaschine|kaffeemaschine|kaffeevollautomat|kapselmaschine|waffeleisen|reiskocher|luftkühler|ventilator|wetterstation|vakuumiergerät|hamburger-maker|hamburger maker|inspektionskamera|range extender|mini-led-tv|qled|e-bike|faltrad|mountainbike|fahrradträger|mähroboter|heckenschere|bohrhammer|abbruchhammer|bohrer|winkelschleifer|meißel|werkstatt|rohrzange|bolzenschneider|kabelbinder|elektrohobel|feinbohrschleifer|spannzwingen|zwingen-set|rasendünger|gartenspritze|gartenhocker|sanitär|montageschlüssel|sekundenkleber|buntlack|abdeckplane|duschtürdichtung|badewannenmatte|duschhocker|steppbett|spannbettlaken|tagesdecke|daunendecke|luftbett|matratze|kleiderschrank|drehtürenschrank|büroschrank|bürostuhl|beistelltisch|wohnzimmertisch|tischgruppe|schuhregal|metallregal|kunststoffregal|regalwürfel|polsterbank|schlafsessel|schminktisch|nischenwagen|akustikpaneel|bilderrahmen|sofa |brotkasten|kartoffelstampfer|schneebesen|kleid|tunika|slips|pyjama|leggings|unterhemden|retroboxer|sandalen|bademantel|freizeitanzug|loungewear|trikot-set|tops |ripptops|jersey|boardcase|reisetasche|rucksack|einkaufstrolley|packbänder|kuppelzelt|autodachzelt|zelt |trampolin|nestschaukel|rutsche|sandkasten|whirlpool|sup |sup-|campingstuhl|spieltipi|matschküche|super soaker|großfahrzeug|mini-fahrzeug|rennboot|inkontinenz|rollator|blutdruckmess|pulsoximeter|lesehilfe|spezialbrille|erste-hilfe|massagematte|haltungstrainer|beintrainer|rückenstütz|körperanalyse|waschhilfe|slipeinlagen|mighty patch|orchidee|phalaenopsis|chrysanthem|alpenveilchen|hortensie|glockenblume|dahlie|aster|eustoma|feigenkaktus|bogenhanf|celosia|zauberglöckchen|prärieenzian|rosenstrauß|bunter strauß|alufolie|frischhaltefolie|netflix|wertkarte|löschdecke|trinkflasche|zitronensäure|insektenschutz|corega|axe ", re.I)

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
        hit = any(e in toks or (" " in e and e in ntext) for e in map(norm, exact)) \
           or any(any(t.endswith(sfx) and t not in SUFFIX_STOP and t not in nblock
                      for t in toks)
                  for sfx in map(norm, suffixes) if len(sfx) >= 4)
        if hit: hits.append(term)
    return hits


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
        if nmarke and nmarke in ntext:
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
def main():
    con = sqlite3.connect(DB)
    rows = con.execute("""select o.title, coalesce(o.subtitle,''), coalesce(o.category,''), m.name
                          from offers o join markets m on m.id=o.market_id
                          where o.valid_until >= date('now')""").fetchall()

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

    json.dump({"begriffe":{t:{"exact":e,"suffix":s,"block":b} for t,(e,s,b) in V.items()},"marken":MARKEN,"kategorien":KAT,"nonfood_cat":NONFOOD_CAT.pattern,"nonfood_terms":NONFOOD_TERMS.pattern,"food_cat":FOOD_CAT.pattern},
              open(os.path.join(os.path.dirname(__file__),"matching-woerterbuch.json"),"w"), ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
