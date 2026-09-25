# ADR-0008: Indexing runtime — plain threads, bounded channels, one writer

- **Status:** Accepted; implemented in `engine.rs`, `index/{scheduler,writer,gate,resources}.rs`
- **Milestone:** M5

## Context

M5 turns the synchronous `index_root` loop into a background engine that keeps
the index in step with the roots while the app is in use. It has to stay polite
(low priority, capped ONNX threads, search never stuck behind indexing), survive
crashes without corrupting SQLite, and run the same way on Windows, macOS and
Linux. `magi-core` has no async runtime and no Tauri dependency.

## Decision

**Plain `std::thread`s connected by `crossbeam-channel`**, no async runtime.
Async exists only at the Tauri command boundary.

| Thread                      | Count                       | Does                                                                        |
| --------------------------- | --------------------------- | --------------------------------------------------------------------------- |
| scheduler                   | 1                           | picks `pending` rows, newest mtime first; dedupe, backoff, stability check  |
| extract workers             | `worker_threads` (auto 1–4) | `pipeline::prepare`: hash-skip, read, extract, chunk; no database           |
| embed worker                | 1                           | owns the text model; batches of 16 chunks; yields to search between batches |
| writer                      | 1                           | the only thread that writes; one transaction per file                       |
| watchers / ticker / monitor | 1 each                      | change events, periodic and recovery scans, memory and power policy         |

- **One writer.** Every write (stores, deletions, reconciliation, root and
  pause changes as `WriteJob::Exec` closures) goes through it, so SQLite never
  sees two writers and there is no lock ordering to get wrong. A result for a
  row that is no longer `indexing` (deleted, re-queued, claimed by a move) is
  dropped (`writer::superseded`).
- **Crash safety from the state machine, not from the threads.** Rows left
  `indexing` at start are reset to `pending`; the store is one transaction.
- **Bounded channels** give back-pressure: a slow embed stage stops the
  scheduler from pulling more work instead of growing a queue in memory.
- **Search priority.** `search_pending()` hands out a guard; the embed worker
  waits for it between batches, capped at 5 s, so a search waits for at most
  one small batch.
- **Resource limits.** Extract and embed threads run at low OS priority
  (`platform::Os::lower_current_thread`). At most 2 image decodes at once, 1
  over 12 MP (`index/gate.rs`). ONNX intra-op threads: 4 on AC, 2 otherwise.
  Below 1 GiB available memory, no new files start and the image model
  unloads; indexing resumes above 1.25 GiB. Models unload after the idle
  timeout.

## Alternatives considered

- **Tokio (or another async runtime) inside `magi-core`.** The work is
  CPU-bound (parsing, ONNX) and file I/O is blocking on every OS anyway, so it
  would run on `spawn_blocking` pools with extra glue and a heavier dependency.
- **`std::sync::mpsc`.** It has no bounded multi-producer channel with
  `select`, which the scheduler and writer need.
- **A connection pool with several writers.** SQLite serialises writers anyway;
  several writers would add `SQLITE_BUSY` handling and make "one transaction
  per file" harder to reason about.

## Consequences

- New dependency: `crossbeam-channel` (pure Rust).
- The scheduler polls every 250 ms when idle. Measured idle CPU is 0.05–0.37%
  of one core (docs/benchmarks.md, "M5 — idle cost").
- Image embedding runs on the extract workers inside `extract_image`, not on
  the embed worker; SigLIP serialises internally, so it is safe, but the
  search-priority guard covers text embedding only.
- `Engine::start` blocks while the first reconciliation walk runs.
- Search latency while indexing is not measured yet. It needs indexing and
  search in one process, which arrives with the desktop app.
