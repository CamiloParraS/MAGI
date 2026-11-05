//! Change detection: hash-skip, model-change re-queue, state recovery.

use super::*;

fn run(
    conn: &mut Connection,
    root_id: i64,
    root_path: &Path,
    embedder: Arc<dyn TextEmbedder>,
) -> IndexSummary {
    index_root(
        conn,
        root_id,
        root_path,
        &default_options(),
        &IndexContext::new(embedder),
    )
    .unwrap()
}

fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Each run takes the next scan id, so a later scan of any kind gets a
/// larger one and reads rows it did not see as deletions.
#[test]
fn index_root_takes_a_new_scan_id_each_run() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root.join("notes.txt"), "words").unwrap();
    run(&mut conn, root_id, &root, Arc::new(FakeEmbedder));
    run(&mut conn, root_id, &root, Arc::new(FakeEmbedder));
    assert_eq!(scalar(&conn, "SELECT seen_scan_id FROM files"), 2);
    assert_eq!(crate::watch::reconcile::next_scan_id(&conn).unwrap(), 3);
}

#[test]
fn an_unchanged_file_is_not_embedded_again() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root.join("notes.txt"), "the same words").unwrap();
    let embedder = Arc::new(CountingEmbedder::new(FakeEmbedder));

    let first = run(&mut conn, root_id, &root, embedder.clone());
    let embedded = embedder.chunks();
    let second = run(&mut conn, root_id, &root, embedder.clone());

    assert_eq!((first.indexed, first.unchanged), (1, 0));
    assert_eq!((second.indexed, second.unchanged), (0, 1));
    assert_eq!(embedder.chunks(), embedded, "nothing may be embedded again");
    assert_eq!(scalar(&conn, "SELECT seen_scan_id FROM files"), 2);
}

/// SPEC.md §7 M5 item 3.
#[test]
fn touching_a_file_without_changing_it_does_not_re_embed() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    let file = root.join("notes.txt");
    fs::write(&file, "the same words").unwrap();
    let embedder = Arc::new(CountingEmbedder::new(FakeEmbedder));
    run(&mut conn, root_id, &root, embedder.clone());
    let embedded = embedder.chunks();
    let old_mtime = scalar(&conn, "SELECT mtime_ns FROM files");

    // Same bytes, new mtime: what `touch` or a re-save does.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let summary = run(&mut conn, root_id, &root, embedder.clone());

    assert_eq!((summary.indexed, summary.unchanged), (0, 1));
    assert_eq!(embedder.chunks(), embedded);
    assert!(
        scalar(&conn, "SELECT mtime_ns FROM files") > old_mtime,
        "the new mtime must be recorded so the next scan does not hash it again"
    );
}

/// SPEC.md §7 M5 item 2: the old chunks, FTS rows and vectors are gone.
#[test]
fn a_modified_file_replaces_its_chunks_search_rows_and_vectors() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    let file = root.join("notes.txt");
    fs::write(&file, "alpha apple").unwrap();
    let embedder = Arc::new(CountingEmbedder::new(FakeEmbedder));
    run(&mut conn, root_id, &root, embedder.clone());
    let embedded = embedder.chunks();

    fs::write(&file, "beta banana and a longer second sentence").unwrap();
    let summary = run(&mut conn, root_id, &root, embedder.clone());

    assert_eq!(summary.indexed, 1);
    assert!(embedder.chunks() > embedded, "the new content is embedded");
    let fts = |term: &str| {
        crate::search::fts::search_fts(&conn, term, 10)
            .unwrap()
            .len()
    };
    assert_eq!((fts("apple"), fts("banana")), (0, 1));
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM files"), 1);
    assert_eq!(
        scalar(&conn, "SELECT COUNT(*) FROM vec_text"),
        scalar(&conn, "SELECT COUNT(*) FROM chunks"),
        "one vector per chunk, none left over from the old content"
    );
}

/// SPEC.md §7 M5 item 3b.
#[test]
fn a_model_change_re_embeds_unchanged_content_and_the_same_model_does_not() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root.join("notes.txt"), "the same words").unwrap();
    let v1 = Arc::new(CountingEmbedder::new(FakeEmbedder));
    run(&mut conn, root_id, &root, v1.clone());

    let same = run(&mut conn, root_id, &root, v1.clone());
    assert_eq!(
        (same.indexed, same.unchanged),
        (0, 1),
        "same id: hash-skip applies"
    );

    let v2 = Arc::new(CountingEmbedder::new(FakeEmbedder).with_model_id("fake-v2"));
    let changed = run(&mut conn, root_id, &root, v2.clone());
    assert_eq!((changed.indexed, changed.unchanged), (1, 0));
    assert!(
        v2.chunks() > 0,
        "the file must be embedded by the new model"
    );
    assert_eq!(
        db::meta::get(&conn, "text_model_id").unwrap().as_deref(),
        Some("fake-v2")
    );

    let after = run(&mut conn, root_id, &root, v2.clone());
    assert_eq!(
        (after.indexed, after.unchanged),
        (0, 1),
        "and then it is settled again"
    );
}

#[test]
fn a_stale_pipeline_version_is_re_extracted() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root.join("notes.txt"), "the same words").unwrap();
    let embedder = Arc::new(CountingEmbedder::new(FakeEmbedder));
    run(&mut conn, root_id, &root, embedder.clone());
    conn.execute("UPDATE files SET pipeline_version = 0", [])
        .unwrap();

    let summary = run(&mut conn, root_id, &root, embedder.clone());

    assert_eq!((summary.indexed, summary.unchanged), (1, 0));
    assert_eq!(
        scalar(&conn, "SELECT pipeline_version FROM files"),
        crate::index::PIPELINE_VERSION
    );
}

struct NamedOcr(&'static str);
impl OcrEngine for NamedOcr {
    fn engine_id(&self) -> &str {
        self.0
    }
    fn recognize(&self, _: &image::RgbImage) -> Result<String> {
        Ok(String::new())
    }
}

#[test]
fn an_ocr_engine_change_re_queues_images_and_nothing_else() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root.join("notes.txt"), "the same words").unwrap();
    let qr = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/qr/qr_url.png");
    fs::copy(qr, root.join("code.png")).unwrap();
    let with = |ocr: &'static str| IndexContext {
        ocr: Some(Arc::new(NamedOcr(ocr))),
        ..IndexContext::new(Arc::new(FakeEmbedder))
    };
    let go = |conn: &mut Connection, ctx: &IndexContext| {
        index_root(conn, root_id, &root, &default_options(), ctx).unwrap()
    };

    go(&mut conn, &with("ocr-a"));
    let same = go(&mut conn, &with("ocr-a"));
    assert_eq!((same.indexed, same.unchanged), (0, 2));

    let changed = go(&mut conn, &with("ocr-b"));
    assert_eq!(
        (changed.indexed, changed.unchanged),
        (1, 1),
        "only the image is read again"
    );
    assert_eq!(
        db::meta::get(&conn, "ocr_engine_id").unwrap().as_deref(),
        Some("ocr-b")
    );
}

/// SPEC.md §7 M5 item 14, the part that does not need file permissions: a
/// file that cannot be read becomes `error` and the others carry on.
#[test]
fn a_corrupt_file_is_an_error_and_the_run_continues() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    let truncated = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/edge/truncated.pdf");
    fs::copy(truncated, root.join("a_broken.pdf")).unwrap();
    fs::write(root.join("z_fine.txt"), "still gets indexed").unwrap();

    let summary = run(&mut conn, root_id, &root, Arc::new(FakeEmbedder));

    assert_eq!((summary.indexed, summary.errored), (1, 1));
    let broken = db::files::get_by_path(&conn, &root.join("a_broken.pdf"))
        .unwrap()
        .unwrap();
    assert_eq!(broken.state, "error");
    assert_eq!(
        crate::search::fts::search_fts(&conn, "indexed", 10)
            .unwrap()
            .len(),
        1
    );
}

/// An `error` row is queued again (touched, or `retry_errors`) with the same
/// bytes: it must be extracted again, not kept and relabelled `indexed`.
#[test]
fn an_errored_file_queued_again_is_re_extracted_not_kept() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    let truncated = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/edge/truncated.pdf");
    let path = root.join("broken.pdf");
    fs::copy(truncated, &path).unwrap();
    run(&mut conn, root_id, &root, Arc::new(FakeEmbedder));
    let state = |conn: &Connection| db::files::get_by_path(conn, &path).unwrap().unwrap().state;
    assert_eq!(state(&conn), "error");

    // Touched: new mtime, same bytes.
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
        .unwrap();
    let touched = run(&mut conn, root_id, &root, Arc::new(FakeEmbedder));
    assert_eq!((touched.errored, touched.unchanged), (1, 0));
    assert_eq!(state(&conn), "error");

    db::files::retry_errors(&conn).unwrap();
    let retried = run(&mut conn, root_id, &root, Arc::new(FakeEmbedder));
    assert_eq!(
        retried.errored, 1,
        "extracted again (the scan saw the same stat)"
    );
    assert_eq!(state(&conn), "error");
}

#[test]
fn a_pending_file_with_the_same_content_is_confirmed_not_re_extracted() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root.join("notes.txt"), "the same words").unwrap();
    let embedder = Arc::new(CountingEmbedder::new(FakeEmbedder));
    run(&mut conn, root_id, &root, embedder.clone());
    let embedded = embedder.chunks();
    let id = scalar(&conn, "SELECT id FROM files");
    // What a reconciliation scan does when it sees a different mtime.
    db::files::mark_pending(&conn, id).unwrap();
    conn.execute("UPDATE files SET mtime_ns = 1", []).unwrap();

    let summary = run(&mut conn, root_id, &root, embedder.clone());

    assert_eq!((summary.indexed, summary.unchanged), (0, 1));
    assert_eq!(embedder.chunks(), embedded);
    let state: String = conn
        .query_row("SELECT state FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "indexed");
}

/// The pipeline half of SPEC.md §7 M5 item 11: rows a crash left
/// `indexing` go back in the queue and are finished.
#[test]
fn rows_left_indexing_by_a_crash_are_finished_on_the_next_run() {
    let (_db, root_dir, mut conn, root_id) = open_test_db();
    let root = crate::paths::canonicalize(root_dir.path()).unwrap();
    fs::write(root.join("notes.txt"), "the same words").unwrap();
    run(&mut conn, root_id, &root, Arc::new(FakeEmbedder));
    conn.execute("UPDATE files SET state = 'indexing'", [])
        .unwrap();

    run(&mut conn, root_id, &root, Arc::new(FakeEmbedder));

    let state: String = conn
        .query_row("SELECT state FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "indexed");
}
