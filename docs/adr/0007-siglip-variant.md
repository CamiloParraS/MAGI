# ADR-0007: SigLIP 2 variant - 256 px, q4f16 towers

- **Status:** Accepted, with one SPEC gate missed by 0.001 (see "Decision")
- **Milestone:** M4

## Context

SPEC.md section 7 M4 asks for the SigLIP 2 variant (resolution and
quantization) to be chosen on size, speed and eval results. Verification
requires cosine >= 0.99 against the reference for fp32 and >= 0.97 for a
quantized variant, on both towers, and image-query recall@5 within 3 points of
fp32. Memory is the binding constraint: e5 already sits at ~770 MB RSS
(ADR-0005) and OCR adds ~310 MB (ADR-0006), against NFR-11's 1.5 GB.

## Sources (onnx-community/siglip2-base-patch16-256-ONNX @ d1114256522a37ffa257a0a58017348ab0058db2)

The vision and text towers ship as separate ONNX files, so each is loaded only
when needed. Every file below was downloaded, hashed locally and matched to
Hugging Face's LFS sha256.

| File | Size | SHA-256 |
| --- | --- | --- |
| `vision_model.onnx` (fp32) | 371,992,072 | `f5cb16728a704703f05516ded628397e11dbca4de2eb5db04b0c0bcee988aa7a` |
| `vision_model_fp16.onnx` | 186,131,676 | `fe9ad8020a6d3d98d394c9be8f07064066135fc2f87ec11692de0b677c0ac4db` |
| `vision_model_int8.onnx` | 94,737,653 | `8243a496e524bf8fa19830ff42a40a4bae2e97acc2e288d4ae97601edf954977` |
| `vision_model_q4f16.onnx` | 54,723,009 | `e077a0e33fcc362a6e741a0c7571ba3044a4f8093bb0eaa9f93f54919191848a` |
| `text_model.onnx` (fp32) | 1,129,469,657 | `d3de4a6bbbfcb429b6615ac496790353cf4a4fc0f19fbbe7179e523ae60daaef` |
| `text_model_fp16.onnx` | 564,862,230 | `80954edffdc689599e5d5bc6a1738380bc9e8139a18e5c8892485f248b6b4890` |
| `text_model_int8.onnx` | 283,438,275 | `6f59b39d880c413042314b79302b74d0dd93b273caf8fbfdb1eb2df61a7fefd4` |
| `text_model_q4f16.onnx` | 442,779,433 | `a623a87e0fbaa31a4bfbbbb3deafea756fee9a436e945b545a027adb13824e48` |
| `tokenizer.json` | 34,363,039 | `cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322` |

Only 256 px was evaluated (224 px is 0.2% smaller in weights and was not worth
a second sweep); 384/512 px cost 2.25x/4x the image-tower compute.

## Measurements

Reference: `transformers` `AutoModel` (PyTorch fp32) for `google/siglip2-base-patch16-256`,
`get_image_features` / `get_text_features`, L2-normalized, text padded to 64
tokens. Each ONNX variant is compared on 15 fixture images and 25 English and
Spanish queries (`fixtures/reference_embeddings/siglip2.json` keeps the 10
non-HEIC, non-personal images). Recall is "an expected image appears in the top
k of the 15". RSS is the process delta from loading one tower (1 intra-op
thread, CPU arena off) and running one inference.

| Variant | Image cosine min / mean | Text cosine min / mean | Recall@1 | Recall@5 | Vision RSS | Text RSS |
| --- | --- | --- | --- | --- | --- | --- |
| reference (torch fp32) | - | - | 0.96 | 1.00 | - | - |
| fp32 | 1.0000 / 1.0000 | 1.0000 / 1.0000 | 0.96 | 1.00 | 366 MB | 1087 MB |
| fp16 | 1.0000 / 1.0000 | 1.0000 / 1.0000 | 0.96 | 1.00 | 372 MB | 719 MB |
| int8 | 0.637 / 0.815 | 0.900 / 0.962 | 0.76 | 1.00 | 108 MB | 288 MB |
| q4f16 | 0.952 / 0.969 | 0.994 / 0.997 | 0.96 | 1.00 | 80 MB | 447 MB |

Latency (Python onnxruntime, 4 threads): ~170 ms/image and ~45 ms/query for
fp32, fp16 and q4f16 alike; int8 is ~2x faster but unusable.

The only reference miss at rank 1 is the query `código QR`, which the
model answers with the Spanish-text screenshot ahead of the QR photos; the QR
chunk text in FTS covers that query in production.

## Decision

**256 px, q4f16 for both towers.**

- **int8 is rejected**: the dynamically quantized vision tower drifts badly
  (cosine 0.64 min), and recall@1 falls from 0.96 to 0.76.
- **fp16 is exact but too large**: its text tower costs 719 MB and its vision
  tower is no smaller in RSS than fp32 (ORT keeps the weights at 372 MB).
- **q4f16 matches fp32's retrieval** (recall@1 0.96, @5 1.00) at 80 MB
  (vision) and 447 MB (text). Its text parity passes SPEC's 0.97; its **image
  parity is 0.969 mean / 0.952 min, 0.001 under the 0.97 gate**. Retrieval is
  unaffected on this eval, but the gate is not met as written.

**Needs a human call:** accept the 0.001 miss (amend the gate to compare
retrieval, or to 0.95), or switch the *vision* tower to fp16 at ~+290 MB of RSS
during indexing, the phase with the least headroom. The choice is a manifest
edit, not a code change: every variant has identical inputs and outputs.

## Consequences

- The text tower is the memory problem at any precision: 447 MB even at 4-bit,
  because of the 256k-token Gemma vocabulary. It must load lazily and unload
  when idle, like e5, and the tokenizer (34 MB) must be parsed once.
- The 1.5 GB total-RSS check (NFR-11) with e5, OCR and SigLIP loaded together
  is still owed and may force the text tower to unload between queries.
- Reference preprocessing: bilinear resize to 256x256 (no crop, no aspect
  preservation), rescale 1/255, normalize mean = std = 0.5, RGB. Text: Gemma
  tokenizer, EOS appended, no BOS, padded with id 0 to exactly 64 tokens.
- The image embedding is the vision model's `pooler_output`; the text
  embedding is the text model's `pooler_output`. Both are L2-normalized.

## Follow-up: the visual list needs a cosine floor (2026-09-20)

Fusing `vec_image` into hybrid search exposed that a KNN always returns its
nearest rows: with no floor, every text query dragged the photo library into
the results and recall on text queries collapsed to keyword-only (docs/eval.md,
"M4: image queries"). `search::IMAGE_MIN_COSINE = 0.10` fixes it, chosen from
measurement: text queries never exceed 0.117 against any fixture image, true
visual matches have a median of 0.139 (weakest non-OCR ~0.105). Measured with
fp32 vectors; q4f16 cosines move by ~0.01, well inside that margin. **The
number belongs to this model and this small fixture set.** Recalibrate on any
model change and on a real photo library.
