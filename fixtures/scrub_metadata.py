#!/usr/bin/env python3
"""Strip identifying metadata from the image fixtures, in place and losslessly.

Camera photos carry GPS coordinates, device make/model, firmware build strings
and timestamps. None of that belongs in a public repo, and none of it is
needed by any test: HEIC orientation lives in the container's `irot`
transform, not in EXIF (verified with `heic_rs::probe`), and no committed JPEG
has a non-trivial EXIF Orientation.

Pixels are never re-encoded. JPEG metadata segments are dropped from the
marker stream; HEIF metadata item payloads are overwritten in place with a
valid empty replacement of the same length, so every `iloc` offset in the
container stays correct.

    python fixtures/scrub_metadata.py fixtures/corpus/images/photo.jpg ...
    python fixtures/scrub_metadata.py --check fixtures/corpus/images/*

`--check` reports what is left and exits non-zero if anything identifying
remains. Run it before committing a new image fixture.
"""

import re
import struct
import sys
from pathlib import Path

# A valid TIFF header with an empty IFD: byte order, magic 42, offset to the
# first IFD, then zero entries and no next IFD.
EMPTY_TIFF = b"II\x2a\x00\x08\x00\x00\x00\x00\x00\x00\x00\x00\x00"
# HEIF stores an Exif item as a 4-byte offset to the TIFF header, then the
# TIFF itself (ISO/IEC 23008-12 A.2.1).
EMPTY_HEIF_EXIF = b"\x00\x00\x00\x06Exif\x00\x00" + EMPTY_TIFF
EMPTY_XMP = (
    b"<?xpacket begin="
    b"'' id='W5M0MpCehiHzreSzNTczkc9d'?>"
    b'<x:xmpmeta xmlns:x="adobe:ns:meta/"></x:xmpmeta>'
    b'<?xpacket end="w"?>'
)

# Things that must not survive a scrub, checked as raw bytes so this works on
# any container without parsing it. `Exif\0\0` is handled separately: it also
# appears structurally, as an `infe` item type.
SMELLS = [
    (b"http://ns.adobe.com/xap", "XMP packet"),
    (b"<rdf:RDF", "XMP/RDF"),
    (b"GPSLatitude", "GPS coordinates"),
    (b"Apple", "device vendor"),
    (b"samsung", "device vendor"),
    (b"Samsung", "device vendor"),
    (b"Galaxy", "device model"),
    (b"iPhone", "device model"),
    (b"Google", "software vendor"),
    (b"sefd", "Samsung extended data"),
    (b"dc:creator", "document author"),
    (b"xmpMM:DocumentID", "document UUID"),
]

# Byte strings that legitimately contain a smell: they name a format, not a
# device. Blanked before scanning so they do not raise a false alarm.
STRUCTURAL = [b"urn:com:samsung:", b"urn:com:apple:"]

# Only binary fixtures carry the metadata this checks for. Anything else --
# a .txt fixture, this script, the README -- would false-positive on the very
# words it is looking for.
BINARY_FIXTURES = {
    ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".tif", ".tiff", ".webp",
    ".heic", ".heif", ".pdf", ".docx", ".xlsx", ".pptx",
}

# A generated PDF's author field. reportlab writes this when none is given.
ANONYMOUS_AUTHORS = {b"", b"anonymous", b"Anonymous", b"unknown"}

# Fixtures reviewed by a human and kept as they are. A gate that always
# reports the same known finding is a gate people learn to ignore, so each
# exemption carries its reason and nothing else is exempt.
REVIEWED = {
    # A PowerPoint export whose metadata names its author and whose slides
    # use example names. Reviewed 2026-09-20: nothing sensitive. Committed
    # since M2. See fixtures/README.md.
    "huge_real.pdf",
}


def scrub_jpeg(data: bytes) -> bytes:
    """Rebuild the marker stream without the metadata segments.

    Kept: APP0 (JFIF) and APP2 when it carries an ICC colour profile, which
    is not identifying. Dropped: APP1 (EXIF and XMP), APP3-APP15 (maker
    notes, IPTC, Photoshop), COM, and the APP2 MPF segment.

    MPF matters more than it looks: a Samsung or Apple camera JPEG appends a
    *second*, complete JPEG after the primary image's EOI, with its own EXIF
    and XMP. Copying everything after SOS verbatim would carry all of it
    along, so the entropy-coded scan is walked to its terminating EOI and
    anything past that is dropped.
    """
    if not data.startswith(b"\xff\xd8"):
        raise ValueError("not a JPEG")
    out = bytearray(b"\xff\xd8")
    i = 2
    while i < len(data):
        if data[i] != 0xFF:
            raise ValueError(f"expected a marker at byte {i}")
        marker = data[i + 1]
        if marker == 0xD9:  # EOI
            out += data[i : i + 2]
            break
        if marker == 0xDA:  # SOS: entropy-coded data up to the matching EOI
            end = _end_of_scan(data, i + 2)
            out += data[i:end]
            break
        (length,) = struct.unpack(">H", data[i + 2 : i + 4])
        payload = data[i + 4 : i + 2 + length]
        drop = (
            marker == 0xFE
            or marker == 0xE1
            or 0xE3 <= marker <= 0xEF
            or (marker == 0xE2 and not payload.startswith(b"ICC_PROFILE\x00"))
        )
        if not drop:
            out += data[i : i + 2 + length]
        i += 2 + length
    return bytes(out)


def _end_of_scan(data: bytes, i: int) -> int:
    """Index just past the EOI that ends the entropy-coded scan at `i`.

    Inside the scan a 0xFF byte is either stuffed (0xFF00), a restart marker
    (0xFFD0-0xFFD7), or the start of a real marker. Only the last ends it.
    """
    while i < len(data) - 1:
        if data[i] != 0xFF:
            i += 1
            continue
        marker = data[i + 1]
        if marker == 0x00 or 0xD0 <= marker <= 0xD7 or marker == 0xFF:
            i += 2
            continue
        if marker == 0xD9:
            return i + 2
        # A real marker other than EOI: a multi-scan (progressive) JPEG.
        # Skip its segment and keep walking.
        (length,) = struct.unpack(">H", data[i + 2 : i + 4])
        i += 2 + length
    return len(data)


def _boxes(data: bytes, start: int, end: int):
    """Yield (type, payload_start, payload_end) for ISOBMFF boxes in a range."""
    i = start
    while i + 8 <= end:
        (size,) = struct.unpack(">I", data[i : i + 4])
        kind = data[i + 4 : i + 8]
        header = 8
        if size == 1:
            (size,) = struct.unpack(">Q", data[i + 8 : i + 16])
            header = 16
        elif size == 0:
            size = end - i
        if size < header:
            return
        yield kind, i + header, i + size
        i += size


def _find_box(data: bytes, path, start=0, end=None):
    end = len(data) if end is None else end
    for kind, s, e in _boxes(data, start, end):
        if kind == path[0]:
            return (s, e) if len(path) == 1 else _find_box(data, path[1:], s, e)
    return None


def _parse_iinf(data: bytes, start: int, end: int):
    """item_ID -> item_type, from the `iinf`/`infe` boxes."""
    version = data[start]
    i = start + 4 + (2 if version == 0 else 4)
    types = {}
    for kind, s, e in _boxes(data, i, end):
        if kind != b"infe":
            continue
        v = data[s]
        p = s + 4
        if v >= 3:
            (item_id,) = struct.unpack(">I", data[p : p + 4])
            p += 4
        else:
            (item_id,) = struct.unpack(">H", data[p : p + 2])
            p += 2
        p += 2  # item_protection_index
        types[item_id] = data[p : p + 4]
    return types


def _parse_iloc(data: bytes, start: int, end: int):
    """item_ID -> [(absolute offset, length)], from the `iloc` box."""
    version = data[start]
    i = start + 4
    offset_size, length_size = data[i] >> 4, data[i] & 0xF
    base_offset_size, index_size = data[i + 1] >> 4, data[i + 1] & 0xF
    i += 2
    if version < 2:
        (count,) = struct.unpack(">H", data[i : i + 2])
        i += 2
    else:
        (count,) = struct.unpack(">I", data[i : i + 4])
        i += 4

    def read(width):
        nonlocal i
        if width == 0:
            return 0
        value = int.from_bytes(data[i : i + width], "big")
        i += width
        return value

    items = {}
    for _ in range(count):
        item_id = read(2 if version < 2 else 4)
        if version in (1, 2):
            i += 2  # reserved(12) + construction_method(4)
        i += 2  # data_reference_index
        base_offset = read(base_offset_size)
        (extent_count,) = struct.unpack(">H", data[i : i + 2])
        i += 2
        extents = []
        for _ in range(extent_count):
            if version in (1, 2) and index_size:
                read(index_size)
            offset = read(offset_size)
            length = read(length_size)
            extents.append((base_offset + offset, length))
        items[item_id] = extents
    return items


def scrub_heif(data: bytes) -> bytes:
    """Blank the Exif and XMP item payloads without moving a single byte.

    ponytail: the items stay declared in `iinf`/`iloc` with an empty payload,
    rather than being removed and every offset in the container rewritten.
    Nothing reads them - HEIC orientation comes from `irot` - and this cannot
    corrupt the file. Remove them properly only if a decoder ever complains.
    """
    meta = _find_box(data, [b"meta"])
    if meta is None:
        raise ValueError("no meta box")
    # `meta` is a FullBox: its children start after version and flags.
    meta = (meta[0] + 4, meta[1])
    iinf = _find_box(data, [b"iinf"], *meta)
    iloc = _find_box(data, [b"iloc"], *meta)
    if iinf is None or iloc is None:
        raise ValueError("no iinf/iloc box")

    types = _parse_iinf(data, *iinf)
    locations = _parse_iloc(data, *iloc)

    out = bytearray(data)
    for item_id, item_type in types.items():
        if item_type == b"Exif":
            filler, pad = EMPTY_HEIF_EXIF, b"\x00"
        elif item_type == b"mime":  # XMP
            filler, pad = EMPTY_XMP, b" "
        else:
            continue
        for offset, length in locations.get(item_id, []):
            body = filler if length >= len(filler) else b""
            out[offset : offset + length] = body + pad * (length - len(body))

    return _drop_trailing_sefd(bytes(out))


def _drop_trailing_sefd(data: bytes) -> bytes:
    """Truncate Samsung's proprietary `sefd` trailer box.

    It is not image data. It carries a capture timestamp, the HDR and scene
    modes, the mobile country code of the network the phone was on, and a
    `PhotoEditor_Re_Edit_Data` JSON blob holding an on-device file path.

    Only a `sefd` that is the last top-level box is removed, by truncation:
    every `iloc` offset points into `mdat` earlier in the file, so nothing
    moves. A `sefd` anywhere else is left alone rather than risking the
    offsets, and `--check` will report it.
    """
    boxes = list(_boxes(data, 0, len(data)))
    if not boxes:
        return data
    kind, start, end = boxes[-1]
    box_start = start - 8  # `sefd` is small, so always an 8-byte header
    if kind != b"sefd" or end != len(data):
        return data
    if struct.unpack(">I", data[box_start : box_start + 4])[0] != end - box_start:
        return data
    return data[:box_start]


# EXIF tags a fixture may legitimately carry. Orientation is the only one a
# test depends on (`edge/rotated_exif.jpg` is tagged on purpose). Everything
# else — including the sub-IFD pointers that lead to the camera and GPS
# blocks — is reported.
ALLOWED_EXIF_TAGS = {0x0112}  # Orientation


def _has_populated_exif(data: bytes) -> bool:
    """True if an EXIF block holds a tag outside `ALLOWED_EXIF_TAGS`.

    `Exif\\0\\0` also occurs structurally in HEIF, as an `infe` item type, and
    a scrubbed file keeps an empty one, so the mere presence of the header
    proves nothing — the IFD has to be read.
    """
    i = data.find(b"Exif\x00\x00")
    while i >= 0:
        tiff = data[i + 6 :]
        if tiff[:4] in (b"II\x2a\x00", b"MM\x00\x2a"):
            endian = "little" if tiff[0] == ord("I") else "big"
            offset = int.from_bytes(tiff[4:8], endian)
            if 0 < offset + 2 <= len(tiff):
                count = int.from_bytes(tiff[offset : offset + 2], endian)
                for n in range(count):
                    entry = offset + 2 + n * 12
                    if entry + 2 > len(tiff):
                        break
                    tag = int.from_bytes(tiff[entry : entry + 2], endian)
                    if tag not in ALLOWED_EXIF_TAGS:
                        return True
        i = data.find(b"Exif\x00\x00", i + 1)
    return False


def _named_author(data: bytes):
    """The PDF `/Author` value, when it names someone.

    An encrypted PDF stores ciphertext there, which is neither readable nor a
    name; a generated one usually says "anonymous".
    """
    marker = b"/Author"
    i = data.find(marker)
    while i >= 0:
        open_paren = data.find(b"(", i, i + 16)
        close_paren = data.find(b")", open_paren + 1) if open_paren > 0 else -1
        if 0 < open_paren < close_paren:
            value = data[open_paren + 1 : close_paren].strip()
            # An encrypted PDF's ciphertext is written as backslash-octal
            # escapes, which are printable but are not a name.
            escaped = re.search(rb"\\[0-7]{3}", value) is not None
            printable = all(32 <= b < 127 for b in value)
            if printable and not escaped and value not in ANONYMOUS_AUTHORS:
                return value.decode("latin1")
        i = data.find(marker, i + 1)
    return None


def check(path: Path):
    # Only binary fixtures carry this kind of metadata. A .txt fixture, this
    # script and the README would all false-positive on the very words it
    # looks for.
    if path.suffix.lower() not in BINARY_FIXTURES or path.name in REVIEWED:
        return []
    data = path.read_bytes()
    for pattern in STRUCTURAL:
        data = data.replace(pattern, b"\x00" * len(pattern))
    found = [why for needle, why in SMELLS if needle in data]
    if _has_populated_exif(data):
        found.append("EXIF tags")
    author = _named_author(data)
    if author is not None:
        found.append(f"document author {author!r}")
    return found


def scrub(path: Path) -> str:
    data = path.read_bytes()
    if data.startswith(b"\xff\xd8"):
        new = scrub_jpeg(data)
    elif data[4:8] == b"ftyp":
        new = scrub_heif(data)
    else:
        return "skipped (not JPEG or HEIF)"
    if new == data:
        return "unchanged"
    path.write_bytes(new)
    return f"scrubbed ({len(data) - len(new)} bytes removed)"


def main(argv) -> int:
    checking = "--check" in argv
    paths = [Path(a) for a in argv if not a.startswith("--")]
    if not paths:
        print(__doc__)
        return 2

    failed = False
    for path in paths:
        if not path.is_file():
            continue
        if checking:
            smells = check(path)
            if smells:
                failed = True
                print(f"FAIL {path}: {', '.join(sorted(set(smells)))}")
            else:
                print(f"ok   {path}")
        else:
            print(f"{path}: {scrub(path)}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
