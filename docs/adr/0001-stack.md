# ADR-0001: Technology stack

## Context

magi needs a cross-platform (Windows/macOS/Linux) desktop app with an
in-process Rust engine, a small idle footprint, and no server/Docker
dependency, per SPEC.md §1 and §2.2 (NFR-1, NFR-6).

## Decision

As specified in SPEC.md §3: Tauri 2.x shell, React + TypeScript + Vite +
Tailwind frontend, Rust (stable) core with `std::thread` + `crossbeam-channel`
concurrency (no async in `magi-core`), SQLite via `rusqlite` (FTS5 +
sqlite-vec) for storage, and `ort` (ONNX Runtime) for on-device ML inference.

PDFium is vendored from prebuilt binaries at
https://github.com/bblanchon/pdfium-binaries, pinned to release
`chromium/8044`, fetched and SHA-256-verified by `cargo xtask fetch-pdfium`
(see `xtask/src/main.rs`).

## Consequences

- No native C/C++ dependencies beyond PDFium (vendored, not system-linked)
  until HEIC support in M4 (its own spike/ADR, ADR-0003) — **superseded**:
  the M2 code extractor added nine `tree-sitter` grammar crates, each a
  native C dependency compiled via `cc`; see ADR-0004.
- All ML inference runs on-device via ONNX Runtime; no GPU execution
  providers in v1.
- Full dependency rationale per layer is in SPEC.md §3; this ADR exists to
  satisfy the M0 deliverable and as an anchor for future stack changes.
