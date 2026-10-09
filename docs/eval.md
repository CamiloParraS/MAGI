# Search-quality eval

recall@k / MRR history, per SPEC.md §7 M3 and §8. Updated whenever the
embedder, ranking, or eval query set changes materially. See `eval/README.md`
for the query-file format and `eval/queries.jsonl` for the 60 queries.

## Reference machine

- AMD Ryzen 7 7445HS (6 cores / 12 threads: 2 Zen 4 + 4 Zen 4c), 16 GB RAM
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

| Mode        |   n | recall@5 | recall@10 |   MRR |
| ----------- | --: | -------: | --------: | ----: |
| fts-only    |  60 |    0.417 |     0.417 | 0.408 |
| vector-only |  60 |    0.983 |     0.983 | 0.818 |
| hybrid      |  60 |    0.983 |     0.983 | 0.807 |

By `lang` (hybrid):

| lang  |   n | recall@5 | recall@10 |   MRR |
| ----- | --: | -------: | --------: | ----: |
| en    |  20 |    1.000 |     1.000 | 1.000 |
| es    |  20 |    1.000 |     1.000 | 1.000 |
| cross |  20 |    0.950 |     0.950 | 0.422 |

**Measured before the ranking path was equalized** (see "Resolved" below):
the `vector-only` row here is raw `search_vector_text` with no filename or
recency boost, while `hybrid` is boosted, so the two columns are not
comparable to each other. They are kept as the fp32 reference point for the
int8 comparison, which is a like-for-like swap of the model file only.

By `lang` (fts-only, for contrast — keyword search has zero cross-lingual
signal by construction):

| lang  |   n | recall@5 | recall@10 |   MRR |
| ----- | --: | -------: | --------: | ----: |
| en    |  20 |    0.550 |     0.550 | 0.550 |
| es    |  20 |    0.700 |     0.700 | 0.675 |
| cross |  20 |    0.000 |     0.000 | 0.000 |

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

| Mode        |   n | recall@5 | recall@10 |       MRR |
| ----------- | --: | -------: | --------: | --------: |
| fts-only    |  70 |    0.486 |     0.486 |     0.486 |
| vector-only |  70 |    0.971 |     0.986 |     0.818 |
| hybrid      |  70 |    0.971 |     0.986 | **0.825** |

By bucket, MRR:

| bucket |   n | fts-only | vector-only |    hybrid |
| ------ | --: | -------: | ----------: | --------: |
| en     |  20 |    0.550 |       0.975 |     0.975 |
| es     |  20 |    0.700 |       1.000 |     1.000 |
| cross  |  20 |    0.000 |       0.413 |     0.413 |
| kw     |  10 |    0.900 |       0.950 | **1.000** |

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

_Superseded 2026-10-08: on the clean corpus both boosts are neutral for
hybrid; see "Pricing the boosts" below._

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

| Metric                           |       Measured | Target (SPEC.md §2.2) |   Result |
| -------------------------------- | -------------: | --------------------: | -------: |
| Cold (model load + first search) | 1,234-1,303 ms |            ≤ 3,000 ms | **PASS** |
| Warm p95 (200 queries)           | 227.5-230.8 ms |              ≤ 300 ms | **PASS** |
| Warm p50 / max                   | ~223 / ~235 ms |                     — |        — |

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
solid: the clean runs of the new code _before_ the harness fix already sat at
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

| Chunks |   fts | vector | hybrid (incl. embed_query) |
| -----: | ----: | -----: | -------------------------: |
|  5,000 | 14 ms |  22 ms |                      42 ms |
| 25,000 | 43 ms |  87 ms |                     144 ms |

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

| Corpus                       | Chunks | Largest batch |     Peak RSS |
| ---------------------------- | -----: | ------------: | -----------: |
| `fixtures/corpus` (37 files) |   ~200 |            39 | **582.9 MB** |
| one 15 MB text file          |  5,716 |            16 | **593.6 MB** |

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

## M4: image queries and the visual list (SigLIP 2, ADR-0007)

**Method:** `magi-cli eval eval/queries.jsonl --corpus fixtures/corpus`, all
real models (int8 e5, PaddleOCR, SigLIP 2 q4f16), fresh temp DB, release
build. The set is now 99 queries: the earlier 70 plus 29 in a new `img`
bucket. `fixtures/corpus` indexed 60 files, with 3 skips and 3 errors. The three
errors are the intentionally broken fixtures (`edge/truncated.jpg`,
`edge/truncated.pdf`, `edge/password_protected.pdf`). The skips are the 20 MP
`.ppm` (over the file-size limit), `edge/bomb.png` (400 MP declared) and
`images/city_landscape.jpg` (75 MP), the last two as `image_too_large`. A Sony
`.arw` RAW in the folder is indexed by filename only: an earlier run of this
eval reported it as an error, because the classifier sniffed its TIFF container
and the TIFF decoder failed (fixed alongside; see docs/progress.md). **The
corpus includes the gitignored local-only fixtures** (`fixtures/README.md`), so
a fresh clone indexes fewer files and its numbers differ slightly. A new
`visual` mode runs `vec_image` alone.

The `img` bucket: 22 visual queries (dogs, cat, car, mountains at sunrise and
sunset, Christmas shelf, anime screenshot, anime figurine; English and Spanish, including the
SPEC.md M4 pair `dog on the beach` / `perro en la playa`), 3 OCR-text queries
(one with accents), and 4 QR queries (including SPEC.md's `qr code` /
`código QR`, where any of the four QR fixtures is a correct hit).

| Mode               |   n |  recall@5 | recall@10 |       MRR |
| ------------------ | --: | --------: | --------: | --------: |
| fts-only           |  99 |     0.434 |     0.434 |     0.434 |
| vector-only (text) |  99 |     0.970 |     0.990 |     0.804 |
| visual-only        |  99 |     0.263 |     0.263 |     0.263 |
| **hybrid**         |  99 | **0.980** | **0.990** | **0.843** |

Hybrid by bucket:

| bucket  |   n |  recall@5 |       MRR |
| ------- | --: | --------: | --------: |
| en      |  20 |     1.000 |     0.975 |
| es      |  20 |     1.000 |     0.925 |
| cross   |  20 |     0.900 |     0.399 |
| kw      |  10 |     1.000 |     0.950 |
| **img** |  29 | **1.000** | **0.966** |

On the `img` bucket alone: fts-only 0.310, visual-only 0.897 (0.963 on the
earlier 27-query set before the cosine floor below), text-vector-only 0.966,
hybrid 1.000. Every image query
lands in the top 5, including SPEC.md's four required ones: `qr code` /
`código QR` return a QR fixture and `dog on the beach` / `perro en la playa`
return a dog photo, all inside the top 3. The visual list earns its place:
without it the text-only modes miss images whose only signal is what they
look like.

**A finding worth keeping: the visual list needs a similarity floor.** The
first hybrid run scored 0.000 on `cross` and 0.55 / 0.70 on `en` / `es`, the
same as keyword-only. A KNN always returns its nearest rows, so every text
query (`car maintenance oil change`) got the whole photo library as visual
hits, and each image, present in both the text-vector and visual lists,
outscored the real document, which sat in one. The fix is a cosine floor on
the visual list, `IMAGE_MIN_COSINE = 0.10`, chosen from measurement: over the
70 text queries the best cosine against any fixture image never exceeded
0.117, while genuine visual matches have a median of 0.139 and the weakest
non-OCR one is ~0.105 (SigLIP's own sigmoid probability is too conservative to
gate on: median 0.25 for true matches). The floor costs the three OCR-text
queries their visual match (visual-only 0.963 -> 0.889), which the OCR chunks
already answer. The threshold belongs to the model: recalibrate it with any
image-model change, and it is only as good as this small fixture set (15
images), so a real photo library needs re-measuring.

`es` MRR is 0.925 against 1.000 before the visual list existed: a leaked
image occasionally takes rank 2 ahead of the expected document. Recall@5 is
unaffected.

Hybrid `cross` recall@5 went from 0.950 (97 queries, 57 files) to 0.900 (99
queries, 60 files): one query dropped out of the top 5 when the corpus grew by
three images. Text-vector-only search dropped by exactly the same amount over
the same change, so this is added distractors, not the visual list. It is one
query of twenty; treat it as noise until the corpus is larger.

**Peak RSS, the whole fixture corpus with all models loaded (NFR-11):
1328 MB** (limit 1.5 GB; 1340 MB on the earlier 57-file run). One process
polled at 50 ms through indexing 60 files (e5 + PaddleOCR + SigLIP vision,
including a real 45 MP JPEG) and all 99 x 4 queries (which loads the SigLIP text
tower, the last +190 MB step). Wall time 1-2 minutes, and not stable between
runs.

## M4: the 2026-09-21 photo batch (113 files, 164 queries)

**Method:** as above (`magi-cli eval`, release build, real models, fresh DB),
now over 113 indexed files, 4 skips and the 3 intentional errors, with 164
queries. New: 51 owner-supplied stock photos, screenshots and QR/barcode
images in `fixtures/corpus/local/` (gitignored, so this only runs on a machine
that has them), and three phone shots of toy cars. New buckets: `img2` (54
visual queries), `ocr` (7), `qr` (3) and `skip` (1); see `eval/README.md`.
Every expected answer was written from looking at the image, not its file name
(`mae-mu-burguer.jpg` is pancakes). **File names leak into the results**: the
`visual` row, which has no filename and no OCR, is the clean measure of the
image model; hybrid is what a user sees.

| Mode               |   n |  recall@5 | recall@10 |       MRR |
| ------------------ | --: | --------: | --------: | --------: |
| fts-only           | 164 |     0.299 |     0.299 |     0.295 |
| vector-only (text) | 164 |     0.866 |     0.933 |     0.710 |
| visual-only        | 164 |     0.494 |     0.494 |     0.486 |
| **hybrid**         | 164 | **0.957** | **0.988** | **0.867** |

Hybrid by bucket:

| bucket                |   n |  recall@5 |   MRR | visual-only recall@5 |
| --------------------- | --: | --------: | ----: | -------------------: |
| en                    |  20 |     1.000 | 0.967 |                    - |
| es                    |  20 |     1.000 | 0.925 |                    - |
| kw                    |  10 |     1.000 | 0.925 |                    - |
| cross                 |  20 | **0.750** | 0.312 |                    - |
| img (first batch)     |  29 |     1.000 | 0.938 |                0.897 |
| **img2 (new photos)** |  54 | **0.981** | 0.954 |            **0.963** |
| ocr                   |   7 |     1.000 | 1.000 |                0.429 |
| qr                    |   3 |     0.667 | 0.700 |                0.000 |
| skip                  |   1 |     1.000 | 1.000 |                0.000 |

**The image model holds up on a bigger, more confusable set.** 53 of 54 new
visual queries land in the top 5 through hybrid, and 52 of 54 through the
visual list alone, with the confusable clusters in play: three burgers and
pancakes and a hot dog, five trucks and three buses and two trains, a wolf
beside dog photos, four forests, three rooms, three guitars. The Spanish
queries (`hamburguesa con queso`, `un camión`, `bosque nevado`, ...) behave
like the English ones. The one miss is `a Hot Wheels package` (rank > 100, top
hit a hot dog): SigLIP does not know the brand from a photo, and the printed
text is reached through OCR instead (`Hot Wheels Nissan Z Proto` finds it).
The query was kept, not reworded after the fact.

**A regression to look at: `cross` fell from 0.900 to 0.750.** Text-vector-only
search fell by the same amount, so it is not the visual list. Growing the
corpus from 60 to 113 files added 100+ chunks to `vec_text` (a filename chunk
per file plus OCR text), and short ones such as `piotr-szajewski-snowy-forest.jpg`
or `MON TUE WED THU FRI TWITTER...` sit at ranks 3-4 for unrelated queries,
pushing the other-language twin from rank <= 5 to 6-10. The five misses are
`cross` queries whose expected document is the twin of the one that ranks first
(`team meeting notes` expects the Spanish notes while the English original is
rank 1), so they are the most rank-sensitive queries in the set; recall@10 is
unchanged at 0.950. The `en`, `es` and `kw` buckets are unaffected (1.000).
Not fixed: it is a ranking-design question (weight filename chunks lower? skip
them when a file has real text?) and the corpus is still small.

**The 0.10 cosine floor is not as clean as it was on 15 images.**
`team meeting notes` and `doctor appointment reminder` now pull
`walls-io-whiteboard.jpg` (a weekly schedule on a whiteboard) into the results
labelled `visual`. That is a defensible match, not a clear false positive, but
it means "text queries never exceed the floor" no longer holds. The floor was
not changed.

**Peak RSS, all models loaded, the whole 113-file corpus: 1271-1348 MB across
two runs** (limit 1.5 GB), including a 50 MP JPEG.

### OCR quality against `fixtures/golden/ocr/fixture_stem.txt`

Character error rate after collapsing whitespace, computed from the indexed
`ocr` chunks:

| image                          |      CER | note                                              |
| ------------------------------ | -------: | ------------------------------------------------- |
| barcode "Hello World!"         |     0.00 |                                                   |
| code screenshot (Rust)         |     0.01 |                                                   |
| Wikipedia article screenshot   |     0.09 | `¿` `¡` lost in the running text                  |
| war-grave headstone            |     0.13 | engraved text on stone                            |
| handwriting-style Spanish page |     0.29 | `¡Hola!` came back as `iHola!`: the known `¡` gap |
| Pepsi can, curved label        | **0.92** | read only "PERS N"                                |

### QR and barcodes

- `3_barcodes_ver1_ver2_ver3.png`: all three codes decode, to `Ver1`,
  `Version 2` and `Version 3 QR Code`. OpenCV, an independent decoder, returns
  the same three. The owner's note in `fixture_stem.txt` says `ver1/ver2/ver3`,
  which is shorthand, not the payload.
- **The grave photo's QR is not decoded.** OpenCV reads it from a cropped,
  enlarged region (`http://en.qrwp.org/Adrian_Warburton`, a QRpedia link), and
  so does our decoder on the same 640 x 306 crop, but neither finds it in the
  full 1600 x 2035 frame. The code is ~100 px in a busy scene. `qr.rs`'s scale
  ladder only downsizes (1, 1/2, 1/4, 1/8), which helps a large code and hurts a
  small one. **Fixed afterwards** by native-resolution tiling (docs/progress.md, slice 8); the `qr` numbers in the table above are from before that.
- The Pepsi can's QR is not decoded by ours or by OpenCV (curved, ~perspective).
- The 1D barcode `barcode-Hello World!.png` decodes with neither rxing nor
  OpenCV; only its printed text is found, by OCR.
- The first version of the `qr` queries used `ver1`, `ver2`, `ver3` and
  `wikipedia`, all of which are in the file names, and passed for the wrong
  reason. They were replaced by payload-only words.

## M4: the `cross` regression, its cause and its fix

The 2026-09-21 photo batch cost `cross` recall@5 0.900 -> 0.750 (above). Text-vector-only
search fell equally, so the cause was in `vec_text`. **Cause: OCR on photos with no
text.** PaddleOCR emits a stray glyph or two for a photo of a forest or a pizza
(`.`, `M`, `2`, `000020`, `TUABO`), and each became an `ocr` chunk embedded as
`passage: .`. A chunk that short sits near almost any short query, so it outranks
real documents. In the pre-fix index 23 of the 60 OCR chunks had fewer than 6
letters and digits, e.g. the only OCR chunk of `piotr-szajewski-snowy-forest.jpg`
was a single full stop, which is why that photo kept turning up as a "semantic"
hit for `team meeting notes`.

**Fix:** `extract_image` drops OCR output with fewer than 6 letters and digits
(`MIN_OCR_ALNUM`). Same corpus, same 164 queries, release build:

|                           | cross r@5 | overall hybrid r@5 |   MRR | img / img2 / ocr / qr         |
| ------------------------- | --------: | -----------------: | ----: | ----------------------------- |
| before (no filter)        |     0.750 |              0.957 | 0.867 | 1.000 / 0.981 / 1.000 / 0.667 |
| threshold 4               |     0.850 |              0.976 | 0.880 | 1.000 / 0.981 / 1.000 / 1.000 |
| **threshold 6 (shipped)** | **0.900** |          **0.982** | 0.880 | 1.000 / 0.981 / 1.000 / 1.000 |

(`qr` 0.667 -> 1.000 is the separate tiled-QR fix.) `cross` is back at its
pre-batch 0.900; the two remaining misses are `informe de ingresos trimestrales`
(rank > 100) and `doctor appointment reminder` (rank 9). Threshold 4 to 6 is one
query, so treat the choice between them as a judgement, not a measurement: 4-5
character OCR output in this corpus is all junk but one partial read (`PERS N`
off a can). The cost is real short text alone in a photo (a sign reading `EXIT`),
which is no longer indexed; the threshold is a marked constant.

**What did not work, so nobody tries it again.** The first hypothesis was that
image _file name_ chunks were the crowders. Splitting the vector list and
down-weighting image-filename-best hits (weights 0.5 and 0) left `cross` at
0.750 and hurt the image buckets: at weight 0 the `img` bucket collapsed
(`a dog on the beach` fell to rank 9, `perro en la playa` out of the top 100).
Down-weighting **every** filename-best hit (weight 0) was far worse for text:
`electrician invoice`, `receta de arepas`, `contrato de alquiler deposito` and
most of `cross` were lost, because for a text document the file name is often
the best cross-language bridge (`receta_arepas` <-> "arepas recipe"). The
filename chunk is doing real work and should stay as the spec says.

**Still true:** `team meeting notes` and `doctor appointment reminder` pull
`walls-io-whiteboard.jpg` (a weekly schedule) in through the visual list, and the
0.10 cosine floor was calibrated on 15 images. Not changed.

## 2026-10-08: one tokenizer for the filename boost, and a baseline that fell

`filename_boost` split the query on whitespace only and the file name on
non-alphanumerics, without camelCase or accent folding, so `budget_report`,
`invoice,`, `irradiacion solar` (vs `irradiacionSolar.py`) and `cancion` (vs
`canción.mp3`, composed or macOS-decomposed) got no boost. Both sides now go
through `fuse::match_words`. Same 164 queries, release build, real models,
fresh DB, run back to back on the same machine:

| Mode        | r@5 before → after | MRR before → after |
| ----------- | -----------------: | -----------------: |
| fts-only    |      0.311 → 0.311 |      0.202 → 0.202 |
| vector-only |      0.848 → 0.848 |      0.579 → 0.585 |
| visual-only |      0.488 → 0.488 |      0.367 → 0.381 |
| **hybrid**  |  **0.951 → 0.951** |  **0.730 → 0.733** |

Small and one-directional: no recall changes, the hybrid miss list is
identical, the only dip is vector-only `img` MRR 0.606 → 0.601. `cross` is
untouched (the boost is still language-blind).

**Resolved: the "before" column was a polluted corpus, not a regression.**
Both columns above sit well below the last recorded state (hybrid r@5 0.982 /
MRR 0.880) because this machine's `fixtures/corpus/` also held
`random_names/`: 126 local, untracked files (`.git/info/exclude`), 125 of them
byte-identical copies of other fixtures under random names, made on
2026-09-29 for a manual test (docs/progress.md). No query expects them, so
each copy that outranked its original pushed the expected file to rank 2:
recall unchanged, MRR roughly halved (`kw` 0.550, `en` 0.727). Newer mtimes
gave the copies a slightly larger `recency_boost` too.

Same build (with the tokenizer change), same 164 queries, on a copy of the
corpus without `random_names/` (`cp` preserving mtimes, 115 indexed):

| Mode        | recall@5 |   MRR |
| ----------- | -------: | ----: |
| fts-only    |    0.311 | 0.307 |
| vector-only |    0.884 | 0.722 |
| visual-only |    0.494 | 0.474 |
| **hybrid**  |    0.982 | 0.881 |

Hybrid by bucket, MRR: `en` 0.975, `es` 0.925, `kw` 1.000, `cross` 0.363,
`img` 0.940, `img2` 0.944, `ocr` / `qr` 1.000. The misses are the three
recorded after the OCR fix (`informe de ingresos trimestrales`, `doctor
appointment reminder`, `a Hot Wheels package`). The tokenizer table above is
still a valid before/after pair (both runs saw the same corpus), but its
absolute numbers are not comparable with the rest of this file.

**Keep `fixtures/corpus/` to the tracked fixtures plus `local/`**: anything
else in it is indexed by `magi-cli eval` and scored against.

## 2026-10-08: keyword search, OR-ed words rejected, last-word prefix kept

Same build, clean corpus (`random_names/` moved to `fixtures/random_names/`),
164 queries, against the clean baseline above (hybrid r@5 0.982 / MRR 0.881).

**OR-ing the query words (plus a last-word prefix): rejected.** fts-only
improved a lot (r@5 0.311 -> 0.707, `en`/`es` MRR 0.550/0.700 -> 1.000), but
hybrid fell: r@5 0.982 -> **0.890**, MRR 0.881 -> **0.773**; `cross` r@5
0.900 -> 0.600, `img` / `img2` 1.000 / 0.981 -> 0.897 / 0.870, `kw` MRR
1.000 -> 0.900. Small words (`a`, `de`, `with`, `of`) match nearly every
chunk, so the keyword list fills to 100 files of noise and RRF credits each
of them; `a stack of pancakes with syrup and walnuts` then ranks a photo of
paper pages first. Making OR work would need a stopword list per language or
a BM25 floor, each a tuning problem of its own. Every word stays required
(`fts::tests::a_chunk_missing_a_word_is_not_a_keyword_match` pins it).

**Last word as a prefix (`"receta" "arep"*`): kept.** Byte-identical eval
output to the baseline, every bucket and the miss list: the queries are
complete words, so only search-as-you-type gains (`arep` finds `arepas`
before the word is finished).

## 2026-10-08: random file names (`just eval-random`)

The case magi exists for: the same files, but named `29ab99be9520eacd.txt`.
`fixtures/random_names/` (local, byte-identical copies of the corpus) with
`eval/random_names.jsonl` (the same queries, each answer swapped for its copy;
`skip` left out, 163 queries). Same build as the prefix change above, release,
real models. The other column is the clean `just eval` run above (164 queries,
including the one `skip` query, which hits).

| Hybrid, by bucket | named r@5 / MRR | random names r@5 / MRR |
| ----------------- | --------------: | ---------------------: |
| en                |   1.000 / 0.975 |          1.000 / 0.975 |
| es                |   1.000 / 0.925 |          1.000 / 0.925 |
| kw                |   1.000 / 1.000 |          1.000 / 1.000 |
| **cross**         |   0.900 / 0.363 |      **0.400 / 0.215** |
| img               |   1.000 / 0.940 |          0.862 / 0.833 |
| img2              |   0.981 / 0.944 |          0.944 / 0.886 |
| ocr / qr          |   1.000 / 1.000 |          1.000 / 1.000 |
| **overall**       | **0.982 / 0.881** |    **0.883 / 0.824** |

- **Same-language text loses nothing.** `en`, `es` and `kw` are identical:
  content carries them.
- **Photos mostly hold, through the visual list.** visual-only `img2` is
  0.963 r@5 either way; the text-vector list drops from 0.815 to 0.130 there,
  so for photos it was finding the file name, not the picture.
- **Cross-lingual search collapses: r@5 0.900 -> 0.400.** With a name, the
  filename chunk is the bridge (`receta_arepas.txt` for "arepas recipe", as
  the `cross` fix above found). Without it, e5 alone puts the other-language
  document in the top 5 for 8 of 20 queries. This is the largest gap on the
  use case magi is for, and the next ranking target.

## 2026-10-08: pricing the boosts (clean corpus)

Each boost switched off in a throwaway build, everything else as shipped,
the 164 queries over the clean `fixtures/corpus`:

| Hybrid                | r@5   | MRR       | cross MRR | es MRR | img / img2 MRR |
| --------------------- | ----- | --------- | --------- | ------ | -------------- |
| both boosts (shipped) | 0.982 | **0.881** | 0.363     | 0.925  | 0.940 / 0.944  |
| no `filename_boost`   | 0.982 | 0.880     | 0.380     | 0.900  | 0.957 / 0.935  |
| no `recency_boost`    | 0.982 | 0.881     | 0.363     | 0.925  | 0.940 / 0.944  |

- **`recency_boost` does nothing here**: hybrid output is identical without
  it. The fixtures' mtimes span a few weeks of checkouts, so this corpus
  cannot price it; it is for a real library with years of mtimes. Kept as the
  spec says.
- **`filename_boost` is neutral overall** (+0.001 MRR) and trades buckets:
  +0.025 on `es`, +0.009 on `img2`, -0.017 on `cross`, -0.017 on `img`. The
  -0.018 of the open finding above was measured before the tokenizer fix and
  on 70 queries; it does not hold now. Kept.
- **`cross` MRR is not a boost problem.** No boost lifts it past 0.380, and
  every one of the 20 `cross` queries has a same-language twin in the corpus
  (`team meeting notes` -> `en/meeting_notes.txt`, ahead of the expected
  Spanish notes), so a correct ranking puts the expected file second: MRR
  sits near 0.5 by design. The `cross` gap that matters is recall without names
  (0.400 above), which no boost can touch: with random names
  `filename_boost` has nothing to match and the copies share one mtime.

## 2026-10-09: realistic file names, and the opaque-name fix rejected

The hex names above are not what a user's folders look like. Real ones are
`IMG_0042.JPG`, `DSC_4301.JPG`, `PXL_20240827_...jpg`, WhatsApp's
`IMG-20240711-WA0967.jpg`, `Screenshot_...png`, `New Text Document (3).txt`,
`Book3.xlsx`, with some hex and UUID downloads. `tools/realistic_names.py`
copies `fixtures/random_names/` (same 126 files, same bytes) to
`fixtures/realistic_names/` under a seeded mix of those;
`just eval-random realistic_names` runs it (163 queries).

**The opaque-name fix** (tried, not committed): no `vec_text` row for a
filename chunk with no word in it (`has_words`: no run of 3+ letters other
than hex digits and the extension). Hybrid, release, real models:

| Hybrid r@5 / MRR     |     overall | cross | img   | img2  |
| -------------------- | ----------: | ----: | ----- | ----- |
| hex names            | 0.883 / 0.824 | 0.400 | 0.862 | 0.944 |
| hex + fix            | 0.613 / 0.535 | 0.650 |       | 0.370 |
| **realistic names**  | **0.902 / 0.834** | **0.600** | 0.931 | 0.889 |
| realistic + fix      | 0.847 / 0.773 | 0.650 | 0.793 | 0.778 |

(A cosine floor of 0.80 on the text-vector list, on top of the fix, brought
hex names to 0.804 / 0.629 but cost the named corpus 0.982 -> 0.957; rejected.)

- **The hex corpus overstated the `cross` gap.** With realistic names `cross`
  is 0.600, not 0.400, and the misses are not crowding: all 8 have their
  same-language twin as the top hit, and vector-only `cross` recall@10 equals
  hybrid's (0.650). It is e5's cross-lingual reach against a twin, which no
  fusion change touches.
- **The fix is a net loss either way** (-0.055 r@5 on realistic names). Every
  photo's junk name vector puts it in the text-vector list as well as the
  visual one, and RRF rewards being in two lists over being #1 in one. The fix
  takes that away from some photos (hex, UUID, `20240107_094959.jpg`) and not
  from others (`IMG_`, `DSC_`, `PXL_` count as words), so the ones that keep it
  outrank the ones that lost it: `a double-decker bus` now ranks `IMG_0494.JPG`
  first. Rejected; `filename_chunk` stays embedded for every file, as the spec
  says.
- Raising the visual list's weight was the next candidate, to let a visual #1
  beat text-vector junk. With the fix gone every photo is in both lists again,
  so it would be tuning against the accident rather than removing it; not run.

## Not yet done

- A larger, messier corpus. No longer the blocker for M3 item 4 — the
  comparison discriminates now — but the `kw` bucket is still 10
  hand-written queries over synthetic fixtures, and the quantization recall
  comparison stays provisional until the corpus has real ambiguity and
  near-duplicates in it.
- Vector search's brute-force scaling beyond 100k chunks (above) — 100k
  now passes with headroom, but the growth is linear, so a much larger
  corpus will need sqlite-vec's partitioning/quantization or an
  ANN/pre-filter strategy.
- Splitting the 585 → 228 ms warm-p95 improvement between the
  `hybrid_search` fix and the harness's WAL checkpoint, by re-running the
  pre-fix code against the fixed harness.
