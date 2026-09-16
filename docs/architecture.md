# Architecture

Stub. Update this document whenever a public contract changes (DB schema,
IPC commands, config format) — see SPEC.md §0.

## Process model

A single process: the Tauri app hosts `magi-core::Engine`. The engine runs
on its own threads; Tauri commands talk to it through an `EngineHandle`. The
CLI (`magi-cli daemon`) hosts the same engine headless. See SPEC.md §5.3 for
the full thread/data-flow diagram.

## Crates

- `magi-core` — all business logic, no Tauri dependency.
- `magi-cli` — dev/test CLI over `magi-core`.
- `apps/desktop/src-tauri` — the Tauri shell; thin command wrappers only.
- `xtask` — cross-platform dev tasks (fetch PDFium, fetch models, ...).

## IPC contract

DTOs live in `crates/magi-core/src/dto.rs` and are exported to
`apps/desktop/src/bindings/` via `ts-rs`. See SPEC.md §5.7 for the full
command table (populated as commands land, starting M1).

## Database schema

See SPEC.md §5.5, implemented by `crates/magi-core/src/db/migrations/0001_init.sql`.
`db::open()` registers `sqlite-vec`, sets `journal_mode=WAL`/`foreign_keys=ON`/
`busy_timeout`, and runs any pending migrations (tracked in
`schema_migrations`, applied at most once each). The database file lives at
`<data_dir>/magi.db`.

## Config

`<config_dir>/config.toml`, shape in SPEC.md §5.2. `config::load()` writes
defaults on first run; `config::save()` writes to a temp file and renames it
into place so a crash mid-save can't corrupt the existing config.
`Config::validate()` rejects malformed exclude globs and roots whose path
doesn't exist on disk.

## Roots

`db::roots` manages the `roots` table. `roots::add()` canonicalizes the path
and rejects it if it's missing, already registered, or nested with (an
ancestor or descendant of) an existing root.

## Embeddings and hybrid search

`embed::TextEmbedder` is the trait every text embedder implements
(`model_id`, `dim`, `embed_passages`, `embed_query`); `embed::FakeEmbedder`
is a deterministic, dependency-free implementation used until the real
`intfloat/multilingual-e5-small` model lands. `index::pipeline::index_root`
takes a `&dyn TextEmbedder`, embeds each file's chunk texts, and passes the
vectors to `db::files::upsert_file`, which writes them into `vec_text`
(`chunk_id -> embedding`) in the same transaction as the `chunks`/FTS rows —
and deletes the matching `vec_text` rows before deleting `chunks`, since
`vec0` virtual tables aren't covered by `FOREIGN KEY` cascades. After each
run, `index_root` records the embedder's `model_id()` in
`meta.text_model_id` and warns (via `tracing`) if it differs from the
previously recorded one — re-embedding on a model change is not yet
triggered automatically (that needs M5's scheduler).

`search::hybrid_search(conn, embedder, query, limit)` runs `search::fts`
(BM25, unchanged from M2) and `search::vector::search_vector_text` (a
`vec_text` KNN query), fuses their per-file-deduplicated results with
`search::fuse::reciprocal_rank_fusion` (SPEC.md §5.6's
`score = Σ w_i / (60 + rank_i)`, weight 1.0 each), then applies
`fuse::filename_boost` and `fuse::recency_boost` as a score multiplier.
`vec_image`/the SigLIP visual list are not wired in yet (M4). `magi-cli
search --mode hybrid` and `index` (which now requires an embedder — set
`MAGI_FAKE_EMBEDDER=1` until the real one exists) exercise this path from
the CLI.
