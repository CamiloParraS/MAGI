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

## Decision

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
next lever** (67 MB over vs. fp32's 400 MB over), but this ADR does not
make that call: switching the shipped default needs `models/manifest.toml`
updated, `e5_parity.rs`'s tolerance revisited per SPEC.md §7 M3's "≥ 0.97 if
the quantized model is chosen" rule (currently asserts ≥ 0.99 against the
fp32 reference), and `docs/eval.md`/this ADR's recall table re-run against
whatever the shipped default becomes — a follow-up task, not a
byproduct of an RSS investigation.

## Consequences

- No manifest change from this ADR; `model.onnx` (fp32) stays the shipped
  default. Production code changed (`embed::e5`, `embed::manager`) to
  eliminate the duplicate-tokenizer waste described above — a real, free
  ~200-260 MB reduction for both variants regardless of which ships.
- The eval harness (`magi-cli eval`) now supports re-running this
  comparison cheaply (swap the file in `<data_dir>/models/text/`, re-run)
  once a better corpus exists.
- The ≤700 MB text-pipeline RSS target (SPEC.md §7 M3) is still not met by
  either variant — fp32 is 400 MB over, int8 is 67 MB over — but the gap is
  now understood (fixed model/session/tokenizer load cost, not thread
  config or per-chunk work) and int8 is close. Switching the default to
  int8 is the recommended next step, tracked as a follow-up rather than
  done here (see "Update" above for what that follow-up needs to touch).
