// Edge Function `gerichte`: Gericht → Zutaten und Liste → Gerichte, beides
// über Claude Haiku. Die App hält keinen Anthropic-Schlüssel; der liegt als
// Secret `ANTHROPIC_API_KEY` nur hier.
//
// Ablauf je Anfrage: Körper prüfen → Kontingent zählen (Migration v26) →
// Modell fragen → Antwort aufs Vokabular schneiden. Was die App schickt, ist
// nur der Text, um den es geht (Gericht oder Listeneinträge, dazu Begriffe
// aus den Angeboten). Kein Name, kein Ort, keine Filiale.
//
// Deploy ohne JWT-Prüfung (`verify_jwt = false` in `supabase/config.toml`):
// Die App spricht mit einem `sb_publishable_…`-Schlüssel, und der ist kein
// JWT. Den Missbrauch fängt das Kontingent ab — je Installation und, weil
// eine Installations-ID nur eine Zufallszahl ist, zusätzlich für alle
// zusammen.

import Anthropic from "npm:@anthropic-ai/sdk@0.132.1";
import {
  type Anfrage,
  leseAnfrage,
  nutzerNachricht,
  SCHEMA,
  schneideGericht,
  schneideVorschlaege,
  systemPrompt,
  zaehlerSchluessel,
} from "./logik.ts";

const MODELL = Deno.env.get("GERICHTE_MODELL") ?? "claude-haiku-5-5";
/// Anfragen je Installation und Berliner Kalendertag.
const GRENZE_JE_TAG = Number(Deno.env.get("GERICHTE_GRENZE") ?? "20");
/// Anfragen aller Installationen zusammen je Tag. Eine Anfrage kostet rund
/// 0,05 Cent (gemessen am 25.10.: ~11 500 Prompt-Tokens, ab der zweiten aus
/// dem Cache, dazu 150–800 Ausgabe-Tokens). 100 am Tag sind höchstens
/// 1,50 $ im Monat — unter dem Monatslimit des Kontos, das sich die Funktion
/// mit dem nächtlichen Taggen teilt.
const GRENZE_GESAMT = Number(Deno.env.get("GERICHTE_GRENZE_GESAMT") ?? "100");

const SYSTEM = systemPrompt();

const KOPF = {
  "Content-Type": "application/json; charset=utf-8",
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
};

function antwort(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: KOPF });
}

interface Zaehlung {
  erlaubt: boolean;
  grund?: "tag" | "gesamt";
  rest: number;
}

/// Zählt die Anfrage über `gerichte_anfrage_zaehlen` (Migration v26). Läuft
/// mit dem Service-Schlüssel, den Supabase jeder Funktion mitgibt; anon darf
/// die Funktion nicht aufrufen.
async function zaehle(installId: string): Promise<Zaehlung> {
  const url = Deno.env.get("SUPABASE_URL");
  const schluessel = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY");
  if (!url || !schluessel) throw new Error("SUPABASE_URL/SUPABASE_SERVICE_ROLE_KEY fehlen");
  const r = await fetch(`${url}/rest/v1/rpc/gerichte_anfrage_zaehlen`, {
    method: "POST",
    headers: {
      apikey: schluessel,
      Authorization: `Bearer ${schluessel}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      p_schluessel: await zaehlerSchluessel(installId),
      p_grenze: GRENZE_JE_TAG,
      p_grenze_gesamt: GRENZE_GESAMT,
    }),
  });
  if (!r.ok) throw new Error(`Kontingent: HTTP ${r.status} ${(await r.text()).slice(0, 200)}`);
  return await r.json();
}

async function frageModell(a: Anfrage): Promise<unknown> {
  const client = new Anthropic({ apiKey: Deno.env.get("ANTHROPIC_API_KEY"), maxRetries: 1, timeout: 45_000 });
  const r = await client.messages.create({
    model: MODELL,
    max_tokens: 3000,
    system: [{ type: "text", text: SYSTEM, cache_control: { type: "ephemeral" } }],
    // Einordnen und aufzählen, nicht nachdenken — wie beim Taggen in
    // `src/llm_tags.rs`. Haiku 5.5 erlaubt `disabled` bis Aufwand `high`.
    thinking: { type: "disabled" },
    output_config: {
      effort: "low",
      format: { type: "json_schema", schema: SCHEMA[a.art] },
    },
    messages: [{ role: "user", content: nutzerNachricht(a) }],
  });
  if (r.stop_reason === "refusal") throw new Error("Modell hat abgelehnt");
  const text = r.content.map((b) => (b.type === "text" ? b.text : "")).join("");
  console.log(JSON.stringify({
    art: a.art,
    eingabe: r.usage.input_tokens,
    cache_lesen: r.usage.cache_read_input_tokens,
    cache_schreiben: r.usage.cache_creation_input_tokens,
    ausgabe: r.usage.output_tokens,
  }));
  return JSON.parse(text);
}

export async function bearbeite(req: Request): Promise<Response> {
  if (req.method === "OPTIONS") return new Response(null, { headers: KOPF });
  if (req.method !== "POST") return antwort(405, { fehler: "methode" });

  let body: unknown;
  try {
    body = await req.json();
  } catch {
    return antwort(400, { fehler: "eingabe" });
  }
  const anfrage = leseAnfrage(body);
  if (!anfrage) return antwort(400, { fehler: "eingabe" });
  if (!Deno.env.get("ANTHROPIC_API_KEY")) return antwort(503, { fehler: "nicht_eingerichtet" });

  let zaehlung: Zaehlung;
  try {
    zaehlung = await zaehle(anfrage.installId);
  } catch (e) {
    console.error(String(e));
    return antwort(503, { fehler: "kontingent_unbekannt" });
  }
  if (!zaehlung.erlaubt) {
    return antwort(429, {
      fehler: zaehlung.grund === "gesamt" ? "gesamt" : "kontingent",
      grenze: GRENZE_JE_TAG,
    });
  }

  let roh: unknown;
  try {
    roh = await frageModell(anfrage);
  } catch (e) {
    console.error(String(e));
    return antwort(502, { fehler: "modell" });
  }

  if (anfrage.art === "zutaten") {
    const gericht = schneideGericht(roh);
    if (!gericht || gericht.zutaten.length === 0) {
      return antwort(422, { fehler: "kein_gericht", rest: zaehlung.rest });
    }
    return antwort(200, { gericht, rest: zaehlung.rest });
  }
  const gerichte = schneideVorschlaege(roh);
  if (gerichte.length === 0) return antwort(422, { fehler: "kein_gericht", rest: zaehlung.rest });
  return antwort(200, { gerichte, rest: zaehlung.rest });
}

if (import.meta.main) Deno.serve(bearbeite);
