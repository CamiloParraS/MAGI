# Search-quality eval

recall@k / MRR history, per SPEC.md §7 M3 and §8. Updated whenever the
embedder, ranking, or eval query set changes materially. See `eval/README.md`
for the query-file format and `eval/queries.jsonl` for the 60 queries.

## Reference machine

- AMD Ryzen 7 7445HS (8 cores / 16 threads), 16 GB RAM
- Windows 11 Home Single Language 10.0.26200, x86_64
- `magi-cli eval` built with `cargo build --release -p magi-cli`

## Baseline: fp32 `intfloat/multilingual-e5-small`

**Method:** `magi-cli eval eval/queries.jsonl --corpus fixtures/corpus`,
real `E5Embedder` (fp32 `model.onnx`, the manifest's current default — see
`models/manifest.toml`), fresh temp DB. `fixtures/corpus` indexed 37 files
(2 errored: `edge/truncated.pdf` and `edge/password_protected.pdf`, both
intentionally-broken fixtures that M2's own tests assert error cleanly —
not a regression).

| Mode        |  n | recall@5 | recall@10 |   MRR |
| ----------- | -:| -------:| --------:| -----:|
| fts-only    | 60 |   0.417 |    0.417 | 0.408 |
| vector-only | 60 |   0.983 |    0.983 | 0.818 |
| hybrid      | 60 |   0.983 |    0.983 | 0.807 |

By `lang` (hybrid):

| lang  |  n | recall@5 | recall@10 |   MRR |
| ----- | -:| -------:| --------:| -----:|
| en    | 20 |   1.000 |    1.000 | 1.000 |
| es    | 20 |   1.000 |    1.000 | 1.000 |
| cross | 20 |   0.950 |    0.950 | 0.422 |

By `lang` (fts-only, for contrast — keyword search has zero cross-lingual
signal by construction):

| lang  |  n | recall@5 | recall@10 |   MRR |
| ----- | -:| -------:| --------:| -----:|
| en    | 20 |   0.550 |    0.550 | 0.550 |
| es    | 20 |   0.700 |    0.700 | 0.675 |
| cross | 20 |   0.000 |    0.000 | 0.000 |

### Open finding: hybrid does not clearly beat vector-only here

SPEC.md §7 M3 requires hybrid to beat both FTS-only and vector-only on
overall recall@5. Measured, hybrid **ties** vector-only on recall@5
(0.983 = 0.983) and is slightly **worse** on MRR (0.807 vs. 0.818) — RRF
fusion plus the filename/recency boosts occasionally demote a vector
search's rank-1 hit by a position or two when FTS contributes nothing
useful (all 20 `cross` queries: FTS can't match different-language terms
at all, so hybrid fuses a real vector ranking with an empty FTS list).

This is real, not fabricated, but the eval corpus is almost certainly too
small and too cleanly separable to be a fair test of the hybrid design:
vector-only is already close to its recall@5 ceiling (0.983 = 59/60), so
there's very little room for anything to "beat" it, and 13 short,
topically-distinct synthetic documents don't reproduce the ambiguity,
exact-ID/keyword-matters cases (invoice numbers, filenames, code
identifiers) that FTS is supposed to contribute value on in a real,
larger, messier personal-files corpus. Not resolved — needs a bigger, more
realistic corpus before concluding whether this is a real fusion-tuning
gap or just an artifact of a too-easy eval set. Left as an open item
rather than tuning RRF weights to fit 60 queries.

## Quantization: int8 vs. fp32, and peak RSS

Same eval, int8 `model_qint8_avx512_vnni.onnx` swapped in for
`model.onnx`: vector-only/hybrid recall@5 drops from 0.983 to 0.967 (1.6
points — within SPEC.md §7 M3's 2-point allowance).

Peak RSS of `magi-cli index fixtures/corpus` (37 files; Windows
`PeakWorkingSet64`, reference machine above): **fp32 1,302.5 MB, int8
1,024.9 MB**. SPEC.md §7 M3's target is ≤ 700 MB for the text-only
pipeline — **neither variant meets it**; int8 saves 277.6 MB but is still
~325 MB over budget. Full decision and analysis in ADR-0005: fp32 stays
the shipped default for now, since a recall-only comparison on this small
eval corpus is weak evidence either way, and the RSS overshoot needs its
own investigation (likely `ort` thread-pool defaults and/or the
tokenizer's ~250k-entry vocab) regardless of which variant ships.

## Latency (NFR-2/NFR-3) on 100k synthetic chunks

`xtask bench-corpus` (`just bench`): 5,000 synthetic files × 20 chunks =
100,000 chunks, random text/vectors (query side uses the real `E5Embedder`
— see the module doc comment in `xtask/src/bench_corpus.rs` for why
synthetic content is fine here but wouldn't be for a recall eval), same
reference machine as above.

| Metric                          |   Measured | Target (SPEC.md §2.2) | Result |
| -------------------------------- | ---------:| ----------------------:| ------:|
| Cold (model load + first search) |  3,176 ms |               ≤ 3,000 ms | **FAIL** (6% over) |
| Warm p95 (200 queries)            |    585 ms |                 ≤ 300 ms | **FAIL** (95% over) |
| Warm p50 / max                   | 533 / 640 ms |                     — | — |

**Root cause, isolated (not fixed here):** `embed_query` alone is fast
(~13 ms/call, measured in isolation) — the embedder is not the bottleneck.
Breaking `hybrid_search` down at smaller corpus sizes (same probe method,
`fts`/`vector`/`hybrid` timed separately) shows both `search_fts` and
`search_vector_text` scaling with corpus size, with the vector side
dominating and growing faster:

| Chunks  | fts    | vector  | hybrid (incl. embed_query) |
| ------: | ------:| -------:| ---------------------------:|
| 5,000   | 14 ms  | 22 ms   | 42 ms |
| 25,000  | 43 ms  | 87 ms   | 144 ms |
| 100,000 | ~170 ms* | ~350 ms* | 533-640 ms (measured) |

(*extrapolated from the 5k/25k points; roughly linear, consistent with
`vec0`'s documented brute-force scan — sqlite-vec's default configuration
has no ANN index, so a KNN query touches every row.)

This is a real scaling limit, not a config knob: at 100k chunks, exhaustive
384-dim vector comparison is the dominant cost and blows the 300 ms warm
budget. Fixing it needs either sqlite-vec's partitioning/quantization
features or a different search-time strategy (pre-filtering, an ANN
index) — out of scope for this slice; tracked here as an open gap rather
than silently passed.

## RSS root-cause and fix

Investigated and partly fixed — see ADR-0005's "Update: RSS root-cause
investigation". Summary: `ort` thread-pool defaults and memory-pattern
settings were **not** the cause (measured, no effect); a duplicated
tokenizer load (`E5Embedder` parsing its own copy of `tokenizer.json`
instead of reusing `chunk::count_tokens`'s shared instance) was, at ~200-260
MB of avoidable RSS. Fixed. New peak RSS for `magi-cli index
fixtures/corpus`: fp32 1,100.2 MB (was 1,302.5 MB), int8 767.1 MB (was
1,024.9 MB) — still both over SPEC.md §7 M3's ≤ 700 MB target, but int8 is
now only 67 MB over (down from 325 MB).

## Not yet done

- A larger, messier corpus to make the hybrid-vs-vector-only comparison
  and the quantization recall comparison above less provisional.
- Switching the manifest default to int8 (now the closer variant on both
  recall and RSS) — needs `e5_parity.rs`'s tolerance revisited per SPEC.md
  §7 M3's "≥ 0.97 if the quantized model is chosen" rule and this eval
  re-run against the new default; not done as a byproduct of the RSS
  investigation (see ADR-0005).
- Vector search's brute-force scaling at 100k+ chunks (above) — needs an
  ANN/partitioning strategy, not addressed here.
