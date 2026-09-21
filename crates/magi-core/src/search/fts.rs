//! Query sanitization, BM25, `snippet()`. Implemented in M2 (see SPEC.md §7 M2).
//!
//! Full hybrid fusion (RRF over keyword + vector lists, §5.6) lands in M3;
//! this module is the FTS5-only path used by `magi-cli search --mode fts`.

use std::path::PathBuf;

use rusqlite::{Connection, params};

use crate::error::Result;
use crate::search::{FileHit, chunk_fetch_limit, first_hit_per_file};

/// Escapes `query` for FTS5 by wrapping each whitespace-separated term in
/// double quotes (doubling any embedded quotes), so user input can never
/// inject FTS syntax (see SPEC.md §5.6 step 1).
pub fn sanitize_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs an FTS5 BM25 search and returns up to `limit` files, best match
/// first, deduplicated so each file appears once (at its best-matching
/// chunk).
pub fn search_fts(conn: &Connection, query: &str, limit: u32) -> Result<Vec<FileHit>> {
    let sanitized = sanitize_query(query);
    if sanitized.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT f.id, f.path, f.file_name, f.mtime_ns,
                snippet(chunks_fts, 0, '[', ']', '...', 10)
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         JOIN files f ON f.id = c.file_id
         JOIN roots r ON r.id = f.root_id
         WHERE chunks_fts MATCH ?1 AND r.enabled = 1 AND r.status <> 'missing'
         ORDER BY bm25(chunks_fts), c.id
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![sanitized, chunk_fetch_limit(limit)], |row| {
            Ok(FileHit {
                file_id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                file_name: row.get(2)?,
                mtime_ns: row.get(3)?,
                snippet: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(first_hit_per_file(rows, limit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::files::{FileRecord, upsert_file};
    use crate::embed::{FakeEmbedder, TextEmbedder};
    use crate::extract::RawChunk;
    use std::path::Path;

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

    fn index_text(conn: &mut Connection, path: &str, body: &str) {
        let path_buf = std::path::PathBuf::from(path);
        let rel = std::path::PathBuf::from(Path::new(path).file_name().unwrap());
        let record = FileRecord {
            root_id: 1,
            path: &path_buf,
            rel_path: &rel,
            file_name: rel.to_str().unwrap(),
            ext: Some("txt"),
            kind: "text",
            size: body.len() as u64,
            mtime_ns: 0,
            lang: None,
            state: "indexed",
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
        };
        let chunks = [RawChunk::body(body.to_string())];
        upsert_file(conn, &record, &chunks, &fake_embeddings(&chunks), None).unwrap();
    }

    #[test]
    fn sanitizes_query_terms() {
        assert_eq!(sanitize_query("hello world"), "\"hello\" \"world\"");
        assert_eq!(sanitize_query("a\"b"), "\"a\"\"b\"");
        assert_eq!(sanitize_query(""), "");
    }

    #[test]
    fn zero_limit_returns_no_results() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/notes.txt", "hello world");

        let hits = search_fts(&conn, "hello", 0).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn finds_matching_file_and_snippet() {
        let (_dir, mut conn) = open_test_db();
        index_text(
            &mut conn,
            "/roots/a/recipe.txt",
            "receta de arepas con queso",
        );
        index_text(
            &mut conn,
            "/roots/a/other.txt",
            "unrelated content entirely",
        );

        let hits = search_fts(&conn, "arepas", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/recipe.txt"));
        assert!(hits[0].snippet.contains('['));
    }

    #[test]
    fn accented_query_matches_unaccented_and_vice_versa() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/song.txt", "una canción muy bonita");
        index_text(
            &mut conn,
            "/roots/a/book.txt",
            "en la primera pagina del libro",
        );

        // tokenize = 'unicode61 remove_diacritics 2' means accents are
        // stripped before indexing, so an unaccented query matches
        // accented text ("cancion" -> "canción") and an accented query
        // matches unaccented text ("página" -> "pagina") — both spec
        // examples from SPEC.md §7 M2.
        assert_eq!(search_fts(&conn, "cancion", 10).unwrap().len(), 1);
        assert_eq!(search_fts(&conn, "página", 10).unwrap().len(), 1);
    }

    #[test]
    fn injection_attempt_is_treated_as_literal_terms() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/notes.txt", "hello world");

        // A naive concatenation of this into an FTS query would be a
        // syntax error or unintended boolean expression; sanitized, it's
        // just literal terms that don't match anything.
        let hits = search_fts(&conn, "\" OR 1=1 --", 10).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn results_are_deduplicated_per_file() {
        let (_dir, mut conn) = open_test_db();
        let path_buf = PathBuf::from("/roots/a/multi.txt");
        let rel = PathBuf::from("multi.txt");
        let record = FileRecord {
            root_id: 1,
            path: &path_buf,
            rel_path: &rel,
            file_name: "multi.txt",
            ext: Some("txt"),
            kind: "text",
            size: 0,
            mtime_ns: 0,
            lang: None,
            state: "indexed",
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
        };
        let chunks = [
            RawChunk::body("apple banana".to_string()),
            RawChunk::body("apple cherry".to_string()),
        ];
        upsert_file(&mut conn, &record, &chunks, &fake_embeddings(&chunks), None).unwrap();

        let hits = search_fts(&conn, "apple", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    /// Two files with byte-identical body text produce an exact BM25 tie
    /// for the same query. Without a secondary sort key, SQLite doesn't
    /// guarantee which one comes back first; the tie-break on `c.id`
    /// (chunk row id) makes it deterministic — the first-inserted file
    /// (lower chunk id) must always win, and that holds across repeated
    /// runs of the same query.
    #[test]
    fn tied_bm25_score_breaks_deterministically_by_chunk_id() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/first.txt", "identical body text");
        index_text(&mut conn, "/roots/a/second.txt", "identical body text");

        for _ in 0..5 {
            let hits = search_fts(&conn, "identical body text", 10).unwrap();
            assert_eq!(hits.len(), 2);
            assert_eq!(hits[0].path, PathBuf::from("/roots/a/first.txt"));
            assert_eq!(hits[1].path, PathBuf::from("/roots/a/second.txt"));
        }
    }
}
