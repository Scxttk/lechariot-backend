# LLM tagging experiment (2026-10-20)

Should a cheap model (Claude Haiku) tag offers instead of, or alongside, the dictionary?

## Numbers

**The eval is stale.** `matching-woerterbuch-eval.py` measures coverage on a July corpus (last week: 2026-08-05). It reports 91–93 % tagged, and production does worse: this week **567 of 2,758 non-nonfood products (20.6 %) have an empty `match_key`**. Autumn and Christmas products (Kürbis, Lebkuchen, Punsch, Adventskalender, Grablichter) aren't in the corpus. The eval also counts coverage, not correctness.

**Gold set** (`gold-2026-10-20.json`): 220 production products from this week, labelled by hand. 120 are drawn from the empty ones and 100 from the tagged ones. Weighted back to production:

| Dictionary today, food products this week (≈2,500) | |
|---|---:|
| correct | ~80 % |
| missing (text says what it is, no tag) | ~11 % |
| wrong tag (Käsewiener→käse, Kürbiskerne→pinienkerne, Kokos-Filzmatte→kokosnuss) | ~9 % |

**Shape of the 120 untagged:**

| | n | an LLM fixes it? |
|---|---:|---|
| non-food the filter misses (LED, Tupperware, jackets): harmless for matching | 55 | yes, but low value |
| word form: plural/compound of a known term (Passionsfrüchte, Säfte, Tafelbirnen, Speisekürbis) | 18 | yes, trivially |
| synonym/foreign word (Triple Sec, Amaro, Crushed Ice, Suppenhuhn, Rehkeule, Gruyère) | 16 | yes |
| brand only (Mon Cheri, Nusspli, Philadelphia, Feldschlösschen, DUPLO) | 22 | mostly |
| term missing from vocabulary (Honig, Lebkuchen) | 2 | no: vocabulary is a product decision |
| text doesn't say (Quattro Formaggi, "Frisches 517 g") | 7 | no: extractor problem |

So yes: almost all food failures are dictionary-shaped. 56 of 58 are word forms, synonyms or brands, and they come back with every new season. Only 9 of 120 are something a model can't fix either.

**LLM score: estimated, not measured** (no API was reachable, see below). The estimate is ~90 % correct (range 88–93 %), against ~80 % for the dictionary. Most of the gain comes from the 11 % missing; a smaller part from the wrong tags, which the prompt sees as `dict` hints. The risk: the model guesses on the 7 unknowable lines and occasionally changes a correct tag.

**Cost** (`llm_tagging.py estimate`, 3,637 distinct products this week, 73 requests of 50, prompt ≈ 2,300 tokens):

| model | per week, standard | batch + prompt cache |
|---|---:|---:|
| Haiku 5.5 | $0.06 | $0.02 |
| Haiku 4.5 | $0.56 | $0.20 |
| Sonnet 5.5 | $1.11 | $0.41 |

That assumes every product is new each week. Reuse across weeks would lower it, but production prunes history, so reuse can't be measured yet. **Latency for users: zero.** Tags are computed in the nightly and stored in `match_key` as today. The nightly gets ~2–5 min longer (sync), or up to ~1 h with the batch API.

## Pipeline (`llm_tagging.py`)

- The nightly computes dictionary tags as today, then sends each *uncached* (title, sub, cat) to Haiku with the vocabulary and the dictionary's tags as a hint. The answer is stored in a cache keyed by model + prompt version + text.
- Same input, same stored tags: the T3 determinism gate from the LLM-Entscheidung note holds by construction. A prompt or model change is a new key, i.e. an explicit re-tag.
- If the API fails, `match_key` falls back to the dictionary. Output is filtered to known vocabulary terms.
- `score` compares dictionary and cached LLM tags against the gold file.

## Not done

- **No model was run.** No Anthropic API key is configured for the project. GitHub Models (used by the Lidl LLM path) answers every request, even with a fake token, with a bare `200 OK` body, so it's dead as a free option.
- The gold labels are my judgement (one pass, no second rater).
- Side finding: running `matching-woerterbuch-eval.py` today rewrites the JSON **without the `warengruppe` fields** (316+/631− lines). The Python source doesn't know about them since #88. The next `/pflegerunde` would silently drop them, the same class of bug as #62.

To measure: put `ANTHROPIC_API_KEY` in the env, `pip install anthropic`, then

```
LLM_TAG_CACHE=/tmp/t.db python3 docs/llm-tagging/llm_tagging.py run docs/llm-tagging/gold-2026-10-20.json
LLM_TAG_CACHE=/tmp/t.db python3 docs/llm-tagging/llm_tagging.py score docs/llm-tagging/gold-2026-10-20.json
```

Cost of that run: < $0.01.
