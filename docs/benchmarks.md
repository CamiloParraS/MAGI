# Benchmarks

Measured evidence for SPEC.md §2.2's NFR targets and §7's milestone
throughput requirement. Updated as each milestone adds a measurable
capability; only M2 (discovery + text extraction, no ML) is covered so far.

## Reference machine

- AMD Ryzen 7 7445HS (6 cores / 12 threads: 2 Zen 4 + 4 Zen 4c), 16 GB RAM
- Windows 11 Home Single Language 10.0.26200, x86_64
- `magi-cli` built with `cargo build --release` (commit at time of
  measurement: `feat/text-extraction`, post-M2 slice 3)

## M2 — indexing throughput (files/s per type)

**Method:** each file type's fixture(s) from `fixtures/corpus/<type>/` were
copied under unique names to reach a larger, steadier sample (25 copies of
each source file — `fixtures/corpus/office` has 3 source files, so 75
total), then indexed in one `magi-cli index <root>` run against a fresh,
empty `MAGI_DATA_DIR`, timed end-to-end (process start to exit). This
dilutes the fixed ~85 ms process/DB-open/migration overhead (measured by
timing a single-file run per type) across many files, giving a rate closer
to steady-state than a bare 1-3-file run would.

| Type      | Files indexed | Wall time |   Throughput |
| --------- | ------------: | --------: | -----------: |
| code      |            50 |    153 ms | ~327 files/s |
| pdf       |            50 |    221 ms | ~226 files/s |
| office    |            75 |    190 ms | ~395 files/s |
| text (en) |            25 |    112 ms | ~223 files/s |
| text (es) |            25 |    124 ms | ~202 files/s |

All runs report `skipped: 0, errors: 0` — every copy indexed successfully.

**Caveats:**

- The underlying fixture corpus is small (1-3 distinct files per type,
  a few KB to low hundreds of KB each) and license-clean by design (SPEC.md
  §5.1); these numbers are indicative of per-file-type extractor cost on
  small files, not a substitute for the NFR-2/NFR-4 reference-machine
  benchmarks (which need realistic file-size and corpus-size distributions
  and land with M3's search latency work and later crash/watch milestones).
  No embedding/ML work runs yet (M2 has no ML), so these numbers only cover
  walk → classify → extract → chunk → FTS-index.
- PDF and code extraction pay one-time setup costs (PDFium binding,
  tree-sitter grammar loading) on the first file of their kind per process;
  amortized here across 50 files each, but a cold single-file run is
  slower (see below).

### Single-fixture-count reference (no replication, 3 trials each)

Included for comparison — shows the fixed per-run overhead that the
replicated numbers above dilute away:

| Type   | Files | Trials (ms)   | Notes                                                                                                                                                                                                                                                                                                                                                         |
| ------ | ----: | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| code   |     2 | 92, 87, 89    | `code/sample.rs`, `code/muestra.py`                                                                                                                                                                                                                                                                                                                           |
| pdf    |     2 | 92, 96, 100   | `pdf/report.pdf`, `pdf/factura_electricista.pdf`                                                                                                                                                                                                                                                                                                              |
| office |     3 | 93, 98, 86    | `office/notes.docx`, `kickoff.pptx`, `inventory.xlsx`                                                                                                                                                                                                                                                                                                         |
| en     |     1 | 99, 88, 89    | `en/onboarding_notes.txt`                                                                                                                                                                                                                                                                                                                                     |
| es     |     1 | 87, 85, 91    | `es/notas_incorporacion.txt`                                                                                                                                                                                                                                                                                                                                  |
| edge   |     5 | 694, 706, 699 | `edge/*` — 3 indexed, 2 errored (password-protected + truncated PDFs), 0 crashes; run under the default 50 MB size cap, so `huge_real.pdf` (~1.4 MB) is indexed rather than skipped here (the pipeline's dedicated 1 MB-cap test covers the skip path — see `crates/magi-core/tests` and `index::pipeline::tests::real_file_over_cap_is_skipped_with_reason`) |

The edge run's higher per-file cost reflects three PDFs in a five-file
batch (one genuinely larger, two exercised via the extraction-error path)
rather than a representative per-type rate — it is not included in the
per-type table above.

## M3 — 100k-chunk search latency (NFR-2/NFR-3)

**Method:** `xtask bench-corpus` (`just bench`) builds a synthetic index —
5,000 files × 20 chunks = 100,000 chunks, random text/vectors — directly
via `db::files::upsert_file` (no real extraction; only the query side uses
the real `E5Embedder`, since retrieval quality isn't what this measures —
see `magi-cli eval` / `docs/eval.md` for that). Same reference machine as
above, release build, fp32 model.

| Metric                           | Measured | Target (SPEC.md §2.2) |   Result |
| -------------------------------- | -------: | --------------------: | -------: |
| Cold (model load + first search) | 3,176 ms |            ≤ 3,000 ms | **FAIL** |
| Warm p95 (200 queries)           |   585 ms |              ≤ 300 ms | **FAIL** |

Neither target is met. Root cause and the fts/vector/hybrid latency
breakdown by corpus size are in `docs/eval.md`'s "Latency (NFR-2/NFR-3) on
100k synthetic chunks" section — in short, vector search scales with corpus
size (consistent with `vec0`'s brute-force scan, no ANN index) and
dominates at 100k chunks; the embedder itself is not the bottleneck
(~13 ms/query in isolation). Not fixed in this slice — needs an
ANN/partitioning strategy, tracked as an open item.

## Indexing performance pass (branch `feat/change_detection`, 2026-09-23)

**Method:** synthetic text corpus (seeded, lognormal sizes: mostly short
notes, a tail of long documents; unique content), backdated an hour so the
scheduler's settle window does not apply. Each run gets a fresh
`MAGI_DATA_DIR` (models linked in) and `pause_on_battery = false`. Builds are
run in alternating order per repetition; the table shows the median. Wall time
covers the whole process: for `index`, start to exit; for `daemon`, start until
the status shows `queued 0` and every file `indexed`. Same reference machine
as above, **on battery** (so absolute numbers are throttled and noisy, about
±5–10 %); release builds.

Builds:

- **base**: branch state before this pass (on top of `348e964`).
- **+1,3,5**: partial `idx_files_size` for the move lookup, `synchronous=NORMAL`,
  vectors bound as raw f32 BLOBs instead of JSON.
- **+2,4**: plus cross-file embed batching (`pipeline::embed_group`) and the
  scheduler's `Ready` hold with the partial `idx_files_pending`.
- **+length split**: plus `embed_group` ending a batch where chunk lengths
  jump, so short chunks (filenames) are not padded to full body chunks.

| Scenario                                            | Files |   base | +1,3,5 |   +2,4 | +length split |
| --------------------------------------------------- | ----: | -----: | -----: | -----: | ------------: |
| `index`, fake embedder (3 reps)                     | 2,000 | 21.3 s | 15.5 s | 15.7 s |             — |
| `index` of a 2nd root, fake embedder (3 reps)       | 2,000 | 30.1 s | 20.9 s | 23.0 s |             — |
| `daemon`, fake embedder (3 reps; last column 1 rep) | 2,000 | 30.6 s | 22.1 s | 10.4 s |         8.8 s |
| `daemon`, real e5 (2 reps)                          |   500 |  306 s |  307 s |  301 s |             — |
| `daemon`, real e5, second session (2 reps)          |   500 |      — |  331 s |      — |     **206 s** |

Reading it:

- With the fake embedder (database and scheduling cost only), 1, 3 and 5 take
  about 30 % off, mostly from not fsyncing every commit. 2 and 4 halve the
  daemon's time again. They don't touch the serial `index` path.
- With the real model, embedding is nearly all of the time (~7 chunks/s on
  battery with 2 intra-op threads), so 1–5 alone change nothing measurable.
  Cross-file batching helps only once short chunks stop being padded to long
  ones: about 1.6× faster (308/331 s → 189/206 s).
- Move lookup (item 1) at scale, measured on the query alone: 2,000 new files
  against 100,000 indexed rows take 10.3 s without `idx_files_size` and 7 ms
  with it (0.56 s vs 3 ms at 10,000 rows). The 2,000-row runs above are too
  small to show it. At real sizes it would be minutes spent inside the
  writer's scan transaction.

## M5 — idle cost of `magi-cli daemon` (manual item, 2026-09-23)

`magi-cli daemon --stats`, release build, real e5 / SigLIP / OCR, reference
machine on AC power, owner doing normal work alongside (about 15 of 16 GB RAM
in use). One root of **232 files** (225 indexed, 4 skipped, 3 errors); first
index finished at 313 s, then about 80 minutes of normal use until Ctrl-C.

| Phase                        | CPU (% of one core, per-minute samples) | Private memory | RSS         |
| ---------------------------- | --------------------------------------- | -------------- | ----------- |
| First minute after the index | 43.6 (tail of indexing)                 | 560 MB         | 579 MB      |
| Models loaded, idle (~5 min) | 0.10–0.23                               | 560 MB         | 579 MB      |
| After idle unload, ~75 min   | **0.05–0.37**, one sample 0.63          | 91–93 MB, flat | 113 → 40 MB |

- **Pass:** idle CPU under 1% (SPEC §7 M5 manual item). Private memory stays
  flat after the models unload, so no leak over the hour. RSS keeps falling only
  because Windows trims the working set.
- **Exception the owner accepted:** SPEC asks for a 20k+ file folder; this run
  used 232 files, to avoid another overnight first index. Idle cost comes from
  the scheduler poll, timers and one watcher handle per root, none of which
  grows with the file count; the 20k-file run is still worth doing later.
- **Found by it:** from ~4,100 s the status flipped `paused` / `idle` as often
  as every 10 s. Available memory hovered around the 1 GiB NFR-13 threshold, and
  the pause had no hysteresis. Fixed: it now resumes only above 1.25 GiB
  (`index::resources::memory_low`, unit test).

## M5 — peak memory: indexing (NFR-11) and search only (NFR-12), 2026-09-24

Release build of `a0cff25`, real e5 / SigLIP 2 (q4f16) / PaddleOCR, reference
machine on AC power. Fresh `MAGI_DATA_DIR` per run with the models linked in,
`pause_on_battery = false`. Memory is sampled from outside the process:
`PeakWorkingSet64` every 50 ms (10 ms for search), private bytes alongside.

**NFR-11 (≤ 1.5 GB):** `magi-cli daemon` on `fixtures/corpus` (125 files:
text, code, PDF, Office, JPEG/PNG/HEIC photos, QR codes), stopped when the
status reports `idle, queued 0`. Result: 113 indexed, 4 skipped, 3 errors
(the `edge/` files), in about 80 s.

| `worker_threads`         | Peak working set         | Peak private             |
| ------------------------ | ------------------------ | ------------------------ |
| auto (3 on this machine) | 1,337 / 1,335 / 1,215 MB | 1,375 / 1,367 / 1,244 MB |
| 2 (the 8 GB-mode count)  | 1,250 / 1,253 MB         | 1,278 / 1,281 MB         |

- **Pass**, with 160–320 MB of headroom. The vision tower, OCR and e5 are
  loaded together here. The SigLIP **text** tower is not, because indexing
  never loads it.
- **Not measured:** a search during the first index. That adds the text tower
  and its tokenizer on top (see NFR-12 below) and would take the process past
  1.5 GB. ADR-0007 already expected this. It needs the engine and the search
  in one process, so the M6 app or a daemon search hook.

**NFR-12 (≤ 900 MB):** `magi-cli search --mode hybrid` on that index, one
process per query (e5 and the SigLIP text tower, no indexing), 5 queries.

| Mode            | Peak working set | Peak private   |
| --------------- | ---------------- | -------------- |
| hybrid (5 runs) | 1,271–1,273 MB   | 1,437–1,442 MB |
| fts (no models) | 8 MB             | 2 MB           |

- **Fail** by about 370 MB. Almost all of it is the two query encoders, loaded
  one after the other but both still resident at the peak.

**Where it goes (2026-09-25).** Same setup. e5 alone was measured by leaving
`models/image` out of the data dir; each tokenizer alone with the ignored
`real_tokenizer_file_loads_and_encodes_plausible_counts` test (7 MB empty-test
baseline subtracted). These are peaks, so they don't add up exactly.

| Part                                 | Peak working set |
| ------------------------------------ | ---------------- |
| e5 model                             | ~225 MB          |
| e5 `tokenizer.json` (XLM-R, 17 MB)   | ~266 MB          |
| SigLIP 2 text tower (q4f16)          | ~400 MB          |
| Gemma `tokenizer.json` (256k, 34 MB) | ~373 MB          |

The tokenizer figures are the peak while the JSON is parsed; how much stays
resident afterwards was not measured.

**Fix: free e5 before SigLIP loads.** `magi-cli search` wraps the text
embedder in `OneShotQuery`, which unloads the e5 session and the shared text
tokenizer as soon as the query is embedded. Only the one-shot CLI does this;
`eval` and a long-lived engine keep e5 cached.

| Build (release, 5 queries × 3) | Peak working set         | Peak private   |
| ------------------------------ | ------------------------ | -------------- |
| before                         | 1,271–1,273 MB           | 1,437–1,442 MB |
| free e5 first                  | 900–909 MB (14/15 ≤ 901) | 1,056–1,069 MB |

Latency, one cold process per query (model load included), 20 runs per build,
the builds interleaved so machine load hits all of them alike:

| Build                               | Median   | Min–max        |
| ----------------------------------- | -------- | -------------- |
| before                              | 3,201 ms | 2,795–4,421 ms |
| free e5 first                       | 3,686 ms | 2,937–4,511 ms |
| free e5 first + lean SigLIP session | 4,231 ms | 3,139–5,024 ms |

- About +0.5 s per search for about −370 MB. Accepted.
- **Lean SigLIP session tried and reverted:** `with_prepacking(false)` and
  `with_memory_pattern(false)` on the SigLIP text session changed the peak by
  less than the noise (900–901 MB) and cost another ~0.5 s.
- That still sat right at the budget. A phase-by-phase trace (working set and
  heap, "beach photo") showed why:

  | Phase                   | Working set | Peak so far |
  | ----------------------- | ----------- | ----------- |
  | e5 query embedded       | 407 MB      | 493 MB      |
  | e5 + tokenizer unloaded | 37 MB       | 493 MB      |
  | Gemma tokenizer parsed  | 101 MB      | 493 MB      |
  | SigLIP session built    | 538 MB      | 901 MB      |

  The Windows heap does return e5's memory. The peak is ONNX Runtime building
  the SigLIP text session: briefly ~800 MB (about twice the 443 MB file),
  settling at ~437 MB. The Gemma tokenizer, already loaded, sat on top of it.

- **Fix: build the SigLIP session before parsing its tokenizer**
  (`embed::siglip::load_text`). Peak **842–848 MB** (15 runs, private
  993–1,001 MB): **pass**. Latency unchanged, interleaved against the build
  before it: median 3,092 ms against 3,055 ms, 20 runs each.
- **mimalloc tried and not adopted** (0.1.52, as `magi-cli`'s global
  allocator). It can't lower this peak, because ONNX Runtime allocates inside
  its own DLL with the C runtime's malloc. With default settings it keeps the
  ~290 MB freed by the e5 unload, so the peak rose to 1,213 MB. With the purge
  delay at 0 it matched the system allocator (893 MB). It made the tokenizer
  parse about twice as fast (~23% lower search latency), but it is a native C
  dependency that needs an ADR and a check on all three OSs.
- Latency on this machine swings by more than a second between runs, so only
  interleaved comparisons are meaningful. Two back-to-back runs of the same
  build gave medians of 3,718 ms and 4,835 ms.

## M6 — NFR-8 search while indexing, 2026-09-26

Reference machine (above). `crates/magi-core/tests/nfr8.rs`
(`search_latency_while_indexing`), run with the real models from the dev data
directory (`just models`, already present):

```
cargo test -p magi-core --release --test nfr8 -- --ignored --nocapture
```

```
NFR-8 while indexing: n 201 p50 185.502ms p95 278.3645ms max 3.340383s
NFR-8 idle, warm:     n 5 p50 138.1647ms p95 144.1232ms max 144.1232ms
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 84.41s
```

Peak working set / private bytes of the test process, sampled every 2 s with
PowerShell `Get-Process -Name 'nfr8-*'` for the whole run: **1,762 MB working
set, 1,800 MB private** (`image_visual` enabled, so this includes SigLIP 2's
vision tower alongside e5 and PaddleOCR while the fixture corpus indexes).

SPEC.md §2.2's NFR-8 ("Background politeness") is: "Indexing threads run at
low OS priority; ONNX intra-op threads capped; search is never blocked
behind an indexing batch for more than one small batch." ("Search stays
responsive" is the M8 QA checklist's phrasing, SPEC.md:1019, not the NFR
itself.)

Graded against that: p95 278 ms while busy against p95 144 ms idle is good
evidence that a search is usually not stuck behind an indexing batch at
all — `EngineHandle::search_pending`/`SearchGuard` stops a *new* batch from
starting while a search is pending, so most searches land between batches.
But the recorded **max of 3.34 s while busy is not explained by that
mechanism**: a `SearchGuard` cannot interrupt a batch already in flight
(`engine::MAX_YIELD`, 5 s, only bounds how long the guard makes *indexing*
wait, not how long a search waits behind an in-progress batch), and this
benchmark did not record each embed batch's own duration, chunk count, or
which queries landed mid-batch. So the "never blocked ... for more than one
small batch" bound is **not verified by this measurement** — it is
plausible the 3.34 s outlier *is* one batch's worth of blocking, but that
has not been shown, only asserted. **Status: p95 recorded and looks good;
the max-latency / one-batch bound is unverified** (see docs/progress.md).

## Reproducing

```
cargo build -p magi-cli --release
MAGI_DATA_DIR=<fresh empty dir> ./target/release/magi-cli index <fixture-type-dir>

# M3 latency benchmark (needs `just models` + `cargo xtask fetch-onnxruntime` first):
just bench
```
