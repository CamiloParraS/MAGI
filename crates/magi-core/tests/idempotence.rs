use std::path::{Path, PathBuf};

use magi_core::config::IndexingConfig;
use magi_core::embed::{CountingEmbedder, FakeEmbedder};
use magi_core::index::pipeline::{IndexContext, IndexRootOptions, index_root};
use magi_core::{db, paths};

fn corpus_dir() -> PathBuf {
    paths::canonicalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus"))
        .unwrap()
}

/// SPEC.md §7 M2: indexing `fixtures/corpus` twice must produce identical
/// DB row counts. Since M5 the second run is also a no-op: every file is
/// recognised as unchanged and nothing is embedded again. Uses `IndexingConfig::default()`'s exclude globs (same
/// ones production indexing applies — `.venv`, `__pycache__`, ...) so a
/// stray local Python venv under `fixtures/corpus/` (used to generate the
/// PDF/Office fixtures, see `fixtures/corpus/generate_*.py`) can't skew
/// the count.
#[test]
fn indexing_fixture_corpus_twice_is_idempotent() {
    let db_dir = tempfile::tempdir().unwrap();
    // Thumbnails go to the cache dir; keep them out of the real one.
    // SAFETY: the only test in this binary, so nothing races the environment.
    unsafe { std::env::set_var("MAGI_DATA_DIR", db_dir.path()) };
    let mut conn = db::open(&db_dir.path().join("magi.db")).unwrap();
    let root_path = corpus_dir();
    let root = db::roots::add(&conn, &root_path).unwrap();
    let options = IndexRootOptions::from_config(&IndexingConfig::default()).unwrap();

    let embedder = std::sync::Arc::new(CountingEmbedder::new(FakeEmbedder));
    let first_summary = index_root(
        &mut conn,
        root.id,
        &root_path,
        &options,
        &IndexContext::new(embedder.clone()),
    )
    .unwrap();
    let embedded_by_first = embedder.chunks();
    let first_files = db::files::count_files(&conn).unwrap();
    let first_chunks = db::files::count_chunks(&conn).unwrap();

    let second_summary = index_root(
        &mut conn,
        root.id,
        &root_path,
        &options,
        &IndexContext::new(embedder.clone()),
    )
    .unwrap();
    let second_files = db::files::count_files(&conn).unwrap();
    let second_chunks = db::files::count_chunks(&conn).unwrap();

    assert!(first_files > 0, "expected fixtures/corpus to yield files");
    assert_eq!(first_files, second_files);
    assert_eq!(first_chunks, second_chunks);
    let first_total = first_summary.indexed + first_summary.skipped + first_summary.errored;
    assert_eq!(
        second_summary.unchanged, first_total,
        "every file is recognised"
    );
    assert_eq!(
        (
            second_summary.indexed,
            second_summary.skipped,
            second_summary.errored
        ),
        (0, 0, 0)
    );
    assert_eq!(
        embedder.chunks(),
        embedded_by_first,
        "nothing is embedded again"
    );
}
