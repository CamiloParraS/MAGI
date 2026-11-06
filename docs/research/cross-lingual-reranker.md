# Cross-lingual reranker candidates (ONNX, CPU)

Research for issue 0001 (`docs/archive/0001-cross-lingual-ranker.md`).
Gathered 2026-11-05 from Hugging Face repo metadata (model API, tree API with
LFS sha256 and sizes, model cards, `config.json`), the authors' papers and the
ONNX Runtime docs. No model weights were downloaded. Commits below are the
`sha` the model API returned on 2026-11-05; pin them, because several repos
are still changing (one was modified the same day).

URL shorthand: `HF/<repo>` is `https://huggingface.co/<repo>`;
`API/<repo>` is `https://huggingface.co/api/models/<repo>` (commit, licence
tags); `TREE/<repo>` is `https://huggingface.co/api/models/<repo>/tree/main?recursive=true`
(file sizes and LFS sha256). Every sha256 and size in this file was copied
from a TREE response; none was computed locally.

## Question and constraints

Which multilingual (at least EN + ES, ideally EN<->ES cross-lingual)
cross-encoder rerankers ship an ONNX export usable from ONNX Runtime, for
reranking the top 30-75 fused results per query on a CPU-only desktop?

Constraints from this repo:

- NFR-12: search peak ≤ 900 MB; currently 842-848 MB, the peak being ORT
  building the SigLIP text session (briefly ~2x its 443 MB file)
  (`docs/benchmarks.md`, "M5 — peak memory"). Headroom at the peak: ~55 MB.
- Measured costs in the same doc: e5-small int8 (118 MB file) peaks at
  ~225 MB working set; parsing the XLM-R `tokenizer.json` (17 MB) peaks at
  ~266 MB; the Gemma 256k `tokenizer.json` (34 MB) at ~373 MB.
- NFR-8: warm p95 ≤ 300 ms; currently 241 ms on the 100k-chunk bench
  (`docs/benchmarks.md`, 2026-11-05), so ~60 ms is left for a synchronous
  reranker.
- Reference machine: Ryzen 7 7445HS, 6 cores; ORT intra-op threads 4 on AC,
  2 on battery (`docs/benchmarks.md`).
- Licence must allow shipping in an MIT app (`Cargo.toml` `license = "MIT"`).

## Candidates

### 1. cross-encoder/mmarco-mMiniLMv2-L12-H384-v1

| Fact | Value | Source |
| --- | --- | --- |
| Repo (original and ONNX) | `cross-encoder/mmarco-mMiniLMv2-L12-H384-v1` | HF/cross-encoder/mmarco-mMiniLMv2-L12-H384-v1 |
| Commit | `1427fd652930e4ba29e8149678df786c240d8825` | API/cross-encoder/mmarco-mMiniLMv2-L12-H384-v1 |
| ONNX files | `onnx/model.onnx` 470,883,696 B, `3e9a03ed1e966f7c5288dd4230e3d6a9bf5e3a170a06f1f4241c5bca12c6487c` (fp32) | TREE |
| | `onnx/model_O1..O3.onnx` ~470.6-470.8 MB (fp32, graph-optimised); `onnx/model_O4.onnx` 235,460,006 B, `613b96ad598e707de925349280296d3fe0f792a03bb17ff4661b11781380a0c3` (precision of O1-O4 not stated in the repo) | TREE |
| | `onnx/model_qint8_avx512_vnni.onnx` 118,620,017 B, `1825907d6c1a9001ff78124780bbde20a614a8c3df3b63409cf3c72c6fe5c8b4`; `model_qint8_avx512.onnx` and `model_qint8_arm64.onnx` have the same sha256 (same bytes) | TREE |
| | `onnx/model_quint8_avx2.onnx` 118,620,016 B, `6c2513767fb63d008a4377bef7a7a3555433d9436342bb53e35a3a72ffc52d4b` | TREE |
| Tokenizer | `tokenizer.json` 17,082,660 B, `62c24cdc13d4c9952d63718d6c9fa4c287974249e16b7ade6d5a85e7bbb75626`; also `sentencepiece.bpe.model` | TREE |
| Licence (card metadata) | apache-2.0 | API tags; README front matter |
| Licence (LICENSE file) | none in repo | TREE |
| Training data terms | trained on mMARCO, "a machine translated version of MS MARCO" (card). mMARCO's card says Apache-2.0, but MS MARCO's site says "The MS MARCO datasets are intended for non-commercial research purposes only". **Unclear: owner to judge.** | HF/cross-encoder/mmarco-mMiniLMv2-L12-H384-v1; HF/datasets/unicamp-dl/mmarco; https://microsoft.github.io/msmarco/ |
| Architecture | `XLMRobertaForSequenceClassification`, 12 layers, hidden 384, FFN 1536, vocab 250,002, base `nreimers/mMiniLMv2-L12-H384-distilled-from-XLMR-Large` | config.json |
| Parameters | 118M (Jina's table) | HF/jinaai/jina-reranker-v2-base-multilingual README |
| Max sequence | `max_position_embeddings` 514, so 512 tokens | config.json |
| Languages | card lists en, ar, zh, nl, fr, de, hi, in, it, ja, pt, ru, es, vi; "performs also well for other languages" | README |
| Benchmarks | MKQA (nDCG@10, 26 langs) **53.37**; BEIR 45.40; MLDR 28.91 (Jina's run, protocol not described in the card) | HF/jinaai/jina-reranker-v2-base-multilingual README, "Evaluation" |
| ONNX quirks | Output: one logit per pair. `sbert_ce_default_activation_function` is `Identity`, so scores are raw logits; no sigmoid needed for ranking. Input names not verified (graph not downloaded); XLM-R with `type_vocab_size` 1, expect `input_ids`, `attention_mask`; check `session.inputs()` first thing in the spike. | config.json |

Notes: same body shape as the e5-small already shipped (12 x 384 x 1536,
verified in e5's config.json at the pinned revision) and the same XLM-R
vocabulary size; the qint8 file is within 0.3 MB of e5's
`model_qint8_avx512_vnni.onnx` (118,346,824 B). The tokenizer.json hash
differs from e5's (`0b44a9d7…`), so sharing e5's tokenizer needs a check
that the two encode identically.

### 2. Horizon-Labs/multilingual-reranker-small

| Fact | Value | Source |
| --- | --- | --- |
| Repo (original and ONNX) | `Horizon-Labs/multilingual-reranker-small` | HF/Horizon-Labs/multilingual-reranker-small |
| Commit | `23139fec6f1f448cdf89a5ac2d2c6bf5e8c7b398` (repo created 2026-10-01, last modified 2026-11-05) | API |
| ONNX files | `onnx/model.onnx` 563,099,934 B, `dfc9f8faff4f8fdff033a20692d0b20807b1d3dd842106ebe01da315fa127f63` (fp32) | TREE; repo `SHA256SUMS` agrees |
| | `onnx/model_quantized.onnx` 268,407,694 B, `f913154682468bfe92d6a7e02ed4e994e548c112fddc74452b573b17c949046e`. Card: "int8 embeddings"; `onnx/quantization_check.json` picks `gather_only`, i.e. only the embedding table is int8, the transformer body stays fp32 | TREE; README; quantization_check.json |
| Tokenizer | `tokenizer.json` 34,363,439 B, `7925f638b5444897efe3b4608eed56c1652f0ed39c2662c8302de1bc264c6088` (256k vocab; hash differs from SigLIP's tokenizer in the manifest) | TREE; config.json |
| Licence (card metadata) | apache-2.0 | API |
| Licence (LICENSE file) | Apache-2.0 LICENSE; NOTICE says the base mmBERT-small is MIT (`jhu-clsp/mmBERT-small` tag `license:mit`) | LICENSE, NOTICE in repo; API/jhu-clsp/mmBERT-small |
| Training data | FineWeb-2/FineWeb (ODC-BY), MIRACL train (Apache-2.0), WebFAQ (CC-BY-4.0); labels from Qwen3-Reranker-4B (Apache-2.0) | README "Training" |
| Architecture | `ModernBertForSequenceClassification`, 22 layers, hidden 384, intermediate 1152, mean pooling, alternating global / 128-token sliding attention, vocab 256,000 | config.json |
| Parameters | 141M (card title) | README |
| Max sequence | trained with 384 tokens per pair; `max_position_embeddings` 8192 | README; config.json |
| Cross-lingual | "Queries and passages can be in different languages"; 15% of non-English passages got English queries in training | README |
| Benchmarks (self-reported, 60 queries per MIRACL language) | MIRACL 18 langs nDCG@10 0.779 (bge-reranker-v2-m3 0.825, Qwen3-Reranker-0.6B 0.785); MIRACL es 0.739 (bge 0.781); ESCI Spanish 0.851 (bge 0.846); mean 0.847 (bge 0.866). No EN<->ES cross-lingual number published. | README "Evaluation"; `eval/this_model.json` |
| ONNX quirks | Inputs `input_ids`, `attention_mask` only; output `logits` shape [batch, 1]; `sigmoid(logit)` gives 0-1 (optional for ranking) | `code/train/export_onnx.py`; README |

Notes: a small, new publisher; all numbers are its own runs on small samples.
The int8 file keeps an fp32 body, so it is no faster than fp32.

### 3. Alibaba-NLP/gte-multilingual-reranker-base (ONNX via onnx-community)

| Fact | Value | Source |
| --- | --- | --- |
| Original | `Alibaba-NLP/gte-multilingual-reranker-base`, commit `8215cf04918ba6f7b6a62bb44238ce2953d8831c`; no ONNX files | API; TREE |
| ONNX repo | `onnx-community/gte-multilingual-reranker-base`, commit `ee64367e35a2db0da46bb6497e13a18f8bd585cb` (Transformers.js conversion) | API; README |
| ONNX files | `onnx/model.onnx` 1,257,814,850 B, `4712afa6eed2ba8c3f00bf5b90f37fa2c58e60f4f22cf37763d4a48f523ffcc9` (fp32) | TREE |
| | `model_fp16.onnx` 629,118,152 B, `baba50ee1c61dfc47601ca54518d325eab03560fd06c4b45685cc4ecc4514df4` | TREE |
| | `model_int8.onnx` = `model_quantized.onnx` 340,858,200 B, `ccf51dba7f8aa9205753761cfaa68c55f741792501463a3bf25d7e5bcdac7c35` | TREE |
| | `model_uint8.onnx` 340,858,200 B, `42593339249e2d0c3e3e0a28fe12c0b099d78ebc594e6d710db00d941580063f` | TREE |
| | `model_q4f16.onnx` 466,333,899 B, `d24aacb414ccd10944c00622eb2d36c7f90019a8c8bdb882c320dc20f746abec`; `model_q4.onnx` 875,616,045 B, `49d20e45df2058fae9677f3366775ebbbe4b2cb9b15b37e3872a55d8ac72c6b4`; `model_bnb4.onnx` 868,538,541 B, `fc410a8ff394f597b653f30c54d9b1cb74e0f0337922544101bd144271cfe2f9` | TREE |
| Quantisation settings | per_channel, reduce_range, symmetric (`quantize_config.json`) | onnx-community repo |
| Tokenizer | `tokenizer.json` 17,082,999 B, `3ffb37461c391f096759f4a9bbbc329da0f36952f88bab061fcf84940c022e98` (XLM-R family, vocab 250,048) | TREE; config.json |
| Licence (original metadata) | apache-2.0 | API |
| Licence (LICENSE file) | none in either repo | TREE |
| Licence (ONNX repo metadata) | **none stated**; derived from an Apache-2.0 model | API/onnx-community/gte-multilingual-reranker-base |
| Architecture | `NewForSequenceClassification` (Alibaba's custom "new" model, RoPE, `trust_remote_code`), 12 layers, hidden 768, FFN 3072 | config.json |
| Parameters | 306M (card); 304M (paper Table 5) | README; arXiv 2407.19669 |
| Max sequence | 8192 | README |
| Benchmarks | Paper Table 5 (rerank of their dense model's candidates): MKQA recall@20 (25 langs) 67.2, MIRACL nDCG@10 68.5, MLDR 78.7, BEIR 55.4; same table: bge-reranker-v2-m3 68.7 / 72.6 / 66.8 / 54.6, jina v2 68.8 / 65.8 / 53.2 / 49.7. Qwen3's card: MMTEB-R 59.44. | HF/Alibaba-NLP/gte-multilingual-reranker-base `images/mgte-reranker.png`; https://arxiv.org/abs/2407.19669; HF/Qwen/Qwen3-Reranker-0.6B README |
| ONNX quirks | Output logits flattened with `.view(-1)` in the card; raw scores. Input names not verified; custom architecture, so check whether `token_type_ids` is an input. | README; config.json |

### 4. BAAI/bge-reranker-v2-m3 (ONNX via onnx-community)

| Fact | Value | Source |
| --- | --- | --- |
| Original | `BAAI/bge-reranker-v2-m3`, commit `953dc6f6f85a1b2dbfca4c34a2796e7dde08d41e`; safetensors only | API; TREE |
| ONNX repo | `onnx-community/bge-reranker-v2-m3-ONNX`, commit `6f5ff65298512715a1e669753bc754d2bc8f367b` ("automatically converted") | API; README |
| ONNX files | fp32 split: `onnx/model.onnx` 656,891 B, `faae32b124a9d54afb7e89b5e9896e03c18a9552d56d1d6b273a709a83012486` + `onnx/model.onnx_data` 2,271,088,656 B, `f009aa6c6cf21986fd7e0021fa66b20ccce27abc6900a57c7109c8496811bcbe` | TREE |
| | `model_fp16.onnx` 1,136,209,678 B, `9b26f9185d2d26051f724c9fc3565e5d9614a9dd459b784c806c246232a70fe3` | TREE |
| | `model_int8.onnx` = `model_quantized.onnx` 570,727,094 B, `912fc1215c2dbff6499700534bd8d31253af01573861abbfc43afd1fab6cce5d`; `model_uint8.onnx` same size, `753fd4a83b13c66f06c3cdc8734397399b7301274900f34a36afa68589a4c1f9` | TREE |
| | `model_q4f16.onnx` 702,120,377 B, `0bf2918eae2d46df72bc2a140f0d49fd1cf62b20d18a6b7a256f956dba142776`; `model_q4.onnx` 1,252,526,149 B; `model_bnb4.onnx` 1,233,652,957 B | TREE |
| Tokenizer | `tokenizer.json` 17,082,900 B, `8bf8afbfd11306bd872018c53bfdf2e160a56f8edbcf49933324404791c148d3` | TREE |
| Licence | original: apache-2.0 metadata, no LICENSE file; ONNX repo: **no licence stated** | API; TREE |
| Architecture | `XLMRobertaForSequenceClassification` from bge-m3, 24 layers, hidden 1024, FFN 4096 | config.json |
| Parameters | 568M | Jina and mGTE tables |
| Max sequence | `max_position_embeddings` 8194; evaluated at inference length 1024 | config.json; `assets/miracl-bge-m3.png` |
| Benchmarks | MIRACL avg 72.84, es 63.71, en 63.45 (BAAI table, metric not labelled in the image); MKQA nDCG@10 54.17 (Jina's table); MMTEB-R 58.36 (Qwen3 card) | HF/BAAI/bge-reranker-v2-m3 README "Evaluation"; Jina README; Qwen3 README |
| ONNX quirks | One raw logit per pair; card: "can be mapped to a float value in [0,1] by sigmoid function". fp32 needs `model.onnx_data` beside `model.onnx` (external data). | README; TREE |

### 5. jinaai/jina-reranker-v2-base-multilingual — **non-commercial**

| Fact | Value | Source |
| --- | --- | --- |
| Repo (original and ONNX) | `jinaai/jina-reranker-v2-base-multilingual`, commit `9cfeff2df7d40d1b78e75e5e9cebec92a99813c9` | API |
| ONNX files | `onnx/model.onnx` 1,114,040,223 B (fp32); `model_fp16.onnx` 557,164,815 B; `model_int8.onnx` = `model_quantized.onnx` 279,577,152 B, `c5220cf8fe023f8aa0ed2a3eb787d4451a7f17cf53f6b787e35718dd4b8815c3`; `model_uint8.onnx` 279,577,151 B; `model_q4.onnx` 827,392,882 B; `model_bnb4.onnx` 822,084,838 B | TREE |
| Tokenizer | `tokenizer.json` 17,082,734 B | TREE |
| Licence | metadata **cc-by-nc-4.0**; card: "licenced for research and evaluation purposes under CC-BY-NC-4.0. For commercial usage, please refer to Jina AI's APIs…". No LICENSE file. | API; README |
| Architecture | XLM-R with Jina's custom flash-attention code, 12 layers, hidden 768; 278M params | config.json; README |
| Max sequence | 1024 | README |
| Benchmarks | MKQA nDCG@10 54.83 (26 langs), BEIR 53.17 (own table); MMTEB-R 63.73 (Qwen3 card) | README; Qwen3 README |
| ONNX quirks | Card's example applies `sigmoid()` to logits | README |

**Excluded: CC-BY-NC-4.0 is incompatible with shipping in magi.**

### 6. Qwen/Qwen3-Reranker-0.6B (ONNX via onnx-community)

| Fact | Value | Source |
| --- | --- | --- |
| Original | `Qwen/Qwen3-Reranker-0.6B`, commit `e61197ed45024b0ed8a2d74b80b4d909f1255473`, apache-2.0 metadata, no LICENSE file | API; TREE |
| ONNX repo | `onnx-community/Qwen3-Reranker-0.6B-ONNX`, commit `9995c50e2310679108a55f5ccd16ba8be9f17c20`, apache-2.0 metadata | API |
| ONNX files | `onnx/model_quantized.onnx` 1,219,344,796 B, `c9428382bb48bb31e01a6034647c86d6270761781735cafbf6d5cb4a396d0450` (int8, MatMul/Gemm only); `onnx/model_q4.onnx` 995,235,866 B, `787193952a9aa58374381ec7647b160ac6c200a386dbfc3bc998a3c458081330` (MatMulNBits). No fp32/fp16. | TREE; README |
| Tokenizer | `tokenizer.json` 11,422,654 B (Qwen BPE) | TREE |
| Architecture | `Qwen3ForCausalLM`, 28 layers, hidden 1024, vocab 151,669; 0.6B params; 32K context | config.json; Qwen README |
| Benchmarks | MMTEB-R 66.36, MTEB-R 65.80, MLDR 67.28 (Qwen's run, top-100 from Qwen3-Embedding-0.6B) | Qwen README |
| ONNX quirks | Not a classifier: chat prompt per pair, read "yes"/"no" logits at the last position, softmax. Output is full-vocabulary logits over every position ([batch, seq, 151,669]); graph uses `com.microsoft.GroupQueryAttention`; left padding | onnx-community README |

**Excluded for this budget:** 1.0-1.2 GB files and per-position full-vocab
logits (50 pairs x 256 tokens x 151,669 floats would be ~7.8 GB unless run
one pair at a time).

### Checked and excluded

- `mixedbread-ai/mxbai-rerank-xsmall-v1` (commit `b5c6e9da…`, Apache-2.0
  LICENSE file, ONNX int8 87,245,802 B) and `mxbai-rerank-base-v1`: card
  metadata `language: en` only. English-only.
- `cross-encoder/msmarco-MiniLM-L6-en-de-v1` / `-L12-en-de-v1`: English-German
  only (by name; not read further).
- `Horizon-Labs/multilingual-reranker-base` (308M, commit `7330f6d0…`, int8
  641,122,836 B): too large for the budget; quality +0.008 mean over small.
- `Xenova/bge-reranker-v2-m3`: the model API returned an error (does not
  exist). Third-party int8 re-uploads of bge-reranker-v2-m3 and mmarco exist
  (e.g. `tss-deposium/…`, `temsa/…`); not considered, since the originals
  ship ONNX.

## Comparison

| Model | Params | Body (non-embedding) params, est. from config | Smallest usable ONNX | Tokenizer | Licence | Cross-lingual evidence |
| --- | --: | --: | --- | --- | --- | --- |
| mmarco-mMiniLMv2-L12-H384 | 118M | ~21M | qint8 118.6 MB | XLM-R 17 MB | Apache-2.0 weights; MS MARCO terms unclear | MKQA nDCG@10 53.37 |
| Horizon multilingual-reranker-small | 141M | ~42M | int8-embeddings 268 MB (fp32 body) | Gemma-style 34 MB | Apache-2.0 + MIT base | trained cross-lingual; no XL number |
| gte-multilingual-reranker-base | 306M | ~85M | int8 341 MB | XLM-R 17 MB | Apache-2.0 (ONNX repo: none stated) | MKQA recall@20 67.2 |
| bge-reranker-v2-m3 | 568M | ~302M | int8 571 MB | XLM-R 17 MB | Apache-2.0 (ONNX repo: none stated) | MKQA nDCG@10 54.17; recall@20 68.7 |
| jina-reranker-v2-base-multilingual | 278M | ~85M | int8 280 MB | XLM-R 17 MB | **CC-BY-NC-4.0** | MKQA nDCG@10 54.83 |
| Qwen3-Reranker-0.6B | 596M | ~440M | q4 995 MB | Qwen 11 MB | Apache-2.0 | MMTEB-R 66.36 |

Body params: per layer 4·H² (attention) + 2·H·FFN (MLP), times layers, from
each config.json; Horizon's ModernBERT MLP counted as gated (H·2·FFN +
FFN·H). These are my arithmetic, not published numbers.

On the one cross-lingual benchmark reported for most of them by the same
party (MKQA, Jina's table), mMiniLM is within 0.8 nDCG@10 of bge-reranker-v2-m3
and 1.5 of Jina v2, while being far behind on monolingual English (BEIR 45.4
vs 53-54). The mGTE paper's MKQA numbers (recall@20) put gte, bge and jina
within 1.6 points of each other.

## Latency and memory: estimates

**These are estimates, not measurements.** The spike must measure them.

Anchor from this repo: e5-small int8 has the same 12 x 384 x 1536 body as
mMiniLM, and indexing ran at ~7 chunks/s (chunks up to 400 tokens) on battery
with 2 threads, ~1.6x faster once batching stopped padding short chunks
(`docs/benchmarks.md`, M5 indexing). That is very roughly 2,000-4,500
tokens/s for a 21M-body int8 model on 2 threads; assume ~2x on AC with 4
threads.

Cost of reranking 50 pairs scales with total tokens (50 x pair length) times
body size:

| Model | 50 x 64 tokens | 50 x 128 tokens | 50 x 256 tokens |
| --- | --: | --: | --: |
| mMiniLM qint8 (~21M body) | ~0.3-0.8 s | ~0.7-1.6 s | ~1.5-3 s |
| Horizon small (~42M body, fp32) | ~1-3 s | ~2-6 s | ~4-12 s |
| gte / jina base int8 (~85M body) | ~1.2-3 s | ~3-6 s | ~6-12 s |
| bge-reranker-v2-m3 int8 (~302M body) | ~4-11 s | ~9-22 s | ~20-45 s |

Ratios come from body size; fp32 bodies are assumed ~2-4x slower than int8
on this CPU. The ranges are wide because the anchor includes indexing
overhead and padding. Even if the anchor is pessimistic by 5x, the smallest
model at 64 tokens is above 60 ms.

Memory, using the repo's measured ratios (e5 int8 118 MB file -> ~225 MB
peak; SigLIP text session build ~2x file; XLM-R tokenizer parse ~266 MB
peak, Gemma tokenizer ~373 MB):

| Model | Session build peak (est.) | Resident after build (est.) | Tokenizer parse peak |
| --- | --: | --: | --: |
| mMiniLM qint8 | ~225-240 MB | ~120-150 MB | ~266 MB, or 0 if e5's tokenizer can be shared |
| Horizon small int8-emb | ~540 MB | ~270-300 MB | ~373 MB |
| gte int8 | ~680 MB | ~340-380 MB | ~266 MB |
| bge-v2-m3 int8 | ~1.1 GB | ~570-620 MB | ~266 MB |

What this implies:

1. **No candidate fits a synchronous 60 ms slot** for 30-75 candidates.
   Reranking has to be progressive: show the fused results at once, rerank
   the top N on a background thread, then re-sort (or rerank only a
   precomputed short text per file, e.g. filename + title + first ~48
   tokens). Warm p95 for first results stays as today; the reranked order
   arrives roughly 0.3-3 s later for mMiniLM-size models.
2. **Only the ~118 MB class fits NFR-12** without other changes. With 845 MB
   of 900 MB used at the SigLIP build peak, the reranker must be built after
   that peak has passed (lazily, after the first results), and its tokenizer
   should be e5's if the two encode identically. Anything ≥ 270 MB (Horizon
   small, gte, jina, bge) needs NFR-12 renegotiated or another model unloaded
   first.
3. Window size matters as much as model size: issue 0001's misses sit at
   ranks 14-27 (one at 74). Reranking the top 30 with short inputs covers six
   of seven at less than half the cost of top 75.

ONNX Runtime notes relevant to the choice: dynamic quantisation is the
recommended mode for transformer models; x86-64 with VNNI gets better int8 performance (whether
the reference CPU's VNNI path is used was not checked), and `reduce_range` is "not needed
on machines supporting VNNI"; int4 uses the `MatMulNBits` contrib op
(https://github.com/microsoft/onnxruntime/blob/gh-pages/docs/performance/model-optimizations/quantization.md).
mMiniLM's `qint8_avx512_vnni`, `qint8_avx512` and `qint8_arm64` files are the
same bytes, so one file serves all three platforms; `quint8_avx2` differs.

## Ranked short list

1. **cross-encoder/mmarco-mMiniLMv2-L12-H384-v1, qint8.** Only candidate
   that plausibly fits NFR-12; same shape and size as e5-small, which the
   repo already runs; MKQA (cross-lingual) close to models 5x its size.
   Weak on English BEIR. Licence: weights Apache-2.0, but the training data
   derives from MS MARCO, whose terms say non-commercial research only:
   **owner to decide whether that matters.**
2. **Horizon-Labs/multilingual-reranker-small.** Trained explicitly for
   cross-lingual pairs, clean licence chain (Apache-2.0, MIT base, listed
   data licences), better self-reported multilingual scores. Against it: new
   publisher (repo created 2026-10-01, changed on 2026-11-05), self-reported
   evals on 60-query samples, 268 MB file with fp32 body, 34 MB tokenizer:
   ~2x mMiniLM's compute and over the memory budget.
3. **Alibaba-NLP/gte-multilingual-reranker-base via onnx-community, int8.**
   Strong peer-reviewed numbers (EMNLP 2024 Industry), Apache-2.0 original.
   ~4x mMiniLM compute, 341 MB file; would need NFR-12 renegotiated. ONNX
   repo carries no licence statement of its own; custom architecture.

bge-reranker-v2-m3 is the quality reference but ~14x mMiniLM's compute and a
571 MB int8 file: not a CPU desktop candidate here. Jina v2 is excluded by
CC-BY-NC-4.0; Qwen3-Reranker-0.6B by size and output shape.

## Recommendation

Spike `cross-encoder/mmarco-mMiniLMv2-L12-H384-v1` first, as a progressive
(after-first-results) rerank of the top 30, and the top 75 for comparison,
over a short per-file text. Record cross-bucket recall@10/MRR, the
rerank's own latency and the peak RSS, as issue 0001 asks. If it does not
fix the cross misses, the next step is Horizon small (quality) and a
decision on NFR-12.

Files the owner would need to approve (all at commit
`1427fd652930e4ba29e8149678df786c240d8825`):

| File | URL | sha256 | Size (bytes) |
| --- | --- | --- | --: |
| model | https://huggingface.co/cross-encoder/mmarco-mMiniLMv2-L12-H384-v1/resolve/1427fd652930e4ba29e8149678df786c240d8825/onnx/model_qint8_avx512_vnni.onnx | `1825907d6c1a9001ff78124780bbde20a614a8c3df3b63409cf3c72c6fe5c8b4` | 118,620,017 |
| tokenizer | https://huggingface.co/cross-encoder/mmarco-mMiniLMv2-L12-H384-v1/resolve/1427fd652930e4ba29e8149678df786c240d8825/tokenizer.json | `62c24cdc13d4c9952d63718d6c9fa4c287974249e16b7ade6d5a85e7bbb75626` | 17,082,660 |
| licence | Apache-2.0 per card metadata; no LICENSE file in the repo; MS MARCO data terms: see above | n/a | n/a |

The sha256 values are Hugging Face's LFS oids from the tree API; per
`models/manifest.toml` practice, recompute them locally after download.

Open items for the spike: confirm the ONNX input names (expected
`input_ids`, `attention_mask`); confirm the output is one raw logit per pair;
check whether e5's `tokenizer.json` encodes identically (to share it and save
~266 MB of parse peak); measure how many tokens the per-file text needs.

## Sources

- Model API / tree API for each repo named above (`API/…`, `TREE/…`), 2026-11-05.
- Model cards and config.json at the pinned commits: HF/cross-encoder/mmarco-mMiniLMv2-L12-H384-v1, HF/Horizon-Labs/multilingual-reranker-small (also LICENSE, NOTICE, SHA256SUMS, `code/train/export_onnx.py`, `onnx/quantization_check.json`, `eval/this_model.json`), HF/Alibaba-NLP/gte-multilingual-reranker-base, HF/onnx-community/gte-multilingual-reranker-base, HF/BAAI/bge-reranker-v2-m3, HF/onnx-community/bge-reranker-v2-m3-ONNX, HF/jinaai/jina-reranker-v2-base-multilingual, HF/Qwen/Qwen3-Reranker-0.6B, HF/onnx-community/Qwen3-Reranker-0.6B-ONNX, HF/mixedbread-ai/mxbai-rerank-xsmall-v1, HF/intfloat/multilingual-e5-small (config.json at `614241f6…`).
- mGTE paper, Table 5: https://arxiv.org/abs/2407.19669
- BGE-M3 paper, MKQA setup ("queries in 25 non-English languages", passages from "the English Wikipedia corpus"): https://arxiv.org/abs/2402.03216
- mMARCO dataset card: https://huggingface.co/datasets/unicamp-dl/mmarco
- MS MARCO terms: https://microsoft.github.io/msmarco/
- ONNX Runtime quantisation docs (gh-pages source; onnxruntime.ai did not resolve from here): https://github.com/microsoft/onnxruntime/blob/gh-pages/docs/performance/model-optimizations/quantization.md
- This repo: `docs/benchmarks.md` (NFR-8, NFR-12, indexing throughput), `docs/archive/0001-cross-lingual-ranker.md`, `models/manifest.toml`.
