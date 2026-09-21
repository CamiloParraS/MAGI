"""Regenerates fixtures/reference_embeddings/siglip2.json (ADR-0007).

The reference is PyTorch fp32 `google/siglip2-base-patch16-256` through
`transformers` (`get_image_features` / `get_text_features`), L2-normalized,
text padded to 64 tokens. Run from the repo root in an environment with
`torch`, `transformers`, `pillow` and `numpy`.
"""
import json

import numpy as np
import torch
from PIL import Image
from transformers import AutoModel, AutoProcessor

MODEL = "google/siglip2-base-patch16-256"
IMAGES = [
    "dog_on_beach.jpg", "dog_on_beach2.jpg", "cat_on_beach.jpg", "mountain_sunrise.jpg",
    "mountain_sunset.jpg", "mustang_landscape.jpg", "phone_text_es.jpg", "receipt_es.jpg",
    "screenshot_en.png", "screenshot_es.png",
]
TEXTS = [
    "a dog on the beach", "dog on the beach", "perro en la playa", "a golden retriever lying on sand",
    "a dog with a tennis ball", "a cat on the beach", "gato en la playa", "a yellow sports car",
    "un coche deportivo amarillo", "a car parked in a parking lot", "a photo of a mustang",
    "mountain sunset", "atardecer en las montañas", "a hiking trail along a grassy ridge",
    "mountain sunrise", "amanecer en la montaña", "snowy mountain peaks at dawn",
    "christmas decorations on a shelf", "decoración navideña en un estante",
    "a shopping receipt", "recibo de compra", "a QR code", "código QR",
    "an anime character with pink hair", "personaje de anime con cabello rosa",
]


def features(out):
    # transformers 5 returns a model output; older versions return the tensor.
    t = getattr(out, "pooler_output", out)
    a = t.numpy()
    return a / np.linalg.norm(a, axis=-1, keepdims=True)


proc = AutoProcessor.from_pretrained(MODEL)
model = AutoModel.from_pretrained(MODEL).eval()
imgs = [Image.open(f"fixtures/corpus/images/{f}").convert("RGB") for f in IMAGES]
with torch.no_grad():
    px = proc(images=imgs, return_tensors="pt")["pixel_values"]
    ids = proc(text=TEXTS, padding="max_length", max_length=64, return_tensors="pt")["input_ids"]
    img_emb = features(model.get_image_features(pixel_values=px))
    txt_emb = features(model.get_text_features(input_ids=ids))

r = lambda v: [round(float(x), 6) for x in v]
json.dump(
    {
        "model": MODEL,
        "note": "PyTorch fp32 reference (transformers get_*_features, L2-normalized); text padded to 64 tokens",
        "images": [{"name": n, "embedding": r(e)} for n, e in zip(IMAGES, img_emb)],
        "texts": [{"text": t, "embedding": r(e)} for t, e in zip(TEXTS, txt_emb)],
    },
    open("fixtures/reference_embeddings/siglip2.json", "w", encoding="utf8"),
    ensure_ascii=False,
)
