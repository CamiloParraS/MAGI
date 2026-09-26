# SPEC.md — magi: Local Semantic File Search

> **Codename:** `magi` (placeholder; see Open Questions). Replace globally once a final name is chosen.
> **Spec version:** 1.1 · **Status:** Approved for implementation · **Audience:** AI coding agents and human contributors
>
> **Changelog**
>
> - 1.1 (2026-09-10): Frontend confirmed as React + TypeScript. **HEIC/HEIF support moved into v1** (M4). **Reference machine set to an 8 GB RAM laptop**: memory budgets, quantized models, and memory-aware concurrency added.
> - 1.0: Initial spec.

---

## 0. How to use this document (rules for AI coding agents)

Read this entire file before writing code. It is the source of truth; if code and spec disagree, the spec wins until the spec is updated.

**Workflow rules**

1. Work **one milestone at a time, in order** (Section 7). Do not start milestone _N+1_ until every verification step of milestone _N_ passes and is recorded in `docs/progress.md`.
2. Keep commits small and use Conventional Commits (`feat(core): ...`, `fix(watch): ...`, `test(extract): ...`).
3. Keywords **MUST**, **MUST NOT**, **SHOULD**, **MAY** follow RFC 2119.
4. When something is ambiguous, conflicts with another section, or requires a decision not covered here, **stop and ask the human**. Log it under Section 9 (Open Questions). Never silently guess on: privacy, network access, anything that writes to user folders, or adding heavy/native dependencies.
5. Significant technical decisions are recorded as ADRs in `docs/adr/NNNN-title.md` (context, options, decision, consequences).

**Hard constraints (never violate)**

- **Read-only on user data.** The app MUST NEVER create, modify, move, or delete files inside user-selected folders. All app data lives in the app's own data/config/cache directories.
- **Only index user-selected folders** ("roots") and their descendants, minus exclusions.
- **No network access** except downloading model files listed in `models/manifest.toml`, after explicit user consent. No telemetry, no analytics, no crash reporting to remote servers.
- **Never invent** URLs, checksums, crate APIs, or model file names. Verify against official sources; if you cannot verify, ask.
- **Never trigger cloud file downloads** (OneDrive/iCloud/Dropbox placeholders) by reading their content.

**Code rules**

- Rust: `cargo fmt` clean; `cargo clippy --all-targets --all-features -- -D warnings` clean.
- No `unwrap()`/`expect()` in non-test code except for documented startup invariants.
- Errors: `thiserror` in library crates, `anyhow` in binaries. Logging via `tracing` only (no `println!` outside the CLI's user-facing output).
- Platform-specific code lives behind `#[cfg(target_os = "...")]` inside `platform/` modules. Business logic MUST NOT contain `cfg` branches.
- New dependencies: justify in the commit message. Prefer pure-Rust crates. New C/C++ native dependencies require an ADR.
- Pin exact versions via `Cargo.lock` / `pnpm-lock.yaml`. Centralize Rust versions in `[workspace.dependencies]`.
- Update `docs/architecture.md` whenever a public contract changes (DB schema, IPC commands, config format).

---

## 1. Product summary

**Problem.** Files with non-descriptive names (screenshots, downloads) are nearly impossible to find later. OS search is limited to filenames and keywords.

**Solution.** A cross-platform desktop app that indexes the contents of files in user-chosen folders (text, code, PDFs, Office documents, images via OCR and visual embeddings, QR codes) and lets the user search by meaning, in English or Spanish, with relevance-ranked results and preview snippets. Everything runs on-device.

**Indexing model (core behavior).**

1. The user selects which folders to index.
2. The app performs **one initial full index** of those folders, most recently modified files first.
3. After that, indexing is **incremental only**. The app detects changes (OS file-watching events plus reconciliation scans) and re-processes only files that were created, modified, moved, or deleted.
4. ML models are loaded **only when there is work to do** (queued files or an active search) and unloaded after an idle timeout.

**Supported platforms (v1)**

| OS      | Versions                                    | Arch                                     |
| ------- | ------------------------------------------- | ---------------------------------------- |
| Windows | 10 22H2+, 11                                | x86_64                                   |
| macOS   | 13 Ventura+                                 | aarch64 (required), x86_64 (best-effort) |
| Linux   | Ubuntu 22.04+, Fedora 39+ (X11 and Wayland) | x86_64                                   |

**Reference machine (minimum hardware that must run well).** All performance budgets are measured on this machine class:

| Component | Minimum                                                                                  |
| --------- | ---------------------------------------------------------------------------------------- |
| RAM       | **8 GB total** (assume ~3 GB free for us while the user works)                           |
| CPU       | 4 cores / 8 threads, ~2019 laptop class (e.g. Intel i5-8250U / Ryzen 5 3500U / Apple M1) |
| Storage   | SSD                                                                                      |
| GPU       | None assumed (CPU inference only)                                                        |

**Non-goals (v1):** audio/video indexing, cloud sync, network-drive watching (polling only), Mac App Store / Microsoft Store distribution, sandboxed packages (Flatpak/Snap), mobile, multi-user/shared indexes, editing or organizing files.

---

## 2. Requirements

### 2.1 Functional requirements

| ID    | Requirement                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ----- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| FR-1  | User can add/remove/enable/disable root folders via a native folder picker. Nested roots (a root inside another root) MUST be rejected with an explanatory message. Adding a parent of existing roots collapses them into it, keeping their indexed files (only new or changed files are indexed).                                                                                                                                                                   |
| FR-2  | Supported content types: plain text & Markdown; source code (common languages); PDF; DOCX, PPTX, XLSX; images (PNG, JPEG, WebP, GIF first frame, BMP, TIFF, **HEIC/HEIF**) with EXIF-orientation correction, OCR, QR/barcode decoding, and visual embeddings. All other files are indexed by **filename/path only**. Data files (`.csv`, `.json`) index only their first 64 KB of content, cut at a line break; the whole file is still hashed for change detection. |
| FR-3  | Initial full index on first run; afterwards incremental indexing driven by file-system events and reconciliation scans (Section 5.4).                                                                                                                                                                                                                                                                                                                                |
| FR-4  | Natural-language search in English and Spanish, including cross-lingual (Spanish query finds English content and vice versa).                                                                                                                                                                                                                                                                                                                                        |
| FR-5  | Hybrid ranking: keyword (BM25) + text-vector + image-vector, fused with Reciprocal Rank Fusion, with filename and recency boosts.                                                                                                                                                                                                                                                                                                                                    |
| FR-6  | Each result shows: file name, path, type icon or thumbnail, matched snippet with highlights, page number (PDF), modified date.                                                                                                                                                                                                                                                                                                                                       |
| FR-7  | Global hotkey opens a Spotlight-style search window. `Enter` opens the file with the default app; `Ctrl/Cmd+Enter` reveals it in the file manager.                                                                                                                                                                                                                                                                                                                   |
| FR-8  | Tray/menu-bar icon with: open search, pause/resume indexing, settings, status line, quit.                                                                                                                                                                                                                                                                                                                                                                            |
| FR-9  | Settings: roots, exclusion globs, enabled file types, max file size, hotkey, pause-on-battery, launch-at-login, search features (enable/disable, download status and size, remove download; ADR-0010), language, window background, index stats, error list with retry, "clear index". |
| FR-10 | First-run onboarding: choose folders → permission check → choose search features with consent to download (sizes shown; "Search by meaning" and "Read text in images" pre-selected, "Find images by what they show" not) → "Start with your computer" (checked) → initial indexing progress (window can be closed; indexing continues in background). Every search feature is optional; with none installed, search is keyword + filename (ADR-0010). |
| FR-11 | Per-root status visible to the user: `ok`, `permission_denied`, `missing`, `watch_failed (polling)`.                                                                                                                                                                                                                                                                                                                                                                 |
| FR-12 | CLI (`magi-cli`) exposing indexing, search, diagnostics, and evaluation for development and testing.                                                                                                                                                                                                                                                                                                                                                                 |

### 2.2 Non-functional requirements (targets — measure and record in `docs/benchmarks.md`)

| ID     | Requirement                                                                | Target                                                                                                                                                |
| ------ | -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| NFR-1  | Idle footprint (indexing complete, models unloaded, window hidden)         | RSS ≤ 150 MB, avg CPU < 1%                                                                                                                            |
| NFR-2  | Search latency, warm models, 100k chunks, reference machine                | p95 ≤ 300 ms                                                                                                                                          |
| NFR-3  | Search latency, cold (models unloaded)                                     | ≤ 3 s                                                                                                                                                 |
| NFR-4  | Change-to-searchable latency, single small text file, idle system          | ≤ 10 s                                                                                                                                                |
| NFR-5  | Crash safety                                                               | No index corruption after `kill -9` at any point; indexing resumes on restart                                                                         |
| NFR-6  | Installer size (models excluded)                                           | ≤ 80 MB                                                                                                                                               |
| NFR-7  | Total model download | Hard limit ≤ 1 GB, target ≤ 700 MB for all features (quantized models; record exact sizes). Default onboarding selection ≈ 148 MB (ADR-0010) |
| NFR-8  | Background politeness                                                      | Indexing threads run at low OS priority; ONNX intra-op threads capped; search is never blocked behind an indexing batch for more than one small batch |
| NFR-9  | Privacy                                                                    | Zero outbound connections except model downloads from the manifest                                                                                    |
| NFR-10 | Accessibility                                                              | Full keyboard operation; follows OS light/dark theme                                                                                                  |
| NFR-11 | Peak memory during initial indexing (all models loaded, reference machine) | RSS ≤ 1.5 GB                                                                                                                                          |
| NFR-12 | Peak memory during search only (query encoders loaded, no indexing)        | RSS ≤ 900 MB                                                                                                                                          |
| NFR-13 | Memory pressure                                                            | When the OS reports < 1 GB available RAM, pause indexing and unload the image model until it recovers                                                 |

---

## 3. Technology stack

| Layer                      | Choice                                                                                                                                              | Rationale / notes                                                                                                                                                                                                                                                                                                |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Desktop shell              | **Tauri 2.x**                                                                                                                                       | Small bundles, low idle RAM, Rust backend in-process. Uses system webviews (WebView2 / WKWebView / WebKitGTK); test Linux early.                                                                                                                                                                                 |
| Frontend                   | **React + TypeScript + Vite**, Tailwind CSS                                                                                                         | Confirmed. State via React state; add Zustand only if needed. Tests: Vitest + Testing Library.                                                                                                                                                                                                                   |
| Core language              | **Rust (stable)**                                                                                                                                   | Pinned in `rust-toolchain.toml`.                                                                                                                                                                                                                                                                                 |
| Concurrency                | `std::thread` + `crossbeam-channel`                                                                                                                 | Avoid async in `magi-core`. Async only at Tauri command boundaries.                                                                                                                                                                                                                                              |
| Database                   | **SQLite** via `rusqlite` (`bundled` feature) + **FTS5** + **sqlite-vec** (`sqlite-vec` crate, statically registered)                               | One file, no server, no Docker. WAL mode.                                                                                                                                                                                                                                                                        |
| File walking               | `ignore`                                                                                                                                            | Fast, glob/gitignore-style filtering.                                                                                                                                                                                                                                                                            |
| File watching              | `notify` + `notify-debouncer-full`                                                                                                                  | ReadDirectoryChangesW / FSEvents / inotify; debouncer tracks renames.                                                                                                                                                                                                                                            |
| Hashing                    | `blake3`                                                                                                                                            | Content hash for change detection and move detection.                                                                                                                                                                                                                                                            |
| Type detection             | extension + `infer` (magic bytes)                                                                                                                   |                                                                                                                                                                                                                                                                                                                  |
| Text decoding              | `chardetng` + `encoding_rs`                                                                                                                         | Legacy Spanish files are often Windows-1252 / Latin-1. Normalize to Unicode NFC (`unicode-normalization`).                                                                                                                                                                                                       |
| PDF                        | `pdfium-render` + bundled PDFium binaries                                                                                                           | Text per page + first-page thumbnail.                                                                                                                                                                                                                                                                            |
| Office                     | `zip` + `quick-xml` (DOCX/PPTX), `calamine` (XLSX)                                                                                                  |                                                                                                                                                                                                                                                                                                                  |
| Code                       | `tree-sitter` + grammars (Rust, Python, JS/TS, Java, C/C++, Go, C#)                                                                                 | Chunk by top-level symbol; fallback to line windows.                                                                                                                                                                                                                                                             |
| Language detection         | `whatlang`                                                                                                                                          | Stored per file; used for stats and optional stemming later.                                                                                                                                                                                                                                                     |
| ML runtime                 | **`ort` (ONNX Runtime) 2.x** + `tokenizers`                                                                                                         | CPU by default. GPU execution providers are out of scope for v1.                                                                                                                                                                                                                                                 |
| Text embeddings            | **`intfloat/multilingual-e5-small`** (384-d, MIT), **int8-quantized ONNX** if eval quality holds                                                    | Requires prefixes: `"query: "` / `"passage: "`. Mean pooling + L2 normalization.                                                                                                                                                                                                                                 |
| Image embeddings           | **SigLIP 2 base** (Apache-2.0), ONNX export, **quantized (int8 or equivalent) required** to fit the 8 GB budget unless eval shows unacceptable loss | Image tower for files, text tower for queries. Text tower: Gemma tokenizer, pad to 64 tokens exactly as the reference implementation.                                                                                                                                                                            |
| OCR                        | `OcrEngine` trait; candidates: `ocrs` (pure Rust) vs PaddleOCR ONNX via `ort`                                                                       | Chosen in M4 by measured accuracy on English + Spanish fixtures (ADR required). Tesseract excluded (native packaging burden).                                                                                                                                                                                    |
| QR / barcodes              | `rxing`                                                                                                                                             | Decoded payload indexed as text.                                                                                                                                                                                                                                                                                 |
| Images                     | `image` crate (Lanczos3 resize)                                                                                                                     | Decode limits to prevent decompression bombs. Apply EXIF orientation (via `kamadak-exif` or the `image` crate's orientation support) before OCR/embedding/thumbnails. `fast_image_resize` not adopted: one Lanczos3 pass costs ~0.55 s at 48 MP (release), and the thumbnail is derived from the OCR-sized copy. |
| HEIC/HEIF                  | **`heic-rs`** (pure Rust, MIT OR Apache-2.0, no `unsafe`)                                                                                           | Chosen in the M4 HEIC spike (ADR-0003) over `libheif-rs`: same output on every fixture, 1.3–2.3× faster, and no native library to install, bundle or license. `libheif-rs` remains the documented fallback. No embedded-thumbnail API — thumbnails come from the same decode as OCR/embedding.                   |
| HTTP (model download only) | `ureq` with rustls                                                                                                                                  | Synchronous, small. Only constructed inside the model manager.                                                                                                                                                                                                                                                   |
| Config                     | `serde` + `toml`; paths via `directories`                                                                                                           |                                                                                                                                                                                                                                                                                                                  |
| TS bindings                | `ts-rs`                                                                                                                                             | Generated DTO types into the frontend; never hand-write IPC types.                                                                                                                                                                                                                                               |
| Tauri plugins              | global-shortcut, single-instance, autostart, dialog, opener, (tray via core `tray-icon` feature)                                                    |                                                                                                                                                                                                                                                                                                                  |
| Task runner                | `just`                                                                                                                                              | Cross-platform command entry points.                                                                                                                                                                                                                                                                             |
| Scripting                  | `xtask` crate                                                                                                                                       | Cross-platform dev tasks (fetch PDFium, fetch models) without bash/PowerShell duplication.                                                                                                                                                                                                                       |
| CI/CD                      | GitHub Actions, `tauri-action` for releases                                                                                                         | Matrix: ubuntu-22.04, macos-14, windows-latest.                                                                                                                                                                                                                                                                  |

---

## 4. Setup — what you need before writing code

### 4.1 Repository decision

Use **one repository (monorepo)** containing a Cargo workspace plus the frontend package. The core engine, CLI, and desktop app share types and must version together; multiple repos would add coordination cost with no benefit.

### 4.2 Accounts and tools

- GitHub account + repository (public recommended for a portfolio project). Enable GitHub Actions.
- No Hugging Face account needed (models are public), but model URLs and SHA-256 hashes must be verified from the official model pages.
- Editor: VS Code with `rust-analyzer`, `Tauri`, `Even Better TOML`, `ESLint`, `Tailwind CSS IntelliSense` (recommended).
- **Optional:** Python 3.11+ with `uv`, only for `tools/reference_embeddings.py` (generates reference vectors used in parity tests). Python is never a runtime dependency.

### 4.3 Per-OS prerequisites

**All platforms**

- Rust via `rustup` (stable, with `rustfmt` and `clippy`).
- Node.js LTS + pnpm (`corepack enable`).
- `just` (`cargo install just --locked`, or the OS package manager).
- Tauri CLI: `pnpm add -D @tauri-apps/cli@^2` in `apps/desktop` (preferred), or `cargo install tauri-cli --version "^2" --locked`.
- **HEIC needs nothing installed.** ADR-0003 chose the pure-Rust `heic-rs`; the libheif install steps this section used to carry are gone. If the fallback to `libheif-rs` is ever taken, they come back — and with them Ubuntu 22.04's libheif 1.12.0, which is below that crate's 1.17.0 minimum (see ADR-0003).

**Windows**

- Visual Studio 2022 Build Tools with the **"Desktop development with C++"** workload (MSVC + Windows SDK).
- WebView2 Runtime (preinstalled on Windows 11; install the Evergreen runtime on Windows 10 if missing).
- Enable long paths for development: Group Policy "Enable Win32 long paths", or registry `LongPathsEnabled=1`.

**macOS**

- Xcode Command Line Tools: `xcode-select --install`.
- For universal builds: `rustup target add aarch64-apple-darwin x86_64-apple-darwin`.

**Linux (Debian/Ubuntu)**

```bash
sudo apt update
sudo apt install -y build-essential curl wget file pkg-config libssl-dev \
  libwebkit2gtk-4.1-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev
```

For Fedora, use the equivalent packages from the Tauri prerequisites page (`webkit2gtk4.1-devel`, `openssl-devel`, `libappindicator-gtk3-devel`, `librsvg2-devel`, and the "C Development Tools and Libraries" group). **Agents MUST check the current official Tauri v2 prerequisites page and update this list if it has changed.**

### 4.4 Bootstrap steps (performed in M0)

1. `git init magi && cd magi`, then add `.gitignore`, `.gitattributes`, `.editorconfig`, `LICENSE` (MIT recommended), and `README.md`.
2. Create `rust-toolchain.toml` pinned to the current stable version, and a workspace `Cargo.toml` (Section 5.1).
3. Create crates: `cargo new --lib crates/magi-core`, `cargo new crates/magi-cli`, `cargo new xtask`.
4. Scaffold the Tauri app with `create-tauri-app` (React + TypeScript template, pnpm) into `apps/desktop`. Move/adjust files to match Section 5.1, and add `apps/desktop/src-tauri` to workspace members.
5. Add `justfile`, CI workflows, `docs/` skeleton, and `fixtures/` skeleton.
6. `cargo xtask fetch-pdfium`: download PDFium prebuilt binaries for the host OS into `vendor/pdfium/<target>/` (git-ignored). The source repository and version are pinned in `xtask` and documented in an ADR.
7. `just check` must pass on all three OSes.

### 4.5 Environment variables

| Variable               | Purpose                                                                                      |
| ---------------------- | -------------------------------------------------------------------------------------------- |
| `MAGI_DATA_DIR`        | Override the data directory (DB, models, thumbnails). Used by tests and portable dev setups. |
| `MAGI_CONFIG_DIR`      | Override the config directory.                                                               |
| `MAGI_LOG`             | `tracing` env-filter (e.g. `info,magi_core::watch=debug`).                                   |
| `MAGI_FAKE_EMBEDDER=1` | Use the deterministic fake embedder (tests, CI, UI development without models).              |

---

## 5. Repository architecture

### 5.1 Directory structure

```
magi/
├── SPEC.md                         # this file (source of truth)
├── AGENTS.md                       # short pointer: "Read SPEC.md §0 first", plus common commands
├── README.md                       # user-facing: what it is, install, privacy, screenshots
├── LICENSE
├── THIRD_PARTY_LICENSES.md         # generated (cargo-about) + manual entries for bundled native libs
├── Cargo.toml                      # workspace root
├── Cargo.lock
├── rust-toolchain.toml
├── rustfmt.toml
├── .editorconfig
├── .gitignore                      # target/, node_modules/, dist/, vendor/, *.db, models/cache/
├── .gitattributes                  # * text=auto eol=lf ; *.png/*.pdf/*.jpg binary
├── justfile
├── .github/
│   └── workflows/
│       ├── ci.yml                  # fmt, clippy, tests, frontend checks — 3-OS matrix
│       ├── eval.yml                # manual/nightly: downloads models (cached), runs eval
│       └── release.yml             # on tag v*: tauri-action builds installers
├── crates/
│   ├── magi-core/                 # ALL business logic; no Tauri dependency
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs              # pub use Engine, EngineHandle, DTOs
│   │   │   ├── error.rs
│   │   │   ├── engine.rs           # Engine: owns threads, channels, lifecycle
│   │   │   ├── dto.rs              # serializable types shared with UI (ts-rs derives)
│   │   │   ├── config/             # load/save/validate config.toml, defaults per OS
│   │   │   ├── paths.rs            # data/config/cache dirs, path canonicalization
│   │   │   ├── db/
│   │   │   │   ├── mod.rs          # connection setup (WAL, pragmas, sqlite-vec registration)
│   │   │   │   ├── migrations/     # 0001_init.sql, 0002_... (embedded via include_str!)
│   │   │   │   ├── files.rs        # file/chunk/vector repositories
│   │   │   │   └── roots.rs
│   │   │   ├── discovery/          # walker, exclusion filters, classifier
│   │   │   ├── extract/            # Extractor trait + text, code, pdf, office, image, heic, filename
│   │   │   ├── ocr/                # OcrEngine trait + chosen implementation
│   │   │   ├── qr.rs
│   │   │   ├── chunk.rs            # token-aware chunking
│   │   │   ├── embed/
│   │   │   │   ├── mod.rs          # TextEmbedder / ImageEmbedder traits, FakeEmbedder
│   │   │   │   ├── manager.rs      # model manifest, download, verify, lazy load, idle unload
│   │   │   │   ├── e5.rs           # multilingual-e5-small
│   │   │   │   └── siglip.rs       # SigLIP 2 image + text towers
│   │   │   ├── index/
│   │   │   │   ├── scheduler.rs    # queue, dedupe, priorities, delays, retries
│   │   │   │   ├── pipeline.rs     # hash → extract → chunk → embed → write
│   │   │   │   └── writer.rs       # single DB writer thread
│   │   │   ├── watch/
│   │   │   │   ├── watcher.rs      # notify + debouncer per root
│   │   │   │   ├── reconcile.rs    # full/partial reconciliation scans
│   │   │   │   └── poller.rs       # polling fallback
│   │   │   ├── search/
│   │   │   │   ├── mod.rs          # hybrid search orchestration
│   │   │   │   ├── fts.rs          # query sanitization, BM25, snippet()
│   │   │   │   ├── vector.rs
│   │   │   │   └── fuse.rs         # RRF + boosts
│   │   │   ├── thumbs.rs           # thumbnail cache keyed by content hash
│   │   │   └── platform/
│   │   │       ├── mod.rs          # traits: PermissionProbe, PowerStatus, CloudPlaceholder, ThreadPriority
│   │   │       ├── windows.rs
│   │   │       ├── macos.rs
│   │   │       └── linux.rs
│   │   └── tests/                  # integration tests (incremental.rs, search.rs, extract.rs)
│   └── magi-cli/                  # dev/test CLI: doctor, roots, index, daemon, search, eval
│       └── src/main.rs
├── apps/
│   └── desktop/
│       ├── package.json
│       ├── pnpm-lock.yaml
│       ├── vite.config.ts
│       ├── tsconfig.json
│       ├── tailwind.config.ts
│       ├── index.html
│       ├── src/
│       │   ├── main.tsx
│       │   ├── bindings/           # GENERATED by ts-rs — do not edit
│       │   ├── lib/ipc.ts          # typed wrappers around invoke()/listen()
│       │   ├── windows/
│       │   │   ├── SearchWindow.tsx
│       │   │   ├── SettingsWindow.tsx
│       │   │   └── Onboarding.tsx
│       │   ├── components/         # ResultItem, Snippet, RootList, StatusBadge, ...
│       │   └── styles/
│       └── src-tauri/
│           ├── Cargo.toml
│           ├── tauri.conf.json
│           ├── Info.plist          # macOS usage-description strings (merged by Tauri)
│           ├── capabilities/
│           │   ├── search.json     # least-privilege permissions per window
│           │   └── settings.json
│           ├── icons/
│           └── src/
│               ├── main.rs
│               ├── state.rs        # AppState { engine: EngineHandle }
│               ├── commands.rs     # #[tauri::command] thin wrappers over magi-core
│               ├── events.rs       # forwards engine events to the frontend
│               ├── tray.rs
│               ├── hotkey.rs
│               └── windows.rs      # create/show/hide search & settings windows
├── models/
│   └── manifest.toml               # model ids, file URLs, sha256, dims, licenses
├── fixtures/
│   ├── corpus/                     # small, license-clean test files
│   │   ├── en/  es/  code/  pdf/  office/  images/  qr/  edge/   # edge = corrupt, huge, odd encodings
│   ├── golden/                     # expected extraction outputs
│   └── reference_embeddings/       # JSON vectors from tools/reference_embeddings.py
├── eval/
│   ├── queries.jsonl               # {"query","lang","expected":[rel paths],"notes"}
│   └── README.md
├── tools/
│   └── reference_embeddings.py     # dev-only (uv run)
├── xtask/
│   └── src/main.rs                 # fetch-pdfium, fetch-models, gen-bindings, bench-corpus
├── vendor/                         # git-ignored: pdfium and ONNX Runtime binaries per target
└── docs/
    ├── architecture.md
    ├── progress.md                 # milestone checklist + verification evidence
    ├── benchmarks.md
    ├── eval.md                     # recall@k / MRR history per model/ranking change
    ├── qa-checklist.md             # manual per-OS test script
    ├── permissions.md              # per-OS permission behavior and user guidance
    └── adr/
        ├── 0001-stack.md
        ├── 0002-storage-sqlite-vec.md
        ├── 0003-heic-decoding.md
        └── ...
```

### 5.2 Key configuration files

**`Cargo.toml` (workspace root)**

```toml
[workspace]
resolver = "2"
members = ["crates/magi-core", "crates/magi-cli", "apps/desktop/src-tauri", "xtask"]

[workspace.package]
edition = "2021"          # or 2024 if the pinned toolchain supports it and all deps build
license = "MIT"
version = "0.1.0"

[workspace.dependencies]
# Centralize ALL shared dependency versions here; member crates use `dep = { workspace = true }`.

[profile.release]
lto = "thin"
codegen-units = 1
strip = true

[profile.dev.package."*"]
opt-level = 2             # keep image decoding/tokenizers usable in debug builds
```

**`rust-toolchain.toml`**

```toml
[toolchain]
channel = "1.XX.0"        # pin to the current stable at bootstrap
components = ["rustfmt", "clippy"]
```

**`justfile`** (required recipes)

| Recipe          | Does                                                                                                  |
| --------------- | ----------------------------------------------------------------------------------------------------- |
| `just setup`    | `pnpm install` in `apps/desktop`, `cargo xtask fetch-pdfium`                                          |
| `just dev`      | `pnpm tauri dev`                                                                                      |
| `just check`    | fmt --check, clippy -D warnings, `cargo test --workspace`, `pnpm lint`, `pnpm typecheck`, `pnpm test` |
| `just test`     | Rust + frontend tests (fake embedder, no model downloads)                                             |
| `just bindings` | Regenerate ts-rs bindings and fail if the git diff is non-empty (in CI)                               |
| `just models`   | `cargo xtask fetch-models` into the dev data dir                                                      |
| `just eval`     | `cargo run -p magi-cli --release -- eval eval/queries.jsonl --corpus fixtures/corpus`                 |
| `just build`    | `pnpm tauri build`                                                                                    |

**User config file** — `<config_dir>/magi/config.toml` (created with defaults on first run)

```toml
schema_version = 1

[[roots]]
path = "/Users/ana/Pictures/Screenshots"
enabled = true

[indexing]
exclude_globs = ["**/node_modules/**", "**/.git/**", "**/target/**", "**/.venv/**",
                 "**/__pycache__/**", "**/*.tmp", "**/*.part", "**/*.crdownload",
                 "**/~$*", "**/.~lock.*",
                 # Unity's regenerated caches and build output
                 "**/Library/PackageCache/**", "**/Library/Bee/**", "**/Library/ShaderCache/**",
                 "**/Library/BurstCache/**", "**/Library/ScriptAssemblies/**", "**/*.meta",
                 "**/*.dll", "**/*.pdb", "**/*.obj", "**/*.o"]
include_hidden = false
follow_symlinks = false
max_file_size_mb = 50
file_types = ["text", "code", "pdf", "office", "image"]
pause_on_battery = true
worker_threads = 0                 # 0 = auto (see §5.3 memory-aware concurrency)
max_image_megapixels = 64          # a 48 MP phone HEIC must pass; larger images are skipped
reconcile_interval_hours = 6

[models]
idle_unload_minutes = 5            # auto-lowered to 2 on machines with ≤ 8 GB RAM

[ui]
hotkey = "CmdOrCtrl+Shift+Space"   # NOT Alt+Space (conflicts with the Windows window menu / PowerToys)
theme = "system"
max_results = 30
launch_at_login = false            # onboarding offers it checked (ADR-0010)
language = "system"                # system | en | es; system = es-* → es, else en
transparency_mode = "match_system" # match_system | always | never (search window only)
transparency_intensity = 0.75      # 0.40–0.95, alpha of the tint over the native effect

[features]                         # desired state; install state is on disk (ADR-0010)
meaning = true                     # e5
image_text = true                  # OCR
image_visual = false               # SigLIP 2
```

Per-OS default exclusions are added in code (not written to the file):

- **Windows:** `%WINDIR%`, `AppData`, `$Recycle.Bin`, `System Volume Information`.
- **macOS:** `~/Library`, `.Trash`; package bundles (`*.app`, `*.photoslibrary`, `*.bundle`, `*.framework`) are treated as opaque (name only, never descended).
- **Linux:** `~/.cache`, `~/.local/share/Trash`, `/proc`, `/sys`.

**Model manifest** — `models/manifest.toml`

```toml
[[model]]
slot = "text"                       # text | image
id = "intfloat/multilingual-e5-small"
revision = "<commit sha>"           # pin the Hugging Face revision
license = "MIT"
dim = 384
max_tokens = 512
query_prefix = "query: "
passage_prefix = "passage: "
pooling = "mean"
files = [
  { name = "model.onnx",     url = "<verified URL>", sha256 = "<computed>", size = 0 },
  { name = "tokenizer.json", url = "<verified URL>", sha256 = "<computed>", size = 0 },
]

[[model]]
slot = "image"
id = "google/siglip2-base-patch16-<res>"   # choose variant in M4; record in ADR
license = "Apache-2.0"
dim = 768
text_max_tokens = 64
files = [ ... ]                     # image tower, text tower, tokenizer
```

Agents MUST fill URLs and hashes by downloading from the official source and computing SHA-256. Placeholders MUST NOT ship.

### 5.3 Runtime architecture

**Process model.** A single process: the Tauri app hosts `magi-core::Engine`. The engine runs on its own threads; Tauri commands talk to it through an `EngineHandle` (channels plus a read-only DB connection pool for search). The CLI hosts the same engine headless (`magi-cli daemon`).

**Threads**

```
Watcher threads (notify, one per root) ──┐
Reconciliation scanner ──────────────────┼──► Scheduler ──► Extract workers (N, low priority)
Poller (fallback roots) ─────────────────┘      (queue,             │
                                                 dedupe,             ▼
                                                 retries)      Embed worker (1 thread, owns ONNX sessions,
                                                                     │        small batches, yields to search)
                                                                     ▼
                                                               DB writer (1 thread, single write connection)
Search requests ──► reader connection pool + query encoders (shared sessions, priority lock)
```

- SQLite runs in **WAL mode** with exactly one writer connection. Search uses separate read connections, so readers never block the writer.
- Every file's DB update (delete old chunks/vectors, insert new ones, update the `files` row) happens in **one transaction**.
- **Priority lock:** the embed worker processes batches of ≤ 16 chunks and checks a `search_pending` flag between batches, so a search waits at most one small batch.

**Memory-aware concurrency (8 GB reference machine).**

- Auto `worker_threads`: `min(physical_cores / 2, total_ram_gb / 4)`, clamped to 1–4. An 8 GB machine gets 2 extract workers.
- **Image decode semaphore:** at most 1 concurrent decode for images > 12 MP, and at most 2 total. A 48 MP image decoded as RGBA is ~190 MB.
- Downscale immediately after decode: long side ≤ 2048 px for OCR, model input size for SigLIP, 256 px for thumbnails. Drop the full-resolution buffer as soon as possible.
- The image model is loaded only while image files are queued, then unloaded. Text-only work never holds it in memory.
- SQLite: `cache_size` ≈ 64 MB, `mmap_size` ≤ 256 MB.
- Available-memory check every 10 s (e.g. via `sysinfo`). Below 1 GB free → pause indexing and unload the image model (NFR-13).

**Engine events** (sent to the UI): `IndexStatus` changes, progress (queued / done / errors / current file), root status changes, permission issues, search-feature status (complete `FeatureStatus[]`, including download and backfill progress).

### 5.4 Change detection and file state machine

**File states:** `pending → indexing → indexed | skipped | error`. Deletions remove the row entirely.

```
            create / modify / reconcile-diff
   ┌──────────────────────────────────────────┐
   ▼                                          │
pending ──► indexing ──► indexed ─────────────┘
               │  ├────► skipped (unsupported, too large, cloud placeholder, excluded)
               │  └────► error (after 3 attempts; manual retry)
   (startup: any "indexing" rows reset to "pending")
```

**Startup sequence**

1. Load config, open the DB, run migrations, reset `indexing → pending`.
2. For each enabled root: run the permission probe (Section 6). If the root is inaccessible, set its status and skip it.
3. **Start the watcher before scanning** and buffer its events, so no change is lost during the scan.
4. **Reconciliation scan** with a fresh `scan_id`:
   - Walk the root with filters. For each file:
     - If unknown, insert it as `pending`.
     - If `size` or `mtime` differ from the stored row, mark it `pending`.
     - Set `seen_scan_id = scan_id`.
   - After the walk, rows in this root with an older `seen_scan_id` are **deletion candidates**. Hold them briefly for move detection (below), then delete.
5. Drain the buffered watcher events into the scheduler.
6. The queue is ordered by **mtime descending**, so recent files become searchable first.

**Runtime events** (debounced ~2 s by `notify-debouncer-full`)

| Event                                 | Action                                                                |
| ------------------------------------- | --------------------------------------------------------------------- |
| Create / Modify                       | Upsert the row as `pending`                                           |
| Remove                                | Delete the file, its chunks, FTS rows, vectors, and thumbnail refs    |
| Rename within indexed roots           | Update `path` / `rel_path` / `root_id` in place. **No re-embedding.** |
| Rename to an excluded or outside path | Delete                                                                |
| Rename from an unknown path           | Treat as Create                                                       |
| Rescan / overflow / watcher error     | Run a reconciliation scan of the affected root                        |

**Processing a `pending` file**

1. `stat` the file. If it is gone, delete the row.
2. **Stability check:** if the file's `mtime` is < 3 s old, or its size changes between two stats 1 s apart, requeue it with a 5 s delay. This handles downloads and files still being written.
3. Cloud placeholder or dataless check (Section 6). If it matches, index filename/path only and mark it `skipped` with reason `cloud_only`.
4. Compute the streaming `blake3` hash.
   - If the hash equals the stored hash and `pipeline_version` is current, update only `size` / `mtime`. Done, no re-embedding.
   - If the hash matches a **deletion candidate** or recently deleted file (move across roots, or a rename the OS reported as delete+create), reassign its chunks and vectors to this path. Done.
5. Extract → chunk → embed → write (one transaction). Per-file extraction timeout: 60 s.
6. On failure: `attempts += 1`, then retry with exponential backoff (30 s, 2 min, 10 min). Windows sharing violations always retry. After 3 attempts the file goes to `error` with a message.

**Periodic safety nets**

- Reconciliation every `reconcile_interval_hours`.
- Reconciliation after a detected wall-clock jump of more than 5 minutes between 1-minute ticks. This is a cross-platform way to notice sleep/resume.
- Roots whose watcher fails (inotify limit, network drive, error) switch to **polling reconciliation every 15 minutes**, with status `watch_failed`.

**Missing roots** (unplugged USB drive, unmounted volume): set status `missing` and **keep the index**, hiding those results. Purge only when the user removes the root.

**Versioning:** a `PIPELINE_VERSION` constant (bumped when extraction or chunking changes) and the stored embedding-model IDs. On mismatch, affected files are re-queued in the background; old results stay searchable until they are replaced.

### 5.5 Database schema (`migrations/0001_init.sql`)

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- keys: pipeline_version, text_model_id, image_model_id, ocr_engine_id, last_scan_id

CREATE TABLE roots (
  id                INTEGER PRIMARY KEY,
  path              TEXT NOT NULL UNIQUE,          -- canonical absolute path
  enabled           INTEGER NOT NULL DEFAULT 1,
  status            TEXT NOT NULL DEFAULT 'ok',    -- ok|permission_denied|missing|watch_failed
  last_full_scan_at INTEGER,
  added_at          INTEGER NOT NULL
);

CREATE TABLE files (
  id               INTEGER PRIMARY KEY,
  root_id          INTEGER NOT NULL REFERENCES roots(id),
  path             TEXT NOT NULL UNIQUE,
  rel_path         TEXT NOT NULL,
  file_name        TEXT NOT NULL,
  ext              TEXT,
  kind             TEXT NOT NULL,                  -- text|code|pdf|office|image|other
  size             INTEGER NOT NULL,
  mtime_ns         INTEGER NOT NULL,
  content_hash     BLOB,                           -- blake3 (32 bytes)
  lang             TEXT,                           -- ISO 639-1
  state            TEXT NOT NULL,                  -- pending|indexing|indexed|skipped|error
  skip_reason      TEXT,
  error            TEXT,
  attempts         INTEGER NOT NULL DEFAULT 0,
  next_attempt_at  INTEGER,
  pipeline_version INTEGER NOT NULL DEFAULT 0,
  seen_scan_id     INTEGER NOT NULL,
  indexed_at       INTEGER,
  thumb_key        TEXT                            -- content-hash-based cache key
);
CREATE INDEX idx_files_state ON files(state, next_attempt_at);
CREATE INDEX idx_files_root  ON files(root_id);
CREATE INDEX idx_files_hash  ON files(content_hash);
CREATE INDEX idx_files_size  ON files(size, kind) WHERE content_hash IS NOT NULL; -- move lookup
CREATE INDEX idx_files_pending ON files(mtime_ns DESC, id) WHERE state = 'pending'; -- scheduler queue

CREATE TABLE chunks (
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES files(id),
  ordinal    INTEGER NOT NULL,
  source     TEXT NOT NULL,        -- body|ocr|qr|filename|code_symbol
  text       TEXT NOT NULL,
  page       INTEGER,
  line_start INTEGER,
  line_end   INTEGER
);
CREATE INDEX idx_chunks_file ON chunks(file_id);

CREATE VIRTUAL TABLE chunks_fts USING fts5(
  text, content = 'chunks', content_rowid = 'id',
  tokenize = 'unicode61 remove_diacritics 2'
);
-- + standard external-content triggers (AFTER INSERT/DELETE/UPDATE on chunks) to sync chunks_fts

CREATE VIRTUAL TABLE vec_text  USING vec0(chunk_id INTEGER PRIMARY KEY, embedding float[384]);
CREATE VIRTUAL TABLE vec_image USING vec0(file_id  INTEGER PRIMARY KEY, embedding float[768]);
```

Rules:

- Deletions MUST explicitly delete `vec_*` rows and chunks in the same transaction. Virtual tables are not covered by foreign keys, so do not rely on cascades.
- All vectors are L2-normalized before insert. Use `distance_metric=cosine` if the pinned sqlite-vec version supports it; otherwise use L2, which gives identical ranking on normalized vectors.
- Every file gets one `filename` chunk: the file name split on `_ - . space` and camelCase, plus the parent folder names. This makes content-less files findable too.
- `magi-cli doctor` MUST verify that `sqlite_compileoption_used('ENABLE_FTS5')` = 1 and print `vec_version()`.

### 5.6 Search algorithm

1. **Sanitize** the query for FTS5. Escape quotes, and wrap each term in double quotes so user input can never inject FTS syntax.
2. Run in parallel:
   - (a) FTS5 BM25, top 100 chunks.
   - (b) `vec_text` KNN, top 100, using `"query: " + q` through e5.
   - (c) `vec_image` KNN, top 50, using the SigLIP text tower.
3. **Aggregate to files:** each file's score in each list is its best chunk rank.
4. **Fuse** with Reciprocal Rank Fusion: `score = Σ w_i / (60 + rank_i)`. Default weights: BM25 1.0, text 1.0, image 0.8. Make them configurable internally, not in the UI.
5. **Boosts:** filename-token overlap (×1.2 max) and mild recency (≤ ×1.1 for files modified in the last 30 days).
6. **Snippet:** use the best-matching chunk. For BM25 hits, use FTS5 `snippet()` with highlight markers. For vector-only hits, show the chunk's first ~200 characters. OCR snippets are labeled "Text in image"; QR snippets "QR code".
7. Return the top `max_results` files.

### 5.7 IPC contract (Tauri commands → `magi-core`)

All DTOs live in `magi-core/src/dto.rs`, derive `Serialize`, `Deserialize`, and `ts_rs::TS`, and are exported to `apps/desktop/src/bindings/`.

| Command                                                                               | Request → Response                                                                                                                |
| ------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `search`                                                                              | `SearchRequest { query, limit?, kinds?, root_ids? }` → `SearchResponse { results: SearchResult[], took_ms }`                      |
| `get_status`                                                                          | → `IndexStatus { state: idle\|scanning\|indexing\|paused, queued, indexed, skipped, errors, current_file?, roots: RootStatus[] }` |
| `list_roots` / `add_root(path)` / `remove_root(id)` / `set_root_enabled(id, enabled)` | Root management. `add_root` runs the permission probe and returns its result.                                                     |
| `pause_indexing` / `resume_indexing` / `rescan_all`                                   | Indexing control                                                                                                                  |
| `open_file(file_id)` / `reveal_file(file_id)`                                         | Use the opener plugin, resolving the path from the DB. The frontend never sends raw paths.                                        |
| `get_settings` / `update_settings(patch)`                                             | Config read/write with validation                                                                                                 |
| `get_permissions_report`                                                              | → `PermissionIssue[]` with per-OS guidance and a settings deep link                                                               |
| `list_errors(limit)` / `retry_errors`                                                 | Error management                                                                                                                  |
| `features_status` / `set_feature_enabled(feature, enabled)` / `download_feature(feature)` / `cancel_download` / `remove_download(feature)` | Search features (ADR-0010). → `FeatureStatus { feature: meaning\|image_text\|image_visual, enabled, install: NotInstalled\|Downloading{bytes,total}\|Installed{size_bytes}\|Failed{code}, backfill?: {done,total} }`. `code`: `DownloadNetworkError\|ChecksumMismatch\|DiskFull\|PermissionDenied`. Cancel returns to `NotInstalled`. |
| `clear_index`                                                                         | Deletes the DB and thumbnails, keeps config and models, then restarts indexing                                                    |

`SearchResult { file_id, path, file_name, kind, score, snippet?: { text, highlights: [start,end][] }, page?, thumb_url?, modified_at, match_sources: ("keyword"|"semantic"|"visual"|"ocr"|"qr"|"filename")[] }`

**Events:** `engine://status`, `engine://progress`, `engine://roots`, `engine://permissions`, `engine://features` (always the complete `FeatureStatus[]`, never a delta).

**Locale neutrality:** user-facing text originating in Rust (errors, permission guidance, failure codes) crosses IPC as a stable code plus parameters (`ts-rs` enums) and is localized by the frontend. Localized text is never a machine-readable contract. Changing `ui.language` changes presentation only, never indexed or search data.

**Security:** Tauri capabilities grant each window only the commands it needs. The frontend has **no** direct filesystem permissions. The asset protocol scope is limited to the thumbnail cache directory; full-size user files are never exposed to the webview. A strict CSP is set in `tauri.conf.json`.

---

## 6. Platform behavior: permissions and background detection

The `platform/` module implements these traits for each OS:

- `PermissionProbe::probe(root) -> RootAccess`
- `CloudPlaceholder::is_cloud_only(path, metadata) -> bool`
- `PowerStatus::on_battery() -> Option<bool>`
- `ThreadPriority::lower_current_thread()`

### 6.1 macOS

| Topic                     | Required behavior                                                                                                                                                                                                                                                                                                                                                                                 |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Sandbox                   | Ship **non-sandboxed** (no App Sandbox entitlement). Security-scoped bookmarks are out of scope.                                                                                                                                                                                                                                                                                                  |
| TCC-protected folders     | Desktop, Documents, Downloads, iCloud Drive, removable volumes, and network volumes trigger system consent prompts. `Info.plist` MUST include `NSDesktopFolderUsageDescription`, `NSDocumentsFolderUsageDescription`, `NSDownloadsFolderUsageDescription`, `NSRemovableVolumesUsageDescription`, and `NSNetworkVolumesUsageDescription`, with clear English text (Spanish localization optional). |
| Probe                     | `read_dir(root)` plus a `metadata()` call on one entry. `PermissionDenied` / `EPERM` → status `permission_denied`.                                                                                                                                                                                                                                                                                |
| Guidance UI               | Explain which permission is missing. Offer a button that opens System Settings via `x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders`, or `...?Privacy_AllFiles` for Full Disk Access. Verify these URLs on macOS 13, 14, and 15, and document results in `docs/permissions.md`.                                                                                   |
| Recovery                  | While any root is `permission_denied`, re-probe every 30 s. On success, auto-resume and reconcile that root.                                                                                                                                                                                                                                                                                      |
| Dev caveat                | TCC grants are bound to the code signature. Ad-hoc/unsigned dev builds may re-prompt after each rebuild, and running from a terminal can attribute access to the terminal app. Document this in `docs/permissions.md`.                                                                                                                                                                            |
| iCloud "Optimize Storage" | Files evicted to the cloud are _dataless_. Check `st_flags & SF_DATALESS` (verify the constant against SDK headers) and never read their content.                                                                                                                                                                                                                                                 |
| Bundles                   | Treat package directories (`.app`, `.photoslibrary`, …) as opaque.                                                                                                                                                                                                                                                                                                                                |
| Dock                      | Tray-only operation: set the activation policy to `Accessory` so there is no Dock icon while hidden.                                                                                                                                                                                                                                                                                              |
| Priority                  | Lower indexing threads to QoS utility or background.                                                                                                                                                                                                                                                                                                                                              |

### 6.2 Windows

| Topic                 | Required behavior                                                                                                                                                                                                                                                        |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Permissions           | There are no consent prompts. Access-denied directories (system folders, other users' profiles) → skip with a debug log. A root that itself is denied → `permission_denied`.                                                                                             |
| Cloud placeholders    | Via `MetadataExt::file_attributes()`, treat `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS (0x00400000)`, `FILE_ATTRIBUTE_RECALL_ON_OPEN (0x00040000)`, and `FILE_ATTRIBUTE_OFFLINE (0x00001000)` as cloud-only. Reading metadata MUST NOT hydrate a file; reading content would. |
| Locked files          | `ERROR_SHARING_VIOLATION (32)` / `ERROR_LOCK_VIOLATION (33)` → retry with backoff and never count toward the error threshold on the first 3 tries. Open files with shared read/write/delete access.                                                                      |
| Long paths            | Test paths longer than 260 characters. Ensure the app manifest declares `longPathAware` if needed.                                                                                                                                                                       |
| Watcher overflow      | ReadDirectoryChangesW buffer overflow surfaces as a rescan event → reconcile the root.                                                                                                                                                                                   |
| Network/mapped drives | Do not watch. Use polling with status `watch_failed (polling)`.                                                                                                                                                                                                          |
| Hotkey                | Default `Ctrl+Shift+Space`. Detect registration failure and prompt the user to choose another shortcut.                                                                                                                                                                  |
| Priority              | `THREAD_PRIORITY_BELOW_NORMAL`, or background mode for indexing threads.                                                                                                                                                                                                 |

### 6.3 Linux

| Topic           | Required behavior                                                                                                                                                                                                                            |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Permissions     | Standard Unix permissions. Unreadable subtrees → skip with a log. Unreadable root → `permission_denied`.                                                                                                                                     |
| inotify limits  | One watch per directory. On `ENOSPC` / "too many watches", mark the root `watch_failed`, fall back to polling, and show guidance: `sysctl fs.inotify.max_user_watches` (and how to persist it). Show the current limit in `magi-cli doctor`. |
| Wayland hotkeys | Global shortcuts may not work under Wayland. Detect `XDG_SESSION_TYPE=wayland`. If registration fails, show instructions to bind a desktop-environment shortcut to `magi --toggle`.                                                          |
| `--toggle`      | Implemented via the single-instance plugin: the second process forwards args to the running instance and exits. Works on all OSes.                                                                                                           |
| Tray            | Requires AppIndicator support. GNOME may need the AppIndicator extension. The app MUST remain fully usable without a tray (launcher opens settings; hotkey or `--toggle` opens search).                                                      |
| Autostart       | `.desktop` file in `~/.config/autostart` (autostart plugin).                                                                                                                                                                                 |
| Priority        | `nice(10)`; optionally `ioprio` idle class.                                                                                                                                                                                                  |

### 6.4 Cross-platform background policy

- Indexing threads run at low OS priority. ONNX `intra_op_num_threads` for the indexing models is capped at 4 on AC power and 2 on battery (or when power status is unknown), chosen when a model loads (docs/perf-investigation.md), and uses all cores for foreground searches.
- **Pause on battery** (configurable). If battery status can't be determined on a platform, the feature is disabled there and documented.
- **Model lifecycle:**
  - Load when the queue is non-empty or the search window opens. Pre-warm the query encoders on hotkey press.
  - Unload after `idle_unload_minutes` with no work.
- The user can pause/resume at any time. The paused state persists across restarts.
- Launch at login is **off by default**; onboarding asks.
- **Low-memory mode** (auto when total RAM ≤ 8 GB): 2 extract workers, idle unload after 2 minutes, image model never loaded at the same time as a running OCR batch if doing so would exceed the NFR-11 budget.

---

## 7. Milestone roadmap

Each milestone lists **Objective**, **Deliverables**, and **Verification**. A milestone is done only when every verification item passes on **all three OSes** (CI plus manual where stated) and evidence is recorded in `docs/progress.md`.

### M0 — Repository bootstrap and CI

**Objective:** A compiling skeleton on Windows, macOS, and Linux, with automated quality gates.

**Deliverables**

- The full directory structure from §5.1, with empty modules and `todo!()`-free stubs.
- Workspace `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `.editorconfig`, `.gitignore`, `.gitattributes`, `justfile`.
- Tauri app shows a "Hello" window rendered by React and calls one Tauri command (`ping → "pong"` from `magi-core`).
- `ci.yml` running on a 3-OS matrix: `cargo fmt --check`, clippy, `cargo test --workspace`, `pnpm lint`, `pnpm typecheck`, `pnpm test`, and `pnpm tauri build --debug --no-bundle` (compile check).
- `xtask fetch-pdfium` (pinned version and checksum).
- `docs/architecture.md` stub, `docs/progress.md`, ADR-0001 (stack).

**Verification**

- [ X ] `just check` passes locally on the developer machine.
- [ X ] CI is green on all three OSes.
- [ X ] `just dev` opens the window and displays "pong" (manual; screenshot per OS in `docs/progress.md`).

### M1 — Config, paths, and database foundation

**Objective:** Persistent, validated configuration and a migrated SQLite database with FTS5 and sqlite-vec.

**Deliverables**

- `paths.rs`: data/config/cache dirs via `directories`, honoring the `MAGI_*` overrides. Path canonicalization: absolute, symlinks resolved per policy, consistent case handling on Windows, no trailing separators.
- `config/`: load, save (atomic write via temp file + rename), validation with helpful errors, per-OS default exclusions.
- `db/`: connection factory (WAL, `foreign_keys`, `busy_timeout`), sqlite-vec registration, migration runner with embedded SQL, schema `0001_init.sql`.
- Root management in core, with nested-root rejection.
- CLI: `magi-cli doctor`, `magi-cli roots add|list|remove`.

**Verification**

- [ ] Unit tests cover:
  - config round-trip
  - invalid config (bad glob, nonexistent root) → typed errors
  - migrations idempotent (run twice)
  - nested root rejected
  - canonicalization cases (Windows drive-letter case, `..` segments, trailing slash)
- [ ] `magi-cli doctor` prints the directories, SQLite version, `ENABLE_FTS5 = 1`, `vec_version()`, and (on Linux) the inotify watch limit.
- [ ] Killing the process during a config save never leaves a corrupt config (test with a simulated failure between write and rename).

### M2 — Discovery and text extraction (keyword search only)

**Objective:** Walk roots, classify files, extract text from all non-image types, and search via FTS5, with no ML yet.

**Deliverables**

- `discovery/`: walker with exclusions, hidden-file policy, size cap, symlink policy, opaque bundles; classifier (extension + magic bytes).
- `extract/` with an `Extractor` trait implemented for:
  - plain text: encoding detection + NFC normalization
  - Markdown
  - code: tree-sitter symbol chunks
  - PDF: pdfium, per page
  - DOCX/PPTX: zip + XML
  - XLSX: calamine, sheet/row text
  - filename chunk (all files)
- Provisional character-based chunker (~1,500 chars, 200 overlap). Replaced by the token-aware chunker in M3.
- Language detection per file.
- Extraction isolation: per-file timeout (60 s) and panic containment (`catch_unwind` at the worker boundary). One bad file never kills the engine.
- CLI: `magi-cli index <root>` (one-shot, no watcher) and `magi-cli search --mode fts "<query>"`.
- Fixture corpus with English and Spanish documents, plus edge cases: empty, 0-byte, truncated PDF, password-protected PDF, Windows-1252 text, huge file over the cap, deeply nested dirs.

**Verification**

- [ ] Golden tests: each extractor's output matches `fixtures/golden/*`.
- [ ] Spanish accents survive end-to-end. The FTS query `cancion` matches a document containing `canción`, and `pagina` matches `página`.
- [ ] Every edge-case fixture yields `skipped` or `error` with a reason. None crash the process.
- [ ] Indexing `fixtures/corpus` twice produces identical DB row counts (idempotence).
- [ ] Throughput is recorded in `docs/benchmarks.md` (files/s per type).

### M3 — Text embeddings and hybrid search

**Objective:** Semantic, cross-lingual search over text content, with measurable quality.

**Deliverables**

- `embed/manager.rs`:
  - reads the manifest; downloads with a progress callback, `.partial` temp files, SHA-256 verification, and atomic rename
  - supports cancel and offline import from a local folder
  - loads models lazily and unloads them after the idle timeout
- `TextEmbedder` trait with an e5 implementation (`ort` + `tokenizers`, correct prefixes, mean pooling, L2 norm, batching), plus a deterministic `FakeEmbedder` for tests.
- Token-aware chunker: ≤ 512 tokens including the prefix, target 256–400 tokens, with overlap.
- `vec_text` writes, the hybrid search pipeline (§5.6), and snippets with highlights.
- Model-ID tracking in `meta`; re-embedding is triggered when the model changes.
- Quantization decision for e5 (fp32 vs. int8) based on eval results and memory, recorded in an ADR.
- `tools/reference_embeddings.py` and the fixture reference vectors.
- `magi-cli eval <queries.jsonl>` → recall@5, recall@10, MRR, broken down by `lang` (`en`, `es`, `cross`).
- `eval/queries.jsonl` with ≥ 60 queries: 20 English, 20 Spanish, 20 cross-lingual.

**Verification**

- [ ] Parity: Rust e5 embeddings vs. reference vectors, cosine ≥ 0.99 for every fixture sentence (≥ 0.97 if the quantized model is chosen; the reference is always the fp32 model).
- [ ] If int8 is chosen, its overall recall@5 is within 2 points of fp32 (report both).
- [ ] Cross-lingual smoke test: the query `electrician invoice` returns the Spanish fixture `factura_electricista.pdf` in the top 3, and `receta de arepas` returns the English recipe fixture in the top 3.
- [ ] Eval baseline recorded in `docs/eval.md` (report fts-only, vector-only
      and hybrid, overall and per query bucket). All three MUST be measured
      through the same ranking path — `search::rank_and_boost` with one input
      list emptied for the baselines — so the comparison isolates fusion
      rather than comparing two different ranking functions. Hybrid MUST
      satisfy both:
  - **(a) No regression:** overall recall@5 >= max(fts-only, vector-only).
  - **(b) Each mode contributes:** on the `kw` (keyword-decisive) bucket
    hybrid > vector-only on MRR, and on the `cross` bucket hybrid >
    fts-only. A fusion that beats neither input on the queries that input
    exists to answer is not doing anything. MRR is the clause-(b) metric
    because recall@5 saturates at 1.000 on this corpus.
- [ ] Latency benchmark on 100k synthetic chunks meets NFR-2 (warm) and NFR-3 (cold) on the reference machine (or a VM limited to 8 GB RAM and 4 vCPUs). Record the specs.
- [ ] Peak RSS while embedding the fixture corpus is recorded; the text-only pipeline stays ≤ 700 MB.
- [ ] Download is interrupted mid-file and resumed or restarted cleanly; a corrupted file (bad hash) is rejected and re-downloaded.
- [ ] `just test` still requires no network (fake embedder).

### M4 — Images: OCR, QR codes, visual embeddings, thumbnails

**Objective:** Screenshots and photos become findable by their text, their QR contents, and their visual meaning.

**Deliverables**

- Image decoding with limits: read dimensions from headers first; skip images over `max_image_megapixels` (default 64) before full decode. EXIF orientation applied for all formats.
- **HEIC spike (first task of M4) — done, ADR-0003.** `heic-rs` (pure Rust) chosen over `libheif-rs` and over native per-OS decoders: identical output on every fixture, 1.3–2.3× faster, and nothing to install, bundle or license. Both items that were owed before M4 sign-off are closed (2026-09-21): CI is green on all three runners, and the 48 MP budget is measured on a synthetic tile-gridded file (see the M4 sign-off in docs/progress.md).
- HEIC extractor: decode the primary image (ignore auxiliary/depth images; ignore the `.MOV` of Live Photos). `heic-rs` exposes no embedded-thumbnail API, so the UI thumbnail is downscaled from the same decode OCR and embedding already require (ADR-0003).
- **OCR spike:** benchmark `ocrs` vs. PaddleOCR-ONNX on the fixture images (English and Spanish screenshots, receipts, UI captures). Measure character error rate (CER) and time per image. Choose one and record it in an ADR. Implement it behind `OcrEngine` and produce `ocr` chunks.
- `rxing` QR/barcode decoding → `qr` chunks containing the payload plus the labels "QR code / código QR".
- SigLIP 2: choose the variant (resolution and quantization) in an ADR based on size, speed, and eval results. Implement the `ImageEmbedder` (image tower) and the query text tower with exact reference preprocessing (resize mode, normalization, 64-token padding). Write `vec_image`.
- Visual list integrated into hybrid search.
- Thumbnail cache (256 px, keyed by content hash) for images and PDF first pages, served through the scoped asset protocol.

**Verification**

- [ ] SigLIP parity vs. reference vectors (cosine ≥ 0.99 fp32; ≥ 0.97 for the quantized variant) for both towers; the quantized variant's image-query recall@5 is within 3 points of fp32. **Amended 2026-09-20 (ADR-0007):** the shipped q4f16 variant is accepted at a _mean_ image cosine of 0.969 (minimum 0.952) with retrieval identical to fp32; the text tower meets 0.97. Choosing fp16 for the vision tower would meet the gate exactly at +290 MB of RSS while indexing, and was declined.
- [ ] **HEIC:** self-shot phone-camera fixtures (12 MP and 48 MP, portrait and landscape, one with text for OCR, one with a QR code) decode correctly on Windows, macOS, and Linux CI. **Any phone that shoots HEIC will do** (answered 2026-09-20): what the decoder has to cope with is the container — a tile grid, an aux HDR gain map, a rotation transform — not the vendor. Orientation is correct in thumbnails (golden thumbnail comparison). OCR and QR work on the HEIC fixtures exactly as on their JPEG equivalents.
- [ ] Decoding a 48 MP HEIC keeps the RSS delta < 400 MB and completes in < 3 s on the reference machine. **Amended 2026-09-20:** no real 48 MP HEIC exists (phones cap HEIC at 12 MP), so this is measured on a synthetic tile-gridded one from `tools/synthetic_heic_48mp.py`.
- [ ] Peak RSS while indexing the full fixture corpus with all models loaded ≤ 1.5 GB (NFR-11).
- [ ] OCR: CER ≤ 10% on the Spanish and English screenshot fixtures (record the actual values). Accented characters (á, é, í, ó, ú, ñ, ¿, ¡) appear in the output. **Exception, accepted 2026-09-21 (ADR-0006): `¡` is not required.** The Latin PP-OCRv5 recognizer's dictionary has `¿` but no `¡`, so the model cannot emit it (`¡Hola!` reads `Hola!` or `iHola!`); every other listed character is recognized. Getting it would take a different or retrained recognizer, or guessing in post-processing, which is not worth it for one punctuation mark. FTS5 treats `¡` as punctuation, so keyword search is unaffected.
- [ ] The QR fixture decodes to its exact payload. Queries `qr code` and `código QR` both return the QR screenshot in the top 3.
- [ ] Visual queries: `dog on the beach` / `perro en la playa` return the matching photo fixture in the top 3.
- [ ] A decompression-bomb fixture is rejected quickly without memory blow-up (RSS delta < 200 MB).
- [ ] Eval extended with ≥ 20 image queries; results recorded in `docs/eval.md`.

### M5 — Incremental indexing and change detection

**Objective:** After the initial index, only changed files are processed, and the index always converges to the true state of the roots.

**Deliverables**

- Scheduler with a persistent state machine (§5.4): dedupe, priorities (mtime desc), delays, retries with backoff.
- Worker pool at low priority, the embed worker with the priority lock, and the single DB writer.
- Per-root watchers (`notify-debouncer-full`), buffered during the startup reconciliation.
- Reconciliation scans: at startup, periodic, after wall-clock jumps, and on rescan/overflow.
- Rename handling in place, and move detection by hash for cross-root moves and delete+create pairs.
- Hash-skip for touched-but-unchanged files, stability checks for in-progress writes, temp-file ignore rules.
- Re-embed trigger on model change: a stored `meta.text_model_id` /
  `meta.image_model_id` that differs from the running embedder's marks the
  affected files `pending` so hash-skip does not preserve vectors from the
  old model. Likewise a stored `meta.ocr_engine_id` that differs from the
  running `OcrEngine::engine_id()` marks image files `pending`, so their
  `ocr` chunks are re-read by the new engine (M5 starts writing this key). M3 delivers the `meta` tracking and logs a warning on a
  mismatch; the trigger belongs here because M5's hash-skip is what makes a
  stale vector survive a re-index (before it, every index run re-embeds
  everything, so the mismatch is latent). See SPEC.md §7 M3's deliverable.
- Cloud-placeholder and dataless skip (Windows, macOS).
- Polling fallback for failed watchers and network drives.
- Missing-root handling that keeps the index.
- Pause/resume (persisted).
- Engine status/progress events.
- `magi-cli daemon` for headless manual testing, with live progress output.
- Test instrumentation: an embed-call counter on `FakeEmbedder` to prove no re-embedding.

**Verification** — `crates/magi-core/tests/incremental.rs` with a real watcher, temp dirs, and `FakeEmbedder`. These run in CI on all three OSes. Use polling with a deadline, never fixed sleeps.

- [ ] 1. New file → indexed and searchable within 10 s.
- [ ] 2. Content modified → re-embedded; old chunks, FTS rows, and vectors are gone (assert row counts).
- [ ] 3. `touch` without a content change → embed-call counter unchanged.
- [ ] 3b. `meta.text_model_id` changed since the last index → affected files
      are re-embedded (embed-call counter rises) even though their content
      hash is unchanged; with the id unchanged, hash-skip still applies.
- [ ] 4. Rename within a root → path updated, embed counter unchanged.
- [ ] 5. Move between two roots → path and `root_id` updated, embed counter unchanged.
- [ ] 6. Move out of all roots → removed from all tables.
- [ ] 7. Delete → removed from `files`, `chunks`, `chunks_fts`, `vec_text`, `vec_image`.
- [ ] 8. Changes made while the engine is stopped (create, modify, delete, rename), then restart → the DB matches the filesystem exactly.
- [ ] 9. Burst of 1,000 files created → all indexed exactly once, no duplicates.
- [ ] 10. File written slowly over 5 s → indexed once, with the final content.
- [ ] 11. Engine killed while files are `indexing` → on restart they are reset and completed. `PRAGMA integrity_check` = ok.
- [ ] 12. Root removed → all of its rows purged.
- [ ] 13. Exclusion glob added → newly excluded files purged. Glob removed → files indexed.
- [ ] 14. Unreadable file (chmod 000 / ACL deny) → `error` state; other files continue.
- [ ] 15. Root directory renamed or unmounted → status `missing`, index retained. Restored → resumes without re-embedding.
- [ ] 16. Windows only: a locked file (held open exclusively by the test) → retried, indexed after release.
- [ ] 17. Simulated memory pressure (test hook forcing "available < 1 GB") → indexing pauses and the image model unloads; clearing the hook resumes indexing.
- [ ] Manual: run `magi-cli daemon` on a real 20k+ file folder for 1 hour of normal use. Idle CPU < 1% after the initial index (record in `docs/benchmarks.md`).

### M6 — Desktop app: search window, tray, settings, onboarding

**Objective:** A polished, keyboard-first desktop experience on top of the engine.

**Deliverables**

- The engine hosted in Tauri managed state and started on app launch. Commands and events per §5.7, with generated ts-rs bindings (`just bindings` clean in CI).
- **Search window:**
  - frameless, centered, always-on-top; hides on blur or `Esc`
  - input with 150 ms debounce; results with thumbnail/icon, name, path, highlighted snippet, page, date, and match-source chips
  - keyboard navigation: `↑/↓`, `Enter` open, `Ctrl/Cmd+Enter` reveal, `Ctrl/Cmd+C` copy path
  - empty, loading, and error states, plus an "indexing in progress (N queued)" hint
- **Tray menu:** status line, Open search, Pause/Resume, Settings, Quit.
- **Global hotkey** (configurable, with conflict detection), single-instance, and `--toggle`.
- **Optional search features (core, first deliverable; ADR-0010):** `meaning` / `image_text` / `image_visual` each optional; pipeline and search skip a missing feature; `.tmp` + SHA-256 downloads with stable failure codes; enabling backfills only files missing that feature's output; disabling keeps derived data; "Remove download" deletes only the asset; `engine://features` carries the complete state; `magi-cli features list|enable|disable [--delete-download]`, `doctor` reports features. Tests first.
- **Settings window:** roots list with status badges and fix actions, add/remove/enable, exclusions editor, file types, max size, hotkey recorder, battery pause, launch at login, search features (status, size, backfill progress, remove download), language, window background, index stats, error list with retry, clear index (with confirmation).
- **Onboarding flow** per FR-10, including the feature-download consent screen that states exact sizes and that no other network access occurs, and "Keep Magi available in the background ☑ Start with your computer".
- **Search hint:** when fewer than 3 results return and a feature that could help is off, a dismissible hint offers to turn it on (or reports its backfill progress).
- **Localization:** complete English and Spanish UI. Typed dictionaries (`es` must satisfy the `en` type), `Intl` for plurals/dates/numbers, `ui.language = system|en|es`, live switch without restart; the Rust tray uses its own string table.
- **Theme and window background:** light/dark following the OS. Search window uses native transparency via `window-vibrancy` (Mica on Windows 11, Acrylic on Windows 10, vibrancy on macOS, solid on Linux or failure), resolved from `ui.transparency_mode` and the OS reduce-transparency preference; `ui.transparency_intensity` slider ("More solid" ↔ "More transparent") is disabled when the effective state is solid. Other windows stay solid.
- **Visual direction** chosen via a throwaway prototype (2–3 directions, fake data) before the UI is built.
- Least-privilege capabilities per window, CSP, asset scope limited to the thumbnail cache.

**Verification**

- [ ] Vitest tests: result rendering (snippet highlights), keyboard navigation reducer, settings form validation, transparency resolution, language resolution, and a type check that the `es` dictionary covers every `en` key.
- [ ] Core tests: search and indexing work with no features installed (keyword + filename); enabling a feature backfills only files missing its output; disabling keeps derived data; a failed or cancelled download never leaves an `Installed` asset.
- [ ] `docs/qa-checklist.md` completed on all three OSes, including:
  - hotkey toggle
  - open and reveal
  - multi-monitor placement
  - HiDPI rendering
  - window hides on blur
  - app works with the tray unavailable (Linux GNOME without the extension)
- [ ] With warm models, the search window is visible < 150 ms after the hotkey and the first results appear < 400 ms after typing stops.
- [ ] The frontend cannot call any filesystem API directly (verify the capability files; attempt a forbidden call in a test build and confirm it's denied).

### M7 — Permissions and background-behavior hardening

**Objective:** The app behaves correctly and explains itself under real OS constraints, and stays polite in the background.

**Deliverables**

- Full implementation of §6 for each OS: probes, guidance UI with deep links, auto-recovery polling.
- macOS `Info.plist` strings and Accessory activation policy.
- Windows placeholder handling and lock handling.
- Linux inotify-limit guidance and Wayland hotkey fallback guidance.
- `get_permissions_report` shown in onboarding and settings.
- Low-priority threads, ONNX thread caps, pause on battery, model idle unload, launch-at-login toggle.
- `docs/permissions.md`: per-OS behavior, screenshots of each prompt and guidance screen, known caveats (TCC and dev builds).

**Verification** — manual per OS on a fresh user account or VM. Record results and screenshots.

- [ ] **macOS:**
  - add `~/Documents` → system prompt appears with our usage text
  - **deny** → root shows `permission_denied` with a working "Open System Settings" button
  - grant in Settings → the root recovers automatically within 60 s and indexes
- [ ] **macOS:** with iCloud "Optimize Mac Storage", evicted files are not downloaded by indexing (confirm they remain cloud-only afterwards).
- [ ] **Windows:** OneDrive folder with "online-only" files → after a full index those files are still online-only (cloud icon unchanged), and they're findable by name.
- [ ] **Windows:** path > 260 chars indexed and openable.
- [ ] **Linux:**
  - with `max_user_watches` set artificially low, the root shows `watch_failed (polling)` with guidance, and changes are still picked up by polling
  - under a Wayland session, a failed hotkey registration shows the `--toggle` instructions, and `magi --toggle` bound to a DE shortcut works
- [ ] **All:** USB-drive root unplugged → `missing`, results hidden, index kept. Replugged → resumes with no re-embedding.
- [ ] **All:** on battery with pause enabled → indexing pauses within 30 s; on AC it resumes.
- [ ] **All:** 10 minutes idle after the index completes → models unloaded, RSS ≤ 150 MB (NFR-1), CPU < 1%. Record the numbers.
- [ ] **8 GB machine or VM:** initial index of a 20k-file mixed folder (including 2k+ phone photos, HEIC and JPEG) while a browser with ~10 tabs is open. Peak RSS ≤ 1.5 GB, no system swapping storms, and searches stay responsive.
- [ ] **All:** during a large initial index, typing a search stays responsive (NFR-8). Record p95 latency while indexing.

### M8 — Packaging and release

**Objective:** Installable builds for all three OSes produced by CI, with clear install instructions.

**Deliverables**

- Bundles:
  - Windows: NSIS installer (x64)
  - macOS: `.dmg` (aarch64 required; universal if x86_64 is supported)
  - Linux: AppImage + `.deb` (optionally `.rpm`)
- PDFium and ONNX Runtime correctly bundled and located at runtime. Use `@rpath` on macOS, next to the executable on Windows, and inside the AppImage on Linux. (HEIC needs no bundling — ADR-0003.)
- `THIRD_PARTY_LICENSES.md` generated with `cargo-about`, plus manual entries for bundled native libraries. An ADR decides static linking vs. dynamic loading for `ort`, verified on clean machines.
- `release.yml`: on tag `v*`, build all bundles with `tauri-action`, attach SHA-256 checksums, create a draft GitHub Release.
- README "Install" section for **unsigned builds**:
  - Windows SmartScreen: "More info → Run anyway"
  - macOS: "System Settings → Privacy & Security → Open Anyway"
  - Linux: `chmod +x` for the AppImage
- README "Privacy" section and "Uninstall / remove data" section (exact data/config/cache paths per OS).
- `CHANGELOG.md`, semantic versioning. The auto-updater stays **disabled** in v1.

**Verification**

- [ ] On a clean VM/machine per OS with no dev tools: install → onboarding → download models → index `fixtures/corpus` copied to the home folder → search works → uninstall.
- [ ] Installer sizes recorded; NFR-6 met.
- [ ] HEIC fixtures index and display correctly on the clean machines.
- [ ] Network check: with a firewall/monitor (e.g. Little Snitch, Windows Firewall logging, `ss`/`nethogs`), the only outbound connections are the model-download hosts, and only during download.

### M9 — Stretch goals (only after M8)

- Cross-encoder reranker on the top 30 results (multilingual, small).
- Per-language stemmed FTS column (`rust-stemmers`: English and Spanish).
- Query filters (`type:pdf`, `in:Screenshots`, `after:2025-01`) and natural hints ("captura", "screenshot" → images).
- Preview pane (PDF page render, image preview) in the search window.
- Audio transcripts via `whisper-rs`.
- Code signing and notarization, auto-updates.
- Spanish UI localization.

---

## 8. Testing strategy

| Layer                             | Tooling                                                             | Runs in                                           |
| --------------------------------- | ------------------------------------------------------------------- | ------------------------------------------------- |
| Unit (Rust)                       | `cargo test`; `FakeEmbedder`; temp dirs via `tempfile`              | CI, all OSes                                      |
| Extraction golden tests           | fixtures + `fixtures/golden`                                        | CI, all OSes                                      |
| Incremental / watcher integration | `tests/incremental.rs` (M5 scenarios)                               | CI, all OSes                                      |
| Model parity                      | reference vectors; requires model files                             | `eval.yml` (manual/nightly, cached models)        |
| Retrieval quality                 | `magi-cli eval` → `docs/eval.md`                                    | `eval.yml` + before merging ranking/model changes |
| Performance                       | `xtask bench-corpus` (synthetic 100k chunks) → `docs/benchmarks.md` | manual, per milestone                             |
| Frontend                          | Vitest + Testing Library                                            | CI                                                |
| End-to-end / OS behavior          | `docs/qa-checklist.md`, `docs/permissions.md`                       | manual, per OS, per milestone M6+                 |

Rules:

- Tests MUST NOT require network access, except in `eval.yml`.
- Watcher tests MUST use deadline-based polling helpers, never fixed sleeps. Flaky tests are fixed or quarantined with an issue link, never silently retried.
- Fixtures MUST be self-created or clearly licensed (record the source in `fixtures/README.md`). No personal files.

---

## 9. Open questions (ask the human before the affected milestone)

| #      | Question                                                                                                                                                       | Blocks | Default if unanswered                                                                      |
| ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------ |
| Q1     | Final product name (replaces `magi`)                                                                                                                           | M8     | Keep `magi`                                                                                |
| ~~Q2~~ | Frontend framework                                                                                                                                             | —      | **Answered 2026-09-10:** React + TypeScript                                                |
| ~~Q3~~ | HEIC/HEIF (phone photos) in v1?                                                                                                                                | —      | **Answered 2026-09-10:** Yes, required (M4)                                                |
| ~~Q4~~ | Minimum target hardware                                                                                                                                        | —      | **Answered 2026-09-10:** 8 GB RAM laptop (§1 reference machine)                            |
| Q5     | Is macOS Intel (x86_64) support required?                                                                                                                      | M8     | Best-effort                                                                                |
| Q6     | License for the repository (MIT assumed)                                                                                                                       | M0     | MIT                                                                                        |
| ~~Q7~~ | Should launch-at-login default to on after onboarding? | — | **Answered 2026-09-25:** Offered in onboarding, checked by default (ADR-0010) |
| ~~Q8~~ | If ADR-0003 must fall back to native HEIC decoders and Windows lacks the HEVC extension, is "HEIC not supported on this PC, install the extension" acceptable? | —      | **Moot 2026-09-19:** ADR-0003 chose `heic-rs`, which decodes in-process on every platform. |

Record answers here (with date) and update affected sections.
