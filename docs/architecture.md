# Architecture

Stub. Update this document whenever a public contract changes (DB schema,
IPC commands, config format) — see SPEC.md §0.

## Process model

A single process: the Tauri app hosts `magi-core::Engine`. The engine runs
on its own threads; Tauri commands talk to it through an `EngineHandle`. The
CLI (`magi-cli daemon`) hosts the same engine headless. See SPEC.md §5.3 for
the full thread/data-flow diagram.

## Crates

- `magi-core` — all business logic, no Tauri dependency.
- `magi-cli` — dev/test CLI over `magi-core`.
- `apps/desktop/src-tauri` — the Tauri shell; thin command wrappers only.
- `xtask` — cross-platform dev tasks (fetch PDFium, fetch models, ...).

## IPC contract

DTOs live in `crates/magi-core/src/dto.rs` and are exported to
`apps/desktop/src/bindings/` via `ts-rs`. See SPEC.md §5.7 for the full
command table (populated as commands land, starting M1).

## Database schema

See SPEC.md §5.5, implemented by `crates/magi-core/src/db/migrations/0001_init.sql`.
`db::open()` registers `sqlite-vec`, sets `journal_mode=WAL`/`foreign_keys=ON`/
`busy_timeout`, and runs any pending migrations (tracked in
`schema_migrations`, applied at most once each). The database file lives at
`<data_dir>/magi.db`.

## Config

`<config_dir>/config.toml`, shape in SPEC.md §5.2. `config::load()` writes
defaults on first run; `config::save()` writes to a temp file and renames it
into place so a crash mid-save can't corrupt the existing config.
`Config::validate()` rejects malformed exclude globs and roots whose path
doesn't exist on disk.

## Roots

`db::roots` manages the `roots` table. `roots::add()` canonicalizes the path
and rejects it if it's missing, already registered, or nested with (an
ancestor or descendant of) an existing root.

## Embeddings and hybrid search

`embed::TextEmbedder` is the trait every text embedder implements
(`model_id`, `dim`, `embed_passages`, `embed_query`); `embed::FakeEmbedder`
is a deterministic, dependency-free implementation used by tests and
`MAGI_FAKE_EMBEDDER=1`. `embed_passages` takes `&[&str]`, so a caller never
copies a file's text just to hand it over. `index::pipeline::index_root`
takes a `&dyn TextEmbedder`, embeds each file's chunk texts, and passes the
vectors to `db::files::upsert_file`, which writes them into `vec_text`
(`chunk_id -> embedding`) in the same transaction as the `chunks`/FTS rows —
and deletes the matching `vec_text` rows before deleting `chunks`, since
`vec0` virtual tables aren't covered by `FOREIGN KEY` cascades. A
chunk/embedding count mismatch is a typed error there, not a silently
truncated `zip`. After each run, `index_root` records the embedder's
`model_id()` in `meta.text_model_id` and warns (via `tracing`) if it differs
from the previously recorded one — re-embedding on a model change is not yet
triggered automatically (that needs M5's scheduler).

`E5Embedder::embed_passages` splits its input into batches of
`BATCH_CHUNKS` (16) before each inference call. A file's chunk count is
unbounded — a 15 MB text file yields ~5,700 chunks — and ONNX Runtime
materializes a `batch × seq_len × 384` f32 `last_hidden_state` for the whole
batch, so an uncapped batch scales peak memory with file size (~4.5 GB at
that chunk count). The cap holds that intermediate at a few MB regardless of
file size, per SPEC.md §5.3's "small batches". Note that `BatchLongest`
padding plus int8 kernels make a vector depend slightly on what it was
batched with (measured worst case 0.9956 cosine against the same text
embedded alone — the same band as int8-vs-fp32 parity).

`search::hybrid_search(conn, embedder, query, limit)` runs `search::fts`
(BM25, unchanged from M2) and `search::vector::search_vector_text` (a
`vec_text` KNN query), fuses their per-file-deduplicated results with
`search::fuse::reciprocal_rank_fusion` (SPEC.md §5.6's
`score = Σ w_i / (60 + rank_i)`, weight 1.0 each), then applies
`fuse::filename_boost` and `fuse::recency_boost` as a score multiplier.
Both searches return `search::FileHit`, which carries `path`, `file_name`,
`mtime_ns` and the matching chunk's snippet — they already join `files`, so
`hybrid_search` computes the boosts from the hits it has instead of issuing
a metadata query per fused result. They also share
`search::chunk_fetch_limit` (over-fetch factor before dedup) and
`search::first_hit_per_file` (per-file dedup). `vec_image`/the SigLIP visual
list are not wired in yet (M4). `magi-cli search --mode hybrid` and `index`
(which needs an embedder — set `MAGI_FAKE_EMBEDDER=1` to skip the model)
exercise this path from the CLI.

## Chunking

`chunk::chunk_text` is token-aware (SPEC.md §7 M3): it splits on word
boundaries (preserving each word's original trailing whitespace, so line
breaks and spacing round-trip exactly for a single-chunk document) and
grows each chunk up to `TARGET_MAX_TOKENS` (400) tokens, with
`OVERLAP_TOKENS` (50) of overlap into the next chunk — comfortably under
the `MAX_TOKENS` (512) ceiling once the embedder's `"query: "`/`"passage:
"` prefix is added. Token counts come from `embed::manager::shared_text_tokenizer()`
when the real e5 `tokenizer.json` has been downloaded, falling back to a
whitespace word-count approximation otherwise (tests, first run before
`just models`) — the boundary logic itself is a pure function
(`chunk::chunk_by_token_counter`) parameterized on the counting function,
so it's unit-tested without needing a model. Counts are memoized per
distinct word unit: a document has orders of magnitude fewer distinct words
than words, and each miss is a real tokenizer call. `extract::text::TextExtractor`
and `extract::paginated_doc` (PDF/DOCX/PPTX/XLSX) call `chunk_text`
unchanged; `extract::code`'s tree-sitter/line-window chunker is unaffected.

## Model manifest and manager

`models/manifest.toml` (repo root) lists each model slot's Hugging Face id,
pinned revision, dimensions, prefixes, and per-file name/URL/SHA-256/size —
verified by downloading from the official source, per SPEC.md §0. It's
embedded into the binary at compile time (`include_str!` in
`embed::manager`), so a production build doesn't depend on the repo layout
at runtime. `embed::manager::ensure_model_file` downloads a file into
`<data_dir>/models/<slot>/` with a `.partial` staging file (resumed via an
HTTP `Range` request, restarting from scratch if the server ignores it),
verifies its SHA-256, and atomically renames it into place; a corrupted
download is rejected and deleted so the next attempt re-downloads it
cleanly. `import_offline_model_file` does the same verification from a
local folder instead of the network. Both the download loop and the
`ByteFetcher` HTTP fetch are separate (dependency-injected) so the
resume/verify/atomic-rename logic is unit-tested without any network
access. `embed::manager::shared_text_tokenizer()` lazily loads and fully
configures `tokenizer.json` from the text slot's directory once (an
`OnceLock`, mirroring `extract::pdf`'s `shared_pdfium`) — `TruncationParams`
(`max_length = chunk::MAX_TOKENS`, i.e. 512) and `PaddingParams`
(`BatchLongest`, `pad_id` looked up via `tokenizer.token_to_id("<pad>")`
rather than trusting the ONNX model's own `config.json`, which disagrees
for this model). Both `chunk::count_tokens` and `embed::e5::E5Embedder`
borrow this single instance rather than each parsing their own copy —
ADR-0005's RSS investigation found the earlier two-copies design cost
~200-260 MB of pure duplication.

Quantization (fp32 vs. int8): **int8 ships as the default** (ADR-0005) —
within SPEC.md §7 M3's 2-point recall allowance (0.967 vs. fp32's 0.983
recall@5) and 66 MB over the ≤700 MB RSS target vs. fp32's 400 MB over.
`e5_parity.rs`'s tolerance is `0.97` (SPEC.md §7 M3's quantized-model rule,
down from `0.99` for fp32) — re-verified against the real int8 model at a
worst-case cosine of 0.9953 vs. the fp32 Python reference. fp32's hash is
kept in `models/manifest.toml`'s comment for rollback.

## Text embedder (`embed::e5::E5Embedder`)

The real `intfloat/multilingual-e5-small` embedder. Its tokenizer is
`embed::manager::shared_text_tokenizer()`'s shared, already-configured
`&'static Tokenizer` (see above) — `E5Embedder` no longer loads its own
copy. The tokenizer's own `TemplateProcessing` post-processor wraps each
sequence in `<s> ... </s>`, so encoding uses `add_special_tokens = true`.

`ort::Session` runs the ONNX graph (`model.onnx`: inputs `input_ids`,
`attention_mask`, `token_type_ids`, all `int64`; one `last_hidden_state`
output, `[batch, seq_len, 384]` — the graph has no pooling layer baked in,
confirmed by inspecting the exported graph's IO, not assumed). `E5Embedder`
mean-pools over the attention mask (ignoring padding) and L2-normalizes,
per SPEC.md §3. `embed_passages`/`embed_query` add the `"passage: "`/
`"query: "` prefixes before tokenizing.

ONNX Runtime itself is loaded dynamically (`ort`'s `load-dynamic` feature,
not its default `download-binaries`, which would fetch a third-party CDN
mirror at build time — see ADR-0001) from `vendor/onnxruntime/<target>/lib/`,
vendored and SHA-256-verified by `cargo xtask fetch-onnxruntime` the same
way `xtask fetch-pdfium` vendors PDFium. `MAGI_ONNXRUNTIME_PATH` overrides
the resolved path, mirroring `MAGI_PDFIUM_PATH`.

Verified end to end on the developer machine (Windows x86_64): real
`model.onnx`/`tokenizer.json` (downloaded from Hugging Face) plus the real
vendored `onnxruntime.dll`, indexed via `magi-cli index` and queried via
`magi-cli search --mode hybrid`, reproduce SPEC.md §7 M3's own
cross-lingual smoke-test example — the English query `electrician invoice`
ranks the Spanish `factura de electricista` fixture first, and `arepas
recipe` ranks the Spanish `receta de arepas` fixture first. This is one
manual run, not the full eval harness below.

`magi-cli`'s `embedder_from_env()` now returns `Box<dyn TextEmbedder>`:
`E5Embedder::load()` by default, `FakeEmbedder` under `MAGI_FAKE_EMBEDDER=1`.

M3's SPEC.md §7 verification checklist is now fully evidenced (see
`docs/progress.md`'s M3 section for the full list). One item is a real,
recorded open gap rather than "not yet implemented": `xtask bench-corpus`
(`just bench`) measures the 100k-synthetic-chunk NFR-2/NFR-3 targets and
both currently **fail** (cold 3,176 ms vs. ≤3,000 ms; warm p95 585 ms vs.
≤300 ms) — root cause is `vec_text`'s brute-force (no ANN index) scan
scaling with corpus size, not the embedder itself, and fixing it needs a
sqlite-vec partitioning/quantization strategy, out of scope for this
slice (`docs/eval.md`). The ≤700 MB RSS target **is met**: 682.8 MB, after
the tokenizer-duplication fix and a CPU-memory-arena fix (ADR-0005).

`cargo xtask fetch-models` (mirroring `fetch-pdfium`/`fetch-onnxruntime`)
downloads and SHA-256-verifies each `models/manifest.toml` file into
`<data_dir>/models/<slot>/` via `embed::manager::ensure_model_file` — the
CLI wiring for that manifest-driven download/verify logic, which existed
and was unit-tested since the first M3 slice but had no way to actually
run outside tests until now.
