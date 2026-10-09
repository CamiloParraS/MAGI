"""Copies fixtures/random_names/ to fixtures/realistic_names/ under names
like the ones real folders are full of: IMG_0042.JPG, PXL_..., WhatsApp's
IMG-...-WA0003.jpg, Screenshot_..., New Text Document (2).txt, Book1.xlsx,
plus a few hex and UUID downloads. Same files and bytes as random_names, so
the two runs differ only in what the names look like. Seeded: re-running
gives the same names.

Run from the repo root: python tools/realistic_names.py
then: python tools/random_names_queries.py realistic_names
"""

import pathlib
import random
import shutil
import uuid

SRC = pathlib.Path("fixtures/random_names")
DST = pathlib.Path("fixtures/realistic_names")
rng = random.Random(0)


def stamp():
    return f"2024{rng.randint(1, 12):02}{rng.randint(1, 28):02}", f"{rng.randint(0, 235959):06}"


def photo(ext):
    day, time = stamp()
    n = rng.randint(1, 9999)
    return rng.choice(
        [
            f"IMG_{n:04}.{ext.upper()}",
            f"IMG_{n:04}.{ext.upper()}",
            f"DSC_{n:04}.{ext.upper()}",
            f"PXL_{day}_{time}{rng.randint(0, 999):03}.{ext}",
            f"IMG-{day}-WA{n % 1000:04}.{ext}",
            f"{day}_{time}.{ext}",
            f"{rng.getrandbits(64):016x}.{ext}",
            f"{uuid.UUID(int=rng.getrandbits(128))}.{ext}",
        ]
    )


def name_for(ext):
    n = rng.randint(1, 12)
    if ext in ("jpg", "heic", "heif"):
        return photo(ext)
    if ext == "arw":
        return f"DSC{rng.randint(1, 99999):05}.ARW"
    if ext == "png":
        day, time = stamp()
        return rng.choice(
            [f"Screenshot_{day}-{time}.png", f"image ({n}).png", f"{rng.getrandbits(64):016x}.png"]
        )
    if ext == "txt":
        return rng.choice([f"New Text Document ({n}).txt", f"Untitled-{n}.txt", f"file{n}.txt"])
    if ext == "pdf":
        day, _ = stamp()
        return rng.choice([f"Scan_{day}_{n}.pdf", f"document ({n}).pdf", f"download ({n}).pdf"])
    return {
        "docx": f"Document{n}.docx",
        "pptx": f"Presentation{n}.pptx",
        "xlsx": f"Book{n}.xlsx",
        "py": f"untitled{n}.py",
        "rs": f"main{n}.rs",
        "java": f"Main{n}.java",
    }.get(ext, f"{rng.getrandbits(64):016x}.{ext}")


if DST.exists():
    shutil.rmtree(DST)
DST.mkdir()
for path in sorted(SRC.iterdir()):
    if not path.is_file():
        continue
    name = name_for(path.suffix.lstrip(".").lower())
    while (DST / name).exists():
        name = name_for(path.suffix.lstrip(".").lower())
    shutil.copy2(path, DST / name)
print(f"copied {len(list(DST.iterdir()))} files to {DST}")
