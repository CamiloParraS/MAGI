# Prior art: file-search projects compared with magi

Surveyed 2026-09-23. Each repo was shallow-cloned and its source read. Commits:

| Repo | Commit | Date |
| --- | --- | --- |
| naaive/orange | `09cfcde` | 2023-10-15 |
| sist2app/sist2 | `5718640` | 2026-09-20 |
| Tanm4y/file-seach-basic | `24e665c` | 2025-06-23 |
| Eul45/omni-search | `8dbf77b` | 2026-08-21 |
| PromtEngineer/agentic-file-search | `83c5b42` | 2026-02-28 |
| bimalendu/semantic-file-search | `32f3e0d` | 2025-08-25 |
| akkshay0107/sift (earlier review) | `563221e` | 2026-05-13 |

Citations use the form `repo:path:line`. "magi:" paths are relative to this repo.

Only two of the six projects extract file content at scale: sist2, and omnisearch-lite in the omni-search repo, where content extraction is currently stubbed out. orange and omni-search index filenames only. The other three are single-file Python demos. Almost all the speed lessons below come from sist2.

## 1. TL;DR

The list is ranked by expected effect on indexing speed first, then search quality. Brief items 4 (OCR recognition batch size) and 5 (ORT optimization level) are not addressed by any surveyed project; see §2.7.

1. **Stop the global re-index on a pipeline-version bump.** sist2 skips unchanged files by `(path, mtime)` before a job is even created (`sist2:src/worker/master.c:567-575`). orange instead wipes its whole index whenever its `VERSION` changes (`orange:src-tauri/src/indexing.rs:117-138`), which is the same shape as magi's single `PIPELINE_VERSION`. **How it maps to magi:** split the version by kind or by stage, for example `text_pipeline_version` and `image_pipeline_version`, alongside the existing `ocr_engine_id` / model-id checks. Then a chunker change re-indexes text without re-running OCR and SigLIP on every image. Files: `magi:crates/magi-core/src/index/mod.rs:21`, `magi:crates/magi-core/src/watch/reconcile.rs:70`.
2. **Index in two phases: cheap first, heavy later.** sist2 builds no embeddings during the scan. They come from a separate pass over the finished index (`sist2:docs/USAGE.md:227-239`, `sist2:docs/scripting.md`). omnisearch-lite writes every name and metadata row first, then runs content extraction in a second parallel pass, and handles low-priority directories (`target`, `build`) last (`omni-search:omnisearch-lite/src/indexer.rs:813-935`). **How it maps to magi:** have the scheduler do filename, text extraction and FTS for every file before any OCR, SigLIP or e5 work. Keyword search then works across the whole corpus within minutes, and semantic coverage fills in afterwards. This needs a new sub-state, or an "embeddings pending" flag, in the §5.4 state machine: a SPEC change in M5 territory.
3. **Commit many files per transaction.** sist2 commits every 1,000 writes (`WRITE_BATCH_SIZE`, `sist2:src/database/database.c:863-887`), and its comment explains why: in autocommit, each row pays for its own journal. omnisearch-lite flushes every 500–1,000 rows (`omni-search:omnisearch-lite/src/indexer.rs:507-540, 842-900`). orange commits on a 5-second timer (`orange:src-tauri/src/idx_store.rs:373-381`). **How it maps to magi:** change the DB writer to "one transaction per batch of N files or T ms". Each file is still all-or-nothing, because a batch is atomic, and crash recovery already resets files left in `indexing`. Also set `PRAGMA synchronous=NORMAL`: `magi:crates/magi-core/src/db/mod.rs:43` sets WAL but leaves `synchronous` at its default. sist2 uses `synchronous=OFF` (`database.c:224`); don't copy that, because it gives up NFR-5 on power loss. Changing this needs an edit to SPEC §5.3 ("one transaction per file"). The gain is large for text-heavy corpora and small where OCR dominates.
4. **Cap how much text a file contributes to embeddings.** sist2 extracts only `--content-size` bytes per document, 32,768 by default (`sist2:docs/USAGE.md:22`; enforced in `sist2:libscan/ebook/ebook.c:453-505`). **How it maps to magi:** e5 cost grows linearly with chunk count. Add a `max_embedded_chunks_per_file` limit, still sending the full text to FTS5, in `magi:crates/magi-core/src/chunk.rs` / `index/pipeline.rs`. Measure recall with `just eval` before choosing the number.
5. **Skip OCR where it can't pay off.** sist2 skips OCR on images smaller than 350×33 px (`sist2:libscan/ocr/ocr.h:7-8, 22-25`), skips image files under 512 bytes (`sist2:src/parsing/parse.c:13, 47`), and OCRs a PDF page only when it has no text layer (`sist2:libscan/ebook/ebook.c:510-512`). OCR is also opt-in per file type (`--ocr-images`, `--ocr-ebooks`; `sist2:docs/USAGE.md:32-33`). **How it maps to magi:** add a minimum-size gate in `magi:crates/magi-core/src/ocr/paddle.rs` and return early when detection finds no boxes. Possibly add a per-root "OCR images" toggle in M6 settings.
6. **Make `magi-cli index` use the same parallel engine.** sist2 has exactly one scan path: a walk/producer thread feeds a bounded queue of 50,000 jobs to N workers, and a single thread owns all writes (`sist2:src/worker/master.c:22, 525-615, 559-560`). **How it maps to magi:** `magi-cli index` should drive `Engine` rather than the one-file-at-a-time `drain_pending` (`magi:crates/magi-core/src/index/pipeline.rs`). Then CLI benchmarks measure the code users actually run.
7. **Add per-stage timing before tuning anything else.** No surveyed project has per-stage timing either. sist2 only reports progress as a completed/submitted count (`master.c:146-162`). This is magi's own item 6, and it goes in the TL;DR because items 1–5 need numbers to rank them on real data. **How it maps to magi:** add `tracing` spans around each stage (hash, extract, OCR detect/recognize, SigLIP, e5, write) plus per-kind totals in `magi-cli index` output.
8. **Search quality: weight filename and path inside BM25.** sist2's FTS5 table indexes `name, content, title, path` with `bm25(8, 3, 8, 5)` (`sist2:src/database/database_schema.c:96-105`). During its bulk load it sets `automerge` to 0 and restores it to 4 afterwards (`sist2:src/database/database_fts.c:218-223, 366-369`). **How it maps to magi:** add a `name`/`path` column to `chunks_fts`, or index a filename chunk, and weight it through `rank`. This could replace part of the ×1.2 filename boost in §5.6. Try `automerge=0` during the initial index. Both are documented FTS5 options (https://www.sqlite.org/fts5.html).

## 2. Per-project notes

### 2.1 sist2 (sist2app/sist2)

**What it is.** A mature C scanner and search server. It parses 20+ formats through libscan (PDF via MuPDF, media via ffmpeg, Office, archives, email), generates WebP thumbnails, and can OCR with Tesseract. Its index is a SQLite file, served with its own SQLite FTS5 or with Elasticsearch.

**How it indexes.**
- **Process model.** A master process, a walk/producer thread, and N worker *processes* that re-exec the same binary with `--worker`, talking over framed pipes (`src/worker/master.c:400-470`). A worker that crashes or exceeds `--job-timeout` is killed and respawned, and its job is written off (`master.c:267-293, 326-370`). The master is the only DB writer (`master.c:559`).
- **Skipping unchanged files.** In incremental mode the producer runs `UPDATE marked SET marked=1 WHERE id=(SELECT ROWID FROM document WHERE path=?) AND mtime=?` and drops the file if it matches, before any IPC (`master.c:567-575`, `database.c:243-251`). Workers repeat the check before sniffing the MIME type (`parse.c:174-182`). Rows left unmarked at the end are the deleted files: mark-and-sweep.
- **Checksums are opt-in** (`--checksums`) and computed while the parser reads the file, not in a separate pass (`parse.c:155, 265-269`).
- **Cheap-first type detection.** It looks the MIME type up by file extension first and runs libmagic on the first 24 KB only when the extension is unknown. `--fast` indexes names and MIME types only (`parse.c:87-146`).
- **Writes** are batched into 1,000-statement transactions (`database.c:863-887`). The FTS index is built afterwards from the document table with one `INSERT … SELECT`, with automerge turned off for the bulk load (`database_fts.c:218-223, 355-369`).
- **Embeddings** are not computed by the scanner. User scripts (Python, e.g. `sist2-script-clip`) add them to the index file later. Query embeddings run in the browser through onnxruntime-web (`docs/USAGE.md:227-239`, `sist2-vue/src/ml/CLIPTransformerModel.js:1, 35-36`). Vector search is a brute-force SQL `cosine_sim` function (`src/database/database_embeddings.c:6-35`); the README calls it O(n) on SQLite.

**Worth borrowing.** TL;DR items 2–6 and 8. Also:
- Contentless FTS5 with `contentless_delete=1` avoids storing text twice (`database_schema.c:92-102`). magi's external-content table (`content='chunks'`) already avoids the duplicate, so this is not needed.
- The per-job deadline plus crash-respawn pattern suits native decoders (PDFium, HEIC) that can hang on bad files. magi runs extraction in-process, so the practical version is a per-file watchdog that marks the file `error` and moves on. Worker processes would be a large change, needing an ADR.

**Avoid.**
- `synchronous=OFF` (`database.c:224`).
- `mtime`-only change detection. magi's size+mtime then blake3 is stricter, and it already checks size and mtime before hashing (`magi:crates/magi-core/src/watch/reconcile.rs:66-73`).
- Tesseract, a native C++ dependency.

### 2.2 orange (naaive/orange)

**What it is.** A Tauri 1 app written in Rust: an Everything-style launcher that searches **file names only**. It walks the whole disk, not user-chosen roots.

**Stack.** tantivy 0.17, jwalk (parallel walk), notify 4, jieba-rs + pinyin for Chinese tokenization, and a `kv` crate for config (`src-tauri/Cargo.toml`).

**How it indexes.**
- `jwalk::WalkDir` with a `process_read_dir` hook prunes excluded directories (`src/walk_exec.rs:178-225`). Each entry becomes one tantivy document: name, path, is_dir, extension (`src/idx_store.rs:411-429`).
- Commits run on a background timer: every 5 s during the full index, every 2 s after it (`idx_store.rs:373-381`). During the full index, `add` skips delete-before-insert (`idx_store.rs:413-417`).
- **Change detection.** It does a full re-walk every 30 days, or whenever `VERSION` changes, and wipes the index first (`src/indexing.rs:79-138`). Otherwise, on Windows it tails the NTFS USN journal from a persisted `next_usn` (`indexing.rs:155-214`, `src/usn_journal_watcher.rs`), falling back to a raw `notify` watcher on each drive (`src/watch_exec.rs`, `src/fs_watcher.rs`). Opening the volume asks for `GENERIC_READ | GENERIC_WRITE` (`usn_journal_watcher.rs:129-137`), which needs elevation.
- **Search.** The name field gets a 4× boost. A CamelCase name is indexed both as-is and split into words, so `DataPatchController.java` also matches `data patch controller` (`idx_store.rs:79-86`, via `convert_case`). With no hits, it falls back to `FuzzyTermQuery::new_prefix` with edit distance 1 (`idx_store.rs:241-263`).

**Worth borrowing.**
- The CamelCase / snake_case split for filenames and code identifiers. magi's `unicode61` tokenizer does not split `DataPatchController`, so the text would need to be pre-split into an extra FTS column.
- Its commit-on-a-timer write batching (TL;DR 3).

**Avoid.**
- The periodic full wipe and re-index.
- USN journal access: it needs admin rights and is Windows/NTFS only. It is a speed-up for watching whole volumes, which magi doesn't do.
- `unwrap()` everywhere.
- Walking the whole disk, which violates magi's "index only user-selected roots" constraint.

### 2.3 omni-search (Eul45/omni-search)

**What it is.** Two Windows apps.
- **OmniSearch** (Tauri 2 + a 3,187-line C++ `scanner.cpp`) is a filename search engine that enumerates the MFT with `FSCTL_ENUM_USN_DATA` and follows `FSCTL_READ_USN_JOURNAL` (`src-tauri/cpp/scanner.cpp:2378-2421, 2526-2554`). It requires Administrator rights (README "Requirements"). Content search (`content:` syntax) is a brute-force read of the name-filtered candidates at query time (`CONTENT_SEARCH_GUIDE.md`).
- **omnisearch-lite** (pure Rust, Slint UI) is a launcher that keeps a SQLite + FTS5 index.

**How omnisearch-lite indexes** (`omnisearch-lite/src/indexer.rs`).
- A walk compares each file's `modified` against the DB (`:799-813`). Names and metadata rows are batched first (`:842-846`); changed content files go to a job list.
- `spawn_extractors` uses `available_parallelism/2`, clamped to 1–4 workers, at `THREAD_PRIORITY_BELOW_NORMAL`, with a bounded result channel of 256 for backpressure (`:554-610`). Results are flushed every 500 (`:877-890`).
- Low-priority directories (`target`, `build`) are handled after everything else (`:17-26, 900-935`).
- **Caveat:** in this commit `is_indexable_content` returns `false` and `extract_content` returns `None` (`:28-35`), so content indexing is switched off. Deleted-file cleanup is explicitly skipped (`:937`).
- OCR, when used, is the OS engine `Windows.Media.Ocr.OcrEngine::TryCreateFromUserProfileLanguages` + `RecognizeAsync` (`indexer.rs:998-1033, 1043-1160`).

**Worth borrowing.**
- The two-pass ordering and the low-priority-directory tail (TL;DR 2).
- The bounded result channel. magi already uses bounded crossbeam channels in the engine; the CLI path does not (TL;DR 6).
- **Windows.Media.Ocr as a candidate OCR engine on Windows.** It ships with the OS, needs no model download, and is reachable through the `windows` crate (https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine). It recognizes only the languages whose OCR packs are installed (`AvailableRecognizerLanguages`), and it has a `MaxImageDimension` limit. Unverified: its speed and its CER against PP-OCRv5 on magi's fixtures. It would be Windows-only (macOS's counterpart is the Vision framework), would belong in `platform/`, would need an `OcrEngine` implementation plus an ADR, and would change `ocr_engine_id`. Worth one spike with the ADR-0006 fixtures, because OCR is the most expensive part of magi's pipeline.
- **The quick signature for duplicates.** It hashes size + first 64 KB + last 64 KB, and computes a full hash only when two signatures collide (`scanner.cpp:1791-1851, 2104-2140`). For magi's move detection by hash (`reconcile.rs`, "same-size rows whose path is gone"), this would avoid reading a large file in full when no candidate matches. It is an optimization only; blake3 remains the identity.

**Avoid.**
- Admin-only raw volume access.
- The scanner creating a USN journal on the volume when none exists (`FSCTL_CREATE_USN_JOURNAL`, `scanner.cpp:2390-2396`). That changes the user's volume, against magi's read-only rule.
- Skipping deletion cleanup.
- The C++ engine.

### 2.4 agentic-file-search (PromtEngineer/agentic-file-search)

**What it is.** A Python "FsExplorer": a Gemini agent that answers questions by scanning, previewing and parsing documents (Docling), with an optional DuckDB index holding chunks, metadata and Gemini embeddings.

**How it indexes** (`src/fs_explorer/indexing/pipeline.py`).
- Pass 1 parses **every** supported file on every run (`:94-103`). There is no mtime or hash skip, even though it stores `content_sha256` (`:135`).
- Metadata extraction runs on a 4-thread pool (`:185-216`).
- Chunking is character-based: 1,500 chars with 150 overlap, preferring `\n\n` boundaries (`indexing/chunker.py`).
- Embeddings come from `gemini-embedding-001` (768-d, batches of 50) through the Google GenAI API (`embeddings.py:16-73`), stored in DuckDB with an HNSW index built through the `vss` extension (`storage/duckdb.py:129, 627-640`).
- Missing files are soft-deleted (`mark_deleted_missing_documents`, `pipeline.py:161`).

**Search.** Semantic and metadata-filter queries run in parallel, merged by `semantic*100 + metadata*10` (`search/query.py`, `search/ranker.py:22-25`). That is score mixing, not rank fusion; magi's RRF is sounder.

**Worth borrowing.**
- **The metadata filter grammar:** `field=value`, `!=`, `>=`, `<`, `in (a,b)`, `field~substring`, joined with `,` or `and` (`search/filters.py:33-44`). It is a concrete reference for M9's query filters, although magi's planned `type:pdf in:Screenshots after:2025-01` style is friendlier for end users.
- Cheap regex metadata flags (`mentions_currency`, `mentions_dates`; `indexing/metadata.py:16-21, 547-558`). These are plausible local filter or boost signals ("invoice", "receipt").

**Avoid.**
- Re-parsing everything on every run.
- Character-based chunk sizes. magi's token-aware chunker is better.
- Score mixing instead of RRF.
- Everything that needs the network (see §4).

### 2.5 semantic-file-search (bimalendu/semantic-file-search)

**What it is.** A single 168-line Streamlit file (`app.py`).

**How it indexes.** It embeds **only the filename** with `all-MiniLM-L6-v2` (`app.py:88`). It MD5-hashes the whole file on every run *before* comparing anything (`app.py:56-61, 79-86`), and it keeps a flat FAISS L2 index in memory.

**Worth borrowing.** Nothing new.

**Avoid.**
- Hashing before the cheap size/mtime check.
- A FAISS/SQLite id mismatch: FAISS positions start at 0 while SQLite `rowid` starts at 1, so results are shifted by one (`app.py:108-111`).
- `load_index_to_faiss()` runs on every query and re-adds every vector each time (`app.py:152`).

### 2.6 file-seach-basic (Tanm4y/file-seach-basic)

**What it is.** One Streamlit page (`file.py`, 87 lines) that sends the user's question to OpenAI's Responses API with the hosted `file_search` tool over a pre-built OpenAI vector store (`file.py:47-62`). The README describes ChromaDB and `text-embedding-3-small`, but the code uses neither. It has no indexing code at all.

**Worth borrowing.** Nothing for indexing. See §4.

### 2.7 magi's own suspected causes: what the survey adds

| magi cause (from the earlier review) | Surveyed evidence |
| --- | --- |
| 1. Global `PIPELINE_VERSION` re-runs everything | sist2 skips work at the walk stage. orange shows the anti-pattern (a full wipe on version change). → TL;DR 1 |
| 2. `magi-cli index` is single-threaded | sist2 has one parallel path for every entry point. → TL;DR 6 |
| 3. `ModelSlot` serializes OCR/SigLIP | No surveyed project shares an ONNX session. Note that `ort 2.0.0-rc.13`'s `Session::run` takes `&mut self` (`ort-2.0.0-rc.13/src/session/mod.rs:236`), so one session can't serve two threads concurrently anyway. The options are one session per worker (costs RAM against NFR-11), or more intra-op threads for the single session while only one image worker is active. Measure both. |
| 4. OCR recognition at batch size 1 | Not covered by any surveyed repo (sist2 uses Tesseract whole-page). The PaddleOCR reference behaviour from the earlier review stands. |
| 5. ORT `Level1` | Not covered by the survey. ORT docs: "extended" optimizations (GELU, LayerNorm, Attention, SkipLayerNorm, BERT-embedding fusions) apply to nodes on the CPU EP (https://onnxruntime.ai/docs/performance/model-optimizations/graph-optimizations.html). Try `Level3`, check parity against the reference vectors (the M3/M4 cosine gates), and time it. Whether the fusions match int8/q4f16-quantized graphs is unverified. |
| 6. No per-stage timing | No surveyed project has it either. → TL;DR 7 |

## 3. sift (akkshay0107/sift): earlier review

Python, Qwen3-VL-2B embeddings, Qdrant in Docker. Most of it is worse than magi: no chunking, vector-only retrieval, points deleted before re-embedding, no deletion handling. Two ideas are worth testing:

- **Prepend the filename to the text being embedded:** `f"File: {path.name}\n\n{body}"` (`sift:src/indexer/pipelines.py:118, 174, 246`). For magi, try it on e5 `passage:` inputs, at least the first chunk of each file, and measure with `just eval`. Changing it changes every text vector, so it needs a re-embed, which is TL;DR 1's per-kind version.
- **Group results into bundles:** an item joins a bundle when `0.7·cosine(centroid) + 0.2·time-similarity + 0.1·filename-token Jaccard ≥ 0.6` (`sift:src/search/bundler.py:97-129`). For magi this is a UX idea for M6/M9: collapse near-duplicate screenshots or document versions into one result row with a "+N similar" count.

## 4. Last resort: non-local options

**Every option in this section violates SPEC hard constraints:** NFR-9 (zero outbound connections except model downloads) and the §0 rule "no network access beyond model downloads listed in `models/manifest.toml`". Adopting any of them requires a SPEC change and an ADR, and it changes the product's privacy promise. They rank below every local option above.

| Source | Remote service | What it would buy magi | Assessment |
| --- | --- | --- | --- |
| agentic-file-search | Hosted embeddings: Gemini `gemini-embedding-001`, 768-d, batches of 50 (`embeddings.py:16-73`) | Moves e5 inference off the CPU | Little speed gain. e5-small int8 is unlikely to be magi's bottleneck: OCR and SigLIP on images are the suspects, and nothing surveyed offers hosted OCR or image embeddings. Adds per-chunk network latency, API cost, rate limits and upload of all file text. Also breaks offline use and cross-lingual parity with the query encoder unless queries go remote too. |
| agentic-file-search | Agentic LLM exploration (Gemini, scan → parse → backtrack; `agent.py:418-450`) | Answers multi-document questions ("which contract references Exhibit C?") | Does not speed up indexing at all; it trades indexing for seconds-to-minutes and paid tokens per query. It is a different product (Q&A) from magi's search box. If ever wanted, a local-model version over magi's existing top-k results is the path that keeps the constraints. |
| agentic-file-search | LLM metadata extraction (`langextract` + Gemini; `indexing/metadata.py:593-660`) | Structured fields (parties, amounts, dates) for filters | Slower indexing, not faster: one LLM call per document. The regex flags in the same file give part of the value locally. |
| file-seach-basic | OpenAI Responses `file_search` + hosted vector store (`file.py:47-62`) | Outsources chunking, embedding, storage and retrieval | Requires uploading the user's files to OpenAI. sqlite-vec handles magi's scale locally (NFR-2 target: 100k chunks). Buys nothing but code deletion. |
| omnisearch-lite | Chat completions to Gemini / OpenAI / Anthropic / xAI / DeepSeek, or local Ollama (`omnisearch-lite/src/ai.rs:14-60`) | An "ask about this file" assistant | Unrelated to indexing speed. Only its Ollama entry (`http://localhost:11434`) is local, and that one would still need an ADR for a new runtime dependency. |

sist2 (Elasticsearch) and sift (Qdrant in Docker) use remote-capable backends but run them on the user's own machine, so they are not cloud options. Neither would be faster than SQLite + sqlite-vec at desktop scale, and both add a separate server process.

**Bottom line:** none of the non-local options addresses magi's likely bottleneck, which is image OCR, SigLIP and repeated work. The local items in §1 do.

## 5. Sources

Repositories (shallow clones, commits in the table at the top):
- https://github.com/sist2app/sist2: `src/worker/master.c`, `src/worker/worker.c`, `src/parsing/parse.c`, `src/database/database.c`, `src/database/database_fts.c`, `src/database/database_schema.c`, `src/database/database_embeddings.c`, `libscan/ocr/ocr.h`, `libscan/media/media.c`, `libscan/ebook/ebook.c`, `docs/USAGE.md`, `docs/scripting.md`, `sist2-vue/src/ml/CLIPTransformerModel.js`
- https://github.com/naaive/orange: `src-tauri/Cargo.toml`, `src-tauri/src/indexing.rs`, `walk_exec.rs`, `idx_store.rs`, `usn_journal_watcher.rs`, `fs_watcher.rs`, `watch_exec.rs`
- https://github.com/Eul45/omni-search: `README.md`, `CONTENT_SEARCH_GUIDE.md`, `src-tauri/cpp/scanner.cpp`, `omnisearch-lite/Cargo.toml`, `omnisearch-lite/src/indexer.rs`, `omnisearch-lite/src/ai.rs`
- https://github.com/PromtEngineer/agentic-file-search: `pyproject.toml`, `ARCHITECTURE.md`, `src/fs_explorer/indexing/pipeline.py`, `indexing/chunker.py`, `indexing/metadata.py`, `embeddings.py`, `search/query.py`, `search/ranker.py`, `search/filters.py`, `storage/duckdb.py`, `agent.py`, `fs.py`
- https://github.com/bimalendu/semantic-file-search: `app.py`
- https://github.com/Tanm4y/file-seach-basic: `file.py`, `README.md`
- https://github.com/akkshay0107/sift: `src/indexer/pipelines.py`, `src/search/bundler.py`

Official documentation:
- ONNX Runtime graph optimizations: https://onnxruntime.ai/docs/performance/model-optimizations/graph-optimizations.html
- SQLite FTS5 (`automerge`, `contentless_delete` since 3.43.0, `rank`/`bm25` weights): https://www.sqlite.org/fts5.html
- Windows.Media.Ocr.OcrEngine: https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine

Local:
- `ort 2.0.0-rc.13` source, `src/session/mod.rs:236` (`Session::run(&mut self, …)`), from the cargo registry magi already builds against.
- magi files cited: `crates/magi-core/src/index/mod.rs`, `index/pipeline.rs`, `watch/reconcile.rs`, `db/mod.rs`, `db/migrations/0001_init.sql`, `onnx.rs`, `ocr/paddle.rs`, `embed/manager.rs`.
