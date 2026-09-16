# Agents: read this first

Read [`SPEC.md`](SPEC.md) §0 ("How to use this document") before writing any
code. It is the source of truth for this project; if code and spec disagree,
the spec wins until the spec is updated.

## Common commands

| Command         | Does |
| ---------------- | ---- |
| `just setup`     | Install frontend deps, fetch PDFium binaries |
| `just dev`       | Run the Tauri app in dev mode |
| `just check`     | fmt, clippy, `cargo test`, frontend lint/typecheck/test — run before committing |
| `just test`      | Rust + frontend tests only (no network, fake embedder) |
| `just bindings`  | Regenerate ts-rs bindings, fails on unexpected diff |
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
