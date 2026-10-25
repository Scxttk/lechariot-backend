// deno test --allow-read --allow-env supabase/functions/gerichte/

import { assert, assertEquals } from "jsr:@std/assert@1.0.13";
import { ausWoerterbuch } from "./erzeuge_vokabular.ts";
import {
  leseAnfrage,
  MAX_LISTE,
  nutzerNachricht,
  schneideGericht,
  schneideVorschlaege,
  schneideZutaten,
  systemPrompt,
  VOKABULAR,
  zaehlerSchluessel,
} from "./logik.ts";
import { bearbeite } from "./index.ts";

const ID = "0f8fad5b-d9cb-469f-a165-70867728950e";

Deno.test("vokabular.json passt zum Wörterbuch", async () => {
  assertEquals(VOKABULAR, await ausWoerterbuch(),
    "Wörterbuch geändert — erzeuge_vokabular.ts laufen lassen");
});

Deno.test("Vokabular enthält Essen und keine Drogerie", () => {
  const begriffe = new Set(VOKABULAR.map((b) => b.begriff));
  for (const b of ["hackfleisch", "nudeln", "tomaten", "zwiebeln", "petersilie"]) assert(begriffe.has(b), b);
  for (const b of ["sonnencreme", "shampoo", "büroklammern", "katzenfutter"]) assert(!begriffe.has(b), b);
});

Deno.test("Anfrage: Gericht wird gelesen, Leerraum zusammengezogen", () => {
  assertEquals(leseAnfrage({ install_id: ID, art: "zutaten", gericht: "  Lasagne   für 4 " }),
    { art: "zutaten", installId: ID, gericht: "Lasagne für 4" });
});

Deno.test("Anfrage: ohne gültige Installations-ID abgelehnt", () => {
  assertEquals(leseAnfrage({ install_id: "abc", art: "zutaten", gericht: "Lasagne" }), null);
  assertEquals(leseAnfrage({ art: "zutaten", gericht: "Lasagne" }), null);
});

Deno.test("Anfrage: leeres oder zu langes Gericht abgelehnt", () => {
  assertEquals(leseAnfrage({ install_id: ID, art: "zutaten", gericht: "   " }), null);
  assertEquals(leseAnfrage({ install_id: ID, art: "zutaten", gericht: "x".repeat(81) }), null);
});

Deno.test("Anfrage: Vorschläge filtern fremde Angebots-Begriffe", () => {
  const a = leseAnfrage({
    install_id: ID, art: "vorschlaege",
    liste: ["Nudeln", "Nudeln", "Tomaten"], angebote: ["hackfleisch", "erfunden", 7],
  });
  assertEquals(a, { art: "vorschlaege", installId: ID, liste: ["Nudeln", "Tomaten"], angebote: ["hackfleisch"] });
});

Deno.test("Anfrage: zu lange Liste abgelehnt", () => {
  const liste = Array.from({ length: MAX_LISTE + 1 }, (_, i) => `Artikel ${i}`);
  assertEquals(leseAnfrage({ install_id: ID, art: "vorschlaege", liste }), null);
});

Deno.test("Nachricht trägt nur den Text, keine Installations-ID", () => {
  const n = nutzerNachricht({ art: "zutaten", installId: ID, gericht: "Lasagne für 4" });
  assert(n.includes("Lasagne für 4"));
  assert(!n.includes(ID));
  assert(!systemPrompt().includes(ID));
});

Deno.test("Zutaten außerhalb des Vokabulars fallen weg", () => {
  const z = schneideZutaten([
    { begriff: "hackfleisch", name: "hackfleisch", menge: "500 g" },
    { begriff: "lorbeer-erfunden", name: "Lorbeer", menge: "2 Blätter" },
    { begriff: "sonnencreme", name: "sonnencreme", menge: "1" },
  ]);
  assertEquals(z.map((x) => x.begriff), ["hackfleisch"]);
});

Deno.test("Name muss ein Wort des Begriffs sein, sonst gilt der Begriff", () => {
  const z = schneideZutaten([
    { begriff: "nudeln", name: "Lasagne", menge: "250 g" },
    { begriff: "käse", name: "Reibekäse vom Markt", menge: "100 g" },
  ]);
  assertEquals(z.map((x) => x.name), ["Lasagne", "Käse"]);
});

Deno.test("Doppelte Zutaten werden zusammengelegt, Begriff klein geschrieben", () => {
  const z = schneideZutaten([
    { begriff: "Tomaten", name: "tomaten", menge: "1 Dose" },
    { begriff: "tomaten", name: "tomaten", menge: "400 g" },
  ]);
  assertEquals(z, [{ begriff: "tomaten", name: "Tomaten", menge: "1 Dose" }]);
});

Deno.test("Portionen außerhalb 1…20 werden 2", () => {
  const g = schneideGericht({ name: "Suppe", portionen: 0, zutaten: [] });
  assertEquals(g?.portionen, 2);
});

Deno.test("Vorschläge: höchstens vier, jeder mit mindestens zwei Zutaten", () => {
  const zwei = [{ begriff: "nudeln", name: "nudeln", menge: "" }, { begriff: "tomaten", name: "tomaten", menge: "" }];
  const gerichte = [
    { name: "Eins", portionen: 2, zutaten: [{ begriff: "nudeln", name: "nudeln", menge: "" }] },
    ...Array.from({ length: 5 }, (_, i) => ({ name: `G${i}`, portionen: 2, zutaten: zwei })),
  ];
  const v = schneideVorschlaege({ gerichte });
  assertEquals(v.map((g) => g.name), ["G0", "G1", "G2", "G3"]);
});

Deno.test("Zählerschlüssel ist ein Hash, nicht die ID", async () => {
  const s = await zaehlerSchluessel(ID);
  assertEquals(s.length, 64);
  assert(!s.includes(ID.slice(0, 8)));
  assertEquals(s, await zaehlerSchluessel(ID));
});

// --- Die ganze Funktion mit nachgebautem Netz ---------------------------------

function mitNetz(
  kontingent: unknown,
  modell: unknown,
  test: (aufrufe: { url: string; body: string }[]) => Promise<void>,
) {
  return async () => {
    const vorher = globalThis.fetch;
    Deno.env.set("SUPABASE_URL", "https://beispiel.supabase.co");
    Deno.env.set("SUPABASE_SERVICE_ROLE_KEY", "service-test");
    Deno.env.set("ANTHROPIC_API_KEY", "sk-test");
    const aufrufe: { url: string; body: string }[] = [];
    globalThis.fetch = async (eingabe: string | URL | Request, init?: RequestInit) => {
      const url = eingabe instanceof Request ? eingabe.url : String(eingabe);
      const body = eingabe instanceof Request ? await eingabe.text() : String(init?.body ?? "");
      aufrufe.push({ url, body });
      if (url.includes("/rpc/gerichte_anfrage_zaehlen")) return Response.json(kontingent);
      return Response.json({
        id: "msg_test", type: "message", role: "assistant", model: "claude-haiku-5-5",
        stop_reason: "end_turn", stop_sequence: null,
        content: [{ type: "text", text: JSON.stringify(modell) }],
        usage: { input_tokens: 10, output_tokens: 10, cache_read_input_tokens: 7000, cache_creation_input_tokens: 0 },
      });
    };
    try {
      await test(aufrufe);
    } finally {
      globalThis.fetch = vorher;
    }
  };
}

function post(body: unknown) {
  return new Request("http://lokal/gerichte", { method: "POST", body: JSON.stringify(body) });
}

Deno.test("Funktion: Gericht → Zutaten, aufs Vokabular geschnitten", mitNetz(
  { erlaubt: true, rest: 19 },
  { name: "Lasagne", portionen: 4, zutaten: [
    { begriff: "nudeln", name: "lasagne", menge: "250 g" },
    { begriff: "hackfleisch", name: "hackfleisch", menge: "500 g" },
    { begriff: "gibtsnicht", name: "x", menge: "1" },
  ] },
  async (aufrufe) => {
    const r = await bearbeite(post({ install_id: ID, art: "zutaten", gericht: "Lasagne für 4" }));
    assertEquals(r.status, 200);
    const j = await r.json();
    assertEquals(j.rest, 19);
    assertEquals(j.gericht.zutaten.map((z: { name: string }) => z.name), ["Lasagne", "Hackfleisch"]);
    // Das Kontingent bekommt den Hash, das Modell den Text — keiner die ID.
    assertEquals(aufrufe.length, 2);
    for (const a of aufrufe) assert(!a.body.includes(ID), a.url);
    const anModell = JSON.parse(aufrufe[1].body);
    assertEquals(anModell.model, "claude-haiku-5-5");
    assertEquals(anModell.output_config.format.type, "json_schema");
  },
));

Deno.test("Funktion: Kontingent erschöpft → 429, Modell wird nicht gefragt", mitNetz(
  { erlaubt: false, grund: "tag", rest: 0 },
  {},
  async (aufrufe) => {
    const r = await bearbeite(post({ install_id: ID, art: "zutaten", gericht: "Lasagne" }));
    assertEquals(r.status, 429);
    assertEquals((await r.json()).fehler, "kontingent");
    assertEquals(aufrufe.length, 1);
  },
));

Deno.test("Funktion: Gesamtdeckel → 429 gesamt", mitNetz(
  { erlaubt: false, grund: "gesamt", rest: 5 },
  {},
  async () => {
    const r = await bearbeite(post({ install_id: ID, art: "zutaten", gericht: "Lasagne" }));
    assertEquals(r.status, 429);
    assertEquals((await r.json()).fehler, "gesamt");
  },
));

Deno.test("Funktion: kein Gericht → 422", mitNetz(
  { erlaubt: true, rest: 3 },
  { name: "Auto", portionen: 2, zutaten: [] },
  async () => {
    const r = await bearbeite(post({ install_id: ID, art: "zutaten", gericht: "Auto waschen" }));
    assertEquals(r.status, 422);
    assertEquals((await r.json()).fehler, "kein_gericht");
  },
));

Deno.test("Funktion: Vorschläge aus der Liste", mitNetz(
  { erlaubt: true, rest: 10 },
  { gerichte: [{ name: "Spaghetti Bolognese", portionen: 2, zutaten: [
    { begriff: "nudeln", name: "spaghetti", menge: "250 g" },
    { begriff: "hackfleisch", name: "hackfleisch", menge: "300 g" },
    { begriff: "tomaten", name: "tomaten", menge: "1 Dose" },
  ] }] },
  async (aufrufe) => {
    const r = await bearbeite(post({
      install_id: ID, art: "vorschlaege", liste: ["Nudeln", "Tomaten"], angebote: ["hackfleisch"],
    }));
    assertEquals(r.status, 200);
    const j = await r.json();
    assertEquals(j.gerichte[0].zutaten.map((z: { name: string }) => z.name), ["Spaghetti", "Hackfleisch", "Tomaten"]);
    assert(JSON.parse(aufrufe[1].body).messages[0].content.includes("hackfleisch"));
  },
));

Deno.test("Funktion: kaputte Eingabe → 400 ohne Netz", mitNetz({}, {}, async (aufrufe) => {
  const r = await bearbeite(post({ install_id: ID, art: "zaubern" }));
  assertEquals(r.status, 400);
  await r.body?.cancel();
  assertEquals(aufrufe.length, 0);
}));
