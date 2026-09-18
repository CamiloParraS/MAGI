# Search-quality eval

recall@k / MRR history, per SPEC.md §7 M3 and §8. Updated whenever the
embedder, ranking, or eval query set changes materially. See `eval/README.md`
for the query-file format and `eval/queries.jsonl` for the 60 queries.

## Reference machine

- AMD Ryzen 7 7445HS (8 cores / 16 threads), 16 GB RAM
- Windows 11 Home Single Language 10.0.26200, x86_64
- `magi-cli eval` built with `cargo build --release -p magi-cli`

## Baseline: fp32 `intfloat/multilingual-e5-small`

**int8 is the manifest's shipped default now** (ADR-0005) — this baseline
is fp32 anyway because it's `tools/reference_embeddings.py`'s reference
implementation and the fixed point everything else (int8's own numbers,
below) is compared against. See "Quantization: int8 vs. fp32" below for
the variant actually shipped.

**Method:** `magi-cli eval eval/queries.jsonl --corpus fixtures/corpus`,
real `E5Embedder` (fp32 `model.onnx`, swapped in for this baseline
measurement only — see `models/manifest.toml`), fresh temp DB.
`fixtures/corpus` indexed 37 files
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

**Measured before the ranking path was equalized** (see "Resolved" below):
the `vector-only` row here is raw `search_vector_text` with no filename or
recency boost, while `hybrid` is boosted, so the two columns are not
comparable to each other. They are kept as the fp32 reference point for the
int8 comparison, which is a like-for-like swap of the model file only.

By `lang` (fts-only, for contrast — keyword search has zero cross-lingual
signal by construction):

| lang  |  n | recall@5 | recall@10 |   MRR |
| ----- | -:| -------:| --------:| -----:|
| en    | 20 |   0.550 |    0.550 | 0.550 |
| es    | 20 |   0.700 |    0.700 | 0.675 |
| cross | 20 |   0.000 |    0.000 | 0.000 |

### Resolved: item 4's comparison was mis-specified, not hybrid

**The earlier version of this section was wrong about the mechanism.** It
attributed hybrid's MRR deficit to "RRF fusion plus the filename/recency
boosts" on the 20 `cross` queries, where FTS returns nothing. RRF's share of
that is exactly zero, and provably so: `reciprocal_rank_fusion`
(`crates/magi-core/src/search/fuse.rs`) adds `weight / (60 + rank)` per list,
which is strictly decreasing in rank, so fusing one non-empty list with one
empty list is **order-preserving**. It cannot demote anything. This is now
pinned by a unit test
(`fuse::tests::fusion_with_one_empty_list_preserves_the_other_order`).

The actual cause was in the harness, not the product: `magi-cli eval`'s
`vector` arm called raw `search_vector_text` with **no boosts**, while
`hybrid` applied `filename_boost * recency_boost`. The two modes were
different ranking functions, so the comparison could not isolate fusion — it
was measuring the boosts and calling the result a fusion verdict.

**Fix:** `search::rank_and_boost` now owns the fuse-boost-sort tail, and all
three eval modes call it — the baselines are hybrid's own ranking function
with one input list emptied. `hybrid_search` is unchanged in behaviour; it
calls the same helper. The baselines also now fetch `FTS_FETCH_LIMIT` /
`VECTOR_FETCH_LIMIT` candidates before truncating to 10, as hybrid does,
instead of fetching only 10.

Separately, the query set could not discriminate: all 60 queries were ones
vector-only wins. 10 `kw` (keyword-decisive) queries were added against
fixtures that already existed — code symbols (`Calculadora sumar`,
`struct Point origin`, `irradiacionSolar potenciaPanel`), Office cell and
slide text (`Gadget Warehouse B`, `Quarterly Kickoff`), and exact phrases
(`circuit breaker grounding wire`). The corpus was **not** grown: the axis
that discriminates is query type, not document count.

### Current baseline: int8, 70 queries, equalized ranking path

`just eval`, reference machine above, shipped int8 model, fresh temp DB,
`fixtures/corpus` indexed 37 files (same 2 intentional errors as above).

| Mode        |  n | recall@5 | recall@10 |   MRR |
| ----------- | -:| -------:| --------:| -----:|
| fts-only    | 70 |   0.486 |    0.486 | 0.486 |
| vector-only | 70 |   0.971 |    0.986 | 0.818 |
| hybrid      | 70 |   0.971 |    0.986 | **0.825** |

By bucket, MRR:

| bucket |  n | fts-only | vector-only | hybrid |
| ------ | -:| -------:| ----------:| -----:|
| en     | 20 |    0.550 |       0.975 |  0.975 |
| es     | 20 |    0.700 |       1.000 |  1.000 |
| cross  | 20 |    0.000 |       0.413 |  0.413 |
| kw     | 10 |    0.900 |       0.950 | **1.000** |

**SPEC.md §7 M3 item 4 now passes, on a comparison that can fail.**

- **(a) No regression:** hybrid overall recall@5 0.971 = max(0.486, 0.971). ✅
- **(b) Each mode contributes:** `kw` MRR hybrid 1.000 > vector-only 0.950;
  `cross` hybrid 0.413 > fts-only 0.000. ✅ Overall MRR also now shows hybrid
  strictly ahead of both (0.825 > 0.818 > 0.486), which the old measurement
  had inverted.

The `kw` bucket is the informative half: both modes there run identical
boosts, so hybrid's +0.050 MRR over vector-only is **fusion alone**. The
`cross` half is near-automatic (fts-only scores 0.000 by construction) and
should not be read as strong evidence.

No RRF weight or boost constant was changed. The improvement is entirely
from measuring the right thing.

### Open finding: the filename/recency boosts look net-negative

Now that baselines run boosted, the boosts can be priced. On the original 60
queries, vector-only's int8 MRR was 0.814 unboosted (the figure recorded in
`docs/progress.md` before this change) and is 0.796 boosted — the boosts **cost** ~0.018 MRR, and hybrid was already paying it.
That is most of what the old section misread as a fusion problem.

`filename_boost` is the acting term: `recency_boost` varies by only ~0.3%
across these fixtures (their mtimes span 1.03 days inside a 30-day window),
while `filename_boost` swings up to ×1.2. It is language-blind token
overlap, so an English query can hand ×1.2 to a wrong English-named file and
demote a correct Spanish hit — a plausible mechanism for the `cross` bucket's
0.413 MRR against 0.900 recall@5 (right file in the top 5, wrong rank).

**Not fixed here, deliberately.** The boosts help on `kw` (that is where
filename overlap is a real signal) and the honest next step is to price them
per bucket rather than tune a constant against 70 queries. Tracked, not
resolved.

## Quantization: int8 vs. fp32, and peak RSS

Same eval, int8 `model_qint8_avx512_vnni.onnx` swapped in for
`model.onnx`: vector-only/hybrid recall@5 drops from 0.983 to 0.967 (1.6
points — within SPEC.md §7 M3's 2-point allowance).

Peak RSS of `magi-cli index fixtures/corpus` (37 files; Windows
`PeakWorkingSet64`, reference machine above), at the time of this initial
comparison (before the tokenizer-duplication fix below): **fp32 1,302.5 MB,
int8 1,024.9 MB** — neither met the ≤ 700 MB target. Full analysis in
ADR-0005.

**Decision executed**: `models/manifest.toml` now ships int8 as the
default (fp32's hash kept in a comment for rollback). `e5_parity.rs`'s
tolerance was lowered to `0.97` per SPEC.md §7 M3's quantized-model rule
and re-verified against the real int8 model: worst-case cosine vs. the
fp32 Python reference is **0.9953**. The recall numbers above and the
cross-lingual smoke test were re-run against int8 as the actual shipped
file and are unchanged (0.967/0.967, smoke test passes). Current peak RSS
with the tokenizer fix applied: **766.4 MB** (see "RSS root-cause and fix"
below) — 66 MB over target, the closest measurement so far.

## Latency (NFR-2/NFR-3) on 100k synthetic chunks

`xtask bench-corpus` (`just bench`): 5,000 synthetic files × 20 chunks =
100,000 chunks, random text/vectors (query side uses the real `E5Embedder`
— see the module doc comment in `xtask/src/bench_corpus.rs` for why
synthetic content is fine here but wouldn't be for a recall eval), same
reference machine as above.

**Both targets now pass.** Six consecutive clean runs, reference machine
above, int8:

| Metric                            |        Measured | Target (SPEC.md §2.2) | Result |
| --------------------------------- | --------------:| ----------------------:| ------:|
| Cold (model load + first search)  | 1,234-1,303 ms |               ≤ 3,000 ms | **PASS** |
| Warm p95 (200 queries)            |  227.5-230.8 ms |                 ≤ 300 ms | **PASS** |
| Warm p50 / max                    | ~223 / ~235 ms |                       — | — |

Per-run p95: 227.5, 228.6, 227.7, 227.8, 230.8, 229.6 ms — a 3.3 ms spread.

**Re-verified for M3 sign-off** (same machine, current branch): cold
**1,443.7 ms**, warm **p50 220.0 / p95 230.0 / max 300.9 ms**. Both targets
pass. The p95 lands inside the six-run band above; cold is ~140 ms above it
because this run started immediately after a `cargo build --release` of
`xtask`, which is exactly the contention the measurement-hygiene note below
warns about. Recorded as measured rather than re-run until it looked better —
it passes with 1.5 s of headroom either way.

### How the earlier FAIL was resolved, and what not to read into it

The previously recorded numbers (cold 3,176 ms, warm p95 585 ms) attributed
the failure to `vec0`'s brute-force KNN scan. That diagnosis was incomplete.
Two things were actually wrong, one in the product and one in this harness:

1. **`hybrid_search` issued a `SELECT ... FROM files WHERE id = ?` per fused
   result** — up to 200 per query — plus two linear scans per result, to
   recover metadata both ranked lists had already joined. Removing it (both
   searches now return `search::FileHit`) is the product-side fix.
2. **`bench-corpus` measured reads against an unchecked-pointed WAL.**
   Bulk-loading 100k rows through 5,000 transactions leaves a large
   write-ahead log that every reader consults, and SQLite's own checkpoint
   landing inside or outside the measured window made warm p95 **bimodal for
   identical code**: 231, 239, 423, 436, 478 ms across five runs. The harness
   now runs `PRAGMA wal_checkpoint(TRUNCATE)` after building, before
   measuring — a real index isn't mid-bulk-load when a query arrives. After
   that change the same five runs land within 1.1 ms of each other.

**Attribution caveat, stated rather than glossed:** the old 585 ms was
measured with the unfixed harness, so it was itself partly WAL noise and the
585 → 228 ms delta cannot be cleanly split between the two fixes. What is
solid: the clean runs of the new code *before* the harness fix already sat at
231-239 ms, so the product-side fix carries most of it, and the current code
under a fixed harness passes both targets repeatably. Re-running the old code
against the fixed harness would settle the split; not done.

**Measurement hygiene:** a run started immediately after a `cargo` build is
not usable — compile contention and a cold page cache put it at p95 488 ms
while settled runs of the same binary sit at 228 ms. Let the machine idle
before measuring.

**Still true about scaling:** `embed_query` is ~13 ms/call and not the
bottleneck; `search_vector_text` remains a brute-force scan (`vec0` has no
ANN index in its default configuration) and still grows with corpus size:

| Chunks  | fts    | vector  | hybrid (incl. embed_query) |
| ------: | ------:| -------:| ---------------------------:|
| 5,000   | 14 ms  | 22 ms   | 42 ms |
| 25,000  | 43 ms  | 87 ms   | 144 ms |

100k chunks now fits inside the 300 ms budget with ~70 ms of headroom, but
the growth is real, so a corpus several times larger will need sqlite-vec's
partitioning/quantization or an ANN/pre-filter strategy. That is an M-later
concern, not an M3 failure.

## RSS root-cause and fix

Investigated and fixed — see ADR-0005's "Update: RSS root-cause
investigation" and "Update: RSS target closed (CPU memory arena)". Two
independent causes, both real `ort`/tokenizer configuration, neither
per-chunk work: `ort`'s thread-pool defaults and memory-pattern settings
were **not** the cause (measured, no effect); a duplicated tokenizer load
(`E5Embedder` parsing its own copy of `tokenizer.json` instead of reusing
`chunk::count_tokens`'s shared instance), at ~200-260 MB, and the CPU
execution provider's memory arena allocator (a pool sized for the largest
batch, held for the session's lifetime), at ~84 MB, were. Both fixed. Peak
RSS for `magi-cli index fixtures/corpus`, int8: 1,024.9 MB → 767.1 MB
(tokenizer fix) → **682.8 MB** (arena fix) — **SPEC.md §7 M3's ≤ 700 MB
target is now met**, a 342 MB (33%) reduction from the original
measurement.

## Peak RSS is now independent of file size

The measurements above were all taken against `fixtures/corpus`, whose
largest file produces **39 chunks**. `index_root` handed every chunk of a
file to `embed_passages` in one call, and ONNX Runtime materializes a
`batch × seq_len × 384` f32 `last_hidden_state` for the whole batch, so peak
memory scaled with a file's chunk count — a dimension the fixture corpus
never exercised. A 15 MB plain-text file (well inside the default
`max_file_size_mb = 50`) indexes to **5,716 chunks**, ~146× the largest
fixture batch; at that size the intermediate tensor alone is ~4.5 GB.

`E5Embedder::embed_passages` now batches in groups of 16
(`BATCH_CHUNKS`), `embed_passages` takes `&[&str]` instead of `&[String]`,
and the tokenizer no longer re-copies its inputs — three copies of a file's
text before inference became one. Re-measured the same way (Windows
`PeakWorkingSet64`, polled every 50 ms, reference machine above), int8:

| Corpus                                  | Chunks | Largest batch |   Peak RSS |
| --------------------------------------- | -----:| ------------:| ---------:|
| `fixtures/corpus` (37 files)             |    ~200 |            39 |  **582.9 MB** |
| one 15 MB text file                      |   5,716 |            16 |  **593.6 MB** |

**Re-verified for M3 sign-off**: `fixtures/corpus` peaks at **581.9 MB**
(1.0 MB under the 582.9 MB above — noise), measured the same way against an
isolated `MAGI_DATA_DIR` holding only the int8 model. The 15 MB single-file
case was **not** re-run: it takes ~23 minutes and nothing since has touched
`BATCH_CHUNKS` or the embed path.

`fixtures/corpus` dropped from 682.8 MB to 582.9 MB, and the 15 MB
single-file case — previously unbounded — now peaks in the same band.
SPEC.md §7 M3's ≤ 700 MB target holds on both, and no longer only because
the corpus happens to be small.

The 15 MB file took ~1360 s wall on the reference machine (int8,
`with_intra_threads(1)`, one dedicated embed thread per SPEC.md §5.3), i.e.
~4 chunks/s. That throughput is the next thing to look at, not memory.

## Not yet done

- A larger, messier corpus. No longer the blocker for M3 item 4 — the
  comparison discriminates now — but the `kw` bucket is still 10
  hand-written queries over synthetic fixtures, and the quantization recall
  comparison stays provisional until the corpus has real ambiguity and
  near-duplicates in it.
- Pricing the filename/recency boosts per bucket (see the open finding
  above): they cost ~0.018 MRR on `en`/`cross` and help on `kw`.
- Vector search's brute-force scaling beyond 100k chunks (above) — 100k
  now passes with headroom, but the growth is linear, so a much larger
  corpus will need sqlite-vec's partitioning/quantization or an
  ANN/pre-filter strategy.
- Splitting the 585 → 228 ms warm-p95 improvement between the
  `hybrid_search` fix and the harness's WAL checkpoint, by re-running the
  pre-fix code against the fixed harness.
