# LLM tagging (`src/llm_tags.rs`)

Claude Haiku 5.5 tags offers in the nightly push, once per distinct product, cached. The dictionary stays as fast path and fallback.

## Turning it on

One place: the GitHub Actions secret **`ANTHROPIC_API_KEY`** on `lechariot-backend` (Settings → Secrets → Actions). The nightly reads it. Without it, nothing changes.

Locally (for `lechariot llm-tags` or a hand push), the same variable goes in `~/.config/lechariot/env`, see `scripts/env.example`.

## How it works

- `push::load_grouped` collects every offer's (title, subtitle, raw category) plus the dictionary's tags, and sends uncached ones to Haiku in requests of 50 (4 in parallel). The model picks only from the dictionary's vocabulary. Anything else is dropped.
- The model's answer gets every parent term added (`salami` → `wurst`, `sekt` → `wein`) before it is written. The app matches tags by equality (`OfferMatcher`, stage 2) and never walks up to a parent, so a Salami without `wurst` is invisible to someone who writes "Wurst". Added at apply time, not in the cache, so cached answers stay valid.
- Cache key = model + prompt fingerprint (prompt and vocabulary) + text. Same input, same tags, so the T3 determinism gate holds. A `/pflegerunde` that changes the vocabulary re-tags everything once: about $0.04.
- CI keeps the cache between nights with `actions/cache` (`llm-tags.json`).
- `LLM_TAGS_MODE=replace` (set in `nightly.yml` since 2026-10-25): the model's tags win over the dictionary's. `fill` only fills products where the dictionary found nothing; it lost to `replace` on the gold set (see Measuring).
- Any API error leaves those products on the dictionary and the run continues. `LLM_TAGS_MAX_PER_RUN` (default 8000) caps a runaway.
- The log line `LLM-Tags (Fill): … ≈ $… ; N of M offers differ from the dictionary` reports the real token usage each run.

## Cost

Prices from platform.claude.com/docs pricing, 2026-10-20, Haiku 5.5 with prompts under 100k tokens: $0.10 input, $0.125 cache write, $0.01 cache read, $0.50 output per MTok.

Tokens estimated from text length (3.2 chars/token, pessimistic): system prompt ≈ 2,270 tokens, ≈ 30 input + 14 output tokens per product.

| | per week | per month |
|---|---:|---:|
| 3,637 products/week (this week's production), every product new every week | $0.04 | **$0.17** |
| same, priced per product off the gold set (longer texts) | $0.06 | $0.24 |

Upper bound, since repeat products cost nothing. Even 10× off, it's ~$2/month. A $5 prepaid credit covers well over a year. Users see no added latency, since tags are computed in the nightly. The nightly runs about 1–3 minutes longer.

## Measuring

`gold-2026-10-20.json`: 220 production products from 2026-10-20, labelled by hand. 120 are drawn from empty `match_key`, 100 from tagged. Measured 2026-10-25 with `claude-haiku-5-5`, weighted back to production:

| | ok | missing | wrong |
|---|---:|---:|---:|
| dictionary | 80.3 % | 11.0 % | 8.8 % |
| Haiku `fill` | 87.3 % | 2.2 % | 10.4 % |
| Haiku `replace` | **92.1 %** | 2.2 % | **5.6 %** |

On the 100 already-tagged products, `replace` is right on 96.0 % against the dictionary's 90.0 %; on the 120 empty ones both modes reach 76.7 % (dictionary 10.8 %). `fill` adds coverage but keeps the dictionary's mistakes, which is why it ends up with more wrong tags than the dictionary alone.

The run: 5 requests, none failed, 12,110 input / 15,572 cache-write / 3,893 cache-read / 3,873 output tokens, **$0.0051**. Projected from those real tokens: $0.08/week, **$0.37/month** — twice the estimate below, still far inside a $5 credit.

```
cargo run --release -- llm-tags              # needs ANTHROPIC_API_KEY, costs < $0.01
cargo run --release -- llm-tags --estimate   # no key, no call
```

It prints dictionary, `fill` and `replace` side by side, plus real token usage and the monthly projection.

Of the 58 food products the dictionary misses, 56 are word forms (Passionsfrüchte, Säfte), synonyms (Triple Sec, Crushed Ice) or brand-only names (Nusspli, DUPLO). That's the work a model does well. 9 of 120 untagged can't be fixed by a model either: 2 terms missing from the vocabulary (Honig, Lebkuchen), 7 lines whose text doesn't say what the product is.

### Spot check against a real store (2026-10-25)

Penny München, 269 offers with a price, `replace` against the dictionary, every differing offer judged by hand. `examples/llm_stichprobe.rs` repeats it for any store.

| | differ | model better | dictionary better | both fine | both wrong |
|---|---:|---:|---:|---:|---:|
| prompt from #103 | 93 (34.6 %) | 41 | 35 | 9 | 8 |
| parent terms + prompt fix | 53 (19.7 %) | 41 | 3 | 7 | 2 |

Of the 35 cases where the dictionary was better, 24 were a dropped parent term (`wurst` on Salami, `käse` on Schnittkäse, `spirituosen` on Rum) and 7 a dropped broader term that is not a formal parent (`wurst` on Fleischwurst and Lyoner, `käse` on Hirtenkäse, `schnitzel` on Puten-Schnitzel). The prompt told the model to "tag the most specific term". Washing powder, toothpaste and deodorant went to `nonfood` although the vocabulary has `waschmittel`, `zahnpasta` and `deo`. The fix: parents are added in code, the prompt asks for the broader term too, and non-food takes a vocabulary term where one exists.

What the dictionary still does better: `fruchtsaft` without `saft` (Granini; `fruchtsaft` has no parent in the vocabulary) and `pralinen` on Kinder bueno.

**Haiku samples, and that is the biggest source of noise.** The same prompt over the same 269 offers, three fresh caches: 64 answers (24 %) differed between runs. Haiku 5.5 rejects `temperature`, so the cache holds one sample per product. Compare prompts over several runs, never one. Gold set, three runs each, `replace`, weighted:

| | ok | missing | wrong | ok without parent |
|---|---:|---:|---:|---:|
| prompt from #103, no parent terms (one run) | 89.9 % | 2.4 % | 7.7 % | 24 of 220 |
| prompt from #103 + parent terms | 90.6 % | 2.5 % | 6.9 % | 0 |
| new prompt + parent terms | **92.3 %** | **1.4 %** | **6.3 %** | 0 |

The gold scorer counts a child and its parent as equal, so it couldn't see the missing parents. `llm-tags` now also prints "richtig, aber ohne Oberbegriff".

## Open

- The first nightly with the key has not run yet. Its `LLM-Tags` log line is the first production number.
- The gold labels are one person's judgement (mine, one pass). Candles are labelled `nonfood` although `kerzen` is a vocabulary term.
- The vocabulary lacks some parents the app needs: `fleischwurst` → `wurst`, `feta` → `käse`, `fruchtsaft` → `saft`. Today the prompt has to catch them.
- The eval script `matching-woerterbuch-eval.py` measures coverage on a July corpus (91–93 %), while production has 20.6 % empty `match_key`. It also strips the `warengruppe` fields when it rewrites the JSON (since #88).
