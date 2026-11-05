# 0001: Cross-language misses need a reranker or a stronger embedder

Status: open. Needs an ADR and a verified model before any download. Not
scheduled; ship only inside a milestone (M7 and M8 are still open).

## Problem

With the same-language twin removed, 7 of 20 `cross` queries on
`fixtures/realistic_names` still miss the top 10 (hybrid recall@10 0.650).
Cause: `multilingual-e5-small` (int8) ranks the right file low for short
queries in the other language. Keyword and visual lists are empty for all
seven, so fusion has nothing to fix. Prefixes, mean pooling, normalisation and
best-chunk-per-file were checked on 2026-10-18 and are correct
(`docs/eval.md`, "2026-10-18: image fusion weight").

Where the file sits in the text-vector list (`just eval-random
realistic_names`, per-list ranks printed for every hybrid miss):

| Query                                  | Rank |
| -------------------------------------- | ---: |
| informe de ingresos trimestrales       |   14 |
| notas de la reunion de equipo          |   17 |
| club de lectura discusion del libro    |   18 |
| team meeting notes                     |   21 |
| contrato de alquiler deposito          |   23 |
| doctor appointment reminder            |   27 |
| reporte de error de software           |   74 |

None is missing from the list, so this is a ranking problem, not a recall
problem of the index.

## Options

1. **Reranker** over the top 30-75 fused results. Six of seven fall in 14-27;
   the seventh needs a window near 75. No re-index. Costs query latency
   (NFR-8) and memory (NFR-12).
2. **Bigger embedder** (e.g. `multilingual-e5-base`, ~3x the small model).
   Needs a full re-index and risks the 845 MB peak (NFR-12; the ORT session
   build is the peak, mimalloc was rejected).

Recommendation: spike the reranker first.

## Spike (throwaway, not shipped)

- Verify with the owner before downloading: URL, checksum, licence, ONNX
  export and quantisation. Add it to `models/manifest.toml` only if kept.
- Measure on all three query files (`just eval`, `just eval-random`,
  `just eval-random realistic_names`), against the baselines in
  `docs/eval.md`:
  - `cross` recall@10 and MRR, then overall and every other bucket (must not
    regress; the image weight change of 2026-10-18 is the new baseline).
  - Warm and cold query latency (NFR-8), `docs/benchmarks.md` method.
  - Peak RSS (NFR-12), currently ~845 MB.
- Decide with numbers; write the result in `docs/eval.md` and an ADR.

## Done when

The spike's numbers and a go/no-go are recorded. A "go" becomes its own
ticket with milestone scope.
