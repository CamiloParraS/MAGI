# Agents: read this first

Read [`SPEC.md`](docs/SPEC.md) §0 ("How to use this document") before writing any
code. It is the source of truth for this project; if code and spec disagree,
the spec wins until the spec is updated.

## Common commands

| Command         | Does |
| ---------------- | ---- |
| `just setup`     | Fetch PDFium and ONNX Runtime binaries |
| `just dev`       | Run the GPUI desktop app (from M6 Plan 3) |
| `just check`     | fmt, clippy, `cargo test` — run before committing |
| `just test`      | `cargo test --workspace` (no network, fake embedder) |
| `just models`    | Download ML models into the dev data directory |
| `just eval`      | Run search-quality evaluation against `eval/queries.jsonl` |
| `just build`     | Production build of the desktop app |

## Rules that matter most

- Work one milestone at a time, in order (SPEC.md §7). Don't start milestone
  N+1 until every verification item of milestone N passes.
- Never write to user-selected folders. Never add network access beyond
  model downloads from `models/manifest.toml`. Never invent URLs, checksums,
  or model file names — verify against official sources or ask.
- Record milestone verification evidence in `docs/progress.md`. Record
  significant technical decisions as ADRs in `docs/adr/`.
