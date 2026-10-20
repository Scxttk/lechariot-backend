#!/usr/bin/env python3
"""Prototype: tag weekly offers with a cheap LLM once per product, cached.

Experiment, not wired into the nightly. The dictionary stays the fast path;
the model only sees each distinct (title, subtitle, category) once — the
cache key also carries the model and the prompt version, so the same input
always yields the same stored tags (determinism gate T3 from the
LLM-Entscheidung note holds by construction, not by temperature).

    llm_tagging.py estimate GOLD_OR_OFFERS.json   # tokens + cost, no API call
    llm_tagging.py run      GOLD_OR_OFFERS.json   # needs ANTHROPIC_API_KEY
    llm_tagging.py score    GOLD.json             # dictionary vs. cached LLM tags
"""
import hashlib, importlib.util, json, os, sqlite3, sys

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL = os.environ.get("LLM_TAG_MODEL", "claude-haiku-5-5")
PROMPT_VERSION = "v1"
CHUNK = 50
CACHE = os.environ.get("LLM_TAG_CACHE", os.path.expanduser("~/.local/share/lechariot/llm-tags.db"))

# $/MTok, input/output, standard API (batch halves both). Cached 2026-10-06.
PREIS = {"claude-haiku-5-5": (0.10, 0.50), "claude-haiku-4-5": (1.00, 5.00),
         "claude-sonnet-5-5": (2.00, 10.00)}
# German shop text with umlauts and brand caps tokenizes denser than English;
# 3.2 chars/token is a deliberately pessimistic estimate until count_tokens runs.
ZEICHEN_JE_TOKEN = 3.2

_spec = importlib.util.spec_from_file_location(
    "ev", os.path.join(HERE, "..", "matching-woerterbuch-eval.py"))
ev = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(ev)


def system_prompt():
    ober = {}
    for kind, vater in ev.OBER.items():
        ober.setdefault(vater, []).append(kind)
    return "\n".join([
        "You tag German supermarket offers with search terms from a fixed vocabulary.",
        "A user types one of these terms into a shopping list; an offer must carry a term",
        "only if that user would want to see this offer. A missing tag costs one hit;",
        "a wrong tag puts the wrong product into someone's shopping. When unsure, leave it out.",
        "",
        "Rules:",
        "- Use only terms from VOCABULARY, spelled exactly. Several terms are fine.",
        "- Non-food (household, clothing, toys, decoration, candles, tools, pet supplies,",
        "  cosmetics, cleaning products): return [\"nonfood\"].",
        "- If the text does not say what the product is (cut-off variant lines like",
        "  \"Gold | 500 g\", bare sizes, layout fragments): return [].",
        # Examples must never come from the gold file, or the score measures the prompt.
        "- Use brand knowledge: \"Ferrero Küsschen\" is pralinen, \"Bresso\" is frischkäse.",
        "- Compounds name the product by their LAST part: \"Käsespätzle\" is spätzle, not käse;",
        "  \"Erdbeerjoghurt\" is joghurt, not erdbeeren.",
        "- Flavours, ingredients and serving suggestions are not the product:",
        "  \"Pudding Vanille-Geschmack\" is pudding only.",
        "- `dict` is what the rule-based tagger found. Keep its tags when they are right,",
        "  drop the wrong ones, add missing ones.",
        "",
        "Broader terms (a product tagged with a child term is also found under the parent;",
        "tag the most specific term that fits):",
        *[f"  {v}: {', '.join(sorted(k))}" for v, k in sorted(ober.items())],
        "",
        "VOCABULARY: " + ", ".join(sorted(ev.V)) + ", nonfood",
        "",
        "Input: a JSON list of {\"i\", \"title\", \"sub\", \"cat\", \"dict\"}.",
        "Output: only a JSON object mapping each i to its list of terms, e.g.",
        "{\"0\": [\"käse\"], \"1\": [\"nonfood\"], \"2\": []}",
    ])


def eingabe(datei):
    rows = json.load(open(datei))
    return [{"title": r["title"], "sub": r.get("sub", ""), "cat": r.get("cat", ""),
             "dict": ev.match_keys(r["title"], r.get("sub", ""), r.get("cat", ""))[0]}
            for r in rows]


def schluessel(o):
    roh = "\x1f".join([MODEL, PROMPT_VERSION, o["title"], o["sub"], o["cat"]])
    return hashlib.sha256(roh.encode()).hexdigest()


def cache():
    con = sqlite3.connect(CACHE)
    con.execute("create table if not exists llm_tags (k text primary key, title text, tags text,"
                " model text, prompt text, created_at text default current_timestamp)")
    return con


def user_text(chunk):
    return json.dumps([{"i": i, **o} for i, o in enumerate(chunk)], ensure_ascii=False)


def estimate(offers):
    sp = len(system_prompt()); chunks = [offers[i:i + CHUNK] for i in range(0, len(offers), CHUNK)]
    ein = sum(sp + len(user_text(c)) for c in chunks) / ZEICHEN_JE_TOKEN
    ein_cached = sum(sp * 0.1 + len(user_text(c)) for c in chunks) / ZEICHEN_JE_TOKEN
    aus = len(offers) * 14  # `"12": ["käse", "wurst"],` ≈ 10–16 tokens
    print(f"{len(offers)} offers, {len(chunks)} requests, system prompt ≈ {sp / ZEICHEN_JE_TOKEN:.0f} tokens")
    print(f"input ≈ {ein:,.0f} tokens ({ein_cached:,.0f} with prompt cache), output ≈ {aus:,} tokens")
    for m, (pi, po) in PREIS.items():
        std = ein / 1e6 * pi + aus / 1e6 * po
        best = (ein_cached / 1e6 * pi + aus / 1e6 * po) / 2
        print(f"  {m:18s} ${std:.3f} standard · ${best:.3f} batch+cache")


def run(offers):
    import anthropic  # pip install anthropic; only this subcommand needs it
    client = anthropic.Anthropic()
    con = cache()
    offen = [o for o in offers if not con.execute("select 1 from llm_tags where k=?", (schluessel(o),)).fetchone()]
    print(f"{len(offers) - len(offen)} cached, {len(offen)} to tag with {MODEL}")
    system = [{"type": "text", "text": system_prompt(), "cache_control": {"type": "ephemeral"}}]
    for s in range(0, len(offen), CHUNK):
        chunk = offen[s:s + CHUNK]
        # Classification: no thinking, low effort. The nightly would send the same
        # requests through client.messages.batches for half the price.
        msg = client.messages.create(model=MODEL, max_tokens=4096, system=system,
                                     thinking={"type": "disabled"}, output_config={"effort": "low"},
                                     messages=[{"role": "user", "content": user_text(chunk)}])
        text = next((b.text for b in msg.content if b.type == "text"), "")
        try:
            antwort = json.loads(text[text.index("{"):text.rindex("}") + 1])
        except ValueError:
            print(f"  chunk {s}: unparsable answer, left uncached"); continue
        erlaubt = set(ev.V) | {"nonfood"}
        for i, o in enumerate(chunk):
            tags = [t for t in antwort.get(str(i), []) if t in erlaubt]
            con.execute("insert or replace into llm_tags (k,title,tags,model,prompt) values (?,?,?,?,?)",
                        (schluessel(o), o["title"], json.dumps(tags, ensure_ascii=False), MODEL, PROMPT_VERSION))
        con.commit()
        print(f"  chunk {s}: in {msg.usage.input_tokens} (cache read {msg.usage.cache_read_input_tokens}),"
              f" out {msg.usage.output_tokens}")


def urteil(tags, gold):
    """ok / fehlt / falsch / harmlos — one product against its gold label.

    gold None: the text does not say what it is; any food tag is a guess.
    gold ["nonfood"]: empty is harmless, a food tag is wrong.
    Otherwise every tag must be a gold tag or a parent/child of one.
    """
    if gold is None:
        return "ok" if tags in ([], ["nonfood"]) else "falsch"
    if gold == ["nonfood"]:
        return "ok" if tags == ["nonfood"] else ("harmlos" if not tags else "falsch")
    if not gold:
        return "ok" if not tags else "falsch"
    if not tags:
        return "fehlt"
    nah = set(gold) | {ev.OBER[g] for g in gold if g in ev.OBER} | {k for k, v in ev.OBER.items() if v in gold}
    return "ok" if all(t in nah for t in tags) else "falsch"


def score(datei):
    from collections import Counter
    gold = json.load(open(datei)); con = cache(); stand = {"dict": Counter(), "llm": Counter()}
    fehlend = 0
    for g in gold:
        o = {"title": g["title"], "sub": g.get("sub", ""), "cat": g.get("cat", "")}
        stand["dict"][urteil(g["dict"], g["gold"])] += 1
        row = con.execute("select tags from llm_tags where k=?", (schluessel(o),)).fetchone()
        if row:
            stand["llm"][urteil(json.loads(row[0]), g["gold"])] += 1
        else:
            fehlend += 1
    for name, c in stand.items():
        n = sum(c.values())
        if n:
            print(f"{name:5s} n={n}  " + "  ".join(f"{k} {v} ({v / n:.0%})" for k, v in sorted(c.items())))
    if fehlend:
        print(f"llm: {fehlend} products not in the cache — run `run` first")


if __name__ == "__main__":
    cmd, datei = sys.argv[1], sys.argv[2]
    {"estimate": lambda: estimate(eingabe(datei)), "run": lambda: run(eingabe(datei)),
     "score": lambda: score(datei)}[cmd]()
