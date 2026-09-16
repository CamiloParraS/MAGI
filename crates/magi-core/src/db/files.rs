//! File/chunk repositories. Implemented in M1+ (see SPEC.md §5.5).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::embed::embedding_to_json;
use crate::error::Result;
use crate::extract::RawChunk;

/// Everything needed to upsert one file's row. `content_hash` is left for
/// M5's change-detection work.
pub struct FileRecord<'a> {
    pub root_id: i64,
    pub path: &'a Path,
    pub rel_path: &'a Path,
    pub file_name: &'a str,
    pub ext: Option<&'a str>,
    pub kind: &'a str,
    pub size: u64,
    pub mtime_ns: i64,
    pub lang: Option<&'a str>,
    pub state: &'a str,
    pub skip_reason: Option<&'a str>,
    pub error: Option<&'a str>,
    pub seen_scan_id: i64,
}

/// Inserts or replaces `record`, its `chunks`, and their `vec_text`
/// embeddings in one transaction: the file row is upserted keyed on its
/// unique `path`, and any previous chunks/vectors for that file are
/// deleted before the new ones are inserted (idempotent re-indexing).
/// `embeddings[i]` is the vector for `chunks[i]` — same length, same order.
///
/// Deletes `vec_text` rows before `chunks` (not after): the delete uses a
/// subquery over `chunks` to find which `vec_text` rows belong to this
/// file, so it must run while those `chunks` rows still exist (SPEC.md
/// §5.5: "Deletions MUST explicitly delete `vec_*` rows ... in the same
/// transaction" — virtual tables aren't covered by `FOREIGN KEY` cascades).
pub fn upsert_file(
    conn: &mut Connection,
    record: &FileRecord,
    chunks: &[RawChunk],
    embeddings: &[Vec<f32>],
) -> Result<i64> {
    debug_assert_eq!(chunks.len(), embeddings.len());
    let tx = conn.transaction()?;
    let path_str = record.path.to_string_lossy();
    let rel_path_str = record.rel_path.to_string_lossy();

    tx.execute(
        "INSERT INTO files (
            root_id, path, rel_path, file_name, ext, kind, size, mtime_ns,
            lang, state, skip_reason, error, pipeline_version, seen_scan_id, indexed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13, unixepoch())
         ON CONFLICT(path) DO UPDATE SET
            root_id = excluded.root_id,
            rel_path = excluded.rel_path,
            file_name = excluded.file_name,
            ext = excluded.ext,
            kind = excluded.kind,
            size = excluded.size,
            mtime_ns = excluded.mtime_ns,
            lang = excluded.lang,
            state = excluded.state,
            skip_reason = excluded.skip_reason,
            error = excluded.error,
            seen_scan_id = excluded.seen_scan_id,
            indexed_at = unixepoch()",
        params![
            record.root_id,
            path_str,
            rel_path_str,
            record.file_name,
            record.ext,
            record.kind,
            record.size as i64,
            record.mtime_ns,
            record.lang,
            record.state,
            record.skip_reason,
            record.error,
            record.seen_scan_id,
        ],
    )?;

    let file_id: i64 = tx.query_row(
        "SELECT id FROM files WHERE path = ?1",
        params![path_str],
        |row| row.get(0),
    )?;

    tx.execute(
        "DELETE FROM vec_text WHERE chunk_id IN (SELECT id FROM chunks WHERE file_id = ?1)",
        params![file_id],
    )?;
    tx.execute("DELETE FROM chunks WHERE file_id = ?1", params![file_id])?;
    for (ordinal, (chunk, embedding)) in chunks.iter().zip(embeddings).enumerate() {
        tx.execute(
            "INSERT INTO chunks (file_id, ordinal, source, text, page, line_start, line_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                file_id,
                ordinal as i64,
                chunk.source.as_str(),
                chunk.text,
                chunk.page,
                chunk.line_start,
                chunk.line_end,
            ],
        )?;
        // Not `tx.last_insert_rowid()`: the `chunks_ai` trigger's own
        // INSERT into `chunks_fts` runs synchronously first and would
        // shadow it. `ordinal` is unique per file, so look the row back up
        // by it (same pattern as `file_id` above).
        let chunk_id: i64 = tx.query_row(
            "SELECT id FROM chunks WHERE file_id = ?1 AND ordinal = ?2",
            params![file_id, ordinal as i64],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO vec_text (chunk_id, embedding) VALUES (?1, vec_f32(?2))",
            params![chunk_id, embedding_to_json(embedding)],
        )?;
    }

    tx.commit()?;
    Ok(file_id)
}

pub struct FileRow {
    pub id: i64,
    pub path: PathBuf,
    pub kind: String,
    pub state: String,
    pub skip_reason: Option<String>,
    pub lang: Option<String>,
}

pub fn get_by_path(conn: &Connection, path: &Path) -> Result<Option<FileRow>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, path, kind, state, skip_reason, lang FROM files WHERE path = ?1",
        params![path.to_string_lossy()],
        |row| {
            Ok(FileRow {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                kind: row.get(2)?,
                state: row.get(3)?,
                skip_reason: row.get(4)?,
                lang: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(crate::error::Error::Db)
}

pub fn count_files(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?)
}

pub fn count_chunks(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::embed::{FakeEmbedder, TextEmbedder};
    use crate::extract::ChunkSource;

    fn open_test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("magi.db")).unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        db::roots::add(&conn, root_dir.path()).unwrap();
        (dir, conn)
    }

    fn fake_embeddings(chunks: &[RawChunk]) -> Vec<Vec<f32>> {
        let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
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
            state: "indexed",
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
        }
    }

    #[test]
    fn inserts_file_and_chunks_and_populates_fts() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");
        let chunks = vec![RawChunk::body("hello world".to_string())];

        let file_id = upsert_file(
            &mut conn,
            &sample_record(&path, &rel),
            &chunks,
            &fake_embeddings(&chunks),
        )
        .unwrap();

        assert_eq!(count_files(&conn).unwrap(), 1);
        assert_eq!(count_chunks(&conn).unwrap(), 1);

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
    fn reindexing_same_path_replaces_chunks_without_duplicating_rows() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");

        let first_chunks = vec![RawChunk::body("version one".to_string())];
        let first_id = upsert_file(
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
        let second_id = upsert_file(
            &mut conn,
            &sample_record(&path, &rel),
            &second_chunks,
            &fake_embeddings(&second_chunks),
        )
        .unwrap();

        assert_eq!(first_id, second_id);
        assert_eq!(count_files(&conn).unwrap(), 1);
        assert_eq!(count_chunks(&conn).unwrap(), 2);

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
        record.state = "skipped";
        record.skip_reason = Some("too_large");

        upsert_file(&mut conn, &record, &[], &[]).unwrap();

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
        upsert_file(
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
    fn upsert_file_writes_one_vec_text_row_per_chunk() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");
        let chunks = vec![
            RawChunk::body("apple banana".to_string()),
            RawChunk::body("cherry date".to_string()),
        ];
        let embeddings = fake_embeddings(&chunks);

        upsert_file(&mut conn, &sample_record(&path, &rel), &chunks, &embeddings).unwrap();

        let vec_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_text", [], |row| row.get(0))
            .unwrap();
        assert_eq!(vec_rows, 2);
    }

    #[test]
    fn reindexing_replaces_vec_text_rows_without_duplicating() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");

        let first_chunks = vec![RawChunk::body("version one".to_string())];
        upsert_file(
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
        upsert_file(
            &mut conn,
            &sample_record(&path, &rel),
            &second_chunks,
            &fake_embeddings(&second_chunks),
        )
        .unwrap();

        let vec_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_text", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            vec_rows, 2,
            "stale vectors from the first version must be gone"
        );
    }
}
