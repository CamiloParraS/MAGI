# magi docs

**Contract and design**

- [`SPEC.md`](SPEC.md): the product spec and milestone roadmap (§7). Wins over
  everything else until updated.
- [`architecture.md`](architecture.md): the current contract per module (DB
  schema, Host API, config, search).
- [`adr/`](adr/): architecture decision records.
- [`PRODUCT.md`](PRODUCT.md) and [`DESIGN.md`](DESIGN.md): who magi is for and
  its visual design system. The root `PRODUCT.md` / `DESIGN.md` are stubs for
  design tooling; edit these.
- [`permissions.md`](permissions.md): what each OS asks for and why.

**Progress and measurements**

- [`progress.md`](progress.md): milestone log and verification evidence
  (read the last `##` sections).
- [`eval.md`](eval.md): search-quality results and ranking decisions.
- [`benchmarks.md`](benchmarks.md): latency and memory measurements.
- [`qa-checklist.md`](qa-checklist.md): manual QA per OS.
- [`CHANGELOG.md`](CHANGELOG.md): user-facing changes per release.

**Open work**

- [`issues/`](issues/): plans and tickets written up as files
  (`m7-plan.md`; `0001` cross-lingual ranker, `0002` negative queries and
  precision in the eval, `0003` "nothing closely related" label). GitHub
  issues track the rest (#11: the z-score reranker).

**History**

- [`archive/`](archive/): finished plans and superseded docs, kept for the
  record: the Tauri-era spec (`SPEC-v1.1-tauri.md`), the M5 plan, the
  indexing performance investigation and the prior-art survey. Code comments
  still cite `perf-investigation.md` for measured constants.
- [`brand/`](brand/), [`screenshots/`](screenshots/): logo prototype and
  milestone screenshots.
