# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Source of truth

**Read [`SPEC.md`](docs\SPEC.md) §0 before writing any code.** It is the authoritative spec (requirements, tech stack, directory layout, DB schema, IPC contract, milestone roadmap) — if code and spec disagree, the spec wins until the spec is updated. `AGENTS.md` is a short pointer to the same rules.

## What this project is

`magi` is a cross-platform (Windows/macOS/Linux) desktop app: local, on-device semantic file search. It indexes user-selected folders (text, code, PDF, Office docs, images via OCR/QR/visual embeddings) into SQLite (FTS5 + `sqlite-vec`) and serves hybrid keyword+vector search through a Tauri desktop shell. No network access except one-time model downloads; never writes to user-selected folders.

## Common commands

| Command         | Does                                                                            |
| --------------- | ------------------------------------------------------------------------------- |
| `just setup`    | Install frontend deps, fetch PDFium binaries                                    |
| `just dev`      | Run the Tauri app in dev mode                                                   |
| `just check`    | fmt, clippy, `cargo test`, frontend lint/typecheck/test — run before committing |
| `just test`     | Rust + frontend tests only (no network, fake embedder)                          |
| `just bindings` | No-op until M6 (`dto.rs` exports no IPC types yet)                              |
| `just models`   | Download ML models into the dev data directory                                  |
| `just eval`     | Run search-quality evaluation against `eval/queries.jsonl`                      |
| `just build`    | Production build of the desktop app                                             |

Single-crate/test equivalents (justfile wraps these): `cargo test -p magi-core <name>`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cd apps/desktop && pnpm test`.

`MAGI_FAKE_EMBEDDER=1` runs indexing/search with a deterministic fake embedder — no model downloads needed for dev/tests. `MAGI_DATA_DIR` / `MAGI_CONFIG_DIR` override storage locations (used by tests). See SPEC.md §4.5.

## Rules that matter most

- **Work one milestone at a time, in order** (SPEC.md §7). Don't start milestone N+1 until every verification item of milestone N passes and is recorded in `docs/progress.md`.
- **Hard constraints, never violate:** read-only on user-selected folders; index only user-selected roots; no network access beyond model downloads listed in `models/manifest.toml`; never invent URLs/checksums/crate APIs/model filenames — verify or ask; never trigger cloud-placeholder (OneDrive/iCloud/Dropbox) downloads by reading their content.
- Rust: `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings` clean. No `unwrap()`/`expect()` outside tests except documented startup invariants. `thiserror` in library crates, `anyhow` in binaries. Logging via `tracing` only (no `println!` outside CLI user-facing output).
- Platform-specific code (`#[cfg(target_os = "...")]`) lives only in `crates/magi-core/src/platform/`. Business logic must not contain `cfg` branches.
- New dependencies must be justified in the commit message; prefer pure-Rust crates; new native C/C++ deps require an ADR in `docs/adr/`.
- Update `docs/architecture.md` whenever a public contract changes (DB schema, IPC commands, config format). Record significant technical decisions as ADRs in `docs/adr/`. Record milestone verification evidence in `docs/progress.md`.
- Conventional Commits (`feat(core): ...`, `fix(watch): ...`, `test(extract): ...`).
- If any unit tests need to be written for a new feature, write them first (TDD).

## Architecture

Single process: the Tauri app (`apps/desktop/src-tauri`) hosts `magi-core::Engine` in-process, talking to it through an `EngineHandle` (channels + a read-only DB connection pool for search). `magi-cli daemon` hosts the same engine headless for dev/testing. Full thread/data-flow diagram: SPEC.md §5.3.

Crates (Cargo workspace, see `Cargo.toml`):

- **`crates/magi-core`** — all business logic, no Tauri dependency, no async (uses `std::thread` + `crossbeam-channel`; async only at Tauri command boundaries). Key modules: `engine.rs` (owns threads/channels/lifecycle), `dto.rs` (DTOs, `ts-rs`-derived, exported to the frontend — never hand-write IPC types), `db/` (SQLite via `rusqlite`, WAL, FTS5, `sqlite-vec`), `discovery/` (walk + classify), `extract/` (per-filetype extractors), `chunk.rs`, `embed/` (text/image embedder traits + model manager with lazy load/idle unload), `index/` (scheduler → pipeline → single DB-writer thread), `watch/` (file-system watcher, reconciliation scans, polling fallback), `search/` (BM25 + vector + Reciprocal Rank Fusion), `platform/` (OS-specific traits: permissions, power status, cloud placeholders, thread priority).
- **`crates/magi-cli`** — dev/test CLI: `doctor`, `roots`, `index`, `daemon`, `search`, `eval`.
- **`apps/desktop/src-tauri`** — thin Tauri command wrappers only; no business logic. `commands.rs` wraps `magi-core`, `events.rs` forwards engine events to the frontend, `state.rs` holds `AppState { engine: EngineHandle }`.
- **`apps/desktop/src`** (React + TypeScript + Vite + Tailwind) — `bindings/` is generated by `ts-rs`, never hand-edit it; `lib/ipc.ts` wraps `invoke()`/`listen()`.
- **`xtask`** — cross-platform dev tasks (fetch PDFium binaries, fetch models, generate bindings, bench corpus) instead of bash/PowerShell duplication.

Data flow for indexing: watcher/reconciliation/poller → scheduler (queue, dedupe, priority, retries) → extract workers → embed worker (1 thread, owns ONNX sessions via `ort`, small batches, yields to search) → single DB writer thread, one transaction per file. Change detection uses `blake3` content hashing plus a `pending → indexing → indexed|skipped|error` file state machine (SPEC.md §5.4).

Search: FTS5 BM25 + `vec_text` (multilingual-e5-small, 384-d) + `vec_image` (SigLIP 2, 768-d) run in parallel, aggregated per file, fused via Reciprocal Rank Fusion with filename/recency boosts (SPEC.md §5.6). DB schema: SPEC.md §5.5. IPC command table: SPEC.md §5.7.

Models are loaded lazily (only when there's queued work or an active search) and unloaded after idle, per `models/manifest.toml` (SPEC.md §3, §4.5).
