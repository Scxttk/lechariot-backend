// Erzeugt `vokabular.json` aus dem Wörterbuch:
//
//   deno run --allow-read --allow-write supabase/functions/gerichte/erzeuge_vokabular.ts
//
// Die Funktion kann `docs/` beim Deploy nicht mitnehmen, deshalb liegt eine
// Kopie neben ihr. `logik_test.ts` prüft, dass die Kopie zum Wörterbuch passt
// — wer das Wörterbuch ändert und das Skript vergisst, sieht es dort.
//
// Nur Lebensmittel: Ein Rezept braucht keine Sonnencreme, und jeder Begriff
// weniger im Prompt ist einer, den das Modell nicht verwechseln kann.

const QUELLE = new URL("../../../docs/matching-woerterbuch.json", import.meta.url);
const ZIEL = new URL("./vokabular.json", import.meta.url);

const ESSEN = new Set([
  "Backwaren",
  "Vorräte & Kochen",
  "Molkerei & Eier",
  "Obst & Gemüse",
  "Fleisch & Wurst",
  "Fisch",
  "Getränke",
  "Alkohol",
  "Süßes & Snacks",
  "Tiefkühl",
]);

// Begriffe ohne Warengruppe (und ohne Oberbegriff mit einer), die trotzdem
// gegessen werden. Ausdrücklich gelistet statt geraten: Die Datei vermischt
// an dieser Stelle Kräuter mit Büroklammern.
const ESSEN_OHNE_GRUPPE = new Set([
  "naturtofu", "koriander", "pfeffer", "chili", "panettone", "trüffel", "pfefferminztee",
  "chicorée", "guave", "kastanien", "kokosnuss", "kresse", "quinoa", "schwarzwurzel",
  "portulak", "artischocken", "lauch", "ahornsirup", "backpulver", "hefe", "semmelbrösel",
  "vanille", "zimt", "muskatnuss", "kurkuma", "safran", "oregano", "pfefferkörner",
  "marzipan", "lebensmittelfarbe", "marinade", "knödel", "grieß", "spätzle", "chiasamen",
  "plätzchen", "dörrobst", "gelee", "kuvertüre", "sirup", "bagel", "roggenbrot",
  "zimtschnecken", "ricotta", "muscheln", "sardellen", "schnitzel", "feigen", "kaki",
  "litschi", "papaya", "rhabarber", "quitten", "fenchel", "pastinaken", "peperoni",
  "rettich", "rosenkohl", "sellerie", "basilikum", "minze", "schnittlauch", "dill",
  "petersilie",
]);

// Einzelne Begriffe aus anderen Gruppen, die in Rezepten vorkommen.
const ESSEN_AUSNAHMEN = new Set(["maultaschen", "nougatcreme"]);

// Sammelbegriffe: Auf einer Liste aus einem Rezept heißt „Konserven" nichts.
// Gemessen am 25.10.: Haiku nahm „konserven" neben „bohnen" für ein Chili.
const SAMMELBEGRIFFE = new Set([
  "konserven", "gewürze", "soßen", "backwaren", "fertiggericht", "eintopf", "protein/fitness",
]);

interface Eintrag {
  exact?: string[];
  warengruppe?: string;
  oberbegriff?: string;
}

export function vokabular(begriffe: Record<string, Eintrag>) {
  const gruppe = (k: string, tiefe = 0): string | undefined => {
    const e = begriffe[k];
    if (!e || tiefe > 5) return undefined;
    if (e.warengruppe) return e.warengruppe;
    return e.oberbegriff ? gruppe(e.oberbegriff, tiefe + 1) : undefined;
  };
  return Object.keys(begriffe)
    .filter((k) => {
      if (SAMMELBEGRIFFE.has(k)) return false;
      const g = gruppe(k);
      return g ? ESSEN.has(g) || ESSEN_AUSNAHMEN.has(k) : ESSEN_OHNE_GRUPPE.has(k);
    })
    .sort((a, b) => a.localeCompare(b, "de"))
    .map((k) => {
      const woerter = [...new Set([k, ...(begriffe[k].exact ?? [])].map((w) => w.toLocaleLowerCase("de-DE")))];
      return { begriff: k, woerter };
    });
}

export async function ausWoerterbuch() {
  const datei = JSON.parse(await Deno.readTextFile(QUELLE));
  return vokabular(datei.begriffe);
}

if (import.meta.main) {
  const v = await ausWoerterbuch();
  await Deno.writeTextFile(ZIEL, JSON.stringify(v, null, 1) + "\n");
  console.log(`${v.length} Begriffe nach ${ZIEL.pathname}`);
}
