# Progress

Milestone checklist and verification evidence, per SPEC.md §7. A milestone
is recorded here only once every verification item passes on all three OSes.

## M0 — Repository bootstrap and CI

- [x] Directory structure from SPEC.md §5.1 scaffolded, with stub modules
      (no `todo!()`).
- [x] Workspace `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`,
      `.editorconfig`, `.gitignore`, `.gitattributes`, `justfile`.
- [x] Tauri app scaffolded (React + TypeScript + Vite + Tailwind); `ping`
      Tauri command calls `magi_core::ping()` and the window displays "pong".
- [x] `ci.yml`: 3-OS matrix running fmt/clippy/tests/frontend checks and a
      no-bundle Tauri build.
- [x] `xtask fetch-pdfium`: pinned to `bblanchon/pdfium-binaries@chromium/8044`,
      with SHA-256 checksums verified against the downloaded assets for
      win-x64, mac-arm64, mac-x64, and linux-x64.
- [x] `just check` passes locally on the developer machine (Windows 11,
      x86_64): `cargo fmt --check`, `cargo clippy --workspace --all-targets
    --all-features -- -D warnings`, `cargo test --workspace`, `pnpm lint`,
      `pnpm typecheck`, `pnpm test` all green.
- [x] CI green on all three OSes — (workflow added at
      `.github/workflows/ci.yml`).
- [x] `just dev` / `pnpm tauri build --debug --no-bundle` opens the window
      and displays "pong" on Windows — see
      `docs/screenshots/m0-windows-pong.png`. macOS and Linux screenshots
      pending (no access to those machines locally; will come from CI/manual
      testing).

## M1 — Config, paths, and database foundation

- [x] Unit tests cover: config round-trip, invalid config (bad glob,
      nonexistent root) → typed errors, migrations idempotent (run twice),
      nested root rejected (both directions), canonicalization cases
      (Windows drive-letter case, `..` segments, trailing slash). 17 tests
      in `cargo test -p magi-core`, all green on Windows 11 x86_64.
- [x] `magi-cli doctor` prints the data/config/cache dirs, SQLite version
      (3.53.2, bundled), `ENABLE_FTS5 = 1`, and `vec_version` (v0.1.9).
      Verified by manual run. Linux inotify watch limit line is implemented
      (`platform::inotify_watch_limit`, reads
      `/proc/sys/fs/inotify/max_user_watches`) but unverified locally
      (Windows dev machine).
- [x] Killing the process during a config save never leaves a corrupt
      config: `config::tests::crash_between_write_and_rename_leaves_previous_config_intact`
      simulates a crash between the temp-file write and the rename and
      asserts the previous config is still loaded intact.
- [x] `magi-cli roots add|list|remove` verified manually: adding a root
      canonicalizes the path, adding its own subdirectory as a second root
      is rejected as nested, and remove/list work.

Not yet verified on macOS/Linux CI (no access to those machines locally).

## M2 — Discovery and text extraction

Three slices landed: walker/classifier/text/filename/lang/FTS search
(slice 1), PDF/Office/Code extraction (slice 2), and golden tests plus the
remaining edge-case fixtures (slice 3, below). A fourth pass closed the
gaps a review found in slice 3's evidence (fixture corpus, accent
coverage, idempotence, throughput) — see "Slice 3 follow-up" below.

### Slice 3 follow-up: closing the evidence gaps

A review of Slice 3's claims found five gaps between what was checked off
and what SPEC.md §7 M2's verification list actually requires. All five are
now closed with real, fixture-driven evidence (no synthetic/manual-only
substitutes):

- [x] **Fixture corpus populated.** `fixtures/corpus/en/onboarding_notes.txt`
      and `fixtures/corpus/es/notas_incorporacion.txt` added (short,
      hand-written, license-clean, matching the SPEC.md §5.1 corpus
      layout). The Spanish fixture contains both `canción` and `página` so
      it doubles as accent-test content. Golden tests added for both
      (`text_en_onboarding_notes_matches_golden`,
      `text_es_notas_incorporacion_matches_golden` in
      `crates/magi-core/tests/golden.rs`), closing the "plain-text golden
      coverage is skipped" gap noted in the original Slice 3 entry above.
      The stray untracked `fixtures/corpus/.venv/` (leftover from Slice
      2's PDF/Office fixture generation, never committed) was deleted;
      it's not needed after generation and was never part of the corpus.
- [x] **`pagina` → `página` accent evidence.** Only `cancion` → `canción`
      had a test before. `search::fts::tests::accented_query_matches_unaccented_and_vice_versa`
      now indexes two documents and asserts both SPEC.md §7 M2 examples:
      unaccented query `cancion` matches accented `canción`, and accented
      query `página` matches unaccented `pagina` — genuinely exercising
      both directions the test's name always claimed. A second, fully
      end-to-end confirmation (decode → index → search, not just the FTS
      layer) lives in the new Windows-1252 test below.
- [x] **Windows-1252 fixture-driven, not just unit-level.** Added a real
      byte-for-byte cp1252-encoded fixture
      (`fixtures/corpus/edge/windows1252_real.txt`, containing `canción`
      and `página`) and
      `index::pipeline::tests::real_windows1252_file_decodes_and_accents_survive`,
      which copies it through the full index → extract → search pipeline
      and asserts both accent-folded queries hit. (0-byte-as-empty was
      already fixture-driven via `real_empty_file_is_indexed_with_only_a_filename_chunk`
      against `fixtures/corpus/edge/empty_real.gitignore` — the review's
      claim that this one was unit-only was incorrect; left as-is.)
- [x] **Idempotence evidence matches the spec's wording.** SPEC.md §7 M2
      asks for indexing `fixtures/corpus` twice with identical row counts,
      not a manual run against `docs/` or the working tree. Added
      `crates/magi-core/tests/idempotence.rs`:
      `indexing_fixture_corpus_twice_is_idempotent` indexes the real
      `fixtures/corpus` directory (using `IndexingConfig::default()`'s
      exclude globs, the same `.venv`/`__pycache__`/etc. exclusions
      production indexing applies) into a fresh DB twice and asserts
      identical `files` and `chunks` row counts, plus an identical
      `IndexSummary` (indexed/skipped/errored counts match exactly).
- [x] **`docs/benchmarks.md` created.** Throughput recorded per file type
      (code/pdf/office/text-en/text-es), measured with the release
      `magi-cli` binary against replicated fixture copies to dilute
      per-run startup overhead, plus a raw single-fixture-count reference
      table and the edge-case batch (3 indexed, 2 errored, 0 crashes) for
      comparison. Reference machine and methodology documented there.

Verification (`cargo test -p magi-core`): 83 unit tests + 9 golden tests +
1 idempotence test, up from 79 unit + 7 golden. `cargo fmt --check` and
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean.

### Slice 3: golden tests and real-file edge-case fixtures

- [x] Golden tests (`crates/magi-core/tests/golden.rs`): each of the code,
      PDF, Office, and (since the follow-up above) plain-text extractors'
      output (`{:#?}`-formatted `ExtractedDoc`) is compared byte-for-byte
      against `fixtures/golden/*.txt`, one file per corpus fixture
      (`code_sample_rs`, `code_muestra_py`, `pdf_report`,
      `pdf_factura_electricista`, `office_notes_docx`, `office_kickoff_pptx`,
      `office_inventory_xlsx`, `text_en_onboarding_notes`,
      `text_es_notas_incorporacion`). Golden files are regenerated by
      re-running with `MAGI_BLESS_GOLDEN=1`.
- [x] Real-file edge-case integration tests (`index::pipeline::tests`),
      using genuine files copied from an actual nested personal document
      tree rather than synthetic tempfile writes, per SPEC.md §7 M2's
      fixture list:
      - `real_empty_file_is_indexed_with_only_a_filename_chunk` — a real
        0-byte `.gitignore` (`fixtures/corpus/edge/empty_real.gitignore`).
      - `real_file_over_cap_is_skipped_with_reason` — a real 1.4 MB PDF
        (`fixtures/corpus/edge/huge_real.pdf`) against a 1 MB test cap.
      - `real_deeply_nested_file_is_indexed_and_searchable` — a real Java
        source file preserved at its original 13-level-deep relative path
        (`fixtures/corpus/edge/deep_real/...`). Building the destination
        path with `root_path.join(rel)` where `rel` contains forward
        slashes silently produced mixed `/`/`\` separators on Windows that
        didn't string-match the walker's all-backslash path in the DB
        lookup — fixed by rebuilding the path component-by-component
        (`rel.components().fold(root_path, |acc, c| acc.join(c))`).
      - `real_windows1252_file_decodes_and_accents_survive` — added in the
        follow-up above.
      The existing walker unit test already covers deep nesting
      synthetically; these add real content going through the full
      index → extract → search pipeline.
- [x] 83 unit tests + 9 golden tests + 1 idempotence test
      (`cargo test -p magi-core`), `cargo fmt --check` and `cargo clippy
      --all-targets --all-features -- -D warnings` clean.

M2 is complete per SPEC.md §7's verification list, pending the
not-yet-verified-on-macOS/Linux caveat noted for earlier milestones.

### Slice 2: PDF, Office, and Code extraction

- [x] PDF extraction (`extract::pdf`, `pdfium-render`): one chunk group
      per page, page numbers stored, language detected. Dynamically binds
      the vendored `vendor/pdfium/<target>/bin/` library at runtime via a
      single process-wide `OnceLock<Pdfium>` — PDFium's C library isn't
      safe for concurrent independent bindings, which surfaced as a real,
      reproducible test race (fixed; verified stable across 5 consecutive
      full-suite runs). Dev-only path resolution (`MAGI_PDFIUM_PATH` env
      override or the `vendor/` layout); production bundling is an M6
      packaging decision.
- [x] Office extraction (`extract::office`): DOCX/PPTX via `zip` +
      `quick-xml` (paragraph/run text, namespace-agnostic local-name
      matching), XLSX via `calamine` (one chunk per sheet). All three
      share one `paginated_doc` helper (in `extract/mod.rs`) for
      chunking/language detection, added after code review flagged the
      original per-format duplication.
- [x] Code extraction (`extract::code`, `tree-sitter`): one chunk per
      top-level parse-tree symbol for Rust/Python/JS/TS/Java/C/C++/Go/C#;
      line-window fallback (100 lines, 10 overlap) for other `Code`-kind
      extensions (added `.rb`/`.php`/`.sh`/`.bash`/`.kt`/`.kts`/`.swift` to
      the classifier specifically to exercise this fallback path).
      ADR-0004 records the native-C-dependency decision (9 grammar
      crates); ADR-0001 updated to point to it since its "no native deps
      beyond PDFium" claim is now stale.
- [x] Pipeline dispatch (`index::pipeline`) unified into one exhaustive
      `dispatch_for(Kind) -> Dispatch` match, so `has_extractor` (decides
      whether to read a file's bytes at all) and `extract_for_kind`
      (which extractor to call) can't drift out of sync when a `Kind`
      variant is added — code review flagged the original two-separate-
      matches version as a silent-failure risk.
- [x] Generated dev-only fixtures via a temporary Python venv
      (`fixtures/corpus/generate_pdfs.py`, `generate_office.py` — mirrors
      `tools/reference_embeddings.py`'s role, not a runtime dependency):
      `pdf/report.pdf` (3-page English), `pdf/factura_electricista.pdf`
      (Spanish), `edge/truncated.pdf`, `edge/password_protected.pdf`,
      `office/notes.docx`, `office/kickoff.pptx`, `office/inventory.xlsx`,
      plus hand-written `code/sample.rs` and `code/muestra.py`.
- [x] 76 unit tests (`cargo test -p magi-core`, up from 62), clippy/fmt
      clean workspace-wide (including the Tauri desktop crate), plus
      manual end-to-end verification: indexed this repo's entire working
      tree (161 files across all four extractors at once, 2 expected
      errors from the truncated/password-protected edge fixtures, 0
      crashes) and spot-checked search quality (Spanish PPTX/PDF/code
      content, Rust `impl` method chunks) against the real results.
- [x] Independent code review (5 background agents; 3 hit a session rate
      limit mid-run) surfaced three real, fixed issues: the PDFium
      concurrency race (above), the missing tree-sitter ADR (above), and
      the `has_extractor`/`extract_for_kind` sync risk (above). A fourth
      finding (paginated-page text is copied 2-3× per page for language
      detection) was accepted as a documented low-severity efficiency
      note, not fixed — see the `paginated_doc` doc comment.

### Slice 1: walker, classifier, text extraction, FTS search

- [x] Discovery walker (`discovery::walk`, using the `ignore` crate):
      exclusion globs, hidden-file policy, symlink policy, and opaque
      bundles (`.app`/`.photoslibrary`/`.bundle`/`.framework`, yielded once
      and never descended). Unit-tested including deeply nested dirs.
- [x] Classifier (`discovery::classify`): extension-first, falling back to
      magic-byte sniffing (`infer`) when the extension is missing/unknown.
- [x] Plain text/Markdown extraction (`extract::text`): encoding detection
      via `chardetng`/`encoding_rs` + NFC normalization
      (`unicode-normalization`). Windows-1252 Spanish text round-trips
      correctly (accents survive).
- [x] Provisional character chunker (1,500 chars / 200 overlap,
      unicode-safe) — `chunk.rs`, already present.
- [x] Filename chunk (`extract::filename`): every file gets one, split on
      `_ - . space` + camelCase, including parent folder names.
- [x] Language detection (`extract::lang`): `whatlang` (ISO 639-3) mapped
      to ISO 639-1 via `isolang`.
- [x] File/chunk DB writer (`db::files::upsert_file`): one transaction per
      file, idempotent re-indexing (chunks replaced, not duplicated).
- [x] FTS5 keyword search (`search::fts`): query sanitization (quoted
      terms, immune to FTS injection), BM25 ranking, per-file dedup,
      `snippet()` highlights. Cross-lingual accent-folding verified
      (`cancion` matches `canción`, per SPEC.md's own example).
- [x] Extraction isolation: per-file 60s timeout (worker thread +
      `recv_timeout`) and panic containment (`catch_unwind`), added as new
      `Error::ExtractionTimeout`/`ExtractionPanicked` variants.
- [x] `magi-cli index <root>` (one-shot, no watcher) and
      `magi-cli search "<query>"` (`--mode fts`, the only mode until M3).
- [x] 62 unit tests (`cargo test -p magi-core`) plus manual end-to-end
      verification: indexed this repo's own `docs/` and full working tree
      (148 files), confirmed idempotent re-indexing (same file count, no
      duplicate rows), and confirmed the SPEC.md cross-lingual example
      query works against the real file.
- [x] Bug fix (found via the above manual testing, not part of M2 proper):
      `db::roots::add` reported re-adding the same root as `NestedRoot`
      (confusingly, "nested under existing root <itself>") instead of
      `RootAlreadyExists`. Fixed in `is_nested`, regression test added.
