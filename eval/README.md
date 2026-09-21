# Search-quality eval

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

The `local/` images are gitignored (`fixtures/README.md`), so the image buckets only run on a machine that has them.

The 10 `kw` queries target code, Office and exact-phrase fixtures
(`code/muestra.py`, `code/sample.rs`, the deep `calculateRequest.java`,
`office/inventory.xlsx`, `office/kickoff.pptx`, and literal phrases in
`en/`) and exist to make SPEC.md §7 M3 item 4 clause (b) measurable: without
them every query in the set is one vector-only wins, so the comparison
cannot discriminate. Every `kw` query's terms were read out of the fixture
before the query was written — a `kw` query whose tokens are not literally
in the indexed text is testing nothing.

Run with `just eval` (`magi-cli eval eval/queries.jsonl --corpus
fixtures/corpus`) — indexes the corpus into a fresh temp DB with the real
`E5Embedder` (or `FakeEmbedder` under `MAGI_FAKE_EMBEDDER=1`, though that
makes the recall numbers meaningless — it has no real semantic signal) and
reports recall@5, recall@10, and MRR for `fts`/`vector`/`hybrid` search,
overall and broken down by `lang` bucket. All three modes run through
`search::rank_and_boost`, so the two baselines are hybrid's own ranking
function with one input list emptied, not a different one. See
`docs/eval.md` for recorded baselines.
