# ADR-0004: tree-sitter for code symbol chunking

## Context

SPEC.md §3 specifies `tree-sitter` + grammars (Rust, Python, JS/TS, Java,
C/C++, Go, C#) for chunking source code by top-level symbol, with a
fallback to line windows for languages without a grammar (M2, §7). Each
tree-sitter grammar crate (`tree-sitter-rust`, `-python`, `-javascript`,
`-typescript`, `-java`, `-c`, `-cpp`, `-go`, `-c-sharp`) bundles a
generated C parser compiled at build time via the `cc` crate — these are
native C dependencies, so CLAUDE.md's "new C/C++ native dependencies
require an ADR" rule applies.

## Decision

Use `tree-sitter` 0.27 plus the nine grammar crates named in SPEC.md §3.
`extract::code::CodeExtractor` maps a file's extension to a grammar,
parses it, and emits one chunk per top-level named node in the syntax
tree (function, struct, class, impl block, etc. — whatever the grammar
groups at file scope). Extensions with no mapped grammar (e.g. `.rb`,
`.php`, `.sh`) fall back to fixed-size line-window chunking
(`extract::code::line_window_chunks`), matching the spec's fallback.

## Consequences

- Every supported platform's build now needs a C compiler toolchain (MSVC
  Build Tools, Xcode CLT, or `build-essential`/gcc) to compile the nine
  grammars — already a prerequisite per SPEC.md §4.3 for other reasons
  (WebView2/Tauri native modules), so this adds no new install step for
  Windows/macOS/Linux dev machines, but it does mean **9 more native C
  parsers are compiled into the binary**, not just PDFium.
- ADR-0001's Consequences section claimed "no native C/C++ dependencies
  beyond PDFium ... until M4" — that statement is now superseded by this
  ADR and has been corrected there to point here.
- No vendoring or dynamic linking is needed: `cc` compiles each grammar's
  bundled `.c`/`.cc` sources directly into `magi-core`, statically, so
  there's nothing to fetch or bundle at runtime (unlike PDFium/libheif).
- Licensing: all nine grammar crates are MIT-licensed, consistent with the
  project's dependency preferences; no entry needed in
  `THIRD_PARTY_LICENSES.md` beyond the standard `cargo-about` sweep.
