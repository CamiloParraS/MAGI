"""Writes eval/random_names.jsonl: eval/queries.jsonl re-pointed at
fixtures/random_names/, a local copy of fixtures/corpus/ under random file
names. Each expected path becomes the copy with the same bytes, so the
queries and answers stay the same and only the names stop helping.

The `skip` bucket is left out: it checks that a file is found by its name.

Run from the repo root: python tools/random_names_queries.py
"""

import collections
import hashlib
import json
import pathlib

FIXTURES = pathlib.Path("fixtures")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


copies = collections.defaultdict(list)
for path in sorted((FIXTURES / "random_names").iterdir()):
    if path.is_file():
        copies[sha256(path)].append(path.name)

corpus = FIXTURES / "corpus"
original = {
    p.relative_to(corpus).as_posix(): sha256(p) for p in corpus.rglob("*") if p.is_file()
}

written = 0
with open("eval/queries.jsonl", encoding="utf-8") as src, open(
    "eval/random_names.jsonl", "w", encoding="utf-8", newline="\n"
) as out:
    for line in src:
        if not line.strip():
            continue
        query = json.loads(line)
        if query["lang"] == "skip":
            continue
        # `local/` differs per machine: an expected file may be missing here.
        expected = [
            name for e in query["expected"] if e in original for name in copies[original[e]]
        ]
        if not expected:
            raise SystemExit(f"no copy of {query['expected']} for {query['query']!r}")
        query["notes"] = f"copy of {', '.join(query['expected'])}"
        query["expected"] = expected
        out.write(json.dumps(query, ensure_ascii=False) + "\n")
        written += 1

print(f"wrote {written} queries to eval/random_names.jsonl")
