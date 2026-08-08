//! Deterministische Produkt-Anreicherung: normalisierte Kategorie + Emoji.
//!
//! Die App zeigt nur die hier definierten Kategorien an (CATEGORIES) — die
//! rohen Scraper-Kategorien der Ketten sind uneinheitlich und bleiben lokal.
//! Emojis kommen aus einer kuratierten Keyword-Tabelle (längstes passendes
//! Keyword gewinnt); ohne Treffer greift das Kategorie-Standard-Emoji.

/// Stabile Kategorienliste — die App hardcodet diese Werte. Änderungen nur
/// bewusst zusammen mit einem App-Update.
pub const CATEGORIES: [&str; 15] = [
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
];

pub const FALLBACK_CATEGORY: &str = "Sonstiges";

/// Die Regale, in denen Essen steht. `Sonstiges` gehört nicht dazu — es ist
/// der Topf für alles, was der Import nicht einsortieren konnte, und teilt
/// sich das Fach mit der Aktionsware.
const FOOD_CATEGORIES: [&str; 10] = [
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
];

/// Standard-Emoji pro Kategorie — greift, wenn kein Keyword passt.
const CATEGORY_EMOJI: [(&str, &str); 15] = [
    ("Obst & Gemüse", "🥬"),
    ("Molkerei & Eier", "🥛"),
    ("Fleisch & Wurst", "🥩"),
    ("Fisch", "🐟"),
    ("Backwaren", "🥖"),
    ("Tiefkühl", "❄️"),
    ("Süßes & Snacks", "🍬"),
    ("Getränke", "🥤"),
    ("Alkohol", "🍺"),
    ("Vorräte & Kochen", "🥫"),
    ("Drogerie", "🧴"),
    ("Haushalt", "🧽"),
    ("Tierbedarf", "🐾"),
    ("Kinder", "🧸"),
    ("Sonstiges", "🛒"),
];

pub fn default_emoji(category: &str) -> &'static str {
    CATEGORY_EMOJI
        .iter()
        .find(|(c, _)| *c == category)
        .map(|(_, e)| *e)
        .unwrap_or("🛒")
}

/// Keyword → (Emoji, Kategorie). Substring-Match auf lowercased
/// Titel+Untertitel; das längste passende Keyword gewinnt, dadurch schlägt
/// z. B. "apfelschorle" (Getränke) das enthaltene "apfel" (Obst).
const RULES: &[(&str, &str, &str)] = &[
    // Obst
    ("apfel", "🍎", "Obst & Gemüse"),
    ("äpfel", "🍎", "Obst & Gemüse"),
    ("banane", "🍌", "Obst & Gemüse"),
    ("erdbeer", "🍓", "Obst & Gemüse"),
    ("himbeer", "🫐", "Obst & Gemüse"),
    ("heidelbeer", "🫐", "Obst & Gemüse"),
    ("blaubeer", "🫐", "Obst & Gemüse"),
    ("traube", "🍇", "Obst & Gemüse"),
    ("zitrone", "🍋", "Obst & Gemüse"),
    ("limette", "🍋", "Obst & Gemüse"),
    ("orange", "🍊", "Obst & Gemüse"),
    ("mandarine", "🍊", "Obst & Gemüse"),
    ("clementine", "🍊", "Obst & Gemüse"),
    ("pfirsich", "🍑", "Obst & Gemüse"),
    ("nektarine", "🍑", "Obst & Gemüse"),
    ("aprikose", "🍑", "Obst & Gemüse"),
    ("kirsche", "🍒", "Obst & Gemüse"),
    ("melone", "🍈", "Obst & Gemüse"),
    ("wassermelone", "🍉", "Obst & Gemüse"),
    ("ananas", "🍍", "Obst & Gemüse"),
    ("mango", "🥭", "Obst & Gemüse"),
    ("kiwi", "🥝", "Obst & Gemüse"),
    ("birne", "🍐", "Obst & Gemüse"),
    ("pflaume", "🟣", "Obst & Gemüse"),
    ("avocado", "🥑", "Obst & Gemüse"),
    // Gemüse
    ("tomate", "🍅", "Obst & Gemüse"),
    ("gurke", "🥒", "Obst & Gemüse"),
    ("zucchini", "🥒", "Obst & Gemüse"),
    ("paprika", "🫑", "Obst & Gemüse"),
    ("möhre", "🥕", "Obst & Gemüse"),
    ("karotte", "🥕", "Obst & Gemüse"),
    ("kartoffel", "🥔", "Obst & Gemüse"),
    ("zwiebel", "🧅", "Obst & Gemüse"),
    ("knoblauch", "🧄", "Obst & Gemüse"),
    ("brokkoli", "🥦", "Obst & Gemüse"),
    ("blumenkohl", "🥦", "Obst & Gemüse"),
    ("salat", "🥬", "Obst & Gemüse"),
    ("spinat", "🥬", "Obst & Gemüse"),
    ("kohlrabi", "🥬", "Obst & Gemüse"),
    ("spargel", "🌱", "Obst & Gemüse"),
    ("pilze", "🍄", "Obst & Gemüse"),
    ("champignon", "🍄", "Obst & Gemüse"),
    ("aubergine", "🍆", "Obst & Gemüse"),
    ("mais", "🌽", "Obst & Gemüse"),
    ("kürbis", "🎃", "Obst & Gemüse"),
    ("radieschen", "🌶️", "Obst & Gemüse"),
    ("ingwer", "🫚", "Obst & Gemüse"),
    // Molkerei & Eier
    ("milch", "🥛", "Molkerei & Eier"),
    ("joghurt", "🥛", "Molkerei & Eier"),
    ("jogurt", "🥛", "Molkerei & Eier"),
    ("quark", "🥛", "Molkerei & Eier"),
    ("sahne", "🥛", "Molkerei & Eier"),
    ("butter", "🧈", "Molkerei & Eier"),
    ("margarine", "🧈", "Molkerei & Eier"),
    ("käse", "🧀", "Molkerei & Eier"),
    ("gouda", "🧀", "Molkerei & Eier"),
    ("mozzarella", "🧀", "Molkerei & Eier"),
    ("feta", "🧀", "Molkerei & Eier"),
    ("eier", "🥚", "Molkerei & Eier"),
    ("skyr", "🥛", "Molkerei & Eier"),
    // Fleisch & Wurst
    ("hähnchen", "🍗", "Fleisch & Wurst"),
    ("hühner", "🍗", "Fleisch & Wurst"),
    ("pute", "🍗", "Fleisch & Wurst"),
    ("schwein", "🥩", "Fleisch & Wurst"),
    ("rind", "🥩", "Fleisch & Wurst"),
    ("steak", "🥩", "Fleisch & Wurst"),
    ("hackfleisch", "🥩", "Fleisch & Wurst"),
    ("gehacktes", "🥩", "Fleisch & Wurst"),
    ("bratwurst", "🌭", "Fleisch & Wurst"),
    ("wurst", "🥓", "Fleisch & Wurst"),
    ("würstchen", "🌭", "Fleisch & Wurst"),
    ("salami", "🥓", "Fleisch & Wurst"),
    ("schinken", "🥓", "Fleisch & Wurst"),
    ("speck", "🥓", "Fleisch & Wurst"),
    ("lamm", "🥩", "Fleisch & Wurst"),
    ("ente", "🦆", "Fleisch & Wurst"),
    // Fisch
    ("lachs", "🐟", "Fisch"),
    ("forelle", "🐟", "Fisch"),
    ("thunfisch", "🐟", "Fisch"),
    ("hering", "🐟", "Fisch"),
    ("matjes", "🐟", "Fisch"),
    ("garnele", "🦐", "Fisch"),
    ("shrimp", "🦐", "Fisch"),
    ("fischstäbchen", "🐟", "Tiefkühl"),
    ("fisch", "🐟", "Fisch"),
    // Backwaren
    ("brot", "🍞", "Backwaren"),
    ("brötchen", "🥐", "Backwaren"),
    ("baguette", "🥖", "Backwaren"),
    ("croissant", "🥐", "Backwaren"),
    ("toast", "🍞", "Backwaren"),
    ("kuchen", "🍰", "Backwaren"),
    ("torte", "🍰", "Backwaren"),
    // Tiefkühl
    ("tiefkühl", "❄️", "Tiefkühl"),
    ("tk-", "❄️", "Tiefkühl"),
    ("pizza", "🍕", "Tiefkühl"),
    ("pommes", "🍟", "Tiefkühl"),
    ("eiscreme", "🍨", "Tiefkühl"),
    ("eis am stiel", "🍦", "Tiefkühl"),
    ("langnese", "🍦", "Tiefkühl"),
    // Süßes & Snacks
    ("schokolade", "🍫", "Süßes & Snacks"),
    ("schoko", "🍫", "Süßes & Snacks"),
    ("praline", "🍫", "Süßes & Snacks"),
    ("bonbon", "🍬", "Süßes & Snacks"),
    ("gummibär", "🍬", "Süßes & Snacks"),
    ("fruchtgummi", "🍬", "Süßes & Snacks"),
    ("keks", "🍪", "Süßes & Snacks"),
    ("cookie", "🍪", "Süßes & Snacks"),
    ("chips", "🥔", "Süßes & Snacks"),
    ("erdnuss", "🥜", "Süßes & Snacks"),
    ("nüsse", "🥜", "Süßes & Snacks"),
    ("popcorn", "🍿", "Süßes & Snacks"),
    ("müsliriegel", "🍫", "Süßes & Snacks"),
    // Getränke
    ("wasser", "💧", "Getränke"),
    ("saft", "🧃", "Getränke"),
    ("schorle", "🧃", "Getränke"),
    ("apfelschorle", "🧃", "Getränke"),
    ("limonade", "🥤", "Getränke"),
    ("cola", "🥤", "Getränke"),
    ("eistee", "🥤", "Getränke"),
    ("energy", "⚡", "Getränke"),
    ("kaffee", "☕", "Getränke"),
    ("espresso", "☕", "Getränke"),
    ("tee", "🍵", "Getränke"),
    ("kakao", "🍫", "Getränke"),
    // Alkohol
    ("bier", "🍺", "Alkohol"),
    ("pils", "🍺", "Alkohol"),
    ("weizen", "🍺", "Alkohol"),
    ("wein", "🍷", "Alkohol"),
    ("sekt", "🥂", "Alkohol"),
    ("prosecco", "🥂", "Alkohol"),
    ("vodka", "🍸", "Alkohol"),
    ("wodka", "🍸", "Alkohol"),
    ("whisky", "🥃", "Alkohol"),
    ("rum", "🥃", "Alkohol"),
    ("gin", "🍸", "Alkohol"),
    ("likör", "🍸", "Alkohol"),
    // Vorräte & Kochen
    ("nudel", "🍝", "Vorräte & Kochen"),
    ("pasta", "🍝", "Vorräte & Kochen"),
    ("spaghetti", "🍝", "Vorräte & Kochen"),
    ("reis", "🍚", "Vorräte & Kochen"),
    ("mehl", "🌾", "Vorräte & Kochen"),
    ("zucker", "🍬", "Vorräte & Kochen"),
    ("salz", "🧂", "Vorräte & Kochen"),
    ("gewürz", "🧂", "Vorräte & Kochen"),
    ("öl", "🫒", "Vorräte & Kochen"),
    ("essig", "🫙", "Vorräte & Kochen"),
    ("ketchup", "🍅", "Vorräte & Kochen"),
    ("senf", "🌭", "Vorräte & Kochen"),
    ("mayonnaise", "🥪", "Vorräte & Kochen"),
    ("honig", "🍯", "Vorräte & Kochen"),
    ("marmelade", "🍓", "Vorräte & Kochen"),
    ("konfitüre", "🍓", "Vorräte & Kochen"),
    ("nutella", "🍫", "Vorräte & Kochen"),
    ("müsli", "🥣", "Vorräte & Kochen"),
    ("cornflakes", "🥣", "Vorräte & Kochen"),
    ("haferflocken", "🥣", "Vorräte & Kochen"),
    ("suppe", "🍲", "Vorräte & Kochen"),
    ("sauce", "🥫", "Vorräte & Kochen"),
    ("soße", "🥫", "Vorräte & Kochen"),
    ("konserve", "🥫", "Vorräte & Kochen"),
    ("dose", "🥫", "Vorräte & Kochen"),
    // Drogerie
    ("shampoo", "🧴", "Drogerie"),
    ("duschgel", "🧴", "Drogerie"),
    ("seife", "🧼", "Drogerie"),
    ("zahnpasta", "🪥", "Drogerie"),
    ("zahnbürste", "🪥", "Drogerie"),
    ("deo", "🧴", "Drogerie"),
    ("creme", "🧴", "Drogerie"),
    ("rasier", "🪒", "Drogerie"),
    ("taschentücher", "🤧", "Drogerie"),
    // Haushalt
    ("waschmittel", "🧴", "Haushalt"),
    ("spülmittel", "🧽", "Haushalt"),
    ("spülmaschine", "🧽", "Haushalt"),
    ("reiniger", "🧽", "Haushalt"),
    ("putzmittel", "🧽", "Haushalt"),
    ("toilettenpapier", "🧻", "Haushalt"),
    ("küchenrolle", "🧻", "Haushalt"),
    ("müllbeutel", "🗑️", "Haushalt"),
    ("batterie", "🔋", "Haushalt"),
    ("kerze", "🕯️", "Haushalt"),
    // Tierbedarf
    ("katzenfutter", "🐱", "Tierbedarf"),
    ("katzenmilch", "🐱", "Tierbedarf"),
    ("katzen", "🐱", "Tierbedarf"),
    ("hundefutter", "🐶", "Tierbedarf"),
    ("hunde", "🐶", "Tierbedarf"),
    ("tierfutter", "🐾", "Tierbedarf"),
    ("katzenstreu", "🐱", "Tierbedarf"),
    // Kinder
    ("windel", "👶", "Kinder"),
    ("babynahrung", "🍼", "Kinder"),
    ("babybrei", "🍼", "Kinder"),
    ("feuchttücher", "👶", "Kinder"),
    ("spielzeug", "🧸", "Kinder"),
    ("spielwaren", "🧸", "Kinder"),
    ("lego", "🧸", "Kinder"),
    ("playmobil", "🧸", "Kinder"),
    ("minecraft", "🧸", "Kinder"),
    ("poké", "🧸", "Kinder"),
    ("puppe", "🧸", "Kinder"),
    ("lupilu", "👶", "Kinder"),
    // Molkerei/Dessert-Zusätze
    ("monte", "🍮", "Molkerei & Eier"),
    ("pudding", "🍮", "Molkerei & Eier"),
    ("dessert", "🍮", "Molkerei & Eier"),
    ("mousse", "🍮", "Molkerei & Eier"),
    ("froop", "🥛", "Molkerei & Eier"),
    ("ehrmann", "🍮", "Molkerei & Eier"),
    ("ghurt", "🥛", "Molkerei & Eier"),
    // Vorräte-Zusätze
    ("fruchtaufstrich", "🍓", "Vorräte & Kochen"),
    ("aufstrich", "🫙", "Vorräte & Kochen"),
    ("tortilla", "🌯", "Vorräte & Kochen"),
    ("wrap", "🌯", "Vorräte & Kochen"),
    ("fertiggericht", "🍲", "Vorräte & Kochen"),
    ("noodle", "🍜", "Vorräte & Kochen"),
    ("knäcke", "🍘", "Vorräte & Kochen"),
    ("wasa", "🍘", "Vorräte & Kochen"),
    ("cracker", "🍘", "Süßes & Snacks"),
    ("granola", "🥣", "Vorräte & Kochen"),
    ("porridge", "🥣", "Vorräte & Kochen"),
    ("würzmisch", "🧂", "Vorräte & Kochen"),
    ("brühe", "🍲", "Vorräte & Kochen"),
    ("pesto", "🌿", "Vorräte & Kochen"),
    ("olive", "🫒", "Vorräte & Kochen"),
    ("antipasti", "🫒", "Vorräte & Kochen"),
    ("couscous", "🍚", "Vorräte & Kochen"),
    ("linsen", "🫘", "Vorräte & Kochen"),
    ("bohnen", "🫘", "Vorräte & Kochen"),
    ("kichererbse", "🫘", "Vorräte & Kochen"),
    ("lasagne", "🍝", "Vorräte & Kochen"),
    // Getränke-Zusätze
    ("drink", "🥤", "Getränke"),
    ("trinkmahlzeit", "🥤", "Getränke"),
    ("smoothie", "🧃", "Getränke"),
    ("volvic", "💧", "Getränke"),
    ("gerolsteiner", "💧", "Getränke"),
    ("nescaf", "☕", "Getränke"),
    ("dallmayr", "☕", "Getränke"),
    ("melitta", "☕", "Getränke"),
    ("tchibo", "☕", "Getränke"),
    ("cappuccino", "☕", "Getränke"),
    ("alpro", "🌱", "Getränke"),
    // Alkohol-Zusätze
    ("whiskey", "🥃", "Alkohol"),
    ("bourbon", "🥃", "Alkohol"),
    ("champagner", "🥂", "Alkohol"),
    ("secco", "🥂", "Alkohol"),
    ("aperitivo", "🍹", "Alkohol"),
    ("aperol", "🍹", "Alkohol"),
    ("weinbrand", "🥃", "Alkohol"),
    ("obstbrand", "🥃", "Alkohol"),
    ("korn", "🥃", "Alkohol"),
    ("radler", "🍺", "Alkohol"),
    ("weißwein", "🍷", "Alkohol"),
    ("rotwein", "🍷", "Alkohol"),
    ("roséwein", "🍷", "Alkohol"),
    ("rosé", "🍷", "Alkohol"),
    ("riesling", "🍷", "Alkohol"),
    ("primitivo", "🍷", "Alkohol"),
    ("grigio", "🍷", "Alkohol"),
    ("burgunder", "🍷", "Alkohol"),
    ("dornfelder", "🍷", "Alkohol"),
    ("merlot", "🍷", "Alkohol"),
    ("sauvignon", "🍷", "Alkohol"),
    ("tempranillo", "🍷", "Alkohol"),
    ("zweigelt", "🍷", "Alkohol"),
    ("chardonnay", "🍷", "Alkohol"),
    ("müller-thurgau", "🍷", "Alkohol"),
    ("cuvée", "🍷", "Alkohol"),
    ("winzer", "🍷", "Alkohol"),
    ("vino", "🍷", "Alkohol"),
    ("cimarosa", "🍷", "Alkohol"),
    ("trocken", "🍷", "Alkohol"),
    ("lieblich", "🍷", "Alkohol"),
    ("feinherb", "🍷", "Alkohol"),
    ("kingshill", "🥃", "Alkohol"),
    ("doppelkorn", "🥃", "Alkohol"),
    // Süßes-Zusätze (Marken)
    ("milka", "🍫", "Süßes & Snacks"),
    ("ferrero", "🍫", "Süßes & Snacks"),
    ("storck", "🍫", "Süßes & Snacks"),
    ("lindt", "🍫", "Süßes & Snacks"),
    ("merci", "🍫", "Süßes & Snacks"),
    ("toffifee", "🍫", "Süßes & Snacks"),
    ("duplo", "🍫", "Süßes & Snacks"),
    ("giotto", "🍫", "Süßes & Snacks"),
    ("yogurette", "🍫", "Süßes & Snacks"),
    ("dickmann", "🍫", "Süßes & Snacks"),
    ("riegel", "🍫", "Süßes & Snacks"),
    ("haribo", "🍬", "Süßes & Snacks"),
    ("kaugummi", "🍬", "Süßes & Snacks"),
    ("wrigley", "🍬", "Süßes & Snacks"),
    ("lakritz", "🍬", "Süßes & Snacks"),
    ("marshmallow", "🍬", "Süßes & Snacks"),
    ("mikado", "🍪", "Süßes & Snacks"),
    ("tuc", "🍪", "Süßes & Snacks"),
    ("waffel", "🧇", "Süßes & Snacks"),
    ("tiramisu", "🍰", "Süßes & Snacks"),
    ("mochi", "🍡", "Süßes & Snacks"),
    ("pom-bär", "🥔", "Süßes & Snacks"),
    ("trockenfrüchte", "🍇", "Süßes & Snacks"),
    ("studentenfutter", "🥜", "Süßes & Snacks"),
    // Tiefkühl-Zusätze
    ("eis", "🍦", "Tiefkühl"),
    ("stieleis", "🍦", "Tiefkühl"),
    ("eistüte", "🍦", "Tiefkühl"),
    ("iglo", "❄️", "Tiefkühl"),
    ("frosta", "❄️", "Tiefkühl"),
    ("tiefgefroren", "❄️", "Tiefkühl"),
    ("nugget", "🍗", "Tiefkühl"),
    // Fleisch-Zusätze
    ("mett", "🥩", "Fleisch & Wurst"),
    ("corned beef", "🥩", "Fleisch & Wurst"),
    ("frikadelle", "🍖", "Fleisch & Wurst"),
    ("burger", "🍔", "Fleisch & Wurst"),
    ("gyros", "🥙", "Fleisch & Wurst"),
    ("döner", "🥙", "Fleisch & Wurst"),
    ("geflügel", "🍗", "Fleisch & Wurst"),
    ("filet", "🥩", "Fleisch & Wurst"),
    // Obst/Gemüse-Zusätze
    ("zwetschge", "🟣", "Obst & Gemüse"),
    ("kohl", "🥬", "Obst & Gemüse"),
    ("beeren", "🫐", "Obst & Gemüse"),
    ("walnuss", "🥜", "Süßes & Snacks"),
    ("nuss", "🥜", "Süßes & Snacks"),
    ("pflanze", "🪴", "Sonstiges"),
    ("blume", "💐", "Sonstiges"),
    ("kräuter", "🌿", "Obst & Gemüse"),
    // Drogerie-Zusätze
    ("sebamed", "🧴", "Drogerie"),
    ("nivea", "🧴", "Drogerie"),
    ("dusche", "🚿", "Drogerie"),
    ("hygiene", "🧴", "Drogerie"),
    ("vitamin", "💊", "Drogerie"),
    ("pflaster", "🩹", "Drogerie"),
    ("tampon", "🩹", "Drogerie"),
    // Haushalt-Zusätze
    ("zewa", "🧻", "Haushalt"),
    ("küchentücher", "🧻", "Haushalt"),
    ("swiffer", "🧽", "Haushalt"),
    ("fleckentferner", "🧴", "Haushalt"),
    ("vanish", "🧴", "Haushalt"),
    ("entkalker", "🧽", "Haushalt"),
    ("weichspüler", "🧴", "Haushalt"),
    ("wc", "🚽", "Haushalt"),
    ("geschirr", "🍽️", "Haushalt"),
    ("pfanne", "🍳", "Haushalt"),
    ("topf", "🍳", "Haushalt"),
    ("bräter", "🍳", "Haushalt"),
    ("messer", "🔪", "Haushalt"),
    ("maschine", "🔌", "Haushalt"),
    ("staubsauger", "🧹", "Haushalt"),
    ("bügel", "🧺", "Haushalt"),
    ("vileda", "🧽", "Haushalt"),
    ("kochtopf", "🍳", "Haushalt"),
    ("knorr", "🍲", "Vorräte & Kochen"),
    ("patros", "🧀", "Molkerei & Eier"),
    ("stitch", "🧸", "Kinder"),
    // NonFood → Sonstiges
    ("parkside", "🛠️", "Sonstiges"),
    ("bohr", "🛠️", "Sonstiges"),
    ("werkzeug", "🛠️", "Sonstiges"),
    ("rasenmäher", "🛠️", "Sonstiges"),
    ("akku", "🔋", "Sonstiges"),
    ("livarno", "🛋️", "Sonstiges"),
    ("silvercrest", "🔌", "Sonstiges"),
    ("tronic", "🔌", "Sonstiges"),
    ("elektro", "🔌", "Sonstiges"),
    ("fernseher", "📺", "Sonstiges"),
    ("kopfhörer", "🎧", "Sonstiges"),
    ("esmara", "👕", "Sonstiges"),
    ("crivit", "🎽", "Sonstiges"),
    ("shirt", "👕", "Sonstiges"),
    ("hoodie", "👕", "Sonstiges"),
    ("pullover", "👕", "Sonstiges"),
    ("jacke", "🧥", "Sonstiges"),
    ("hose", "👖", "Sonstiges"),
    ("shorts", "🩳", "Sonstiges"),
    ("jeans", "👖", "Sonstiges"),
    ("socken", "🧦", "Sonstiges"),
    ("slip", "🩲", "Sonstiges"),
    ("sneaker", "👟", "Sonstiges"),
    ("schuh", "👟", "Sonstiges"),
    ("bettwäsche", "🛏️", "Sonstiges"),
    ("kissen", "🛏️", "Sonstiges"),
    ("decke", "🛏️", "Sonstiges"),
    ("matratze", "🛏️", "Sonstiges"),
    ("teppich", "🛋️", "Sonstiges"),
    ("leuchte", "💡", "Sonstiges"),
    ("lampe", "💡", "Sonstiges"),
    ("lichterkette", "💡", "Sonstiges"),
    ("ventilator", "🌀", "Sonstiges"),
    ("koffer", "🧳", "Sonstiges"),
    ("tasche", "🧳", "Sonstiges"),
    ("rucksack", "🎒", "Sonstiges"),
    ("wenger", "🧳", "Sonstiges"),
    ("ultimate speed", "🚗", "Sonstiges"),
    ("auto", "🚗", "Sonstiges"),
    ("fahrrad", "🚲", "Sonstiges"),
    ("garten", "🪴", "Sonstiges"),
    // `vegan`, `soja` und `soya` sagen für sich genommen nichts über das
    // Regal — sie stehen hier für ihr Emoji, das Regal holt die Zeile über
    // ihren Begriff. `tofu` steht dagegen weiter unten bei den Vorräten:
    // Es ist der einzige der vier, der auch ein Wörterbuch-BEGRIFF ist, und
    // ein Begriff, der auf „Sonstiges" zeigt, ist für `enrich` kein
    // fehlendes Regal, sondern ein gültiges — er verdeckte damit jeden
    // feineren Begriff derselben Zeile.
    ("vegan", "🌱", "Sonstiges"),
    ("soja", "🌱", "Sonstiges"),
    ("soya", "🌱", "Sonstiges"),
    // Runde 4: Long-Tail aus echten Daten
    ("ice tea", "🥤", "Getränke"),
    ("capri-sun", "🧃", "Getränke"),
    ("säfte", "🧃", "Getränke"),
    ("nektar", "🧃", "Getränke"),
    ("brause", "🥤", "Getränke"),
    ("ayran", "🥛", "Getränke"),
    ("schwarztee", "🍵", "Getränke"),
    ("früchtetee", "🍵", "Getränke"),
    ("kräutertee", "🍵", "Getränke"),
    ("lichtenauer", "💧", "Getränke"),
    ("adelholzener", "💧", "Getränke"),
    ("water", "💧", "Getränke"),
    ("limoncello", "🍸", "Alkohol"),
    ("spritz", "🍹", "Alkohol"),
    ("edelbrand", "🥃", "Alkohol"),
    ("amaro", "🍸", "Alkohol"),
    ("smirnoff", "🍸", "Alkohol"),
    ("qualitätswein", "🍷", "Alkohol"),
    ("rebsortenwein", "🍷", "Alkohol"),
    ("bräu", "🍺", "Alkohol"),
    ("weißbier", "🍺", "Alkohol"),
    ("camembert", "🧀", "Molkerei & Eier"),
    ("cheddar", "🧀", "Molkerei & Eier"),
    ("hochland", "🧀", "Molkerei & Eier"),
    ("hirtenkäse", "🧀", "Molkerei & Eier"),
    ("rama", "🧈", "Molkerei & Eier"),
    ("streichzart", "🧈", "Molkerei & Eier"),
    ("gustavo", "🍕", "Tiefkühl"),
    ("hörnchen", "🍦", "Tiefkühl"),
    ("ice", "🍦", "Tiefkühl"),
    ("dip", "🫙", "Vorräte & Kochen"),
    ("remoulade", "🫙", "Vorräte & Kochen"),
    ("homann", "🫙", "Vorräte & Kochen"),
    ("cashew", "🥜", "Süßes & Snacks"),
    ("macadamia", "🥜", "Süßes & Snacks"),
    ("pinienkerne", "🥜", "Vorräte & Kochen"),
    ("mandel", "🥜", "Süßes & Snacks"),
    ("frucht", "🍓", "Obst & Gemüse"),
    ("früchte", "🍓", "Obst & Gemüse"),
    ("sülze", "🥩", "Fleisch & Wurst"),
    ("cevapcici", "🥩", "Fleisch & Wurst"),
    ("kabanossi", "🥓", "Fleisch & Wurst"),
    ("krakauer", "🥓", "Fleisch & Wurst"),
    ("wachtel", "🍗", "Fleisch & Wurst"),
    ("spanferkel", "🥩", "Fleisch & Wurst"),
    ("braten", "🥩", "Fleisch & Wurst"),
    ("knoppers", "🍫", "Süßes & Snacks"),
    ("leibniz", "🍪", "Süßes & Snacks"),
    ("celebrations", "🍫", "Süßes & Snacks"),
    ("mars", "🍫", "Süßes & Snacks"),
    ("tafel", "🍫", "Süßes & Snacks"),
    ("lambertz", "🍪", "Süßes & Snacks"),
    ("gebäck", "🍪", "Süßes & Snacks"),
    ("törtchen", "🧁", "Süßes & Snacks"),
    ("laugen", "🥨", "Backwaren"),
    ("brezel", "🥨", "Backwaren"),
    ("gillette", "🪒", "Drogerie"),
    ("odol", "🪥", "Drogerie"),
    ("mundspül", "🪥", "Drogerie"),
    ("scholl", "🦶", "Drogerie"),
    ("hornhaut", "🦶", "Drogerie"),
    ("haarbürste", "🧴", "Drogerie"),
    ("kamm", "🧴", "Drogerie"),
    ("lenor", "🧴", "Haushalt"),
    ("spee", "🧴", "Haushalt"),
    ("fairy", "🧽", "Haushalt"),
    ("airwick", "🌸", "Haushalt"),
    ("wäsche", "🧺", "Haushalt"),
    ("küchen", "🍽️", "Haushalt"),
    ("gläser", "🥂", "Haushalt"),
    ("löffel", "🥄", "Haushalt"),
    ("fritteuse", "🍳", "Haushalt"),
    ("bürste", "🧹", "Haushalt"),
    ("bodenkehrer", "🧹", "Haushalt"),
    ("geschenkpapier", "🎁", "Haushalt"),
    ("aufbewahrung", "📦", "Haushalt"),
    ("feuerlöscher", "🧯", "Sonstiges"),
    ("schlafsack", "🛏️", "Sonstiges"),
    ("werkstatt", "🛠️", "Sonstiges"),
    ("blutdruck", "🩺", "Sonstiges"),
    ("lesehilfe", "👓", "Sonstiges"),
    ("brille", "👓", "Sonstiges"),
    ("guthabenkarte", "💳", "Sonstiges"),
    ("kreuzworträtsel", "📚", "Sonstiges"),
    ("playtive", "🧸", "Kinder"),
    ("hot wheels", "🧸", "Kinder"),
    ("plüsch", "🧸", "Kinder"),
    ("funko", "🧸", "Kinder"),
    ("schreibwaren", "✏️", "Kinder"),
    ("kugelschreiber", "✏️", "Kinder"),
    ("whiskas", "🐱", "Tierbedarf"),
    ("dreamies", "🐱", "Tierbedarf"),
    ("sheba", "🐱", "Tierbedarf"),
    ("felix", "🐱", "Tierbedarf"),
    ("pedigree", "🐶", "Tierbedarf"),
    // Runde 5: Long-Tail aus echten Daten
    ("weißkohl", "🥬", "Obst & Gemüse"),
    ("spitzkohl", "🥬", "Obst & Gemüse"),
    ("rotkohl", "🥬", "Obst & Gemüse"),
    ("grünkohl", "🥬", "Obst & Gemüse"),
    ("broccoli", "🥦", "Obst & Gemüse"),
    ("pfifferling", "🍄", "Obst & Gemüse"),
    ("pistazie", "🥜", "Süßes & Snacks"),
    ("chrysantheme", "💐", "Sonstiges"),
    ("dahlie", "💐", "Sonstiges"),
    ("bacon", "🥓", "Fleisch & Wurst"),
    ("lyoner", "🥓", "Fleisch & Wurst"),
    ("prosciutto", "🥓", "Fleisch & Wurst"),
    ("chipolata", "🥓", "Fleisch & Wurst"),
    ("haxe", "🥩", "Fleisch & Wurst"),
    ("kaninchen", "🥩", "Fleisch & Wurst"),
    ("wiener", "🌭", "Fleisch & Wurst"),
    ("grillfackel", "🥩", "Fleisch & Wurst"),
    ("ribs", "🍖", "Fleisch & Wurst"),
    ("wings", "🍗", "Fleisch & Wurst"),
    ("kotelett", "🥩", "Fleisch & Wurst"),
    ("mehrkornbrot", "🍞", "Backwaren"),
    ("bauernbrot", "🍞", "Backwaren"),
    ("steinofenbrot", "🍞", "Backwaren"),
    ("ofenbrot", "🍞", "Backwaren"),
    ("knusperbrot", "🍘", "Backwaren"),
    ("vollkornbrot", "🍞", "Backwaren"),
    ("blätterteig", "🥐", "Backwaren"),
    ("eintopf", "🍲", "Vorräte & Kochen"),
    ("gnocchi", "🍝", "Vorräte & Kochen"),
    ("barilla", "🍝", "Vorräte & Kochen"),
    ("ajvar", "🫙", "Vorräte & Kochen"),
    ("miracel whip", "🫙", "Vorräte & Kochen"),
    ("expressreis", "🍚", "Vorräte & Kochen"),
    ("ben’s", "🍚", "Vorräte & Kochen"),
    ("ben's", "🍚", "Vorräte & Kochen"),
    ("piccolini", "🍕", "Tiefkühl"),
    ("wagner", "🍕", "Tiefkühl"),
    ("pirulo", "🍦", "Tiefkühl"),
    ("brownie", "🍫", "Süßes & Snacks"),
    ("doppelkeks", "🍪", "Süßes & Snacks"),
    ("prinzen", "🍪", "Süßes & Snacks"),
    ("hobbits", "🍪", "Süßes & Snacks"),
    ("brandt", "🍪", "Süßes & Snacks"),
    ("hitschies", "🍬", "Süßes & Snacks"),
    ("bueno", "🍫", "Süßes & Snacks"),
    ("pingui", "🍫", "Süßes & Snacks"),
    ("kinder country", "🍫", "Süßes & Snacks"),
    ("kinder tris", "🍫", "Süßes & Snacks"),
    ("lorenz", "🥔", "Süßes & Snacks"),
    ("kaba", "🍫", "Getränke"),
    ("jacobs", "☕", "Getränke"),
    ("senseo", "☕", "Getränke"),
    ("caffè crema", "☕", "Getränke"),
    ("pepsi", "🥤", "Getränke"),
    ("schweppes", "🥤", "Getränke"),
    ("schwip schwap", "🥤", "Getränke"),
    ("limo", "🥤", "Getränke"),
    ("granini", "🧃", "Getränke"),
    ("naturelle", "💧", "Getränke"),
    ("budweiser", "🍺", "Alkohol"),
    ("budvar", "🍺", "Alkohol"),
    ("benediktiner", "🍺", "Alkohol"),
    ("lübzer", "🍺", "Alkohol"),
    ("cinzano", "🥂", "Alkohol"),
    ("asti", "🥂", "Alkohol"),
    ("lillet", "🍹", "Alkohol"),
    ("goldkrone", "🥃", "Alkohol"),
    ("grappa", "🥃", "Alkohol"),
    ("géramont", "🧀", "Molkerei & Eier"),
    ("maasdamer", "🧀", "Molkerei & Eier"),
    ("arla", "🧈", "Molkerei & Eier"),
    ("ariel", "🧴", "Haushalt"),
    ("kuschelweich", "🧴", "Haushalt"),
    ("reinigungstücher", "🧻", "Haushalt"),
    ("frischhaltedose", "📦", "Haushalt"),
    ("schüssel", "🥣", "Haushalt"),
    ("schneidebrett", "🔪", "Haushalt"),
    ("backform", "🍳", "Haushalt"),
    ("auflauf", "🍳", "Haushalt"),
    ("kontaktgrill", "🍳", "Haushalt"),
    ("mixer", "🔌", "Haushalt"),
    ("sandwichmaker", "🍳", "Haushalt"),
    ("karaffe", "🥂", "Haushalt"),
    ("sodastream", "🫧", "Haushalt"),
    ("spül", "🧽", "Haushalt"),
    ("regal", "🗄️", "Sonstiges"),
    ("wecker", "⏰", "Sonstiges"),
    ("geschenkkarte", "💳", "Sonstiges"),
    ("gutscheinkarte", "💳", "Sonstiges"),
    ("trinkflasche", "🍶", "Sonstiges"),
    ("bastelset", "🎨", "Kinder"),
    ("radierer", "✏️", "Kinder"),
    ("überraschung", "🧸", "Kinder"),
    ("autan", "🦟", "Drogerie"),
    ("bebe", "🧴", "Drogerie"),
    ("trimmer", "🪒", "Drogerie"),
    ("lockenstab", "💇", "Drogerie"),
    // Runde 6: Long-Tail aus echten Daten
    ("fleisch", "🥩", "Fleisch & Wurst"),
    ("serrano", "🥓", "Fleisch & Wurst"),
    ("chicken", "🍗", "Fleisch & Wurst"),
    ("activia", "🥛", "Molkerei & Eier"),
    ("actimel", "🥛", "Molkerei & Eier"),
    ("danone", "🥛", "Molkerei & Eier"),
    ("babybel", "🧀", "Molkerei & Eier"),
    ("mascarpone", "🧀", "Molkerei & Eier"),
    ("grana padano", "🧀", "Molkerei & Eier"),
    ("scamorza", "🧀", "Molkerei & Eier"),
    ("becel", "🧈", "Molkerei & Eier"),
    ("dinkel", "🌾", "Vorräte & Kochen"),
    ("flocken", "🥣", "Vorräte & Kochen"),
    ("kühne", "🫙", "Vorräte & Kochen"),
    ("balsamico", "🫙", "Vorräte & Kochen"),
    ("kathi", "🍰", "Vorräte & Kochen"),
    ("sherry", "🍷", "Alkohol"),
    ("bordeaux", "🍷", "Alkohol"),
    ("chianti", "🍷", "Alkohol"),
    ("amaretto", "🍸", "Alkohol"),
    ("ouzo", "🥃", "Alkohol"),
    ("metaxa", "🥃", "Alkohol"),
    ("akvavit", "🥃", "Alkohol"),
    ("spirituosen", "🥃", "Alkohol"),
    ("lagerbier", "🍺", "Alkohol"),
    ("vitamalz", "🥤", "Getränke"),
    ("fanta", "🥤", "Getränke"),
    ("sprite", "🥤", "Getränke"),
    ("mezzo mix", "🥤", "Getränke"),
    ("grapefruit", "🍊", "Obst & Gemüse"),
    ("nimm2", "🍬", "Süßes & Snacks"),
    ("lachgummi", "🍬", "Süßes & Snacks"),
    ("muffin", "🧁", "Süßes & Snacks"),
    ("grissini", "🥖", "Süßes & Snacks"),
    ("taft", "💇", "Drogerie"),
    ("haar", "💇", "Drogerie"),
    ("axe", "🧴", "Drogerie"),
    ("softlan", "🧴", "Haushalt"),
    ("weißer riese", "🧴", "Haushalt"),
    ("biff", "🧽", "Haushalt"),
    ("haushaltstücher", "🧻", "Haushalt"),
    ("handtuch", "🧺", "Haushalt"),
    ("trinkhalm", "🥤", "Haushalt"),
    ("zahnstocher", "🍴", "Haushalt"),
    ("hammer", "🛠️", "Sonstiges"),
    ("heimwerk", "🛠️", "Sonstiges"),
    ("kajak", "🛶", "Sonstiges"),
    ("sofa", "🛋️", "Sonstiges"),
    ("hemd", "👕", "Sonstiges"),
    ("dorade", "🐟", "Fisch"),
    ("calamares", "🦑", "Fisch"),
    ("sandwich", "🥪", "Sonstiges"),
    ("snackbox", "🥪", "Sonstiges"),
    ("spiel", "🧸", "Kinder"),
    // Runde 08.08.: Begriffe, die das Wörterbuch als Essen führt und für die
    // diese Tabelle kein Regal kannte. Ohne Eintrag fällt die Zeile in den
    // Sonstiges-Topf, in dem die Aktionsware steht.
    ("tofu", "🌱", "Vorräte & Kochen"),
    ("tempeh", "🌱", "Vorräte & Kochen"),
    ("speisestärke", "🌾", "Vorräte & Kochen"),
    ("hummus", "🫙", "Vorräte & Kochen"),
    ("kapern", "🫙", "Vorräte & Kochen"),
    ("schoten", "🫘", "Obst & Gemüse"),
    // Beide vom Prüftest gefunden, nicht von einer Zeile: Im Korpus dieser
    // Woche steht keines der beiden. „pflanzendrink" ist länger als
    // „pflanze" und sticht es damit — sonst wäre Hafermilch Gartenbedarf.
    ("pflanzendrink", "🌱", "Getränke"),
    ("kohletabletten", "💊", "Drogerie"),
    // Länger als "schoten" und damit Vorrang: die Vanilleschote ist eine
    // Backzutat, keine Zuckerschote.
    ("vanilleschote", "🌿", "Vorräte & Kochen"),
    // "filet" (Fleisch) und "lachs" sind gleich lang — Compounds explizit,
    // sonst entscheidet bei Gleichstand die Tabellenreihenfolge
    ("lachsfilet", "🐟", "Fisch"),
    ("fischfilet", "🐟", "Fisch"),
    ("forellenfilet", "🐟", "Fisch"),
    ("seelachs", "🐟", "Fisch"),
    ("zaziki", "🥣", "Molkerei & Eier"),
    ("tzatziki", "🥣", "Molkerei & Eier"),
];

/// Substring-Match; kurze Keywords (≤ 4 Bytes) müssen am Wortanfang stehen,
/// sonst träfe "eis" auch "Preis" und "reis" auch "Preise".
fn keyword_matches(hay: &str, kw: &str) -> bool {
    if kw.len() > 4 {
        return hay.contains(kw);
    }
    let mut from = 0;
    while let Some(pos) = hay[from..].find(kw) {
        let abs = from + pos;
        if abs == 0 || !hay[..abs].chars().next_back().is_some_and(|c| c.is_alphabetic()) {
            return true;
        }
        from = abs + kw.len();
    }
    false
}

/// Rohkategorie eines Scrapers → normalisierte Kategorie, wenn eindeutig.
fn map_raw_category(raw: &str) -> Option<&'static str> {
    let r = raw.to_lowercase();
    if r == "eis" {
        // exakt, sonst träfe "eis" auch "Fleisch"
        return Some("Tiefkühl");
    }
    let table: &[(&[&str], &str)] = &[
        // vor der Alkohol-Zeile: "Alkoholfreie Weine & Biere" sind Getränke
        (&["alkoholfrei"], "Getränke"),
        // vor der Obst-Zeile, sonst fiele "Obstbrand" unter Obst & Gemüse
        (&["obstbrand", "weinbrand"], "Alkohol"),
        (&["obst", "gemüse", "gemuese"], "Obst & Gemüse"),
        (&["molkerei", "käse", "milchprodukt", "joghurt"], "Molkerei & Eier"),
        (&["fleisch", "wurst", "geflügel"], "Fleisch & Wurst"),
        (&["fisch"], "Fisch"),
        (&["backware", "bäckerei", "brot"], "Backwaren"),
        (&["tiefkühl", "tiefkuehl", "tk "], "Tiefkühl"),
        (&["süß", "suess", "snack", "knabber", "keks"], "Süßes & Snacks"),
        (&["getränk", "getraenk", "kaffee", "tee"], "Getränke"),
        (
            &[
                "bier", "wein", "spirituosen", "alkohol", "whisk", "likör", "korn", "wodka",
                "rum", "gin", "obstbrand", "champagner", "sekt", "schaumwein", "cocktail",
                "aperitif",
            ],
            "Alkohol",
        ),
        (&["vorrat", "vorräte", "grundnahrung", "nährmittel", "kochen", "frühstück", "konserve", "feinkost"], "Vorräte & Kochen"),
        (&["drogerie", "pflege", "kosmetik", "gesundheit"], "Drogerie"),
        (&["haushalt", "reinigung", "wasch"], "Haushalt"),
        (&["tiernahrung", "tierfutter", "tierbedarf", "haustier"], "Tierbedarf"),
        (&["baby", "kind", "schul"], "Kinder"),
    ];
    table
        .iter()
        .find(|(keys, _)| keys.iter().any(|k| r.contains(k)))
        .map(|(_, cat)| *cat)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Enrichment {
    pub category: &'static str,
    pub emoji: &'static str,
    /// true, wenn das Emoji nur der Kategorie-Fallback ist (kein Keyword-Treffer).
    pub emoji_is_fallback: bool,
}

/// Kategorie, die die Keyword-Tabelle einem einzelnen Wort gibt.
///
/// Eigene Funktion, weil hier nicht ein Titel gefragt wird, sondern ein
/// **Wörterbuch-Begriff** — `käse`, `bier`, `tiefkühlgemüse`. Das ist die
/// sauberere Sonde: Der Begriff ist bereits das Ergebnis aller Sperr- und
/// Kompositaregeln, während der rohe Titel sie noch vor sich hat.
///
/// Kennt die Tabelle das Wort nicht, heißt der Begriff manchmal einfach schon
/// wie das Regal (`backwaren`, `obst`) — dann entscheidet der Name.
fn kategorie_fuer_begriff(begriff: &str) -> Option<&'static str> {
    RULES
        .iter()
        .filter(|(kw, _, _)| keyword_matches(begriff, kw))
        .max_by_key(|(kw, _, _)| kw.len())
        .map(|(_, _, c)| *c)
        .or_else(|| {
            CATEGORIES
                .iter()
                .find(|c| c.to_lowercase().starts_with(begriff))
                .copied()
        })
}

/// Titel/Untertitel/Rohkategorie → Kategorie + Emoji.
///
/// **Was Essen ist, entscheidet das Wörterbuch** und nicht diese Datei. Bis
/// zum 2026-08-08 wusste `matching.rs` es (über `NONFOOD_CAT`,
/// `NONFOOD_TERMS` und die Markenliste, auf der RUSSELL HOBBS längst stand)
/// und `enrich` wusste es noch einmal, an seiner eigenen Keyword-Tabelle —
/// zwei Stellen, die nichts voneinander wussten. Der Fehler kam aus beiden
/// Richtungen und wurde am 06.08. beim Entschlacken der Top-Deals gefunden:
///
/// * `GOURMETMAXX Ölsprüher` und `RUSSELL HOBBS Food Processor` standen unter
///   **Vorräte & Kochen**, weil die Rohkategorie der Kette „Kochen und
///   Grillen" heißt. Das ist ihr Regal, nicht die Beschaffenheit der Ware.
/// * `MAGGI Fix` und `Lätta` standen unter **Sonstiges**, weil kein Keyword
///   passte — im selben Topf wie die Aktionsware.
///
/// Jetzt fragt `enrich` einmal das Wörterbuch und lässt dessen Urteil in
/// beide Richtungen gelten: Non-Food kommt in kein Essensregal, und Essen
/// fällt nicht in den Sonstiges-Topf, solange sein Begriff ein Regal kennt.
pub fn enrich(title: &str, subtitle: Option<&str>, raw_category: Option<&str>) -> Enrichment {
    // Weiches Trennzeichen (U+00AD) entfernen — Scraper-Titel wie
    // "Fertig­gericht" enthalten es mitten im Wort und brechen sonst den Match.
    let hay = format!("{} {}", title, subtitle.unwrap_or(""))
        .to_lowercase()
        .replace('\u{ad}', "");

    // Längstes passendes Keyword gewinnt ("kinderschokolade" → Schokolade,
    // nicht Kinder; "katzenmilch" → Katzen, nicht Milch).
    let find_best = |text: &str| {
        RULES
            .iter()
            .filter(|(kw, _, _)| keyword_matches(text, kw))
            .max_by_key(|(kw, _, _)| kw.len())
    };
    // Erst Titel+Untertitel; ohne Treffer die Rohkategorie ("Sekt &
    // Schaumwein", "Pflanzen", "Eis" …) als zweite Keyword-Quelle.
    let best = find_best(&hay).or_else(|| {
        raw_category.and_then(|raw| find_best(&raw.to_lowercase().replace('-', " ")))
    });

    let mut category = raw_category
        .and_then(map_raw_category)
        .or(best.map(|(_, _, c)| *c))
        .unwrap_or(FALLBACK_CATEGORY);

    let tags = crate::matching::match_keys(title, subtitle, raw_category);
    if tags == [crate::matching::NONFOOD_KEY] {
        // Non-Food gehört in kein Essensregal. Die Rohkategorie der Kette
        // darf hier nicht mitreden — „kochen-und-grillen" ist ein Gang im
        // Markt und sagt nichts darüber, ob man das Ding essen kann.
        if FOOD_CATEGORIES.contains(&category) {
            category = best
                .map(|(_, _, c)| *c)
                .filter(|c| !FOOD_CATEGORIES.contains(c))
                .unwrap_or(FALLBACK_CATEGORY);
        }
    } else if category == FALLBACK_CATEGORY {
        // Umgekehrt: Was das Wörterbuch als Essen erkannt hat, gehört nicht
        // in den Topf für das Unsortierbare. Der Begriff weiß, in welches
        // Regal es gehört.
        //
        // „Sonstiges" zählt dabei nicht als Antwort, und das ist der Fund vom
        // 08.08.: Die Suche endet beim ersten Begriff, der ein Regal nennt —
        // ein Begriff, der auf den Sonstiges-Topf zeigt, verdeckte damit
        // jeden feineren Begriff derselben Zeile. `tofu` stand bei den
        // Non-Food-Zeilen und nahm so `hummus` und `krautsalat` das Wort.
        // Sieben Begriffe zeigen weiter dorthin (Handschuhe, Blumenerde,
        // Socken …) — bei denen ist es richtig, und seitdem folgenlos.
        if let Some(c) = tags
            .iter()
            .filter_map(|t| kategorie_fuer_begriff(t))
            .find(|c| *c != FALLBACK_CATEGORY)
        {
            category = c;
        }
    }

    match best {
        Some((_, emoji, _)) => Enrichment { category, emoji, emoji_is_fallback: false },
        None => Enrichment { category, emoji: default_emoji(category), emoji_is_fallback: true },
    }
}
