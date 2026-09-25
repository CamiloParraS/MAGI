# Indexing performance investigation (2026-09-23)

Question: is ~6 h for a 20k-file first index acceptable, and where can
indexing get faster? Four read-only investigations (text embedding, image/OCR/PDF
extraction, pipeline concurrency, external baselines), then two fixes tried and
measured on the reference machine. Numbers labelled _estimate_ were not measured.

**Correction to `docs/benchmarks.md`:** the Ryzen 7 7445HS has **6 cores / 12
threads** (2 Zen 4 + 4 Zen 4c), not 8C/16T.

## Verdict: 6 h for 20k files is reasonable

- Back-of-envelope for a mixed 20k folder (≈12k documents × ~5 chunks, ≈6k
  images, ≈2k name-only) at the current 2 intra-op threads: **~4.5–5 h**
  (_estimate_). ~6 h is within ~25 % of what this model stack costs.
- Floor with these models and every core busy: **~1–1.5 h** (_estimate_).
  Going lower needs smaller models or a GPU/NPU execution provider.
- Comparable tools: Windows Search (keywords only) "can take up to a couple
  hours" ([Microsoft](https://support.microsoft.com/en-us/windows/experience/performance-optimization/search-indexing-in-windows));
  Spotlight "can take hours or even days" and finishes faster idle and on power
  ([Apple 102321](https://support.apple.com/en-us/102321)); Recoll (keywords, no
  OCR) did 18k PDFs in 11 min 40 s ([recoll](https://www.recoll.org/pages/perfs.html));
  Apple Photos analysis runs idle-and-on-power over days (secondary sources only).
- Model-level references: multilingual-e5-small ~226 docs/s on a 16-core
  7950X with OpenVINO ([model card](https://huggingface.co/hotchpotch/bekko-embedding-v1-a8m));
  PP-OCRv5 mobile 1.31–1.75 s/image on Xeon CPU
  ([PaddleOCR](http://www.paddleocr.ai/main/en/version3.x/algorithm/PP-OCRv5/PP-OCRv5.html)),
  consistent with ADR-0006.
- The owner's 232-file / 313 s run (AC power, owner working alongside) is
  ~1.35 s/file: an image-heavy folder, not a clean benchmark.

The biggest _perceived_ win is not throughput: make everything keyword-searchable
first (M2 extraction + FTS runs 200–400 files/s, so ~1–2 min for 20k), then embed
in the background (prior-art.md item 2).

## Measured: per-stage cost of one image (release build, AC power)

Temporary probe over fixture images, timing each stage of `extract_image`
separately; median-ish of 3 reps. The machine flips between a fast and a slow
power state ~2× apart, so absolute numbers vary by run; ratios within a row hold.

| Image                | Size     |     decode |            QR |     SigLIP | Lanczos → 2048 |        OCR |          OCR text |
| -------------------- | -------- | ---------: | ------------: | ---------: | -------------: | ---------: | ----------------: |
| `cat_on_beach.jpg`   | 948×1280 |      12 ms |         55 ms |     280 ms |          <1 ms |     175 ms |              none |
| `Portrait_photo.jpg` | 12 MP    |  90–175 ms |    290–580 ms | 300–560 ms |     200–420 ms | 185–320 ms | 5 chars (dropped) |
| `VW_beetle.jpg`      | 45 MP    | 240–460 ms | **1.2–2.9 s** | 420–850 ms |  **0.5–1.1 s** | 150–290 ms | 3 chars (dropped) |
| `phone_text_es.jpg`  | 12 MP    |   45–85 ms |    300–630 ms | 310–570 ms |     200–470 ms |  0.5–1.5 s |         466 chars |
| `screenshot_en.png`  | 180×247  |      <1 ms |         <1 ms | 300–550 ms |              — |  70–200 ms |          67 chars |

Takeaways:

- **On photos without text, OCR is not the bottleneck** (~0.2–0.3 s, detection
  only). QR scanning and the Lanczos resize together cost 2–4× more.
- SigLIP is a flat ~0.3–0.5 s per image, whatever the size.
- The ~7 text chunks/s e5 rate reproduced: 8.3–10.6 chunks/s at `Level1`,
  64 SPEC.md chunks in batches of 16.

## Fix 1 — ONNX graph optimization `Level1` → `Level3`: **rejected**

`crates/magi-core/src/onnx.rs` sets the level for every session (e5, SigLIP,
OCR det/rec).

- **Speed:** e5 is ~20–30 % faster (8.3 → 10.0 chunks/s in the slow power
  state, 10.6 → 14.0 in the fast one, alternating builds). SigLIP/OCR changes
  were inside the noise.
- **Parity tests:** all pass (`e5_parity`, `batched_passages_match_one_at_a_time`,
  `towers_match_the_pytorch_reference`, `reads_spanish_screenshot_with_accents`,
  plus the other ignored model tests).
- **`just eval`: regresses.** Level1 is deterministic (two identical runs);
  Level2 and Level3 give identical, worse results:

  | Hybrid            | Level1 | Level2 / Level3 |
  | ----------------- | -----: | --------------: |
  | overall recall@5  |  0.982 |           0.970 |
  | `cross` recall@5  |  0.900 |           0.800 |
  | `cross` recall@10 |  0.950 |           0.900 |
  | overall MRR       |  0.885 |           0.880 |

  New misses: `team meeting notes` (rank 6), `doctor appointment reminder`
  (rank 10 → out of the list), `software bug report watcher` (rank 7). The extended
  fusions change the int8 e5 graph's numerics enough to move cross-language
  ranks. The cosine parity tolerance does not catch this; `just eval` does.

Kept `Level1`. Revisit only together with a cross-language eval gate, or for the
SigLIP/OCR sessions alone if a measurement shows they gain (not shown here).

## Fix 3 — skip OCR recognition on images without text: **already in place**

The investigation proposed "run recognition only if detection finds text
boxes". `PaddleOcr::recognize` (`ocr/paddle.rs`) already does exactly that:
recognition loops over the detector's boxes, so a photo with no text pays only
detection (~0.2–0.3 s, above). The other half, skipping tiny images, was not
applied: `screenshot_en.png` is 180×247 and carries 67 characters of real
text, so a size gate would lose real content for ~0.1 s saved. No code change.

## Where the time actually goes, ranked by measured or evidenced gain

| #   | Change                                                                                                              | Where                              | Gain                                                    | Risk / effort                  |
| --- | ------------------------------------------------------------------------------------------------------------------- | ---------------------------------- | ------------------------------------------------------- | ------------------------------ |
| 1   | _(done)_ 4 intra-op threads on AC, 2 on battery; memory arena back on for e5, e5 batch 16 → 8                       | `onnx.rs`, `embed/e5.rs`           | Measured: eval workload 285 s → ~190–230 s              | Done; see "Follow-up: threads" |
| 2   | ~~QR format restriction; Triangle resize~~ — tried, see "Follow-up" below                                           |                                    |                                                         |                                |
| 3   | _(done)_ QR ladder: grayscale once, halve each rung from the previous one                                           | `qr.rs`                            | Measured: 280 → 158 ms at 12 MP, 1.14 → 0.85 s at 45 MP | Done; eval identical           |
| 4   | Keyword-searchable first, embed later. Design: `docs/superpowers/specs/2026-09-24-keyword-first-indexing-design.md` | engine / scheduler                 | Perceived: hours → ~1–2 min                             | Medium                         |
| 5   | Cap embedded chunks per file (FTS keeps the full text). A 15 MB file is ~5,700 chunks, ~13 min.                     | `index/pipeline.rs`                | Large on long-document folders                          | Small; tune with `just eval`   |
| 6   | Collect ~64 chunks per embed group (batches stay 16), split on length ratio 1.25× instead of 2×.                    | `engine.rs:728`, `pipeline.rs:449` | 10–20 % (_estimate_)                                    | Tiny                           |
| 7   | Batched OCR line recognition (plan Task 4).                                                                         | `ocr/paddle.rs:186`                | 2–3× on text-heavy images (_estimate_)                  | Medium                         |
| 8   | Parse each PDF once (text + thumbnail).                                                                             | `extract/pdf.rs:59,75`             | ~2× on PDF parse                                        | Low                            |

Not worth touching: 256-token chunks (overlap eats the gain, ~0 %), dropping
the filename chunk (≤5 %), settle windows, per-file SQL, hashing, the per-file
isolate thread (20–50 µs), mid-index model unload (does not happen).

## Follow-up: QR and resize changes (2026-09-23)

Same fixture probe, one build per change, same power state.

- **QR restricted to `QR_CODE` via `PossibleFormats`: no measurable gain**
  (298 → 285 ms at 12 MP, 1.24 → 1.16 s at 45 MP). The investigation's guess
  was wrong: rxing's format list is not where the time goes. Reverted, so 1D
  barcodes still decode.
- **QR ladder, grayscale once, each rung halved from the previous: kept.**
  Before, every rung resized the full-resolution RGB frame and converted it to
  luma again. 12 MP photo without a code: 280 → 158 ms. 45 MP: 1.14 → 0.85 s
  (the rest is the native-resolution pass). All QR tests pass, including the
  local tiling fixture; `just eval` is byte-identical to the baseline.
- **`Lanczos3` → `Triangle` for the 2048 px OCR/thumbnail copy: rejected.**
  Faster (194 → 151 ms at 12 MP, 467 → 214 ms at 45 MP), but it changes the OCR
  input, and `just eval` drifts: hybrid recall@10 0.988 → 0.982
  (`doctor appointment reminder` falls from rank 10 out of the list), MRR
  0.885 → 0.881; `thumb_golden` also fails. Same rule as Level3: search quality
  must not drop. It is worth ~40 ms per 12 MP photo, so not worth re-blessing.

## Follow-up: threads and the memory arena (2026-09-24)

Probe: 64 SPEC.md chunks through e5, plus SigLIP and OCR on `phone_text_es.jpg`,
two rounds per setting.

| Intra-op threads |      e5, arena off | e5, arena on | SigLIP (arena off) | OCR (arena off) |
| ---------------: | -----------------: | -----------: | -----------------: | --------------: |
|                2 | 7.0 / 5.2 chunks/s |   8.2 / 10.7 |         273–283 ms |      525–540 ms |
|                4 |          7.8 / 5.9 |     **11.8** |             205 ms |      450–470 ms |
|                6 |          6.6 / 6.9 |         12.4 |         173–180 ms |      415–440 ms |
|               10 |                6.7 |            — |         186–190 ms |          527 ms |

- **e5 did not scale with threads because the memory arena was off**
  (ADR-0005's RSS fix). Every layer allocated and page-faulted a fresh
  ~100–200 MB attention tensor. Tokenization is not it (5 ms for 64 chunks).
  Two parallel e5 sessions did not help either (~7–8 chunks/s combined), which
  rules out ONNX's thread pool.
- SigLIP and OCR scale without the arena, so it stays off for them.
- 4 threads rather than 6: most of the gain, and it leaves cores for the
  extract workers and the user.

Whole fixture corpus with all models (`magi-cli eval`, indexing plus 164
queries, the NFR-11 workload), Windows `PeakWorkingSet64` polled every 200 ms,
AC power:

| Setting                                            |      Wall |      Peak working set | Eval vs baseline     |
| -------------------------------------------------- | --------: | --------------------: | -------------------- |
| Baseline: 2 threads, no arena                      |     285 s |              1,359 MB | —                    |
| 4 threads, arena on all sessions                   |     185 s |              1,601 MB | identical            |
| 4 threads, arena on e5 only, batch 16              | 187–232 s |     1,530 MB (3 runs) | identical            |
| **4 threads, arena on e5 only, batch 8 (shipped)** | 190–234 s | **1,448 MB** (2 runs) | one borderline query |

Batch 8 is the only setting under NFR-11's 1.5 GB. Cost: `doctor appointment
reminder` (cross-language) drops from rank 10 to out of the top 10. Hybrid
recall@10 0.988 → 0.982, MRR 0.885 → 0.884, recall@5 unchanged. The same query
flipped under the Triangle resize: it sits at exactly rank 10. The int8 e5
model's dynamic quantization scales over the whole batch, so a chunk's vector
depends slightly on its batch mates. Any batching change nudges it. The owner
accepted this trade on 2026-09-24.

Wall times swing ±20 % between identical runs on this laptop, so treat the
speedup as ~1.25–1.5×. Threads are read once per model load: plugging in or
unplugging mid-index takes effect after the next idle unload.

## Other findings (not performance)

- **`pause_on_battery` defaults to `true`** (`config/mod.rs:66`): on battery the
  first index makes zero progress. UX decision.
- **Memory pause at < 1 GiB free** (`index/resources.rs:57`) can stall indexing on
  a 16 GB machine that sits at ~15 GB used.
- **Priority bug:** `isolate::run` spawns a fresh thread per file
  (`index/isolate.rs:76`); new Windows threads start at normal priority, so
  extraction, OCR and SigLIP skip the below-normal priority the pipeline sets.
  Fix: call `lower_current_thread()` inside the spawned thread.
- **Silent OCR loss:** a worker that waits > 30 s for the shared OCR engine
  (`ocr/paddle.rs:32`, `BUSY_WAIT`) drops that image's OCR text.
- **Scanned PDFs** index with no text; PDF pages are never OCR'd.
- e5 ships `model_qint8_avx512_vnni.onnx`; on AVX2-only x86 or ARM it may run
  slower or lose accuracy (unverified).
