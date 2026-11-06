# 0002: Measure precision: negative queries and wrongly folded answers

Status: done 2026-11-05. Results and baselines: `docs/eval.md`,
"2026-11-05: Precision, fresh queries, and the z-score reranker confirmed or
not". The false-weak rate is reported per label setting in the strong/weak
sweep, and the 0.5x-top fold's count stays as `answer folded`.

## Problem

`magi-cli eval` only scores whether the expected file is found (recall@k,
MRR). Nothing measures the other half: how much unrelated filler a result
list shows, and whether the app should have said "nothing found". So every
threshold tried so far was tuned blind on that axis:

- the minimum text similarity floor (`docs/progress.md`, M6 carry-overs: "a
  floor needs a precision measure ... Still open");
- the z gate and "weaker matches" fold of GitHub #9 (`docs/eval.md`,
  2026-11-05), tuned on the same queries it was scored on.

The 2026-11-05 runs also showed the cost of getting this wrong: under both
RRF and z-score fusion, folding results below 0.5x the top score would have
hidden the right answer for 2-7 queries per corpus.

## What to build

1. **Negative queries** in the three query files (`eval/queries.jsonl`,
   `eval/random_names.jsonl`, `eval/realistic_names.jsonl`), as a new
   `neg` bucket with `"expected": []`. Two kinds, about 15-20 in total, in
   English and Spanish:
   - nonsense (`zxqv blorft`, `qwpoeiruty`; the five in `eval.rs`'s
     `NONSENSE_QUERIES` move here);
   - plausible but absent: things a user might search for that are not in
     the corpus (`tax return 2019`, `wedding photos`, `factura de la luz`,
     `a red sports car`, `python decorator tutorial`). Check each against the
     corpus first, including `fixtures/corpus/local/`.
2. **Fresh positive queries** (shared with GitHub #11): about 20-30 new ones
   written after 2026-11-05, some `cross`, some `img`/`img2`, a few `kw`. Keep
   them in their own bucket or flag (`"fresh": true`) so they can be reported
   apart from the set every threshold so far was tuned on.
3. **New numbers in `magi-cli eval`**, per mode:
   - For `neg`: how many queries return a list whose top result would be
     labelled "no close match" (0003's signal), and results per query.
     recall/MRR are not computed for `neg`.
   - For every positive query: **false-weak rate**, the share of queries whose
     expected file is found but would sit under the "weaker matches" fold
     (today's `answer folded` in the fold stats, made a first-class column).
   - Keep the existing recall@5/@10 and MRR unchanged for the positive buckets,
     so earlier baselines stay comparable.

`eval/README.md` documents the `neg` bucket and the `fresh` flag.

## Done when

`just eval`, `just eval-random` and `just eval-random realistic_names` report
the `neg` bucket and the false-weak rate, the fresh queries are in, and a
baseline for RRF (and the #11 reranker variant) is recorded in
`docs/eval.md`. No threshold is chosen in this ticket.
