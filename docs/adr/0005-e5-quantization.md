# ADR-0005: `multilingual-e5-small` quantization (int8 vs. fp32)

## Context

SPEC.md §7 M3 requires a quantization decision for the text embedder,
"based on eval results and memory," recorded in an ADR: "If int8 is
chosen, its overall recall@5 is within 2 points of fp32 (report both)."

Both variants are published by the model's own Hugging Face repo (`onnx/`
subfolder) — no conversion/quantization work of our own, just choosing
which pinned file `models/manifest.toml` ships. Their real, downloaded
sizes: `model.onnx` (fp32) 470,268,510 bytes; `model_qint8_avx512_vnni.onnx`
(int8) 118,346,824 bytes — the int8 file is ~4x smaller on disk.

## Measurement

`magi-cli eval eval/queries.jsonl --corpus fixtures/corpus`, each variant
copied to `<data_dir>/models/text/model.onnx` in turn (same tokenizer,
same 60 queries, same corpus, same machine — see `docs/eval.md`'s
reference-machine section):

| Variant | vector-only recall@5 | hybrid recall@5 | hybrid MRR | peak RSS, `magi-cli index fixtures/corpus` |
| ------- | --------------------:| ----------------:| -----------:| -------------------------------------------:|
| fp32    |                 0.983 |             0.983 |       0.807 |                                   1,302.5 MB |
| int8    |                 0.967 |             0.967 |       0.796 |                                   1,024.9 MB |
| Δ       |                -0.016 |            -0.016 |      -0.011 |                                    -277.6 MB |

(Peak RSS via Windows' own `PeakWorkingSet64`, polled every 50 ms across
the process's lifetime — same reference machine as `docs/eval.md`.)

## Decision (superseded — see "Update: decision executed" below)

Ship fp32 as the manifest's default for now — **not** because int8 fails
the recall bar (a 1.6-point recall@5 drop is well within SPEC's 2-point
allowance), but because neither variant is close to a real decision yet:
the eval corpus is small and synthetic (see `docs/eval.md`'s "open
finding" — vector-only recall@5 is already near its ceiling on this set
regardless of variant, so the recall comparison alone is weak evidence),
and **both variants blow through SPEC.md §7 M3's ≤700 MB text-pipeline RSS
target** — int8 saves 277.6 MB (a real, meaningful 21% reduction) but
still peaks at 1,024.9 MB, nowhere near 700 MB. Switching to int8 alone
would not fix the memory problem; something else in the pipeline (`ort`
session/thread-pool defaults are the leading suspect — `with_intra_threads`
was never set, so `ort` likely defaults to one thread per logical core on
this 16-thread machine, and `tokenizers`' ~250k-entry Unigram vocab plus
its precompiled normalizer charsmap add up too) needs investigating before
either variant meets the budget. Revisit once that's profiled and a
larger, more realistic eval corpus exists.

`models/manifest.toml` already records the int8 file's verified SHA-256 in
a comment, so switching later is a one-line change plus re-verifying that
comment's freshness against whatever revision is pinned at the time.

## Update: RSS root-cause investigation

This ADR named two suspects for the RSS overshoot: `ort`'s thread-pool
defaults and `tokenizers`' vocab/charsmap footprint. Both were profiled for
real, on the same reference machine, same method (`PeakWorkingSet64`,
`magi-cli index fixtures/corpus`):

1. **Thread-pool defaults: not the cause.** Setting `.with_intra_threads(1)`
   on the ONNX session (`embed::e5::E5Embedder::load`) made **no measurable
   difference** to peak RSS (1,302.5 MB before and after, to within 0.4 MB
   noise across two runs) — kept anyway since the embed worker is
   architecturally single-threaded (SPEC.md §5.3) and there's no reason to
   let `ort` default to one thread per logical core, but it isn't the fix.
   `.with_memory_pattern(false)` (ort's own docs recommend disabling it
   when input shape varies, which ours does — `BatchLongest` padding)
   was tested the same way and also made no measurable difference; not
   kept, since it has no evidence behind it and defaults to enabled for a
   reason.
2. **Real cause, found and fixed: a duplicated tokenizer.** `chunk::count_tokens`
   already loads the ~17 MB `tokenizer.json` once process-wide via
   `embed::manager::shared_text_tokenizer` (a `OnceLock`). `E5Embedder::load`
   was loading its **own separate copy** via `Tokenizer::from_file` instead
   of reusing that static — two independent parses of the same file in one
   process. Isolating the tokenizer alone (a throwaway probe binary that
   loads only `tokenizer.json`, no ONNX session) measured **275.5 MB** for
   one copy — so the duplication alone accounted for roughly a fifth of
   total pipeline RSS. Fixed by moving the truncation/padding configuration
   `E5Embedder` needs into `manager::load_tokenizer_from` (the same
   function `shared_text_tokenizer` calls) and having `E5Embedder` borrow
   `shared_text_tokenizer()`'s `&'static Tokenizer` instead of loading its
   own. Safe for `count_tokens`' existing single-word, no-special-tokens
   calls: `BatchLongest` padding on a batch of one is a no-op, and a lone
   word never approaches the 512-token truncation ceiling. Verified against
   the real model/tokenizer after the change: `e5_parity`'s cosine-vs-Python-
   reference test and the cross-lingual smoke test both still pass
   unchanged (`cargo test -p magi-core --release -- --ignored`).

Re-measured after the tokenizer fix (thread/memory-pattern settings kept,
having no cost either way):

| Variant | peak RSS (before this ADR's fix) | peak RSS (after) |    Δ | vs. ≤700 MB target |
| ------- | ---------------------------------:| -----------------:|-----:| -------------------:|
| fp32    |                        1,302.5 MB |        1,100.2 MB | -202 MB |            +400 MB over |
| int8    |                        1,024.9 MB |          767.1 MB | -258 MB |             +67 MB over |

(Each an average of two runs, which agreed to within 0.5 MB; int8's model
file swapped in the same way as the original measurement.)

Neither variant is dominated by corpus size — a run against a single
189-byte fixture file measured 1,100.1 MB with fp32, essentially identical
to the 37-file `fixtures/corpus` run — so the remaining budget is almost
entirely fixed model+session+tokenizer load cost, not per-chunk work.

**int8 is now close enough to the target that switching is the obvious
next lever** (67 MB over vs. fp32's 400 MB over) — see "Update: decision
executed" below.

## Update: decision executed

Switched. `models/manifest.toml`'s text slot now points at
`onnx/model_qint8_avx512_vnni.onnx` (fp32's hash kept in a comment for
rollback); `e5_parity.rs`'s tolerance is now `0.97` per SPEC.md §7 M3's
"≥ 0.97 if the quantized model is chosen" rule (was `0.99` against the
fp32 reference, which no longer applies since the reference is always
fp32 but the model under test is now int8).

Re-verified against the real int8 model, same reference machine:

- **Parity**: `e5_parity.rs` vs. the fp32 Python reference — worst-case
  cosine **0.9953** (well above the 0.97 floor).
- **Cross-lingual smoke test** (`embed::e5::tests::real_model_embeds_plausible_vectors`):
  still passes.
- **Eval** (`magi-cli eval eval/queries.jsonl --corpus fixtures/corpus`):
  identical to the comparison measurement above — vector-only/hybrid
  recall@5 0.967, confirming the earlier number wasn't a fluke of how it
  was measured.
- **Peak RSS** (`magi-cli index fixtures/corpus`): **766.4 MB** (matches
  the 767.1 MB post-tokenizer-fix measurement above within noise) — still
  66 MB over the 700 MB target, not zero, but the closest this project has
  gotten to it.

## Update: RSS target closed (CPU memory arena)

This ADR's RSS investigation tested two `ort` session settings —
`with_intra_threads` and `with_memory_pattern` — and found neither moved
peak RSS. A third, distinct setting wasn't tried: the CPU execution
provider's **memory arena** allocator. ORT's arena grows a pool sized for
the largest batch it's seen and keeps it for the session's lifetime rather
than returning it to the OS — a different mechanism from
`with_memory_pattern` (which controls whether ORT precomputes reusable
memory-reuse *patterns* across repeated calls with the same input shape,
not whether an arena is used at all).

Disabled via `ep::CPU::default().with_arena_allocator(false).build()`
passed to `Session::builder().with_execution_providers([...])` in
`embed::e5::E5Embedder::load`. Re-measured the same way as above (Windows
`PeakWorkingSet64`, `magi-cli index fixtures/corpus`, average of two runs,
same reference machine):

| Measurement                          | Peak RSS |
| ------------------------------------- | --------:|
| Before (post-tokenizer-fix, confirms table above reproduces) | 766.55 MB (767.8/765.3) |
| After (arena disabled)                | 682.8 MB (683.8/681.8) |

**SPEC.md §7 M3's ≤ 700 MB text-pipeline RSS target is now met** (682.8 MB),
not just closer. Re-verified this is a pure allocation-strategy change, not
a correctness regression: `e5_parity` (cosine 0.9953, unchanged),
`real_model_embeds_plausible_vectors` (cross-lingual smoke test, unchanged),
and `magi-cli eval` (vector-only/hybrid recall@5 0.967, unchanged) all still
pass against the same real int8 model and tokenizer.

This measurement required actually downloading the real model for the
first time via a genuinely working fetch path — see `docs/progress.md`'s
"`xtask fetch-models` gap found and fixed" slice: the CLI command this
ADR's own earlier measurements implicitly assumed existed (`just models` /
`cargo xtask fetch-models`) had never been implemented; `embed::manager`'s
download/verify logic existed and was unit-tested, but nothing called it
outside tests. Fixed as part of closing this out.

## Consequences

- `models/manifest.toml` now ships int8 as the default; fp32's hash stays
  in a comment for rollback. Production code (`embed::e5`,
  `embed::manager`) was already changed to eliminate the
  duplicate-tokenizer waste described above — a real, free ~200-260 MB
  reduction that applied regardless of which variant ships. The CPU memory
  arena is now also disabled for the same session, another real, free
  reduction (~84 MB on the reference machine).
- The eval harness (`magi-cli eval`) supports re-running this comparison
  cheaply (swap the file in `<data_dir>/models/text/`, re-run) once a
  better corpus exists.
- The ≤700 MB text-pipeline RSS target (SPEC.md §7 M3) is **met**: 682.8 MB,
  down from int8's original 1,024.9 MB across the tokenizer-dedup and
  arena-allocator fixes combined (a 342 MB, 33% reduction).
