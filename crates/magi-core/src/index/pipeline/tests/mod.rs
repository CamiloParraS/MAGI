use super::*;
use crate::db;
use crate::embed::{CountingEmbedder, FakeEmbedder, TextEmbedder};
use std::fs;

mod change;
mod kinds;

/// Returns `(db_dir, root_dir, conn, root_id)`. Both temp dirs must
/// stay in scope for the duration of the test, or they're deleted.
fn open_test_db() -> (tempfile::TempDir, tempfile::TempDir, Connection, i64) {
    let db_dir = tempfile::tempdir().unwrap();
    let conn = db::open(&db_dir.path().join("magi.db")).unwrap();
    let root_dir = tempfile::tempdir().unwrap();
    let root = db::roots::add(&conn, root_dir.path()).unwrap();
    (db_dir, root_dir, conn, root.id)
}

/// `index_root` with the defaults every test but two uses.
fn index_fake(conn: &mut Connection, root_id: i64, root_path: &Path) -> IndexSummary {
    index_root(
        conn,
        root_id,
        root_path,
        &default_options(),
        &IndexContext::new(Arc::new(FakeEmbedder)),
    )
    .unwrap()
}

fn default_options() -> IndexRootOptions {
    IndexRootOptions {
        exclude_globs: vec![glob::Pattern::new("**/node_modules/**").unwrap()],
        include_hidden: false,
        follow_symlinks: false,
        max_file_size_mb: 1,
        max_image_megapixels: 64,
        scan: Default::default(),
        file_types: vec![Kind::Text, Kind::Code, Kind::Pdf, Kind::Office, Kind::Image],
    }
}

#[test]
fn indexes_text_files_and_makes_them_searchable() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(
        root_path.join("english.txt"),
        "The quick brown fox jumps over the lazy dog.",
    )
    .unwrap();
    fs::write(
        root_path.join("spanish.txt"),
        "El veloz murciélago hindú comía feliz cardillo y kiwi.",
    )
    .unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.indexed, 2);
    assert_eq!(summary.skipped, 0);
    assert_eq!(summary.errored, 0);
    assert_eq!(db::files::count_files(&conn).unwrap(), 2);

    let hits = crate::search::fts::search_fts(&conn, "murciélago", 10).unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn image_over_the_megapixel_cap_is_skipped_not_errored() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    // Declares 20000 x 20000 (400 MP) in 1.1 MB, so it is over the default
    // 64 MP cap and refused from its header.
    let bomb = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/edge/bomb.png");
    fs::copy(bomb, root_path.join("panorama.png")).unwrap();

    // The test default caps files at 1 MB and the bomb is 1.1 MB: raise it
    // so the image, not the file-size check, is what refuses it.
    let options = IndexRootOptions {
        max_file_size_mb: 10,
        ..default_options()
    };
    let summary = index_root(
        &mut conn,
        root_id,
        &root_path,
        &options,
        &IndexContext::new(Arc::new(FakeEmbedder)),
    )
    .unwrap();

    assert_eq!((summary.skipped, summary.errored), (1, 0));
    let row = db::files::get_by_path(&conn, &root_path.join("panorama.png"))
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "skipped");
    assert_eq!(row.skip_reason.as_deref(), Some("image_too_large"));
}

/// SPEC.md §5.4 step 3: a cloud placeholder is indexed by name only and
/// its content is never read (reading it would download it).
#[test]
fn a_cloud_only_file_is_indexed_by_name_and_never_read() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let path = root_path.join("report.txt");
    fs::write(&path, "confidential zeppelin").unwrap();
    let entry = WalkEntry {
        cloud_only: true,
        ..discovery::stat(&path).unwrap()
    };
    files::insert_pending(
        &conn,
        root_id,
        &path,
        Path::new("report.txt"),
        entry.size,
        entry.mtime_ns,
        1,
    )
    .unwrap();
    let job = Job {
        root_id,
        root_path: root_path.clone(),
        stored: files::get_stored(&conn, &path).unwrap().unwrap(),
        entry,
    };

    let step = index_file(
        &IndexContext::new(Arc::new(FakeEmbedder)),
        &default_options(),
        job,
    )
    .unwrap();
    let status = lifecycle::apply_alone(&mut conn, step).unwrap();

    assert!(matches!(status, Some(Status::Skipped)));
    let row = db::files::get_by_path(&conn, &path).unwrap().unwrap();
    assert_eq!(row.state, "skipped");
    assert_eq!(row.skip_reason.as_deref(), Some("cloud_only"));
    let hits = |q| crate::search::fts::search_fts(&conn, q, 10).unwrap().len();
    assert_eq!(hits("zeppelin"), 0, "content must not be read");
    assert_eq!(hits("report"), 1, "the name is still searchable");
}

/// A package bundle (`.app`, `.photoslibrary`) is one item: settled by name
/// at once, never read. Reading the folder used to fail with EISDIR, so it
/// waited in the queue through every retry and then showed as unreadable
/// (macOS checks workflow, 2026-11-06: "Indexing, 1 file to go").
#[test]
fn a_package_bundle_is_one_item_found_by_name() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let bundle = root_path.join("Thing.app");
    fs::create_dir_all(bundle.join("Contents")).unwrap();
    fs::write(bundle.join("Contents/inner.txt"), "zebrabundle").unwrap();
    let entry = discovery::stat(&bundle).unwrap();
    assert!(entry.is_dir);
    files::insert_pending(
        &conn,
        root_id,
        &bundle,
        Path::new("Thing.app"),
        entry.size,
        entry.mtime_ns,
        1,
    )
    .unwrap();
    let job = Job {
        root_id,
        root_path: root_path.clone(),
        stored: files::get_stored(&conn, &bundle).unwrap().unwrap(),
        entry,
    };

    let step = index_file(
        &IndexContext::new(Arc::new(FakeEmbedder)),
        &default_options(),
        job,
    )
    .unwrap();
    let status = lifecycle::apply_alone(&mut conn, step).unwrap();

    assert!(matches!(status, Some(Status::Skipped)));
    let row = db::files::get_by_path(&conn, &bundle).unwrap().unwrap();
    assert_eq!(row.state, "skipped");
    assert_eq!(row.skip_reason.as_deref(), Some("bundle"));
    let hits = |q| crate::search::fts::search_fts(&conn, q, 10).unwrap().len();
    assert_eq!(hits("zebrabundle"), 0, "its contents are not read");
    assert_eq!(hits("Thing"), 1, "the bundle is found by name");
}

/// Extracts one new file into a job for the embed stage.
fn extracted(conn: &Connection, root_id: i64, root_path: &Path, name: &str) -> (Job, Fresh) {
    let path = root_path.join(name);
    fs::write(
        &path,
        format!("notes about {name} and the electrician invoice ").repeat(12),
    )
    .unwrap();
    let entry = discovery::stat(&path).unwrap();
    files::insert_pending(
        conn,
        root_id,
        &path,
        Path::new(name),
        entry.size,
        entry.mtime_ns,
        1,
    )
    .unwrap();
    let job = Job {
        root_id,
        root_path: root_path.to_path_buf(),
        stored: files::get_stored(conn, &path).unwrap().unwrap(),
        entry,
    };
    let ctx = IndexContext::new(Arc::new(FakeEmbedder));
    let Extracted::Fresh(fresh) = prepare(&ctx, &default_options(), &job) else {
        panic!("{name} should need embedding");
    };
    (job, *fresh)
}

/// Small files share model calls: three files of two chunks each take two
/// calls, one for the three names and one for the three bodies (a short
/// chunk is not padded to a long one), and each file still gets exactly
/// the vectors it would alone.
#[test]
fn small_files_are_embedded_together_with_the_same_vectors() {
    let (_db_dir, root_dir, conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let group: Vec<_> = ["a.txt", "b.txt", "c.txt"]
        .iter()
        .map(|n| extracted(&conn, root_id, &root_path, n))
        .collect();
    let alone: Vec<Vec<Vec<f32>>> = group
        .iter()
        .map(|(_, fresh)| {
            let texts: Vec<&str> = fresh.chunks.iter().map(|c| c.text.as_str()).collect();
            FakeEmbedder.embed_passages(&texts).unwrap()
        })
        .collect();

    let counting = Arc::new(CountingEmbedder::new(FakeEmbedder));
    let ctx = IndexContext::new(counting.clone());
    let embedded = embed_group(&ctx, group, &|| {});

    assert_eq!((counting.calls(), counting.chunks()), (2, 6));
    let together: Vec<Vec<Vec<f32>>> = embedded
        .into_iter()
        .map(|(_, e)| e.unwrap().embeddings.unwrap())
        .collect();
    assert_eq!(together, alone);
}

#[test]
fn unsupported_file_type_is_indexed_by_filename_only() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root_path.join("archive.zip"), b"PK\x03\x04").unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.indexed, 1);
    let row = db::files::get_by_path(&conn, &root_path.join("archive.zip"))
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "indexed");
    assert_eq!(row.kind, "other");
    let hits = crate::search::fts::search_fts(&conn, "archive", 10).unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn excluded_directory_is_never_indexed() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::create_dir(root_path.join("node_modules")).unwrap();
    fs::write(root_path.join("node_modules/pkg.js"), "ignored").unwrap();
    fs::write(root_path.join("keep.txt"), "keep me").unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.indexed, 1);
    assert_eq!(db::files::count_files(&conn).unwrap(), 1);
}

#[test]
fn indexing_writes_vec_text_rows_and_records_model_id() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root_path.join("a.txt"), "hello there").unwrap();

    index_fake(&mut conn, root_id, &root_path);

    let chunk_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    let vec_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM vec_text", [], |r| r.get(0))
        .unwrap();
    assert_eq!(vec_count, chunk_count);
    assert!(chunk_count > 0);

    assert_eq!(
        crate::db::meta::get(&conn, "text_model_id").unwrap(),
        Some(FakeEmbedder.model_id().to_string())
    );
}

/// Fails any batch containing a content chunk (more than one text);
/// filename-only batches succeed.
struct FlakyEmbedder;

impl TextEmbedder for FlakyEmbedder {
    fn model_id(&self) -> &str {
        FakeEmbedder.model_id()
    }

    fn dim(&self) -> usize {
        FakeEmbedder.dim()
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.len() > 1 {
            return Err(Error::Model("boom".into()));
        }
        FakeEmbedder.embed_passages(texts)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        FakeEmbedder.embed_query(text)
    }
}

#[test]
fn disabled_file_type_is_indexed_by_filename_only() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root_path.join("a.txt"), "zebrafish").unwrap();

    let mut options = default_options();
    options.file_types.retain(|&k| k != Kind::Text);
    let summary = index_root(
        &mut conn,
        root_id,
        &root_path,
        &options,
        &IndexContext::new(Arc::new(FakeEmbedder)),
    )
    .unwrap();

    assert_eq!(summary.indexed, 1);
    let fts = |q| crate::search::fts::search_fts(&conn, q, 10).unwrap().len();
    assert_eq!(fts("zebrafish"), 0);
    assert_eq!(fts("a"), 1);
}

#[test]
fn embedding_failure_marks_file_errored_and_run_continues() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root_path.join("a.txt"), "hello there").unwrap();
    fs::write(root_path.join("b.bin"), [0u8; 4]).unwrap(); // filename-only: embeds fine

    let summary = index_root(
        &mut conn,
        root_id,
        &root_path,
        &default_options(),
        &IndexContext::new(Arc::new(FlakyEmbedder)),
    )
    .unwrap();

    assert_eq!(summary.errored, 1);
    assert_eq!(summary.indexed, 1);
    let row = db::files::get_by_path(&conn, &root_path.join("a.txt"))
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "error");
}
