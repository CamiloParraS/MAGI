//! The state machine, retries, counts, deletion and purge.

use super::*;

fn add_file(conn: &mut Connection, root_id: i64, name: &str, mtime_ns: i64, body: &str) -> i64 {
    let path = PathBuf::from(format!("/roots/{root_id}/{name}"));
    let rel = PathBuf::from(name);
    let mut record = sample_record(&path, &rel);
    record.root_id = root_id;
    record.file_name = name;
    record.mtime_ns = mtime_ns;
    let chunks = vec![RawChunk::body(body.to_string())];
    upsert_file(
        conn,
        &record,
        &chunks,
        Some(&fake_embeddings(&chunks)),
        Some(&[0.5; 768]),
    )
    .unwrap()
}

fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// SPEC.md §7 M5 item 7: gone from files, chunks, chunks_fts, vec_text and
/// vec_image.
#[test]
fn deleting_a_file_removes_every_trace_of_it() {
    let (_dir, mut conn) = open_test_db();
    let keep = add_file(&mut conn, 1, "keep.txt", 1, "kept words");
    let gone = add_file(&mut conn, 1, "gone.txt", 2, "vanishing words");

    delete_file(&mut conn, gone).unwrap();

    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM files"), 1);
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM chunks"), 1);
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM vec_text"), 1);
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM vec_image"), 1);
    assert_eq!(
        scalar(
            &conn,
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'vanishing'"
        ),
        0
    );
    assert_eq!(
        scalar(
            &conn,
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'kept'"
        ),
        1
    );
    assert_eq!(scalar(&conn, "SELECT id FROM files"), keep);
    delete_file(&mut conn, gone).unwrap(); // already gone: a no-op
}

#[test]
fn purging_a_root_removes_only_its_files() {
    let (_dir, mut conn) = open_test_db();
    let other_dir = tempfile::tempdir().unwrap();
    let other = db::roots::add(&conn, other_dir.path()).unwrap().id;
    add_file(&mut conn, 1, "a.txt", 1, "first root");
    add_file(&mut conn, 1, "b.txt", 2, "first root too");
    add_file(&mut conn, other, "c.txt", 3, "second root");

    let tx = conn.transaction().unwrap();
    purge_root(&tx, 1).unwrap();
    tx.commit().unwrap();

    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM files"), 1);
    assert_eq!(scalar(&conn, "SELECT root_id FROM files"), other);
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM vec_text"), 1);
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM vec_image"), 1);
}

#[test]
fn counts_indexed_files_per_root() {
    let (_dir, mut conn) = open_test_db();
    let other_dir = tempfile::tempdir().unwrap();
    let other = db::roots::add(&conn, other_dir.path()).unwrap().id;
    add_file(&mut conn, 1, "a.txt", 1, "x");
    add_file(&mut conn, 1, "b.txt", 2, "x");
    let queued = add_file(&mut conn, other, "c.txt", 3, "x");
    mark_pending(&conn, queued).unwrap();

    let counts = count_indexed_by_root(&conn).unwrap();
    assert_eq!(counts.get(&1), Some(&2));
    assert_eq!(counts.get(&other), None, "a queued file is not indexed yet");
}

/// The status thread runs it twice a second: it must count from an
/// index alone, never reading the table row by row.
#[test]
fn indexed_per_root_counts_from_a_covering_index() {
    let (_dir, conn) = open_test_db();
    let plan: Vec<String> = conn
        .prepare(
            "EXPLAIN QUERY PLAN \
             SELECT root_id, COUNT(*) FROM files WHERE state = 'indexed' GROUP BY root_id",
        )
        .unwrap()
        .query_map([], |row| row.get(3))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    let plan = plan.join("\n");
    assert!(plan.contains("COVERING INDEX"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
}

#[test]
fn next_pending_is_newest_first_and_waits_out_a_retry_delay() {
    let (_dir, mut conn) = open_test_db();
    let old = add_file(&mut conn, 1, "old.txt", 10, "x");
    let new = add_file(&mut conn, 1, "new.txt", 30, "x");
    let waiting = add_file(&mut conn, 1, "waiting.txt", 20, "x");
    let done = add_file(&mut conn, 1, "done.txt", 40, "x");
    for id in [old, new, waiting] {
        mark_pending(&conn, id).unwrap();
    }
    conn.execute(
        "UPDATE files SET next_attempt_at = 1000 WHERE id = ?1",
        params![waiting],
    )
    .unwrap();

    let ids = |now| -> Vec<i64> {
        next_pending(&conn, now, 10)
            .unwrap()
            .iter()
            .map(|f| f.id)
            .collect()
    };

    assert_eq!(
        ids(999),
        vec![new, old],
        "not the waiting one, not the settled one"
    );
    assert_eq!(ids(1000), vec![new, waiting, old], "by mtime, newest first");
    assert!(!ids(1000).contains(&done));
    assert_eq!(
        next_pending(&conn, 1000, 1).unwrap().len(),
        1,
        "limit respected"
    );
}

/// SPEC.md §6.2: a file another program holds open is retried with
/// backoff and is never given up on for it.
#[test]
fn a_locked_file_backs_off_but_never_goes_to_error() {
    let (_dir, mut conn) = open_test_db();
    let id = add_file(&mut conn, 1, "open.docx", 1, "x");
    mark_indexing(&conn, id).unwrap();

    assert_eq!(
        record_locked(&conn, id, FileErrorCode::Locked, "locked", 1000).unwrap(),
        Failure::Retry {
            attempts: 1,
            next_attempt_at: 1030
        }
    );
    for now in 2000..2010 {
        assert_eq!(
            record_locked(&conn, id, FileErrorCode::Locked, "locked", now).unwrap(),
            Failure::Retry {
                attempts: MAX_ATTEMPTS - 1,
                next_attempt_at: now + 120
            }
        );
    }
    let state: String = conn
        .query_row("SELECT state FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "pending");
}

#[test]
fn file_states_read_back_as_written_and_are_counted() {
    let (_dir, mut conn) = open_test_db();
    let id = add_file(&mut conn, 1, "a.txt", 1, "x");
    let all = [
        FileState::Pending,
        FileState::Indexing,
        FileState::Indexed,
        FileState::Skipped,
        FileState::Error,
    ];
    for state in all {
        conn.execute(
            "UPDATE files SET state = ?2 WHERE id = ?1",
            params![id, state],
        )
        .unwrap();
        assert_eq!(state_of(&conn, id).unwrap(), Some(state));
        assert_eq!(count_states(&conn).unwrap().get(&state), Some(&1));
    }
}

#[test]
fn files_waiting_in_a_switched_off_root_are_not_counted_as_queued() {
    let (_dir, mut conn) = open_test_db();
    let id = add_file(&mut conn, 1, "a.txt", 1, "x");
    mark_pending(&conn, id).unwrap();
    assert_eq!(
        count_states(&conn).unwrap().get(&FileState::Pending),
        Some(&1)
    );
    crate::db::roots::set_enabled(&conn, 1, false).unwrap();
    assert_eq!(count_states(&conn).unwrap().get(&FileState::Pending), None);
}

#[test]
fn retrying_errors_queues_them_afresh_and_leaves_the_rest() {
    let (_dir, mut conn) = open_test_db();
    let broken = add_file(&mut conn, 1, "broken.txt", 1, "x");
    let fine = add_file(&mut conn, 1, "fine.txt", 2, "y");
    for now in [1000, 2000, 3000] {
        record_failure(&conn, broken, FileErrorCode::ReadFailed, "unreadable", now).unwrap();
    }
    assert_eq!(
        count_states(&conn).unwrap().get(&FileState::Error),
        Some(&1)
    );

    assert_eq!(retry_errors(&conn).unwrap(), 1);
    let row: (String, i64, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT state, attempts, next_attempt_at, error FROM files WHERE id = ?1",
            [broken],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    // The error stays until a result replaces it, so change detection
    // re-reads the file instead of keeping it.
    assert_eq!(row, ("pending".into(), 0, None, Some("unreadable".into())));
    assert_eq!(state_of(&conn, fine).unwrap(), Some(FileState::Indexed));
    assert_eq!(retry_errors(&conn).unwrap(), 0);
}

#[test]
fn a_disabled_or_missing_root_has_nothing_to_hand_out() {
    let (_dir, mut conn) = open_test_db();
    let id = add_file(&mut conn, 1, "queued.txt", 1, "x");
    mark_pending(&conn, id).unwrap();
    assert_eq!(next_pending(&conn, 0, 10).unwrap().len(), 1);

    conn.execute("UPDATE roots SET enabled = 0", []).unwrap();
    assert!(next_pending(&conn, 0, 10).unwrap().is_empty());
    conn.execute("UPDATE roots SET enabled = 1, status = 'missing'", [])
        .unwrap();
    assert!(next_pending(&conn, 0, 10).unwrap().is_empty());
    conn.execute("UPDATE roots SET status = 'watch_failed'", [])
        .unwrap();
    assert_eq!(next_pending(&conn, 0, 10).unwrap().len(), 1);
}

#[test]
fn a_failing_file_backs_off_then_goes_to_error() {
    let (_dir, mut conn) = open_test_db();
    let id = add_file(&mut conn, 1, "flaky.txt", 1, "x");
    mark_indexing(&conn, id).unwrap();

    assert_eq!(
        record_failure(&conn, id, FileErrorCode::ReadFailed, "locked", 1000).unwrap(),
        Failure::Retry {
            attempts: 1,
            next_attempt_at: 1030
        }
    );
    assert_eq!(
        record_failure(&conn, id, FileErrorCode::ReadFailed, "locked", 2000).unwrap(),
        Failure::Retry {
            attempts: 2,
            next_attempt_at: 2120
        }
    );
    let state = |conn: &Connection| -> String {
        conn.query_row("SELECT state FROM files", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(state(&conn), "pending");

    assert_eq!(
        record_failure(&conn, id, FileErrorCode::ReadFailed, "still locked", 3000).unwrap(),
        Failure::GaveUp { attempts: 3 }
    );
    assert_eq!(state(&conn), "error");
    let (message, next): (String, Option<i64>) = conn
        .query_row("SELECT error, next_attempt_at FROM files", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((message.as_str(), next), ("still locked", None));
    assert_eq!(
        (
            backoff_secs(1),
            backoff_secs(2),
            backoff_secs(3),
            backoff_secs(9)
        ),
        (30, 120, 600, 600)
    );
}

#[test]
fn a_crashed_run_leaves_indexing_rows_that_reset_to_pending() {
    let (_dir, mut conn) = open_test_db();
    let a = add_file(&mut conn, 1, "a.txt", 1, "x");
    let b = add_file(&mut conn, 1, "b.txt", 2, "x");
    mark_indexing(&conn, a).unwrap();

    assert_eq!(reset_indexing_to_pending(&conn).unwrap(), 1);

    let state_of = |id: i64| -> String {
        conn.query_row("SELECT state FROM files WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(
        (state_of(a).as_str(), state_of(b).as_str()),
        ("pending", "indexed")
    );
}

#[test]
fn invalidating_marks_files_stale_so_the_hash_skip_cannot_keep_them() {
    let (_dir, mut conn) = open_test_db();
    let text = add_file(&mut conn, 1, "a.txt", 1, "x");
    let photo = {
        let path = PathBuf::from("/roots/1/p.jpg");
        let rel = PathBuf::from("p.jpg");
        let mut record = sample_record(&path, &rel);
        record.kind = "image";
        upsert_file(&mut conn, &record, &[], Some(&[]), None).unwrap()
    };

    assert_eq!(invalidate(&conn, Some("image")).unwrap(), 1);
    let row = |id: i64| -> (String, i64) {
        conn.query_row(
            "SELECT state, pipeline_version FROM files WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    assert_eq!(row(photo), ("pending".to_string(), 0));
    assert_eq!(row(text).0, "indexed");

    assert_eq!(invalidate(&conn, None).unwrap(), 2);
    assert_eq!(row(text), ("pending".to_string(), 0));
}
