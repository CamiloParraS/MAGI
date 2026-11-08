# Search-quality eval

## Adopting a ranking change

1. On the base commit: `just eval-all before`.
2. On the change: `just eval-all after`. A run takes 3-4 minutes per corpus.
3. `just eval-diff before after` prints the buckets that moved and every drop.

A change is adopted when overall MRR improves and no bucket regresses on
recall@5, recall@10 or MRR, on any corpus, `fresh` queries included. `kw` and
`cross` matter most. Only the project owner grants an exception, and it is
recorded with its numbers in `docs/eval.md`. Results go in `docs/eval.md`
whether the change is adopted or not. `eval-diff` compares only the corpora
present in both runs; the summary says how many, and the rule needs all three.

## Query file format

`queries.jsonl`: one JSON object per line, `{"query", "lang", "expected", "notes"}`.

- `query` — the search string.
- `lang` — the breakdown bucket the query is reported under. Despite the
  name it is not strictly a language: `"en"` and `"es"` are, `"cross"` means
  the query language differs from the expected document's (tests semantic
  cross-lingual matching, which keyword search can't do at all), and `"kw"`
  means keyword-decisive — a rare literal token with thin semantic signal,
  the case FTS exists to answer. `magi-cli eval` groups by whatever string
  is here, so adding a bucket needs no code change.
- `expected` — relative paths (forward slashes, relative to the corpus
  root) of files that count as a correct top-k hit. Usually one path.
- `ignore` — optional; paths removed from the hit list before ranking,
  neither right nor wrong. Every `cross` query lists its same-language twin
  here (`team meeting notes` ignores `en/meeting_notes.txt` and expects
  `es/notas_reunion.txt`): the twin correctly ranks first, and counting it
  would cap the bucket's MRR near 0.5 whatever the ranking does. With it,
  `cross` measures how far up the other-language document gets.
  `tools/random_names_queries.py` maps these paths to the copies too.
- `fresh` — optional, `true` for the queries written on 2026-11-05 after
  the #9 decision. No threshold was tuned on them; `magi-cli eval` reports
  them again on their own as `fresh` and `f-<bucket>`. Mark new queries
  `fresh` only until something is tuned on them.
- `notes` — free text; `"keyword+semantic"` means the query's terms
  literally appear in the document, `"semantic paraphrase"` means they
  don't (only a real embedder can find it), `"cross-lingual"` self-explains.

The original 70 queries: 20 `en`, 20 `es`, 20 `cross`, 10 `kw`, covering 13 topics under
`fixtures/corpus/{en,es}/` plus two of the existing PDF fixtures
(`pdf/report.pdf`, `pdf/factura_electricista.pdf`) reused as cross-lingual
targets. Two of SPEC.md §7 M3's own smoke-test queries (`electrician
invoice`, `receta de arepas`) are included verbatim under `lang: "cross"`.

The `img`, `img2`, `ocr`, `qr` and `skip` buckets are the image queries. `img` (29) is the first batch. `img2` (54) covers the 2026-09-21 photo batch: food, vehicles, animals, landscapes, rooms, instruments, toy cars. `ocr` (7) is text printed in a picture, scored against `fixtures/golden/ocr/fixture_stem.txt`. `qr` (3) is text that exists only in a decoded QR payload, so the words are deliberately absent from file names (an earlier version used `ver1` and `wikipedia`, which are in the file names, and passed for the wrong reason). `skip` (1) checks that an image skipped for size is still found by name.

**File names leak.** A file called `pizza.jpg` is found for `pizza` by its filename chunk alone. The `visual` row in `magi-cli eval` output (no filename, no OCR) is the clean measure of the image model; hybrid is what a user sees. Expected answers for the photo batch were written from looking at every image, not from its file name: `mae-mu-burguer.jpg`, for example, is a stack of pancakes.

`neg` (18) has `"expected": []`: queries with no answer in the corpus, five
nonsense strings and thirteen plausible-but-absent searches in English and
Spanish. Each was checked against the corpus, `local/` and the OCR golden
text before it was added; check again when adding fixtures. They are left out
of recall and MRR, and scored for precision: how many lists the strong/weak
label would head "Nothing closely related" (docs/issues/0002, 0003).

The `local/` images are gitignored (`fixtures/README.md`), so the image buckets only run on a machine that has them.

The 10 `kw` queries target code, Office and exact-phrase fixtures
(`code/muestra.py`, `code/sample.rs`, the deep `calculateRequest.java`,
`office/inventory.xlsx`, `office/kickoff.pptx`, and literal phrases in
`en/`) and exist to make SPEC.md §7 M3 item 4 clause (b) measurable: without
them every query in the set is one vector-only wins, so the comparison
cannot discriminate. Every `kw` query's terms were read out of the fixture
before the query was written — a `kw` query whose tokens are not literally
in the indexed text is testing nothing.

**Random names (`random_names.jsonl`, `just eval-random`).** The problem
magi exists for is a file whose name says nothing. `fixtures/random_names/`
holds byte-identical copies of the corpus under names like
`29ab99be9520eacd.txt`; it is local only (`.git/info/exclude`), like
`local/`. `random_names.jsonl` is `queries.jsonl` with each expected path
swapped for its copy (same bytes), generated by
`python tools/random_names_queries.py`; re-run it when either side changes.
Same queries and answers, so the gap between the two runs is what names were
contributing. `skip` is left out: it checks that a file is found by its name.
The folder must stay outside `fixtures/corpus/`, or `just eval` indexes the
copies and scores them as wrong answers (docs/eval.md).
`fixtures/realistic_names/` is the same files under camera-style and
"New Text Document" names (`python tools/realistic_names.py`, then
`python tools/random_names_queries.py realistic_names`; run with
`just eval-random realistic_names`), also local only.

Run with `just eval` (`magi-cli eval eval/queries.jsonl --corpus
fixtures/corpus`) — indexes the corpus into a fresh temp DB with the real
`E5Embedder` (or `FakeEmbedder` under `MAGI_FAKE_EMBEDDER=1`, though that
makes the recall numbers meaningless — it has no real semantic signal) and
reports recall@5, recall@10, and MRR for `fts`/`vector`/`hybrid` search,
overall and broken down by `lang` bucket. All three modes run through
`search::rank_and_boost`, so the two baselines are hybrid's own ranking
function with one input list emptied, not a different one. Hybrid then
runs a second time on the same index with the experimental z-score +
word-coverage reranker (`search::zfusion`, GitHub #11), scaled two ways.
The RRF and z-score sections also print result counts, how many right
answers a 0.5x-top fold would hide, the `neg` result counts, and a sweep of
the strong/weak label (`search::strength`) over three scales: neg lists
labelled "nothing close" against false-weak answers. See `docs/eval.md` for
recorded baselines.
