# CLAUDE.md

`magi`: cross-platform desktop app for local, on-device semantic file search. Indexes user-selected folders (text, code, PDF, Office, images via OCR/QR/visual embeddings) into SQLite (FTS5 + `sqlite-vec`); hybrid keyword+vector search through a GPUI app (ADR-0011). Single process: `apps/desktop` hosts `magi-core::Engine` through `host::Host`. No network except one-time model downloads; never writes to user-selected folders.

## Where to look

Open only the row matching your task. Do not read `docs/SPEC.md` or `docs/progress.md` whole (1,000+ and 2,500+ lines); read `docs/SPEC.md` §0 and the sections below. `docs/architecture.md` is the current contract per module; `docs/SPEC.md` is the original design, and the spec wins on conflict until updated.

| Task touches                   | Code                                                        | Read                                                                |
| ------------------------------ | ----------------------------------------------------------- | ------------------------------------------------------------------- |
| Indexing pipeline, scheduling  | `crates/magi-core/src/index/`, `extract/`, `chunk.rs`       | architecture.md "Change detection", "Engine", "Resource policy"; SPEC §5.3–5.4 |
| File watching, reconciliation  | `crates/magi-core/src/watch/`                               | architecture.md "Watching"; ADR-0009                                |
| DB schema, migrations          | `crates/magi-core/src/db/` (`migrations/`)                  | architecture.md "Database schema"; SPEC §5.5                        |
| Search, ranking                | `crates/magi-core/src/search/`                              | architecture.md "Embeddings and hybrid search", "Search features"; SPEC §5.6; ADR-0010 |
| Embedders, model loading       | `crates/magi-core/src/embed/`, `onnx.rs`, `models/manifest.toml` | architecture.md "Model manifest and manager", "Text embedder"; ADR-0005, ADR-0007 |
| Images: OCR, QR, thumbnails    | `crates/magi-core/src/ocr/`, `qr.rs`, `thumbs.rs`, `extract/image.rs` | architecture.md "Images"; ADR-0006                       |
| Host API, DTOs                 | `crates/magi-core/src/host.rs`, `dto.rs`, `engine.rs`       | architecture.md "Host API", "Control surface"; SPEC §5.7            |
| Config, paths, roots           | `crates/magi-core/src/config/`, `paths.rs`, `db/roots.rs`   | architecture.md "Config", "Roots"; SPEC §4.5                        |
| OS-specific behaviour          | `crates/magi-core/src/platform/`                            | architecture.md "Platform behaviour"; `docs/permissions.md`; SPEC §6 |
| Desktop UI (search, settings, onboarding) | `apps/desktop/src/{search,settings,onboarding,ui}/`, `theme.rs`, `i18n/` | `DESIGN.md`, `PRODUCT.md`; architecture.md "Desktop host", "Desktop app"; ADR-0011 |
| Tray, hotkey, single-instance  | `apps/desktop/src/{tray,hotkey,instance}.rs`                | ADR-0011                                                            |
| Packaging, release             | `packager.json`, `.github/workflows/`, `xtask/`             | ADR-0012                                                            |
| Performance, memory            | n/a                                                         | `docs/benchmarks.md`, `docs/perf-investigation.md`                  |
| Search quality                 | `eval/` (`queries.jsonl`), `crates/magi-cli`                | `docs/eval.md`                                                      |
| Manual QA per OS               | n/a                                                         | `docs/qa-checklist.md`                                              |

Current milestone status: the last `##` sections of `docs/progress.md` (`grep -n '^## ' docs/progress.md`, then read the tail). SPEC §7 (milestone roadmap) is only for starting a new milestone.

Ignore: `docs/SPEC-v1.1-tauri.md` (frozen, superseded), screenshot folders under `docs/`, `target/`, `vendor/`, `graphify-out/` (generated). Whole-repo structure questions: `graphify-out/GRAPH_REPORT.md`.

## Commands

`just --list` shows all recipes; `just check` (fmt, clippy, tests) before committing. Single test: `cargo test -p magi-core <name>`.

`MAGI_FAKE_EMBEDDER=1` runs indexing/search with a deterministic fake embedder (no model downloads). `MAGI_DATA_DIR` / `MAGI_CONFIG_DIR` override storage locations (SPEC §4.5).

## Rules that matter most

- **Work one milestone at a time, in order** (SPEC.md §7). Don't start milestone N+1 until every verification item of milestone N passes and is recorded in `docs/progress.md`.
- **Hard constraints:** read-only on user-selected folders; index only user-selected roots; network only for model downloads listed in `models/manifest.toml`; verify or ask before using any URL, checksum, crate API or model filename; read cloud placeholders (OneDrive/iCloud/Dropbox) only by metadata, because reading content triggers a download.
- Rust: `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings` clean. `unwrap()`/`expect()` only in tests or documented startup invariants. `thiserror` in library crates, `anyhow` in binaries. `tracing` for logging; `println!` only for CLI user-facing output.
- `#[cfg(target_os = "...")]` lives only in `crates/magi-core/src/platform/`; business logic stays cfg-free.
- `magi-core` has no UI dependency and no async (`std::thread` + `crossbeam-channel`). `apps/desktop` views are thin over `Host`, with UI logic in plain functions tested by `cargo test`, and open/reveal files only by `file_id`.
- New dependencies: justify in the commit message, prefer pure Rust; new native C/C++ deps need an ADR in `docs/adr/`.
- Update `docs/architecture.md` when a public contract changes (DB schema, Host API, config format). Record significant decisions as ADRs in `docs/adr/` and milestone evidence in `docs/progress.md`.
- Conventional Commits (`feat(core): ...`, `fix(watch): ...`). Write unit tests first (TDD).
