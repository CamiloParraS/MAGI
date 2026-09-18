# /// script
# requires-python = ">=3.10"
# dependencies = ["sentence-transformers"]
# ///
"""Computes reference `intfloat/multilingual-e5-small` embeddings for a
fixed set of sentences, for `crates/magi-core`'s e5 parity test (SPEC.md
§7 M3: "cosine >= 0.99 for every fixture sentence"). Dev-only.

Run with `uv run tools/reference_embeddings.py`. Pinned to the same
Hugging Face revision as `models/manifest.toml`'s text slot, so the
Python (PyTorch, via sentence-transformers' own mean-pooling + L2-norm
modules) and Rust (ONNX, via `embed::e5::E5Embedder`) embeddings are
computed from the identical model weights and the identical pooling.
"""

import json
from pathlib import Path

from sentence_transformers import SentenceTransformer

REVISION = "614241f622f53c4eeff9890bdc4f31cfecc418b3"
MODEL_ID = "intfloat/multilingual-e5-small"

# (prefix, text) pairs: SPEC.md §7 M3's own cross-lingual smoke-test
# sentences plus a spread of short/long English/Spanish text, so parity
# coverage isn't limited to one language or sentence length.
SENTENCES = [
    ("query", "electrician invoice"),
    ("query", "arepas recipe"),
    ("query", "quarterly report revenue"),
    ("query", "cancion de bienvenida"),
    (
        "passage",
        "Factura de electricista: reparacion del panel electrico. Total: 150 euros.",
    ),
    (
        "passage",
        "Receta de arepas con queso: mezcla la harina de maiz con agua y sal, "
        "forma las arepas y cocina en la parrilla.",
    ),
    ("passage", "Quarterly Report - Page One. Revenue grew steadily this quarter."),
    (
        "passage",
        "Onboarding notes. New hires should read the welcome packet before "
        "their first day.",
    ),
    (
        "passage",
        "Notas de incorporacion. El nuevo empleado debe escuchar la cancion "
        "de bienvenida.",
    ),
    (
        "passage",
        "The quick brown fox jumps over the lazy dog near the riverbank.",
    ),
]


def main() -> None:
    model = SentenceTransformer(MODEL_ID, revision=REVISION)
    out = []
    for prefix, text in SENTENCES:
        prefixed = f"{prefix}: {text}"
        embedding = model.encode(prefixed, normalize_embeddings=True)
        out.append(
            {
                "prefix": prefix,
                "text": text,
                "embedding": [round(float(x), 8) for x in embedding],
            }
        )

    dest = (
        Path(__file__).resolve().parent.parent
        / "fixtures"
        / "reference_embeddings"
        / "e5_small.json"
    )
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(out)} reference embeddings to {dest}")


if __name__ == "__main__":
    main()
