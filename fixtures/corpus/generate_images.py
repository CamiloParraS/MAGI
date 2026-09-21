"""Dev-only fixture generator for synthetic image fixtures (see SPEC.md §5.1
fixtures/corpus, §7 M4). Not a runtime dependency. Run once locally; outputs
are checked into the repo.

These are generated rather than shot because they need an exact property a
camera will not reliably produce: a known EXIF orientation, and declared
dimensions that no decoder should ever allocate for.

Usage: python fixtures/corpus/generate_images.py
Requires: pip install pillow
"""

import struct
import zlib
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).parent


def make_rotated_exif_jpeg(path: Path) -> None:
    """A landscape JPEG whose EXIF says "rotate 90 CW to display".

    Every committed photo fixture had its EXIF stripped (see
    fixtures/README.md), so without this one nothing exercises SPEC.md §7
    M4's "EXIF orientation applied for all formats" on a JPEG. The four
    quadrants differ so a wrong rotation is visible, not just a wrong size.
    """
    width, height = 64, 32
    image = Image.new("RGB", (width, height))
    quadrants = [(220, 40, 40), (40, 180, 60), (50, 90, 230), (240, 220, 40)]
    for index, colour in enumerate(quadrants):
        x = (index % 2) * (width // 2)
        y = (index // 2) * (height // 2)
        image.paste(colour, (x, y, x + width // 2, y + height // 2))

    exif = Image.Exif()
    exif[0x0112] = 6  # Orientation: rotate 90 CW
    image.save(path, "JPEG", quality=95, exif=exif)


def make_decompression_bomb_png(path: Path, side: int = 50000) -> None:
    """A tiny PNG declaring `side` x `side` pixels.

    At 50000 x 50000 an RGB decode would want ~7.5 GB. The point is that the
    header alone is enough to refuse it, so the file itself stays small: the
    image data is a single zlib stream of one repeated scanline.
    """

    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", side, side, 8, 2, 0, 0, 0)  # 8-bit RGB
    compressor = zlib.compressobj(level=9)
    scanline = b"\x00" + b"\x7f" * (side * 3)
    body = b"".join(compressor.compress(scanline) for _ in range(side))
    body += compressor.flush()

    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", body)
        + chunk(b"IEND", b"")
    )


def main() -> None:
    edge = ROOT / "edge"
    edge.mkdir(parents=True, exist_ok=True)
    make_rotated_exif_jpeg(edge / "rotated_exif.jpg")
    print(f"wrote {edge / 'rotated_exif.jpg'}")
    # bomb.png is already committed; regenerate it only if it goes missing.
    if not (edge / "bomb.png").exists():
        make_decompression_bomb_png(edge / "bomb.png")
        print(f"wrote {edge / 'bomb.png'}")


if __name__ == "__main__":
    main()
