# ADR-0006: OCR engine — PaddleOCR PP-OCRv5 (ONNX) over `ocrs`

- **Status:** Accepted; implemented in `ocr/paddle.rs`
- **Milestone:** M4

## Context

SPEC.md §7 M4 asks for an OCR spike (`ocrs` vs. PaddleOCR-ONNX) and requires
accented characters (á é í ó ú ñ ¿ ¡) to appear in the output.

## Decision

Use **PaddleOCR PP-OCRv5 mobile** detection + **Latin** recognition, run
through the existing `ort` dependency, behind `ocr::OcrEngine`. `ocrs` is not
implemented.

`ocrs` 0.13.1 was ruled out on inspection, not by benchmark: its
`DEFAULT_ALPHABET` (`src/lib.rs:34`) is ASCII only — no á é í ó ú ñ ¿ ¡ — so
it cannot meet the accent requirement without a retrained recognizer. It would
also add `rten` as a second inference runtime beside `ort`.

## Pinned sources (official `PaddlePaddle` org, Apache-2.0)

| File | Repo @ revision | Size | SHA-256 |
| --- | --- | --- | --- |
| det `inference.onnx` | `PaddlePaddle/PP-OCRv5_mobile_det_onnx` @ `e6f4fa85f00e168c862bc462aebca69eef9b3d3d` | 4,826,518 | `a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d` |
| rec `inference.onnx` | `PaddlePaddle/latin_PP-OCRv5_mobile_rec_onnx` @ `89d3a50e2c27e2e7cceeab0e944c25c807d5db4f` | 8,042,023 | `7888113072263cb471b93f66dd5e2ad70548dc526fa1ace760d0d973dd121498` |
| rec `inference.yml` (character dict, 836 entries) | same as rec | 6,817 | — |

Hashes were computed locally on the downloaded files and match the Hugging Face
LFS values. Total ~12.9 MB.

## Spike results (Python + onnxruntime CPU, 4 threads, reference-style pipeline)

| Fixture | CER | Time/image |
| --- | --- | --- |
| `screenshot_en.png` | 0.000 | 0.09 s |
| `screenshot_es.png` | 0.000 | 0.5 s |
| `phone_text_es.jpg` (photo of a screen) | 0.027 | 0.5 s |

SPEC target is CER ≤ 10% on the two screenshots: met. Every accent in
`screenshot_es` is recognized (á é í ó ú ñ ü ¿ « » —). Process RSS with both
models loaded and after several images: ~200–230 MB including the Python
runtime, so the Rust figure will be lower. These are **Python numbers**; the
Rust engine must be re-measured against the M4 RSS budget.

## Known gap

The Latin dictionary contains `¿` but **not `¡`**. Opening exclamation marks
are dropped (`¡Hola!` → `Hola!`). Accepted: the SPEC accent list is otherwise
covered and search is unaffected in practice. Revisit with a later PP-OCR
release or a custom dictionary.

## Rust engine results (release build, 2 intra-op threads, full-size decode)

| Fixture | CER | Time/image |
| --- | --- | --- |
| `screenshot_en.png` | 0.031 | 0.1 s |
| `screenshot_es.png` | 0.000 | 1.3-1.5 s |
| `phone_text_es.jpg` | 0.022 | 1.0 s |
| `phone_text_es.heic` | 0.013 | 0.8-1.5 s |

`screenshot_en`'s 0.031 is one stray `T` line detected in the anime
illustration; the caption is exact. The pipeline feeds OCR a copy capped at
2048 px, so photos will be faster than the 12 MP figures here. Peak RSS of the
Rust engine has **not** been measured yet (owed with M4's other RSS budgets).

Two things the port taught us, both fixed and worth keeping:

- An axis-aligned box per text line collapsed CER on the tilted phone photo to
  0.85-0.94: the tilt inflates the box, unclip then overlaps neighbouring
  lines. Detection now fits a rotated rectangle (principal axis) and crops it
  straight.
- Detection scores each candidate over its rotated rectangle, not its pixels.

Known ceiling: the rectangle is a PCA fit, not the reference's minimum-area
rectangle, and curved or perspective-skewed text is cropped as a straight strip.

## Memory and the hard photos (2026-09-20)

**Peak RSS**, Rust engine through the real `extract_image` path (2048 px OCR
cap), one process running seven fixtures back to back, including the 12 MP
phone HEICs and the receipt, polled every 10 ms:

| | Peak RSS |
| --- | --- |
| Image pipeline, OCR off (`NoOcr`) | 576 MB |
| Image pipeline, OCR on | 885 MB |

So loading and running OCR costs **about 310 MB of peak RSS**. That is the
OCR-only figure to budget: it does not include e5 (ADR-0005) or SigLIP, and
the 1.5 GB NFR-11 check has to be taken with all models loaded in M4's final
slice. Latency is 0.7-1.5 s per image, 3.7 s for the receipt (many lines).

**Hard samples**, character error rate against the local-only ground truth
(`fixtures/golden/ocr/personal.md`), Rust vs. the Python reference pipeline
on the same 2048 px input:

| Fixture | Rust | Python reference |
| --- | --- | --- |
| `receipt_es.jpg` (thermal receipt photo) | 0.773 | 0.784 |
| `phone_12mp_portrait.heic` | 0.401 | 0.339 |
| `phone_12mp_landscape.heic` | 0.463 | 0.451 |

The two pipelines agree, so these numbers are the model's limit on these
photos, not a porting bug. SPEC.md §7 M4 only requires CER on the two
screenshots (met); the receipt was always the hard extra sample. It reads
about 60-70% of its characters. Search over it is still useful (the words that
are read are indexed), but do not expect line-item accuracy.

## Not yet done

- The 1.5 GB total-RSS check with every model loaded (needs SigLIP).
