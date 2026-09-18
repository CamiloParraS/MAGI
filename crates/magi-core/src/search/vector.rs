//! Vector KNN search over `vec_text` / `vec_image`. Implemented in M3/M4.

use std::path::PathBuf;

use rusqlite::{Connection, params};

use crate::embed::embedding_to_json;
use crate::error::Result;
use crate::search::{FileHit, chunk_fetch_limit, first_hit_per_file};

/// Runs a `vec_text` KNN search and returns up to `limit` files, closest
/// first, deduplicated so each file appears once at its best-matching
/// chunk (SPEC.md §5.6 steps 2b, 3).
///
/// The `k = ?` constraint (rather than a bare `LIMIT`, which only works on
/// SQLite 3.41+) and the CTE-before-join shape both match sqlite-vec's own
/// documented-safe usage pattern for `vec0` KNN queries joined back to
/// source tables — see
/// https://github.com/asg017/sqlite-vec/blob/v0.1.9/site/features/knn.md.
pub fn search_vector_text(
    conn: &Connection,
    query_embedding: &[f32],
    limit: u32,
) -> Result<Vec<FileHit>> {
    if query_embedding.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "WITH knn_matches AS (
            SELECT chunk_id, distance
            FROM vec_text
            WHERE embedding MATCH vec_f32(?1) AND k = ?2
         )
         SELECT f.id, f.path, f.file_name, f.mtime_ns, substr(c.text, 1, 200),
                knn_matches.distance, knn_matches.chunk_id
         FROM knn_matches
         JOIN chunks c ON c.id = knn_matches.chunk_id
         JOIN files f ON f.id = c.file_id
         ORDER BY knn_matches.distance",
    )?;
    // vec0 KNN queries only permit a single-column `ORDER BY distance` in
    // the statement (sqlite-vec rejects a compound ORDER BY here, even in
    // the outer SELECT over the CTE), so the `chunk_id` tie-break has to
    // happen in Rust instead of SQL. Distance and `chunk_id` are read
    // purely as sort keys and don't outlive this function.
    let mut rows = stmt
        .query_map(
            params![embedding_to_json(query_embedding), chunk_fetch_limit(limit)],
            |row| {
                Ok((
                    FileHit {
                        file_id: row.get(0)?,
                        path: PathBuf::from(row.get::<_, String>(1)?),
                        file_name: row.get(2)?,
                        mtime_ns: row.get(3)?,
                        snippet: row.get(4)?,
                    },
                    row.get::<_, f64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.sort_by(|(_, a_distance, a_chunk_id), (_, b_distance, b_chunk_id)| {
        a_distance
            .total_cmp(b_distance)
            .then_with(|| a_chunk_id.cmp(b_chunk_id))
    });

    Ok(first_hit_per_file(
        rows.into_iter().map(|(hit, _, _)| hit),
        limit,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::files::{FileRecord, upsert_file};
    use crate::embed::{FakeEmbedder, TextEmbedder};
    use crate::extract::RawChunk;

    fn open_test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("magi.db")).unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        db::roots::add(&conn, root_dir.path()).unwrap();
        (dir, conn)
    }

    fn index_text(conn: &mut Connection, path: &str, body: &str) {
        let path_buf = PathBuf::from(path);
        let rel = PathBuf::from(std::path::Path::new(path).file_name().unwrap());
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
        };
        let chunks = vec![RawChunk::body(body.to_string())];
        let embeddings = FakeEmbedder.embed_passages(&[body]).unwrap();
        upsert_file(conn, &record, &chunks, &embeddings).unwrap();
    }

    #[test]
    fn zero_limit_or_empty_embedding_returns_no_results() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/notes.txt", "hello world");

        assert!(
            search_vector_text(&conn, &[0.1, 0.2], 0)
                .unwrap()
                .is_empty()
        );
        assert!(search_vector_text(&conn, &[], 10).unwrap().is_empty());
    }

    #[test]
    fn finds_closest_file_by_embedding() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/cats.txt", "cat sat on the mat");
        index_text(
            &mut conn,
            "/roots/a/market.txt",
            "stock market rallied today",
        );

        let query = FakeEmbedder.embed_query("cat sat on a mat").unwrap();
        let hits = search_vector_text(&conn, &query, 10).unwrap();

        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/cats.txt"));
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
        };
        let chunks = vec![
            RawChunk::body("apple banana".to_string()),
            RawChunk::body("apple cherry".to_string()),
        ];
        let embeddings = FakeEmbedder
            .embed_passages(&chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>())
            .unwrap();
        upsert_file(&mut conn, &record, &chunks, &embeddings).unwrap();

        let query = FakeEmbedder.embed_query("apple").unwrap();
        let hits = search_vector_text(&conn, &query, 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    /// Two files with byte-identical body text embed to the exact same
    /// vector, so their KNN distance to any query ties exactly. Without a
    /// secondary sort key, SQLite doesn't guarantee which one comes back
    /// first; the tie-break on `chunk_id` makes it deterministic — the
    /// first-inserted file (lower `chunk_id`) must always win, and that
    /// holds across repeated runs of the same query.
    #[test]
    fn tied_distance_breaks_deterministically_by_chunk_id() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/first.txt", "identical body text");
        index_text(&mut conn, "/roots/a/second.txt", "identical body text");

        let query = FakeEmbedder.embed_query("identical body text").unwrap();
        for _ in 0..5 {
            let hits = search_vector_text(&conn, &query, 10).unwrap();
            assert_eq!(hits.len(), 2);
            // Identical bodies produce identical vectors, so the two
            // distances tie exactly by construction; only the `chunk_id`
            // tie-break decides this order.
            assert_eq!(hits[0].path, PathBuf::from("/roots/a/first.txt"));
            assert_eq!(hits[1].path, PathBuf::from("/roots/a/second.txt"));
        }
    }
}
