// Alles an der Funktion, was ohne Netz geht: Eingabe prüfen, Prompt bauen,
// Antwort aufs Vokabular zurückschneiden. `index.ts` hängt nur noch HTTP,
// Kontingent und Modell daran — und die Tests in `logik_test.ts` brauchen
// weder Schlüssel noch Datenbank.

import vokabularDatei from "./vokabular.json" with { type: "json" };

/// Ein Begriff des Wörterbuchs und die Wörter, mit denen die App ihn findet.
/// Erzeugt von `erzeuge_vokabular.ts` aus `docs/matching-woerterbuch.json`.
export interface Begriff {
  begriff: string;
  woerter: string[];
}

export const VOKABULAR: Begriff[] = vokabularDatei as Begriff[];

const NACH_BEGRIFF = new Map(VOKABULAR.map((b) => [b.begriff, b]));

// Obergrenzen der Eingabe. Ein Gericht ist eine Zeile, eine Liste hat selten
// mehr als 40 Einträge; was darüber liegt, ist kein Mensch, sondern ein
// Versuch, das Kontingent in Tokens umzurechnen.
export const MAX_GERICHT = 80;
export const MAX_LISTE = 60;
export const MAX_EINTRAG = 40;
export const MAX_ANGEBOTE = 80;

export type Anfrage =
  | { art: "zutaten"; installId: string; gericht: string }
  | { art: "vorschlaege"; installId: string; liste: string[]; angebote: string[] };

export interface Zutat {
  begriff: string;
  name: string;
  menge: string;
}

export interface Gericht {
  name: string;
  portionen: number;
  zutaten: Zutat[];
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function zeile(wert: unknown, max: number): string | null {
  if (typeof wert !== "string") return null;
  const t = wert.replace(/\s+/g, " ").trim();
  return t.length > 0 && t.length <= max ? t : null;
}

/// Liest den Anfragekörper. `null` heißt 400 — die App schickt nie etwas
/// anderes, also ist eine falsche Form kein Fall, der eine Erklärung braucht.
///
/// Angebots-Begriffe außerhalb des Vokabulars fallen still weg statt die
/// Anfrage abzulehnen: Die App kennt dasselbe Wörterbuch, aber ein älterer
/// Build kann einen Begriff schicken, den es inzwischen nicht mehr gibt.
export function leseAnfrage(body: unknown): Anfrage | null {
  if (typeof body !== "object" || body === null) return null;
  const b = body as Record<string, unknown>;
  const installId = typeof b.install_id === "string" && UUID.test(b.install_id)
    ? b.install_id.toLowerCase()
    : null;
  if (!installId) return null;

  if (b.art === "zutaten") {
    const gericht = zeile(b.gericht, MAX_GERICHT);
    return gericht ? { art: "zutaten", installId, gericht } : null;
  }
  if (b.art === "vorschlaege") {
    if (!Array.isArray(b.liste) || b.liste.length > MAX_LISTE) return null;
    const liste = [...new Set(b.liste.map((e) => zeile(e, MAX_EINTRAG)).filter((e): e is string => !!e))];
    if (liste.length === 0) return null;
    const roh = Array.isArray(b.angebote) ? b.angebote.slice(0, MAX_ANGEBOTE) : [];
    const angebote = [...new Set(roh.filter((t): t is string => typeof t === "string" && NACH_BEGRIFF.has(t)))];
    return { art: "vorschlaege", installId, liste, angebote };
  }
  return null;
}

const ZUTAT_SCHEMA = {
  type: "object",
  properties: {
    begriff: { type: "string" },
    name: { type: "string" },
    menge: { type: "string" },
  },
  required: ["begriff", "name", "menge"],
  additionalProperties: false,
};

const GERICHT_SCHEMA = {
  type: "object",
  properties: {
    name: { type: "string" },
    portionen: { type: "integer" },
    zutaten: { type: "array", items: ZUTAT_SCHEMA },
  },
  required: ["name", "portionen", "zutaten"],
  additionalProperties: false,
};

export const SCHEMA = {
  zutaten: GERICHT_SCHEMA,
  vorschlaege: {
    type: "object",
    properties: { gerichte: { type: "array", items: GERICHT_SCHEMA } },
    required: ["gerichte"],
    additionalProperties: false,
  },
};

// Pro Begriff höchstens so viele Wörter im Prompt. Die ersten stehen in der
// Datei vorn, weil sie die gebräuchlichen sind; der Rest sind Marken- und
// Schreibvarianten, die einem Rezept nichts sagen.
const WOERTER_IM_PROMPT = 10;

/// Der feste Teil des Prompts. Er ändert sich nur mit dem Vokabular und ist
/// deshalb der Teil, der im Prompt-Cache liegt.
export function systemPrompt(): string {
  const zeilen = VOKABULAR.map((b) =>
    `${b.begriff}: ${b.woerter.slice(0, WOERTER_IM_PROMPT).join(", ")}`
  ).join("\n");
  return `Du hilfst beim Einkaufen in einer deutschen Einkaufslisten-App. Jede Zutat, die du nennst, landet als Eintrag auf der Liste und wird dort mit den Wochenangeboten der Supermärkte verglichen. Deshalb muss jede Zutat auf einen Begriff aus VOKABULAR zeigen.

Regeln:
- "begriff" ist genau ein Schlüssel aus VOKABULAR (der Teil vor dem Doppelpunkt), exakt so geschrieben.
- "name" ist das Wort, das auf die Liste kommt: eines der Wörter hinter dem Doppelpunkt desselben Begriffs. Nimm das genaueste, das das Rezept wirklich braucht, nicht den Oberbegriff: Lasagneplatten -> begriff "nudeln", name "lasagne"; Spaghetti -> name "spaghetti"; Kidneybohnen -> name "kidneybohnen". Den Begriff selbst nur, wenn kein Wort genauer passt.
- Zutaten ohne passenden Begriff lässt du weg. Erfinde keinen Begriff.
- Wasser, Salz, Pfeffer und Speiseöl lässt du weg; das hat jeder zu Hause.
- "menge" ist eine kurze Mengenangabe auf Deutsch für die genannte Portionenzahl ("500 g", "2 Stück", "1 Dose"). Ohne Portionenangabe rechnest du für 2.
- Jede Zutat nur einmal. Gerichtnamen auf Deutsch, so wie man sie zu Hause nennt.
- Ist der Text kein Gericht und kein Getränk, das man zubereitet, gibst du eine leere Zutatenliste zurück.

VOKABULAR:
${zeilen}`;
}

/// Die Nutzernachricht. Nur der Text, um den es geht — kein Name, kein Ort,
/// keine Filiale.
export function nutzerNachricht(a: Anfrage): string {
  if (a.art === "zutaten") {
    return `Gericht: ${a.gericht}\nNenne die Zutaten für dieses Gericht.`;
  }
  const angebote = a.angebote.length > 0
    ? `\nDiese Woche im Angebot (Begriffe): ${a.angebote.join(", ")}`
    : "";
  return `Auf der Einkaufsliste: ${a.liste.join(", ")}${angebote}
Schlage 4 verschiedene alltägliche Gerichte vor, die möglichst viel von der Liste verwenden und wenig Zusätzliches brauchen. Was im Angebot ist, darf bevorzugt dazukommen. Nenne für jedes Gericht alle Zutaten, auch die, die schon auf der Liste stehen, für 2 Portionen.`;
}

function klein(s: string): string {
  return s.toLocaleLowerCase("de-DE").trim();
}

function gross(w: string): string {
  return w.length === 0 ? w : w[0].toLocaleUpperCase("de-DE") + w.slice(1);
}

/// Das Wörterbuch schreibt alles klein. Auf der Liste steht das erste Wort
/// groß und das letzte, weil das bei einer Zutat das Hauptwort ist:
/// „passierte tomaten" → „Passierte Tomaten".
function anzeigeName(wort: string): string {
  const teile = wort.split(" ");
  teile[0] = gross(teile[0]);
  teile[teile.length - 1] = gross(teile[teile.length - 1]);
  return teile.join(" ");
}

/// **Das Modell darf nur sagen, was das Wörterbuch kennt.** Derselbe Gedanke
/// wie `filter_vocab` in `src/llm_tags.rs`: Ein erfundener Begriff fände in
/// der App kein Angebot und wäre auf der Liste ein Eintrag, der nie trifft.
///
/// Zwei Stufen:
/// 1. `begriff` außerhalb des Vokabulars → die Zutat fällt weg.
/// 2. `name` ist nicht unter den Wörtern dieses Begriffs → der Begriff selbst
///    wird der Name. Sonst stünde „Lasagneplatten" auf der Liste, und das Wort
///    kennt die Suche nicht; „Lasagne" kennt sie.
export function schneideZutaten(roh: unknown): Zutat[] {
  if (!Array.isArray(roh)) return [];
  const aus: Zutat[] = [];
  const gesehen = new Set<string>();
  for (const z of roh) {
    if (typeof z !== "object" || z === null) continue;
    const { begriff, name, menge } = z as Record<string, unknown>;
    if (typeof begriff !== "string") continue;
    const eintrag = NACH_BEGRIFF.get(klein(begriff));
    if (!eintrag) continue;
    const gewuenscht = typeof name === "string" ? klein(name) : "";
    const wort = eintrag.woerter.includes(gewuenscht) ? gewuenscht : eintrag.begriff;
    const anzeige = anzeigeName(wort);
    if (gesehen.has(anzeige)) continue;
    gesehen.add(anzeige);
    aus.push({
      begriff: eintrag.begriff,
      name: anzeige,
      menge: typeof menge === "string" ? menge.trim().slice(0, 30) : "",
    });
  }
  return aus;
}

export function schneideGericht(roh: unknown): Gericht | null {
  if (typeof roh !== "object" || roh === null) return null;
  const g = roh as Record<string, unknown>;
  const name = zeile(g.name, MAX_GERICHT);
  if (!name) return null;
  const portionen = typeof g.portionen === "number" && g.portionen >= 1 && g.portionen <= 20
    ? Math.round(g.portionen)
    : 2;
  return { name, portionen, zutaten: schneideZutaten(g.zutaten) };
}

/// Höchstens vier Vorschläge, jeder mit mindestens zwei Zutaten — ein
/// „Gericht" aus einer Zutat ist ein Artikel, kein Vorschlag.
export function schneideVorschlaege(roh: unknown): Gericht[] {
  if (typeof roh !== "object" || roh === null) return [];
  const liste = (roh as Record<string, unknown>).gerichte;
  if (!Array.isArray(liste)) return [];
  return liste
    .map(schneideGericht)
    .filter((g): g is Gericht => g !== null && g.zutaten.length >= 2)
    .slice(0, 4);
}

/// Schlüssel für den Kontingent-Zähler: SHA-256 der Installations-ID. Die
/// Tabelle muss nur wiedererkennen, nicht zurückrechnen können — und eine
/// Zeile, die keine Installations-ID enthält, ist keine, die Auskunft oder
/// Löschung betreffen müsste.
export async function zaehlerSchluessel(installId: string): Promise<string> {
  const daten = new TextEncoder().encode(`gerichte:${installId}`);
  const hash = await crypto.subtle.digest("SHA-256", daten);
  return Array.from(new Uint8Array(hash), (b) => b.toString(16).padStart(2, "0")).join("");
}
