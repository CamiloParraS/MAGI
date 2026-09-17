# Search-quality eval

`queries.jsonl`: one JSON object per line, `{"query", "lang", "expected", "notes"}`.

- `query` — the search string.
- `lang` — `"en"`, `"es"`, or `"cross"` (query language differs from the
  expected document's language — tests semantic cross-lingual matching,
  which keyword search can't do at all).
- `expected` — relative paths (forward slashes, relative to the corpus
  root) of files that count as a correct top-k hit. Usually one path.
- `notes` — free text; `"keyword+semantic"` means the query's terms
  literally appear in the document, `"semantic paraphrase"` means they
  don't (only a real embedder can find it), `"cross-lingual"` self-explains.

60 queries: 20 `en`, 20 `es`, 20 `cross`, covering 13 topics under
`fixtures/corpus/{en,es}/` plus two of the existing PDF fixtures
(`pdf/report.pdf`, `pdf/factura_electricista.pdf`) reused as cross-lingual
targets. Two of SPEC.md §7 M3's own smoke-test queries (`electrician
invoice`, `receta de arepas`) are included verbatim under `lang: "cross"`.

Run with `just eval` (`magi-cli eval eval/queries.jsonl --corpus
fixtures/corpus`) — indexes the corpus into a fresh temp DB with the real
`E5Embedder` (or `FakeEmbedder` under `MAGI_FAKE_EMBEDDER=1`, though that
makes the recall numbers meaningless — it has no real semantic signal) and
reports recall@5, recall@10, and MRR for `fts`/`vector`/`hybrid` search,
overall and broken down by `lang`. See `docs/eval.md` for recorded
baselines.
