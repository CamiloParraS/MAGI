use super::queue::NEXT_PENDING_SQL;
use super::scan::HASHED_OF_SIZE_SQL;
use super::*;
use crate::error::Error;

fn upsert(
    conn: &mut Connection,
    record: &FileRecord,
    chunks: &[RawChunk],
    embeddings: &[Vec<f32>],
) -> Result<i64> {
    upsert_file(conn, record, chunks, Some(embeddings), None)
}
use crate::db;
use crate::embed::{FakeEmbedder, TextEmbedder};

mod lifecycle;
use crate::extract::ChunkSource;

fn open_test_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open(&dir.path().join("magi.db")).unwrap();
    let root_dir = tempfile::tempdir().unwrap();
    db::roots::add(&conn, root_dir.path()).unwrap();
    (dir, conn)
}

fn fake_embeddings(chunks: &[RawChunk]) -> Vec<Vec<f32>> {
    let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    FakeEmbedder.embed_passages(&texts).unwrap()
}

fn sample_record<'a>(path: &'a Path, rel_path: &'a Path) -> FileRecord<'a> {
    FileRecord {
        root_id: 1,
        path,
        rel_path,
        file_name: "notes.txt",
        ext: Some("txt"),
        kind: "text",
        size: 11,
        mtime_ns: 123,
        lang: Some("en"),
        state: FileState::Indexed,
        skip_reason: None,
        error: None,
        seen_scan_id: 1,
        content_hash: None,
        thumb_key: None,
        features_missing: 0,
    }
}

#[test]
fn paths_are_resolved_by_id() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/notes.txt");
    let rel = PathBuf::from("notes.txt");
    let id = upsert(&mut conn, &sample_record(&path, &rel), &[], &[]).unwrap();
    assert_eq!(path_of(&conn, id).unwrap(), path);
    assert!(matches!(
        path_of(&conn, id + 1),
        Err(Error::FileIdNotFound(_))
    ));
}

#[test]
fn details_have_the_size_and_the_last_page_read() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/notes.txt");
    let rel = PathBuf::from("notes.txt");
    let paged = |text: &str, page| RawChunk {
        page: Some(page),
        ..RawChunk::body(text.into())
    };
    let chunks = [paged("one", 1), paged("four", 4), paged("two", 2)];
    let id = upsert(
        &mut conn,
        &sample_record(&path, &rel),
        &chunks,
        &fake_embeddings(&chunks),
    )
    .unwrap();
    assert_eq!(
        details_of(&conn, id).unwrap(),
        FileDetails {
            size: 11,
            pages: Some(4)
        }
    );

    let plain = PathBuf::from("/roots/a/plain.txt");
    let rel = PathBuf::from("plain.txt");
    let chunks = [RawChunk::body("no pages".into())];
    let id = upsert(
        &mut conn,
        &sample_record(&plain, &rel),
        &chunks,
        &fake_embeddings(&chunks),
    )
    .unwrap();
    assert_eq!(details_of(&conn, id).unwrap().pages, None);
    assert!(matches!(
        details_of(&conn, id + 1),
        Err(Error::FileIdNotFound(_))
    ));
}

#[test]
fn a_failure_records_its_code_and_a_clean_index_clears_it() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/notes.txt");
    let rel = PathBuf::from("notes.txt");
    let mut record = sample_record(&path, &rel);
    record.state = FileState::Error;
    record.error = Some((FileErrorCode::ExtractFailed, "bad pdf xref"));
    let id = upsert(&mut conn, &record, &[], &[]).unwrap();
    let listed = &list_errors(&conn, 10).unwrap()[0];
    assert_eq!(
        (listed.file_id, listed.code, listed.detail.as_str()),
        (id, FileErrorCode::ExtractFailed, "bad pdf xref")
    );

    record_failure(&conn, id, FileErrorCode::PermissionDenied, "denied", 1000).unwrap();
    let code = |conn: &Connection| {
        conn.query_row(
            "SELECT error_code FROM files WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<FileErrorCode>>(0),
        )
        .unwrap()
    };
    assert_eq!(code(&conn), Some(FileErrorCode::PermissionDenied));

    record.state = FileState::Indexed;
    record.error = None;
    upsert(&mut conn, &record, &[], &[]).unwrap();
    assert_eq!(
        code(&conn),
        None,
        "a clean index clears the code with the text"
    );
}

#[test]
fn inserts_file_chunks_and_vectors_and_populates_fts() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/notes.txt");
    let rel = PathBuf::from("notes.txt");
    let chunks = vec![RawChunk::body("hello world".to_string())];

    let file_id = upsert(
        &mut conn,
        &sample_record(&path, &rel),
        &chunks,
        &fake_embeddings(&chunks),
    )
    .unwrap();

    assert_eq!(count_files(&conn).unwrap(), 1);
    assert_eq!(count_chunks(&conn).unwrap(), 1);
    let vec_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM vec_text", [], |row| row.get(0))
        .unwrap();
    assert_eq!(vec_rows, 1, "one vector per chunk");

    let matched: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'hello'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(matched, 1);

    let row = get_by_path(&conn, &path).unwrap().unwrap();
    assert_eq!(row.id, file_id);
    assert_eq!(row.state, "indexed");
}

#[test]
fn reindexing_same_path_replaces_chunks_and_vectors_without_duplicating_rows() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/notes.txt");
    let rel = PathBuf::from("notes.txt");

    let first_chunks = vec![RawChunk::body("version one".to_string())];
    let first_id = upsert(
        &mut conn,
        &sample_record(&path, &rel),
        &first_chunks,
        &fake_embeddings(&first_chunks),
    )
    .unwrap();
    let second_chunks = vec![
        RawChunk::body("version two".to_string()),
        RawChunk::body("more text".to_string()),
    ];
    let second_id = upsert(
        &mut conn,
        &sample_record(&path, &rel),
        &second_chunks,
        &fake_embeddings(&second_chunks),
    )
    .unwrap();

    assert_eq!(first_id, second_id);
    assert_eq!(count_files(&conn).unwrap(), 1);
    assert_eq!(count_chunks(&conn).unwrap(), 2);
    let vec_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM vec_text", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        vec_rows, 2,
        "stale vectors from the first version must be gone"
    );

    let stale: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'one'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
}

#[test]
fn skipped_file_records_reason() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/huge.bin");
    let rel = PathBuf::from("huge.bin");
    let mut record = sample_record(&path, &rel);
    record.state = FileState::Skipped;
    record.skip_reason = Some("too_large");

    upsert(&mut conn, &record, &[], &[]).unwrap();

    let row = get_by_path(&conn, &path).unwrap().unwrap();
    assert_eq!(row.state, "skipped");
    assert_eq!(row.skip_reason.as_deref(), Some("too_large"));
}

#[test]
fn chunk_source_is_stored_verbatim() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/notes.txt");
    let rel = PathBuf::from("notes.txt");
    let chunk = RawChunk {
        source: ChunkSource::Filename,
        text: "notes txt".to_string(),
        page: None,
        line_start: None,
        line_end: None,
    };

    let chunks = [chunk];
    upsert(
        &mut conn,
        &sample_record(&path, &rel),
        &chunks,
        &fake_embeddings(&chunks),
    )
    .unwrap();

    let source: String = conn
        .query_row("SELECT source FROM chunks LIMIT 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(source, "filename");
}

#[test]
fn fewer_embeddings_than_chunks_is_an_error_not_a_silent_truncation() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/notes.txt");
    let rel = PathBuf::from("notes.txt");
    let chunks = vec![
        RawChunk::body("one".to_string()),
        RawChunk::body("two".to_string()),
    ];

    let err = upsert(
        &mut conn,
        &sample_record(&path, &rel),
        &chunks,
        &fake_embeddings(&chunks[..1]),
    )
    .unwrap_err();

    assert!(matches!(err, crate::error::Error::Model(_)), "got {err:?}");
    assert_eq!(count_chunks(&conn).unwrap(), 0, "nothing partially written");
}

#[test]
fn hash_key_and_image_vector_round_trip_and_are_replaced_on_reindex() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/photo.jpg");
    let rel = PathBuf::from("photo.jpg");
    let mut record = sample_record(&path, &rel);
    let hash = [7u8; 32];
    record.content_hash = Some(&hash);
    record.thumb_key = Some("abc");

    upsert_file(&mut conn, &record, &[], Some(&[]), Some(&[0.5; 768])).unwrap();
    upsert_file(&mut conn, &record, &[], Some(&[]), Some(&[0.25; 768])).unwrap();

    let (stored, key): (Vec<u8>, String) = conn
        .query_row("SELECT content_hash, thumb_key FROM files", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((stored, key.as_str()), (hash.to_vec(), "abc"));
    let vectors: i64 = conn
        .query_row("SELECT COUNT(*) FROM vec_image", [], |r| r.get(0))
        .unwrap();
    assert_eq!(vectors, 1);

    upsert_file(&mut conn, &record, &[], Some(&[]), None).unwrap();
    let vectors: i64 = conn
        .query_row("SELECT COUNT(*) FROM vec_image", [], |r| r.get(0))
        .unwrap();
    assert_eq!(vectors, 0);
}

/// Vectors are bound as raw f32 bytes: what `vec0` stores is exactly them.
#[test]
fn vectors_are_stored_as_their_raw_f32_bytes() {
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/notes.txt");
    let rel = PathBuf::from("notes.txt");
    let chunks = vec![RawChunk::body("hello".into())];
    let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.1 - 7.3).collect();

    upsert_file(
        &mut conn,
        &sample_record(&path, &rel),
        &chunks,
        Some(std::slice::from_ref(&embedding)),
        None,
    )
    .unwrap();

    let stored: Vec<u8> = conn
        .query_row("SELECT embedding FROM vec_text", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, embedding_to_blob(&embedding));
}

/// The move lookup runs once per new file during a scan: it must use an
/// index, not read the whole table each time.
#[test]
fn the_move_lookup_uses_the_size_index() {
    let (_dir, conn) = open_test_db();
    let plan: Vec<String> = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {HASHED_OF_SIZE_SQL}"))
        .unwrap()
        .query_map(params![1, "text"], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert!(
        plan.iter().any(|step| step.contains("idx_files_size")),
        "plan: {plan:?}"
    );
}

/// The scheduler asks for the newest pending rows on every poll: that must
/// walk an index in order, not sort every pending row each time.
#[test]
fn the_pending_queue_is_read_in_index_order() {
    let (_dir, conn) = open_test_db();
    let plan: Vec<String> = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {NEXT_PENDING_SQL}"))
        .unwrap()
        .query_map(params![0, 10], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert!(
        plan.iter().any(|s| s.contains("idx_files_pending"))
            && !plan.iter().any(|s| s.contains("TEMP B-TREE")),
        "plan: {plan:?}"
    );
}
