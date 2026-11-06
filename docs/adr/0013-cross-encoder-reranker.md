# ADR-0013: Cross-encoder reranking of text results (deferred)

- **Status:** Deferred (2026-11-05). The spike passed its eval rule; the owner decided not to ship it now (see "Why deferred"). The shipping plan is kept in `docs/archive/0004-ship-text-reranker.md`.
- **Milestone:** none
- **Answers:** `docs/archive/0001-cross-lingual-ranker.md` (the spike's go/no-go), and gives `docs/issues/0003-no-close-matches-label.md` its signal.

## Context

With the same-language twin removed, a third of the `cross` queries miss the top 10 on the renamed corpora. `multilingual-e5-small` ranks the other-language document 14th to 74th, and no other list finds it (0001). Two cosine-based fixes were measured and rejected on 2026-11-05: the z-score reranker of GitHub #11 (worse on fresh queries, over the latency budget) and a cosine-based "nothing closely related" label (no threshold separates absent answers from weak right ones). Both are in `docs/eval.md`.

A cross-encoder reads the query and the document together and returns one relevance score. Candidates were researched in `docs/research/cross-lingual-reranker.md`. The owner approved the first one on 2026-11-05: `cross-encoder/mmarco-mMiniLMv2-L12-H384-v1`, qint8 ONNX (118,620,017 bytes, sha256 `1825907d…c8b4`), with its `tokenizer.json` (17,082,660 bytes, sha256 `62c24cdc…5626`), at commit `1427fd652930e4ba29e8149678df786c240d8825`. Both hashes were re-computed locally. Its inputs are `input_ids` and `attention_mask`, and it returns one raw logit per pair.

## Options measured

All on the three eval corpora, 195/194/194 positive queries (31 of them fresh) and 18 `neg` queries, against RRF on the same lists (`docs/eval.md`, "2026-11-05: cross-encoder spike"):

1. **Rerank every result of the top 30 or 75.** `cross` MRR goes to 0.93–1.00, but the model cannot judge a photo: `img2` MRR falls from ~0.9 to 0.05–0.37 and overall MRR from ~0.87 to 0.54–0.71. Scoring 30 results takes ~2.4 s (p50), and 75 results ~6.2 s. Rejected.
2. **Rerank only the text results of the top 30, by the model's order alone.** Photos keep their places. `cross` and overall rise, but `es` (0.94–0.96 → 0.83–0.88) and `kw` drop: the model is happy to put the English twin above the Spanish answer. Rejected under the "no bucket regresses" rule.
3. **Rerank only the text results of the top 30, blending RRF's order with the model's** (RRF of the two ranks). No bucket regresses on any corpus. Overall MRR +0.013 to +0.016, `cross` +0.07 to +0.09, fresh queries +0.027 to +0.042. **Chosen.**

At 128 tokens per pair instead of 256, option 3 is 2–4x faster, but `en` drops slightly on two corpora, so 256 is kept.

## Decision

**Not shipped for now.** If it is ever shipped, option 3 is the design: after RRF, re-sort the text results (not images) among the top 30 by blending RRF's rank with the cross-encoder's, at 256 tokens per (query, file text) pair. Its logit is also the strength signal for 0003: a text result is strong when keyword search found it or its logit is at least −4. Images are not judged by it.

## Consequences

- **Latency: progressive, not inline.** Scoring the text results of a top 30 takes 135–617 ms p50 and 0.9–1.6 s p95 on the development machine, against ~60 ms left under NFR-2. The search window shows RRF's list first, then re-sorts when the scores arrive. Results must not jump under the pointer or a moved selection.
- **Memory (NFR-12): fits only in order.** A cold hybrid search plus the reranker peaks at 998 MB if SigLIP's session is still loaded. With SigLIP unloaded before the reranker session is built, it peaks at 842 MB, the same as without the reranker (845 MB). The shipped path must keep that order.
- **A fourth optional feature**, downloaded like the others (`models/manifest.toml`, ADR-0010), about 136 MB. It is only useful with "search by meaning" on.
- **Licence.** The weights are Apache-2.0, but they were trained on mMARCO, a translation of MS MARCO, whose site says it is "intended for non-commercial research purposes only". The owner accepted this model for the spike. Shipping it is a separate call to make in 0004.
- **0003 gets a usable signal for text.** At logit −4 the text results of 12, 16 and 17 of the 18 `neg` lists are labelled "nothing closely related", while 1, 0 and 1 found text answers (1, 0 and 0 fresh) would be labelled weak. Photos still need their own signal.

## Why deferred

The owner's call on 2026-11-05, on these grounds:

- **Small gain where it counts.** Overall MRR +0.013 to +0.016, mostly
  right answers moving from rank 2 to rank 1; recall@10 barely moves. The
  real gain is `cross`, a narrow case: a search in one language for a file
  written in the other.
- **Thin evidence.** Option 3 was picked after seeing these numbers, on 13
  synthetic topics where every document has a translated twin, and checked
  on only 31 fresh queries: the trap that sank GitHub #11.
- **Half a label.** The 0003 signal covers text only; photos, where most
  unrelated results come from, stay unjudged.
- **Cost.** A fourth optional download (~136 MB); results re-sorting 0.1 to
  1.6 s after they appear, with the UI work that needs; a memory limit that
  holds only if SigLIP is released first.
- **Licence.** The training data (MS MARCO) is "intended for non-commercial
  research purposes only"; for a distributed app that ambiguity alone is
  reason to wait.

**Revisit when** a multilingual cross-encoder trained on permissively
licensed data exists (`docs/research/cross-lingual-reranker.md` lists the
field as of 2026-11-05), a stronger multilingual embedder fits NFR-12, or a
faster target machine makes inline reranking possible. The spike code stays
as measurement only (`search::rerank`, `magi-cli eval` with
`MAGI_RERANKER_DIR`, `magi-cli search --rerank`), so a new candidate is
measured with the same tables in minutes.
