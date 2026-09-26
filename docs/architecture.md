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

So far (M5 Slice 7, serde only; the `ts-rs` derive comes with M6):
`IndexStatus { state: idle|scanning|indexing|paused, queued, indexed, skipped,
errors, current_file?, roots: RootStatus[] }` and `RootStatus { id, path,
enabled, status }`. Paths are strings (lossy for non-UTF-8 names). See
"Control surface" below for the `EngineHandle` methods behind them.

M6 Plan 1 adds `FeatureStatus { feature, enabled, download_size, install,
backfill? }`, `Install` (`NotInstalled | Downloading { bytes, total } |
Installed { size_bytes } | Failed { code }`, tagged by `state`), `Backfill
{ done, total }` and `DownloadError` (`DownloadNetworkError`,
`ChecksumMismatch`, `DiskFull`, `PermissionDenied`, `WriteFailed`). See
"Search features" below.

## Database schema

See SPEC.md §5.5, implemented by `crates/magi-core/src/db/migrations/`
(`0001_init.sql`; `0002_files_indexes.sql` adds the partial indexes
`idx_files_size`, for the move lookup, and `idx_files_pending`, which
`next_pending` reads in order with `INDEXED BY`; `0003_features_missing.sql`
adds `files.features_missing`, see "Search features"). `db::open()` registers `sqlite-vec`,
sets `journal_mode=WAL`/`synchronous=NORMAL`/`foreign_keys=ON`/`busy_timeout`,
and runs any pending migrations (tracked in `schema_migrations`, applied at
most once each). Vectors are bound to `vec_f32()` as raw f32 BLOBs
(`embed::embedding_to_blob`), not JSON text. The database file lives at
`<data_dir>/magi.db`.

## Config

`<config_dir>/config.toml`, shape in SPEC.md §5.2. `config::load()` writes
defaults on first run; `config::save()` writes to a temp file and renames it
into place so a crash mid-save can't corrupt the existing config.
`Config::validate()` rejects malformed exclude globs, roots whose path
doesn't exist on disk, and `ui.transparency_intensity` outside 0.40–0.95.
M6 adds `[features] meaning = true, image_text = true, image_visual = false`
(desired state only) and `ui.language` (`system|en|es`),
`ui.transparency_mode` (`match_system|always|never`) and
`ui.transparency_intensity` (default 0.75). Unknown enum values fail to parse.
The default `exclude_globs` also skip Unity's regenerated `Library` caches,
`*.meta` files and build output (`*.dll`, `*.pdb`, `*.obj`, `*.o`). Defaults
apply only when the config file is first written: an existing config keeps
its own list.

## Roots

`db::roots` manages the `roots` table. `roots::add()` canonicalizes the path
and rejects it if it's missing, already registered, or inside an existing
root. Existing roots _inside_ the new one are collapsed into it
(`roots::add_collapsing`): in one transaction their files are re-pointed via
`files::rename_file_to` (new `root_id`, `rel_path` relative to the parent, and
filename chunk text), and the child root rows are deleted. Chunks and vectors
are kept, so the parent's first scan finds those files unchanged and indexes
only what's new. Roots never overlap, so every file belongs to exactly one root.

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

`search::hybrid_search(conn, embedder, image_embedder, query, limit)` runs `search::fts`
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
`search::first_hit_per_file` (per-file dedup). When `image_embedder` is
given, a third list joins the fusion: `search::vector::search_vector_image` (a
`vec_image` KNN, top 50, weight 0.8) using the SigLIP text tower, reported as
match source `"visual"`. It only keeps images whose query-image cosine is at
least `search::IMAGE_MIN_COSINE` (0.10): a KNN always returns its nearest rows,
so without a floor every text query pulled the whole photo library into the
ranking and collapsed text recall to keyword-only (docs/eval.md). The floor is
calibrated to SigLIP 2 on the fixture set, so recalibrate it with the model. A
missing or broken visual model degrades to text-only search rather than
failing the query. `magi-cli search --mode hybrid` and `index`
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
unchanged. `TextExtractor` passes a `.csv`/`.json` file's first 64 KB only,
cut after the last line break (data, not prose: a 200 MB export would
otherwise be ~150k chunks). `extract::code` merges adjacent top-level
symbols while their summed token count fits `TARGET_MAX_TOKENS`, so a run of
`use`/`mod` lines is one chunk; any piece still too long goes through
`chunk_text`.

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
configures `tokenizer.json` from the text slot's directory into a static
`ModelSlot<Arc<Tokenizer>>`, which the resource monitor frees after the same
idle timeout as the models (`unload_text_tokenizer_if_idle`); callers hold
an `Arc` clone, so unloading never pulls it out from under one — `TruncationParams`
(`max_length = chunk::MAX_TOKENS`, i.e. 512) and `PaddingParams`
(`BatchLongest`, `pad_id` looked up via `tokenizer.token_to_id("<pad>")`
rather than trusting the ONNX model's own `config.json`, which disagrees
for this model). Both `chunk::count_tokens` and `embed::e5::E5Embedder`
share this single instance rather than each parsing their own copy —
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
the resolved path, mirroring `MAGI_PDFIUM_PATH`. Loading it and building a
session (CPU, arena off) live in `crate::onnx`, shared by the text embedder,
the SigLIP towers and OCR.

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

## Images (M4)

**One decode point.** `extract::image::decode_bounded` is the only place an
image is decoded (HEIC routes to `extract::heic`, everything else to the
`image` crate), so the decompression-bomb defence (header dimensions checked
against `max_image_megapixels` before any allocation) and the EXIF/`irot`
orientation fix run exactly once. `extract::image::extract_image` decodes once
and derives everything from that buffer, in this order: QR/barcode payloads
(`qr::decode_barcodes`: a shrink-only scale ladder, then, for images up to 6 MP, overlapping native-resolution tiles so a small code in a big busy photo is found), the visual embedding
(`ImageEmbedder::embed_image`, from the full-resolution decode), then the
full buffer is dropped, a 2048 px copy goes to OCR, and a 256 px copy becomes
the thumbnail. A failed embedding or OCR-less run costs the file that signal,
never its index entry. Everything comes back in one `extract::ExtractedDoc`
(chunks, language, `thumbnail`, `image_embedding`), the same type every
extractor returns; the PDF path fills only its `thumbnail`.

**OCR** is `ocr::OcrEngine`: `paddle::PaddleOcr` (PaddleOCR PP-OCRv5 mobile
detection + Latin recognition over `ort`, ADR-0006) or `NoOcr` when its
models are absent. Detection fits rotated rectangles to the probability map's
components and straightens each line; recognition is a CTC decode over the
dictionary read from `rec.yml`. OCR adds ~310 MB of peak RSS. Known gap: the
recognizer's dictionary has no inverted exclamation mark. Output shorter than 6 letters and digits is discarded
(`extract::image::MIN_OCR_ALNUM`): PaddleOCR emits stray glyphs for photos with no
text, and as `ocr` chunks they crowd real documents out of the text-vector list
(docs/eval.md).

**SigLIP 2** is `embed::ImageEmbedder` (`embed::siglip::SigLipEmbedder`, 256 px
q4f16, ADR-0007). Each tower is its own lazy, idle-unloadable `ModelSlot`, so
indexing only loads the ~80 MB vision tower and search only the ~450 MB text
tower. Vectors are L2-normalized 768-d and stored in `vec_image` (one per
file, replaced in the same transaction as the file's chunks). Preprocessing is
the reference's: bilinear squash to 256 x 256, scaled to [-1, 1]; queries are
Gemma-tokenized with EOS, padded to 64.

**Pipeline wiring.** `index::pipeline::IndexContext` carries `embedder`, `ocr`
(`Arc<dyn OcrEngine>`) and an optional `image_embedder`
(`Arc<dyn ImageEmbedder>`; `None` leaves `vec_image` empty). `IndexContext::new`
uses `NoOcr` and no image embedder. `meta.text_model_id`, `meta.image_model_id`
and `meta.ocr_engine_id` record what produced the index (see "Change detection"). `files.content_hash` (blake3, every
file whose bytes are read) and `files.thumb_key` are filled by the pipeline.
`indexing.file_types` deserializes straight into `discovery::Kind`, so an
unknown name fails config loading with serde's list of valid ones. Extraction
runs under `index::isolate::run` (timeout, panic containment, stuck-thread cap).

**Thumbnails** are 256 px JPEGs at `<cache_dir>/thumbs/<first two hex>/<key>.jpg`
(`thumbs::store`), keyed by content hash, for images and PDF first pages.
The desktop app enables Tauri's asset protocol with an empty static scope and
grants exactly `thumbs::thumbs_dir()` at startup, so the webview can read
thumbnails and nothing else. It is granted at runtime because the cache lives
under the magi data directory, which the Tauri identifier cannot name.

**Model manifest.** Two slots were added: `ocr` (`det.onnx`, `rec.onnx`,
`rec.yml`) and `image` (`vision_model.onnx`, `text_model.onnx`,
`tokenizer.json`), both pinned by revision and SHA-256. `ModelEntry::dim` is
optional (OCR has no embedding width).

## Change detection (M5)

Slice 1 of M5 (docs/m5-plan.md) made indexing incremental. These are the
contracts the rest of M5 builds on.

**File states** (`files.state`): `pending -> indexing -> indexed | skipped |
error`, with `attempts` and `next_attempt_at` for retries. `db::files` holds the
transitions: `mark_pending`, `mark_indexing`, `reset_indexing_to_pending` (a
crash leaves rows `indexing`; startup puts them back), `next_pending(now, limit)`
(newest `mtime_ns` first, honouring `next_attempt_at`), and `record_failure`,
which retries after 30 s, then 2 min, and gives up to `error` on the third
failure (`backoff_secs`, `MAX_ATTEMPTS`).

**Is this file unchanged?** Every rule lives in `index::change`. The scan
queues a row whose root, size, mtime or `pipeline_version` differs from disk
(`unchanged_on_disk`). For a queued file, `pipeline::prepare` then asks
`change::keeps`, which updates only `size`, `mtime_ns` and `state` on a
yes. A queued row's `state` is `pending`, so `keeps` reads the last result from
the columns queuing leaves alone. It needs the current `PIPELINE_VERSION` and
no `error` (kept until a result replaces it, even through `retry_errors`), and:

1. for a file that will be extracted, a streamed blake3 hash equal to the stored
   `content_hash` and no `skip_reason`, so `touch` and re-saves cost no
   embedding;
2. for a file that is not extracted (unsupported, disabled kind, too large), no
   stored hash, and the same kind and `skip_reason` it would get again.

Anything else is extracted, embedded and written by `upsert_file` in one
transaction, which also stamps `pipeline_version`.

**Stale rows.** `index::PIPELINE_VERSION` is bumped when extraction or chunking
changes what a file's rows would contain. `index::requeue_on_model_change`
compares `meta.text_model_id`, `meta.image_model_id` and `meta.ocr_engine_id`
with the running components and, on a mismatch, calls `files::invalidate`:
`state = 'pending'` and `pipeline_version = 0`, which is what defeats the
hash-skip above. A text-model change invalidates every file, an image-model or
OCR change only `kind = 'image'`. The stored ids are updated in the same
transaction. A missing stored id is not a mismatch, and no image model configured
leaves `image_model_id` alone. Old rows stay searchable until replaced.

**Deleting.** `files::delete_files` (call inside a transaction) removes
`vec_text`, chunks (their FTS rows follow through the trigger), `vec_image` and
the file row, and returns the thumbnail keys; `remove_unreferenced_thumbnails`
deletes a thumbnail after the commit only when no other row shares its key.
`delete_file` wraps one file; `purge_root` covers a root. `roots::remove` now
purges the root's files first, in the same transaction: before, it failed with a
foreign-key error on any root that had been indexed.

**Test instrumentation.** `embed::CountingEmbedder` wraps a `TextEmbedder` and
counts `embed_passages` calls and chunks (`CountingEmbedder::new(FakeEmbedder)`), with
`with_model_id` to simulate a model change.

**Reconciliation** (`watch::reconcile`, Slice 2). `index_root` is
`reconcile_root` -> `resolve_moves` -> drain `pending`. `reconcile_root` walks a
root and, in one transaction, inserts unknown files as `pending`, marks files
whose size, mtime or `pipeline_version` differ as `pending` (storing the new
stat), and stamps everything else with the scan's `seen_scan_id`. Rows of the
root with an older `seen_scan_id` come back as deletion candidates.
`resolve_moves` renames a candidate in place (`files::rename_file`) when a
brand-new `pending` row has the same size, kind and blake3 hash, and deletes the
rest. Scan ids come from `next_scan_id` (`meta.last_scan_id`); every scan
takes its own, `index_root` included, and only scans write `seen_scan_id`
(storing a result does not). Unseen rows carry the id of the scan that missed
them (`reconcile::Unseen`). A move that claims a row still in the pipeline puts
it back to `pending`, so its stale result, read at the old path, is dropped.

**Root status.** `platform::FsProbe` maps a failed `read_dir`/`metadata` to
`permission_denied` or `missing`; `roots::set_access` stores it. The rules live
on `roots::Health` and `Root`, with SQL twins the queries splice in
(`indexable_sql!`, `searchable_sql!`, `readable_sql!`):

- **Indexable:** enabled and readable (`ok` or `watch_failed`). Only these
  roots are scanned and indexed; the others keep their rows.
- **Searchable:** enabled and not `missing`. An unreadable root stays
  searchable, because its index is still right.

## Search features (M6 Plan 1, ADR-0010)

`features::Feature` is `meaning` (e5, manifest slot `text`, bit 1),
`image_text` (OCR, slot `ocr`, bit 2) or `image_visual` (SigLIP 2, slot
`image`, bit 4).

- **`Components { text, ocr, image }`**, every member an `Option`, is what
  `Engine::start(config, db_path, Components)` runs with. A missing component
  is skipped: no `vec_text` rows without meaning (chunks still go to FTS), no
  OCR chunks without image text, no `vec_image` without image visual.
  `hybrid_search(conn, Option<&dyn TextEmbedder>, ..)` skips the text vector
  query when meaning is off; old `vec_text` rows stay unused, never deleted.
  `ModelIds` is all-optional: a component that is not running never triggers
  a model-change re-queue.
- **`load_components(&FeaturesConfig)`** loads each enabled, installed
  feature (fakes under `MAGI_FAKE_EMBEDDER=1`, except OCR); a load failure
  leaves that feature out with a warning.
- **`files.features_missing`** records, in the file's own transaction, the
  bits of the features it was indexed without. Only images extracted as
  images can miss image features.
- **Backfill:** a feature becomes available → the host restarts the engine →
  `index::requeue_missing` re-queues `indexed` and `skipped` files carrying the bit of any
  running feature and stores `meta.backfill_total_<feature>` → the normal
  pipeline re-indexes them → the bit clears. `backfill_progress` is
  `(total - left, total)` while files carrying the bit are `pending` or
  `indexing`; an `error` row counts as done. A restart mid-backfill keeps the
  original total.
- **`Features`** (`Features::start(db_path)`) owns the complete state: desire
  from `config.toml`, availability from disk (`installed_size`: every
  manifest file at its final name with the manifest size) plus the download in
  flight, and backfill progress from the database. One `features` thread runs
  queued downloads one at a time and re-reads progress every 250 ms;
  `subscribe()` sends the current `Vec<FeatureStatus>` and then every change,
  always complete. `cancel_download` stops the running download and drops the
  queue (all back to `NotInstalled`); `remove_download` deletes only
  `<data_dir>/models/<slot>/` and is refused while that feature downloads.
- **CLI:** `magi-cli features list | enable <f> | disable <f>
  [--delete-download]`; `doctor` prints the same table.

## Engine (M5 Slice 3)

`Engine::start` runs SPEC §5.4 startup steps 1, 2, 4 and 6, then five kinds of
thread:

```
scheduler ──► extract workers (N) ──► embed worker (1) ──► writer (1) ──► DB
    ▲  │            │ unchanged / retry ─────────────────────▲              │
    │  └ deletes, state changes ───────────────────────────►│              │
    └──────────────────── done (file id) ◄───────────────────┴──────────────┘
```

- **Scheduler** (own read connection): `pending` rows are the queue. It holds
  each file until it has been stable (mtime over 3 s old and unchanged size
  across two stats 1 s apart), hands out newest first, and never has more than
  `2 * workers + 2` files in the pipeline. A file proven stable while the
  pipeline is full waits as `Ready`, handed out when room frees without being
  stat'd again. A row is released when the writer reports it done.
- **Extract workers**: `pipeline::prepare`; unchanged files go straight to the
  writer, the rest to the embed worker over a bounded channel.
- **Embed worker**: takes one file plus whatever else is already queued, up to
  16 chunks, and embeds them together (`pipeline::embed_group`: shortest
  chunks first, a batch ends where lengths jump so a filename chunk is not
  padded to a full body chunk; a failed batch falls back to one file at a
  time). Batches of
  at most 16 chunks; before each batch it waits while any `SearchGuard` is
  alive (the priority lock), at most 5 s.
- **Writer**: the only thread that writes. Jobs: mark indexing, store, keep,
  retry, delete, reconcile. One transaction per file.

`EngineHandle` (clone) offers `stats()`, `rescan()`, `search_pending()` and
`shutdown()`. Shutdown drops queued pipeline work (rows stay `indexing`; the
next start resets them) and applies everything already handed to the writer.
Thread and channel details: `engine.rs`.

## Watching (M5 Slice 4)

`Engine::start` starts one debounced (2 s) `notify` watcher per accessible root
before the first scan. A batch of events becomes `WriteJob::Paths(paths)`; the
event _kinds_ are ignored except that access events are dropped and an error or
overflow becomes a full rescan. The writer runs `reconcile::scan_paths`, which
looks at each path on disk (file: queue it if wanted; folder: walk it; missing or
now excluded: its rows are deletion candidates). Moves are matched from the new
side: an unknown path whose size, kind and blake3 hash equal those of a row whose
own path is gone takes over that row in the scan's transaction
(`find_old_home`), so a rename, a folder rename or a move between roots keeps the
row, chunks and vectors whichever half the OS reports first. Deletion candidates
are held 5 s before deleting (`settle_held`), which gives the other root's
watcher time to report the new path.

A ticker thread (`watch::poller`) asks for a full scan every
`reconcile_interval_hours`, after a wall-clock jump over 5 minutes, and every 15
minutes while a root has no watcher (status `watch_failed`, or not accessible).

Every 30 s tick also sends `WriteJob::Reprobe`: a root that is
`permission_denied` or `missing` and probes readable again is reconciled
(`reconcile::recover`), which clears its status. Only the roots that came back
are walked (`reconcile_roots`).

`EngineHandle::apply_indexing_config` replaces the shared indexing options and
rescans. The stale-result rule: a result is stored only if its row is still
`indexing`. A file re-queued by a watcher event while the pipeline works on it
is processed again; one deleted, or whose root was removed, in the meantime is
not brought back. Both drivers make every per-file state change through
`index::lifecycle` (`begin`, then `apply` in the engine or `apply_alone` in the
one-shot `index_root`, which as the only writer skips the `indexing` mark and
this check).

## Platform behaviour (M5 Slice 5)

`platform::Os` implements SPEC §6's `CloudPlaceholder`, `PowerStatus` and
`ThreadPriority`; the OS code lives in `platform/{windows,macos,linux}.rs`, the
pure decoders (attribute bits, `pmset` output, `power_supply` entries) in
`platform/mod.rs` so every OS tests them.

- **Cloud placeholders.** `WalkEntry::cloud_only` comes from metadata the walk
  already has: Windows attributes `RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN |
OFFLINE`, macOS `st_flags & SF_DATALESS` (0x40000000), never on Linux. Such a
  file is stored `skipped` with `skip_reason = 'cloud_only'` and only its
  filename chunk; nothing opens it (`plan_entry` checks first, move matching
  does not hash it).
- **Locked files.** An I/O error that `platform::is_locked` recognises (Windows
  32/33) is retried through `files::record_locked`: the usual backoff, but the
  attempt count stops one short of `MAX_ATTEMPTS`, so it never becomes `error`.
- **Priority.** Extract and embed threads lower themselves: Windows
  `THREAD_PRIORITY_BELOW_NORMAL`, Linux `setpriority(PRIO_PROCESS, 0, 10)` (per
  thread on Linux), macOS QoS utility.
- **Power.** `Os.on_battery()`: Windows `GetSystemPowerStatus`, Linux
  `/sys/class/power_supply`, macOS `pmset -g batt`. Read by the monitor for
  pause on battery (below).
- **Unwatched roots.** A network share or mapped network drive (Windows) is
  never watched, and a root whose watcher fails (inotify `ENOSPC` logs the
  `sysctl` fix) is marked `watch_failed`; both are polled every 15 minutes.

## Resource policy (M5 Slice 6)

`index::resources` holds the policy as pure functions and one monitor thread
(`magi-monitor`) started by the engine.

- **Workers.** `worker_threads = 0` means `min(physical_cores / 2, total_ram_gb / 4)`
  clamped to 1-4, with RAM rounded to whole GB (an "8 GB" machine reports about
  7.8 GiB and still counts as 8). `sysinfo` (feature `system` only) gives total
  RAM and physical cores.
- **Low-memory mode** (8 GB or less): `idle_unload_minutes` is capped at 2. The
  worker formula already gives at most 2 there.
- **Models load on first use and unload when idle.** `TextEmbedder`,
  `ImageEmbedder` and `OcrEngine` have `unload_if_idle(idle)` (no-op by default).
  `E5Embedder`, `SigLipEmbedder` and `PaddleOcr` keep their ONNX sessions in a
  `ModelSlot`; their `load()` only checks the files are there (and reads the
  tokenizer / OCR dictionary), so a missing model still fails at startup.
  `ModelSlot::unload_if_idle` uses `try_lock`: a model in use is not idle and is
  never waited on. `PaddleOcr` waits at most 30 s for another extract worker's
  OCR (`get_or_load_within`), so a hung run cannot pile up stuck threads.
- **Monitor.** Every 10 s (or when woken) it reads available memory; below 1 GiB
  (NFR-13) it pauses indexing and unloads the image model at once, and resumes
  only above 1.25 GiB so memory hovering near 1 GiB does not flip the pause. With
  `pause_on_battery`, being on battery (read at most once a minute) also pauses.
  Each check also unloads models idle longer than the idle timeout, including
  after searches.
- **Pause.** `EngineHandle::is_paused()`. While paused the scheduler starts no
  new files; files already in the pipeline finish, and watcher events are still
  recorded as `pending`. `EngineHandle::simulate_low_memory(bool)` is the test
  hook for item 17 (hidden from docs).

## Control surface (M5 Slice 7)

`EngineHandle` methods, the core of SPEC.md §5.7's commands:

| Method                                 | Does                                                                                                                                                                                                                                                                                                                                           |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `status()`                             | `IndexStatus`: counts by state from a read connection, `paused` (user or monitor) over `scanning` (a full reconcile running) over `indexing` (anything `pending`/`indexing`) over `idle`. `current_file` is the file an extract worker last started, shown only while a row is `indexing`.                                                     |
| `subscribe()`                          | A channel of `IndexStatus`, sent when it changes. The status thread checks twice a second but reads the database only after the writer applied a job, or the pause or scan flag flipped. Root status (including `permission_denied`) travels in `roots`.                                                                                       |
| `pause()` / `resume()` / `is_paused()` | User pause, persisted in `meta.paused` (`1`/`0`) and restored at start. Same effect as the monitor's pause (`resources::Pause`).                                                                                                                                                                                                               |
| `add_root(path)`                       | `roots::add_collapsing` (missing, duplicate and already-covered paths rejected against every root including disabled and missing ones: FR-1; roots inside the new path are collapsed into it, their files kept, and unwatched), probe, watch, then scan that root only (`WriteJob::ReconcileRoot`). Returns the root's status after the probe. |
| `remove_root(id)`                      | Stops its watcher, purges its rows (`roots::remove`).                                                                                                                                                                                                                                                                                          |
| `set_root_enabled(id, on)`             | Off: rows kept, hidden from search, not watched, its `pending` rows not handed out. On: watched, and that root scanned.                                                                                                                                                                                                                        |
| `retry_errors()`                       | Every `error` row back to `pending` with attempts and backoff cleared.                                                                                                                                                                                                                                                                         |
| `rescan()`, `apply_indexing_config()`  | As before (`rescan_all`, exclusions).                                                                                                                                                                                                                                                                                                          |

Every write goes through the writer: `WriteJob::Exec` carries a closure and the
handle waits for its reply, so root management stays ordered with the rest.
`files::next_pending` hands out only rows of enabled roots with status `ok` or
`watch_failed`.

Scanning one root (`reconcile::reconcile_roots`) is enough for a root added,
enabled or back: moves into it are matched from the new side, and a file moved
out of it while it was disabled or missing was already claimed by the other
root's watcher (a polled root may re-embed it once instead).

`magi-cli daemon [--stats]` runs the engine headless on the dev data dir,
prints a line per status change (and the roots when they change), and shuts
down cleanly on Ctrl-C. `--stats` prints the process's CPU (averaged over the
minute, 100% = one core), RSS and private memory once a minute. Private
memory (`PrivateUsage` on Windows) is the idle number to compare: RSS moves
with however much the OS trims the working set. Elsewhere that column is the
virtual size.
