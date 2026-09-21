# M5 implementation plan: incremental indexing and change detection

**Status:** approved 2026-09-21. Progress per slice is recorded in `docs/progress.md`; this file is the plan, not the log.

## Context

M4 is signed off (`docs/progress.md`, "M4 sign-off"), so CLAUDE.md's gate for starting M5 is met.

M5's objective (SPEC.md §7): after the initial index, only changed files are processed, and the index always converges to the true state of the roots. Today indexing is a **one-shot, synchronous** function (`index/pipeline.rs::index_root`) that the CLI calls, and it re-extracts and re-embeds every file on every run. M5 turns it into a long-running engine: scheduler, workers, watchers, reconciliation, hash-skip, retries, platform behaviour, memory policy, and status events.

## What exists, and what is missing (from reading the code)

**Reuse as-is**
- `discovery::walk` (`WalkEntry` already has `size`, `mtime_ns`), `discovery::classify`.
- `index/pipeline.rs::process_entry` (extraction, blake3 hash, thumbnail) and `index/isolate.rs` (60 s per-file timeout, stuck-thread cap).
- `db::open` (WAL, `foreign_keys`, busy timeout), `db::files::upsert_file` (one transaction per file, deletes old vec rows before chunks), `db::meta::{get,set}`, `db::roots`.
- `embed::manager::ModelSlot::unload_if_idle`, `SigLipEmbedder::unload_if_idle`, `OcrEngine::engine_id`, `thumbs::thumb_key/thumb_path`.
- `platform/mod.rs` trait skeletons (`PermissionProbe`, `CloudPlaceholder`, `PowerStatus`, `ThreadPriority`); config fields `worker_threads`, `pause_on_battery`, `reconcile_interval_hours`, `idle_unload_minutes`.
- Schema needs **no migration**: `files` already has `attempts`, `next_attempt_at`, `pipeline_version`, `seen_scan_id`, `state`; `roots.status` has `missing|watch_failed|permission_denied`.

**Missing (all new work)**
- `engine.rs` is a 17-line stub; `index/scheduler.rs`, `index/writer.rs`, `watch/{watcher,poller,reconcile}.rs` are one-line placeholders; `platform/{windows,macos,linux}.rs` implement none of the traits.
- No DB operations for delete, rename-in-place, state transitions, pending-queue query, reset `indexing -> pending`, or root purge. `roots::remove` only deletes the root row, so its files, chunks and vectors survive (verification item 12).
- No `PIPELINE_VERSION` constant (`upsert_file` hardcodes `0`), no hash-skip, no re-embed trigger (a `tracing::warn!` only), `meta.ocr_engine_id` is never written.
- `FakeEmbedder` has no call counter (required by SPEC for items 3, 3b, 4, 5).
- Search does not hide results of `missing` roots.
- `IndexContext<'a>` borrows its embedder, so threads need `Arc<dyn ...>`.
- Dependencies not yet declared: `notify`, `notify-debouncer-full`, `crossbeam-channel` (all named in SPEC §3), plus what the platform work needs (below).

## Design decisions

1. **Synchronous core first.** The state machine, reconciliation, hash-skip, move detection and purge are plain functions over `&mut Connection`, unit-testable without threads or a watcher. Threads (Slice 3) and watchers (Slice 4) only wrap them. This keeps most of the 17 items deterministic.
2. **The DB is the queue's source of truth.** `pending` rows are the queue; the in-memory scheduler is a cache ordered by `mtime` descending. A crash loses nothing: startup resets `indexing -> pending` and rebuilds.
3. **Split the per-file pipeline into three stages** in Slice 1 (`extract` -> `embed` -> `store`), so Slice 3 only adds channels between them. Today they are fused inside `index_root`.
4. **Single writer thread** owns the only write connection; every delete, rename, state change and purge goes through it. Readers (search) use separate connections in WAL.
5. **Embed worker owns the models**, batches of at most 16 chunks, checks a `search_pending` `AtomicBool` between batches (the "priority lock").
6. **Hash-skip uses a streaming blake3** pass and compares hash, `pipeline_version` and the model ids; only if all match is extraction skipped.
7. **Test instrumentation:** a `CountingEmbedder` wrapper (with `FakeEmbedder::counting()`), not a counter inside `FakeEmbedder`, because `FakeEmbedder` is a `Copy` unit struct used across dozens of tests.

## Slices

Each slice ends with: `cargo fmt`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`, an entry in `docs/progress.md`, `docs/architecture.md` updated where a contract changed, and one commit (new dependencies justified in the message, per CLAUDE.md). Push after Slices 4, 5 and 8 and confirm CI on all three OSes.

### Slice 1: file state machine, DB operations, hash-skip, re-embed trigger (synchronous)
- `db/files.rs`: `insert_pending`, `set_state`, `mark_pending`, `reset_indexing_to_pending`, `next_pending(now, limit)` ordered by `mtime_ns DESC` honouring `next_attempt_at`, `record_failure` (attempts + backoff 30 s / 2 min / 10 min, `error` after 3), `delete_file` (vec_text via chunk subquery, chunks, vec_image, row, and the thumbnail file only if no other row shares its `thumb_key`), `rename_file` (path, rel_path, file_name, ext, root_id in place, no re-embed), `find_by_hash`, `purge_root`, `purge_excluded(root, globs)`.
- `index/mod.rs`: `PIPELINE_VERSION` (start at 1); `upsert_file` writes it instead of `0`.
- Refactor `pipeline.rs`: extract `index_one`-style stages; keep `index_root` as a thin wrapper so `magi-cli index`, `eval` and `xtask bench_corpus` keep working.
- Hash-skip (SPEC §5.4 step 4): unchanged hash + current `pipeline_version` + current model ids -> update `size`/`mtime_ns` only.
- Re-embed trigger: compare `meta.text_model_id`, `image_model_id`, and new `ocr_engine_id` with the running components; on mismatch mark affected files `pending` (all files for text, `kind = 'image'` for image and OCR) in the same transaction that updates `meta`. Old results stay searchable until replaced.
- `CountingEmbedder` + `FakeEmbedder::counting()`.
- **Proves (as DB/function-level tests):** items 2, 3, 3b, 7, 12 (root purge), 14 (error state, others continue).

### Slice 2: reconciliation, move detection, root lifecycle (still synchronous)
- `watch/reconcile.rs`: `reconcile_root(conn, root, options, scan_id)`: walk; unknown -> `insert_pending`; size/mtime differ -> `mark_pending`; set `seen_scan_id`; return the deletion candidates (rows with an older scan id). New `meta.last_scan_id`, `roots.last_full_scan_at`.
- Move detection (SPEC §5.4 step 4b): a hashed file matching a deletion candidate's `content_hash` reassigns that row (rename in place, `root_id` may change) instead of re-embedding; unmatched candidates are deleted after a short hold.
- `platform::PermissionProbe`: one shared implementation (`read_dir` + `metadata` on one entry; `PermissionDenied` -> `permission_denied`, not found -> `missing`). Missing root keeps its rows.
- Search hides results of roots with status `missing` or `enabled = 0` (small join in `search/fts.rs` and `search/vector.rs`).
- `magi-cli index` becomes incremental (reconcile + drain pending), which makes "changes while stopped" directly testable.
- **Proves:** items 4, 5, 6, 8, 12, 13, 15 at function level.

### Slice 3: runtime, scheduler, writer, workers, `Engine` skeleton (no watcher yet)
- `index/scheduler.rs`: queue ordered by mtime desc, dedupe by path, delays, retry/backoff, stability check (mtime < 3 s old, or size changing between two stats 1 s apart -> requeue +5 s).
- `index/writer.rs`: one writer thread, one transaction per file, job enum for upsert/delete/rename/state/purge.
- Extract worker pool (`worker_threads`, placeholder `available_parallelism()/2` until Slice 6), image decode semaphore (at most 1 for images over 12 MP, 2 total), embed worker with the priority lock.
- `engine.rs`: `Engine::start(config, db_path, Arc<dyn TextEmbedder>, Option<Arc<dyn ImageEmbedder>>, Arc<dyn OcrEngine>) -> EngineHandle`; startup sequence steps 1, 2, 4, 6; graceful shutdown; a `SearchPending` guard for M6 to use.
- `tests/incremental.rs` created here with the harness (`wait_until(deadline, cond)`, temp roots, counting embedder) and the items that need threads but not a watcher.
- **Proves:** items 9 (1,000 files exactly once), 10 (slow write, via the stability check), 11 (rows left `indexing` are reset and completed, `PRAGMA integrity_check = ok`; plus one real process kill in `magi-cli/tests`), 14.

### Slice 4: watchers, polling fallback, periodic safety nets
- `watch/watcher.rs`: `notify` + `notify-debouncer-full` (~2 s) per root, started **before** the scan, events buffered and drained afterwards; the event table from SPEC §5.4 (create/modify -> pending, remove -> delete, rename in place, rename out/excluded -> delete, unknown-source rename -> create, overflow/error -> rescan).
- `watch/poller.rs`: roots whose watcher fails (inotify limit, network drive) get status `watch_failed` and a 15-minute polling reconcile.
- Periodic reconcile every `reconcile_interval_hours`; wall-clock jump detection (1-minute ticks, jump over 5 minutes).
- Rename handling relies on both paths (paired rename events where the OS gives them, hash-based move detection where it does not); tests assert the outcome, not the mechanism.
- **Proves end-to-end with a real watcher:** items 1, 2, 3, 4, 5, 6, 7, 9, 10, 13. **CI on all three OSes here.**

### Slice 5: platform behaviour
- `CloudPlaceholder`: Windows attributes `0x00400000`, `0x00040000`, `0x00001000` via `MetadataExt`; macOS `SF_DATALESS` (verify the constant against SDK headers, as SPEC §6.1 requires); Linux `false`. A cloud-only file is `skipped` with `cloud_only` and never read. Attribute decoding is a pure function so it is unit-testable everywhere.
- Locked files: Windows `ERROR_SHARING_VIOLATION (32)` / `ERROR_LOCK_VIOLATION (33)` retry with backoff and do not count toward the error threshold for the first 3 tries.
- `ThreadPriority`: Windows `THREAD_PRIORITY_BELOW_NORMAL`, Linux `setpriority`, macOS QoS utility.
- `PowerStatus`: Windows `GetSystemPowerStatus`, Linux `/sys/class/power_supply`, macOS via `pmset -g batt` (see open points).
- Permission recovery: re-probe `permission_denied` roots every 30 s and reconcile on success; inotify `ENOSPC` -> `watch_failed`; network/mapped drives are not watched on Windows.
- Long-path test (over 260 characters) on Windows.
- **Proves:** items 14, 15, 16 (Windows only).

### Slice 6: resource policy
- Memory monitor (`sysinfo`, every 10 s) behind a small `MemoryProbe` trait so tests can inject "available < 1 GB" (NFR-13): pause indexing and unload the image model; resume when cleared.
- `worker_threads` auto formula `min(physical_cores / 2, total_ram_gb / 4)` clamped to 1-4; low-memory mode for machines at or under 8 GB (2 workers, idle unload after 2 minutes).
- Model idle unload on a timer using `idle_unload_minutes`; models load only while work is queued.
- Pause on battery (`pause_on_battery`).
- **Proves:** item 17.

### Slice 7: control surface and daemon
- `EngineHandle`: `status()` (`IndexStatus`: state, queued, indexed, skipped, errors, current file, per-root status), an events channel (status, progress, root status, permission issues), `pause`/`resume` persisted in `meta`, `add_root`, `remove_root` (purge), `set_root_enabled`, `apply_exclusions` (purge newly excluded, index newly included), `rescan_all`, `retry_errors`. DTOs go in `dto.rs` with serde only; the `ts-rs` derive waits for M6 (it is not a dependency yet).
- `magi-cli daemon`: runs the engine headless, prints live progress, Ctrl-C shuts down cleanly, `--stats` samples process CPU/RSS each minute so the manual benchmark is reproducible.
- **Proves:** items 12 and 13 through the real engine, plus persisted pause/resume.

### Slice 8: full integration suite, benchmark, docs, sign-off
- `crates/magi-core/tests/incremental.rs`: one test per verification item 1-17 (Windows-only gating for 16), polling with deadlines, never fixed sleeps. Retry-on-flake is not allowed; a flaky test is a bug to fix.
- Re-measure peak RSS with the engine running (NFR-11) and search latency while indexing (feeds M6's NFR-8 check).
- Manual 20k+ file, 1-hour `magi-cli daemon` run: idle CPU under 1%, recorded in `docs/benchmarks.md` (needs your machine).
- Docs: `docs/architecture.md` (engine, threads, state machine, watchers); ADR-0008 (runtime and threading model, sync-core design); ADR-0009 (watcher, rename and reconciliation policy); "M5 sign-off" section in `docs/progress.md` in the same table format as M4.

## Verification map (SPEC §7 M5)

| # | Item | First provable | Final (real engine + watcher) |
| - | --- | --- | --- |
| 1 | new file searchable within 10 s | S4 | S8 |
| 2 | modified -> old chunks/FTS/vectors gone | S1 (function) | S4 / S8 |
| 3 | touch only -> embed counter unchanged | S1 | S4 / S8 |
| 3b | model id changed -> re-embed; unchanged -> hash-skip | S1 | S8 |
| 4 | rename in a root, no re-embed | S2 | S4 / S8 |
| 5 | move between roots, `root_id` updated | S2 | S4 / S8 |
| 6 | move out of all roots removes it | S2 | S4 / S8 |
| 7 | delete removes all tables | S1 | S4 / S8 |
| 8 | changes while stopped converge | S2 | S8 |
| 9 | 1,000-file burst, exactly once | S3 | S4 / S8 |
| 10 | slow write indexed once, final content | S3 | S4 / S8 |
| 11 | killed while `indexing` recovers, integrity ok | S3 | S8 |
| 12 | root removed purges rows | S1 | S7 / S8 |
| 13 | exclusion add/remove | S2 | S7 / S8 |
| 14 | unreadable file -> `error`, others continue | S1 | S5 / S8 |
| 15 | root missing keeps index, restore without re-embed | S2 | S5 / S8 |
| 16 | Windows locked file retried | S5 | S8 |
| 17 | memory pressure pauses and unloads | S6 | S8 |
| manual | 20k files, 1 h, idle CPU < 1% | S7 (`--stats`) | S8 |

## New dependencies (each justified in its slice's commit; versions and APIs verified at add time, not assumed)
- `notify`, `notify-debouncer-full`, `crossbeam-channel`: named in SPEC §3.
- `sysinfo`: memory and physical-core count; SPEC §5.3 names it as the example.
- `libc` (Unix) and `windows-sys` (Windows, thread-priority and power APIs only): thin bindings; no native library, so no ADR needed.
- `ctrlc`: clean Ctrl-C for `magi-cli daemon`; tiny, pure Rust.

## Risks
1. **Real-watcher tests are the flakiest part** (FSEvents coalescing on macOS, buffer overflow on Windows). Mitigation: the deterministic synchronous tests from Slices 1-2 carry the logic; watcher tests only prove wiring, with generous deadlines and outcome-based assertions.
2. **macOS and Linux platform code cannot run on your Windows machine.** It is verified only by CI, so Slice 5 keeps it minimal, unit-tests the pure parts (attribute decoding, `power_supply` parsing), and expects an extra CI iteration.
3. **Memory:** extract workers, the embed worker and models coexist; NFR-11 (1.5 GB) is re-measured in Slice 8.
4. **`index_root` callers** (`magi-cli index/eval`, `xtask bench_corpus`) must keep working through the Slice 1 refactor.
5. **Existing databases** have `pipeline_version = 0`, so the first M5 run re-queues everything once (acceptable; documented).

## Defaults I chose (say so if you disagree)
- **One ONNX session for both search and indexing at 2 intra-op threads** in M5, instead of the spec's "cap at 2 while indexing, all cores for search" via two sessions (which would double the e5 memory). Revisit with the NFR-2 measurement in M6.
- **macOS battery status via `pmset -g batt`**, polled at most once a minute; on failure the feature is disabled there and documented, as SPEC §6.4 allows.

## Open points for you
- Slice 8's 1-hour benchmark needs a real 20k+ file folder on your machine.
- Adding `ctrlc` for the daemon is a small dependency; the alternative is a stop-file.
