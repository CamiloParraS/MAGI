# 0003: Say "nothing closely related" without hiding results

Status: open. Depends on 0002 (needs its precision numbers to pick a
threshold). Changes the Host API, so `docs/architecture.md` is updated with
it; ship inside a milestone.

## Problem

Search always returns a full list, even for a typo or for something that is
not in any indexed folder: RRF ranks the nearest vectors whatever the query
(`docs/eval.md`, 2026-11-05: 100 results for each nonsense query). The list
looks like the app is guessing, and the user cannot tell "not indexed" from
"poorly ranked". `docs/progress.md` (M6 carry-overs) already notes
"nearest-but-irrelevant vectors fill the list".

A hard cut-off is the wrong fix. In a file finder, an empty list reads as
"the file does not exist". GitHub #9's z gate did exactly that to
cross-language matches: the right document was removed, not ranked low.
Similarity floors have failed the same way three times (text cosine floor,
opaque-name fix, z gate). Search also runs as the user types, so a gate
makes half-typed queries flicker to empty.

## Proposal: label results, don't remove them

- **Strong results** are shown as today.
- **Weak results** collapse under "Weaker matches (N)", one click to open.
- **No strong result at all:** show "Nothing closely related found" with the
  nearest results collapsed under it, never an empty page.

A wrong label then costs one click instead of a lost file.

**What makes a result strong**

- **Keyword evidence is always strong:** every query word matched (the FTS
  list) or the file name matched. The label only ever applies to results
  found by meaning alone.
- **A meaning-only result is strong when its z-score clears a threshold**
  (z = cosine against this query's own distribution, as in `search::zfusion`),
  with a separate, stricter threshold for images: SigLIP puts a few photos
  above z = 2 for any text query.
- **The scale** does not need the full scan `search::zfusion` does. Options:
  the mean and spread of a fixed random sample of ~500 vectors per table
  (cheap, no 4,096 KNN cap), or the gap between the top hit and the rest of
  the fetched 100. Measure which separates 0002's `neg` queries better.
- **Search as you type:** apply the label once typing pauses, or above a
  minimum query length, so a half-typed word is not labelled weak and then
  strong.

**Where it lives**

- `magi-core`: each `SearchHit` gains a strength (`Strong` / `Weak`) and
  the response a "no strong result" flag. Host API and DTO change: update
  `docs/architecture.md` "Host API" and SPEC §5.6/§5.7.
- `apps/desktop`: renders the sections and the header only; strings in en
  and es. View logic in plain functions with tests, as the other views.

Works with RRF as it ships today; it does not wait for GitHub #11. If #11's
reranker is adopted, it shares the z-scale.

## Calibrate before shipping

With 0002's measures, on all three query files:

- **False-weak rate** (an expected file under the fold) near zero: this is
  the number to protect. Today's 0.5x-top fold misses this.
- **`neg` labelled "nothing closely related"**: as high as possible
  without raising the false-weak rate.
- Check the chosen thresholds on the fresh queries (0002) before deciding,
  and record the numbers and decision in `docs/eval.md`.

## Done when

Thresholds chosen from measured numbers, the label shipped behind them, the
Host API change documented, and the before/after recorded in `docs/eval.md`.
