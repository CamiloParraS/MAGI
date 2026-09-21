"""Builds a synthetic 48 MP HEIC for the decode budget (SPEC.md section 7 M4).

Phones cap HEIC at 12 MP and no real 48 MP HEIC was obtainable, so this
upscales a committed 12 MP phone HEIC 2x and re-encodes it. libheif tiles an
image that large into a `grid` item over `hvc1` tiles, the same structure as
the phone fixtures, so it exercises the decode path the app ships. The content
is smoother than a real 48 MP photograph, so decode *time* may be a little
optimistic; peak *memory* is dominated by pixel buffers and does not depend on
content.

    pip install pillow pillow-heif
    python tools/synthetic_heic_48mp.py out.heic
    MAGI_HEIC_48MP=out.heic cargo test -p magi-core --test heic_budget -- --ignored

Not committed: the output is several MB and trivially regenerable.
"""
import sys

import pillow_heif
from PIL import Image

pillow_heif.register_heif_opener()
out = sys.argv[1] if len(sys.argv) > 1 else "synthetic_48mp.heic"
src = Image.open("fixtures/corpus/images/shelf_christmas.heic").convert("RGB")
src.resize((src.width * 2, src.height * 2), Image.LANCZOS).save(out, quality=60)
print(f"wrote {out}")
