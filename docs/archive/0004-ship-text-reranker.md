# 0004: Ship the text reranker (and the "nothing closely related" label for text)

Status: closed, not shipped (2026-11-05). The owner deferred the reranker:
ADR-0013, "Why deferred". Kept as the plan to pick up if it is revisited.

## What

1. **Licence call first.** `cross-encoder/mmarco-mMiniLMv2-L12-H384-v1` is
   Apache-2.0 but trained on MS MARCO data whose site says "non-commercial
   research purposes only". The owner decides whether that is acceptable
   for Magi before anything else here starts.
2. **Model as an optional feature** (ADR-0010): add it to
   `models/manifest.toml` with the approved URLs, sha256 and sizes from
   ADR-0013. "Better cross-language ranking" depends on "search by meaning".
   Update `THIRD_PARTY_LICENSES.md` (`licenses/models.md`, `just licenses`).
3. **Core:** move `search::rerank` out of spike status. Text results (not
   images) in the top 30 are re-sorted by blending RRF's rank with the
   model's (option 3 of ADR-0013), 256 tokens per pair. The session is
   built only after SigLIP's query session is released (NFR-12: 842 MB in
   that order, 998 MB otherwise), and unloads when idle like the others.
4. **Host API:** search returns RRF's list at once, then pushes a reranked
   list for the same query. Each text hit carries its strength; the response
   says whether any text hit is strong. Update `docs/architecture.md` "Host
   API" and SPEC §5.6/§5.7.
5. **Desktop:** apply the re-sort without moving the selected row or the row
   under the pointer; label per 0003 (text hits below logit −4 fold under
   "Weaker matches"; "Nothing closely related found" when no hit is strong).
   en and es strings.

## Verify

- `just eval` / `just eval-random` / `just eval-random realistic_names`
  through the shipped path match ADR-0013's option 3.
- NFR-12 with the reranker (method in `docs/benchmarks.md`), NFR-2 for the
  first list unchanged, and the rerank's own p95 recorded.
- Images: decide their strength signal, or leave images unlabelled and say
  so in the UI copy.
