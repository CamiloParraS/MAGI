use super::*;
use crate::db;

fn open_test_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open(&dir.path().join("magi.db")).unwrap();
    (dir, conn)
}

/// Every status reads back as written, and each rule gives the same answer
/// in Rust and in the SQL the queries splice in.
#[test]
fn root_rules_agree_with_their_sql() {
    let (_dir, conn) = open_test_db();
    let root_dir = tempfile::tempdir().unwrap();
    let id = add(&conn, root_dir.path()).unwrap().id;
    let sql = |rule: &str| -> bool {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM roots WHERE id = ?1 AND {rule}"),
            [id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    };
    let all = [
        Health::Ok,
        Health::PermissionDenied,
        Health::Missing,
        Health::WatchFailed,
    ];
    for health in all {
        for enabled in [true, false] {
            set_status(&conn, id, health).unwrap();
            set_enabled(&conn, id, enabled).unwrap();
            let root = get(&conn, id).unwrap();
            let case = format!("{health} enabled={enabled}");
            assert_eq!(root.status, health, "{case}");
            assert_eq!(root.indexable(), sql(indexable_sql!()), "{case}");
            assert_eq!(root.searchable(), sql(searchable_sql!()), "{case}");
            assert_eq!(health.readable(), sql(readable_sql!()), "{case}");
        }
    }
}

#[test]
fn an_unreadable_root_stays_searchable_but_is_not_indexed() {
    let root = |status, enabled| Root {
        id: 1,
        path: PathBuf::new(),
        enabled,
        status,
    };
    assert!(
        root(Health::WatchFailed, true).indexable(),
        "polled instead"
    );
    assert!(!root(Health::PermissionDenied, true).indexable());
    assert!(root(Health::PermissionDenied, true).searchable());
    assert!(!root(Health::Missing, true).searchable());
    assert!(!root(Health::Ok, false).indexable());
    assert!(!root(Health::Ok, false).searchable());
}

#[test]
fn a_root_can_be_disabled_enabled_and_read_back() {
    let (_dir, conn) = open_test_db();
    let root_dir = tempfile::tempdir().unwrap();
    let root = add(&conn, root_dir.path()).unwrap();

    set_enabled(&conn, root.id, false).unwrap();
    assert!(!get(&conn, root.id).unwrap().enabled);
    set_enabled(&conn, root.id, true).unwrap();
    assert_eq!(get(&conn, root.id).unwrap(), root);
    assert!(matches!(
        set_enabled(&conn, root.id + 1, true),
        Err(Error::RootIdNotFound(_))
    ));
    assert!(matches!(
        get(&conn, root.id + 1),
        Err(Error::RootIdNotFound(_))
    ));
}

#[test]
fn add_and_list_roundtrip() {
    let (_dir, conn) = open_test_db();
    let root_dir = tempfile::tempdir().unwrap();

    let added = add(&conn, root_dir.path()).unwrap();
    let roots = list(&conn).unwrap();

    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].id, added.id);
    assert_eq!(roots[0].path, added.path);
}

#[test]
fn nested_root_is_rejected() {
    let (_dir, conn) = open_test_db();
    let parent = tempfile::tempdir().unwrap();
    let child = parent.path().join("child");
    std::fs::create_dir(&child).unwrap();

    add(&conn, parent.path()).unwrap();
    let err = add(&conn, &child).unwrap_err();
    assert!(matches!(err, Error::NestedRoot { .. }));
}

/// Adding a parent of an existing root takes the child's files over as
/// they are: re-pointed, not re-indexed.
#[test]
fn adding_parent_collapses_child_and_keeps_its_files() {
    use crate::db::files::upsert_file;
    use crate::embed::{FakeEmbedder, TextEmbedder};
    use crate::extract::RawChunk;

    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open(&dir.path().join("magi.db")).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let child_dir = parent.path().join("child");
    std::fs::create_dir(&child_dir).unwrap();
    let child = add(&conn, &child_dir).unwrap();
    let path = child.path.join("notes.txt");
    let rel = PathBuf::from("notes.txt");
    let chunks = vec![
        RawChunk::body("hello world".to_string()),
        crate::extract::filename::filename_chunk(&rel),
    ];
    let embeddings = FakeEmbedder
        .embed_passages(&["hello world", "notes"])
        .unwrap();
    upsert_file(
        &mut conn,
        &indexed_record(child.id, &path, &rel),
        &chunks,
        Some(&embeddings),
        None,
    )
    .unwrap();

    let (root, collapsed) = add_collapsing(&conn, parent.path()).unwrap();

    assert_eq!(collapsed, vec![child.id]);
    assert_eq!(list(&conn).unwrap(), vec![root.clone()]);
    let (root_id, rel_path, state): (i64, String, String) = conn
        .query_row("SELECT root_id, rel_path, state FROM files", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(root_id, root.id);
    assert_eq!(
        PathBuf::from(rel_path),
        Path::new("child").join("notes.txt")
    );
    assert_eq!(state, "indexed");
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count("SELECT COUNT(*) FROM vec_text"), 2);
    assert_eq!(
        count("SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'child'"),
        1,
        "the filename chunk follows the new rel_path"
    );
}

/// A child file mid-pipeline goes back to `pending`, so the writer drops
/// the in-flight result (stamped with the deleted child's id and rel path)
/// instead of failing it on the foreign key and burning a retry.
#[test]
fn collapsing_requeues_child_files_mid_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open(&dir.path().join("magi.db")).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let child_dir = parent.path().join("child");
    std::fs::create_dir(&child_dir).unwrap();
    let child = add(&conn, &child_dir).unwrap();
    let path = child.path.join("notes.txt");
    let rel = PathBuf::from("notes.txt");
    let mut record = indexed_record(child.id, &path, &rel);
    record.state = crate::db::files::FileState::Indexing;
    crate::db::files::upsert_file(&mut conn, &record, &[], Some(&[]), None).unwrap();

    add(&conn, parent.path()).unwrap();

    let state: String = conn
        .query_row("SELECT state FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "pending");
}

fn indexed_record<'a>(
    root_id: i64,
    path: &'a Path,
    rel: &'a Path,
) -> crate::db::files::FileRecord<'a> {
    crate::db::files::FileRecord {
        root_id,
        path,
        rel_path: rel,
        file_name: "notes.txt",
        ext: Some("txt"),
        kind: "text",
        size: 11,
        mtime_ns: 1,
        lang: None,
        state: crate::db::files::FileState::Indexed,
        skip_reason: None,
        error: None,
        seen_scan_id: 1,
        content_hash: None,
        thumb_key: None,
        features_missing: 0,
    }
}

#[test]
fn readding_same_root_reports_already_exists_not_nested() {
    let (_dir, conn) = open_test_db();
    let root_dir = tempfile::tempdir().unwrap();

    add(&conn, root_dir.path()).unwrap();
    let err = add(&conn, root_dir.path()).unwrap_err();
    assert!(matches!(err, Error::RootAlreadyExists(_)));
}

#[test]
fn missing_root_is_rejected() {
    let (_dir, conn) = open_test_db();
    let err = add(&conn, Path::new("/does/not/exist/hopefully")).unwrap_err();
    assert!(matches!(err, Error::RootNotFound(_)));
}

#[test]
fn remove_unknown_id_errors() {
    let (_dir, conn) = open_test_db();
    assert!(matches!(
        remove(&conn, 999),
        Err(Error::RootIdNotFound(999))
    ));
}

/// SPEC.md §7 M5 item 12: removing a root purges everything under it. This
/// used to fail outright on the `files.root_id` foreign key.
#[test]
fn removing_a_root_purges_its_files_chunks_and_vectors() {
    use crate::db::files::{FileRecord, upsert_file};
    use crate::embed::{FakeEmbedder, TextEmbedder};
    use crate::extract::RawChunk;

    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open(&dir.path().join("magi.db")).unwrap();
    let root_dir = tempfile::tempdir().unwrap();
    let root = add(&conn, root_dir.path()).unwrap();
    let path = root.path.join("notes.txt");
    let rel = std::path::PathBuf::from("notes.txt");
    let chunks = vec![RawChunk::body("hello world".to_string())];
    let embeddings = FakeEmbedder.embed_passages(&["hello world"]).unwrap();
    let record = FileRecord {
        root_id: root.id,
        path: &path,
        rel_path: &rel,
        file_name: "notes.txt",
        ext: Some("txt"),
        kind: "text",
        size: 11,
        mtime_ns: 1,
        lang: None,
        state: crate::db::files::FileState::Indexed,
        skip_reason: None,
        error: None,
        seen_scan_id: 1,
        content_hash: None,
        thumb_key: None,
        features_missing: 0,
    };
    upsert_file(
        &mut conn,
        &record,
        &chunks,
        Some(&embeddings),
        Some(&[0.5; 768]),
    )
    .unwrap();

    remove(&conn, root.id).unwrap();

    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    for table in ["roots", "files", "chunks", "vec_text", "vec_image"] {
        assert_eq!(count(table), 0, "{table} must be empty");
    }
    assert_eq!(
        count("chunks_fts WHERE chunks_fts MATCH 'hello'"),
        0,
        "and nothing searchable is left"
    );
}
