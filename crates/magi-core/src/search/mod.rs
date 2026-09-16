//! Hybrid search orchestration (FTS + text vector + image vector, fused
//! with RRF). Implemented in M3 (text) and M4 (image) — see SPEC.md §5.6.

pub mod fts;
pub mod fuse;
pub mod vector;

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};

use crate::embed::TextEmbedder;
use crate::error::Result;
use crate::search::fuse::{RankedList, filename_boost, recency_boost, reciprocal_rank_fusion};

pub const FTS_FETCH_LIMIT: u32 = 100;
pub const VECTOR_FETCH_LIMIT: u32 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub file_id: i64,
    pub path: PathBuf,
    pub score: f64,
    pub snippet: String,
    pub match_sources: Vec<&'static str>,
}

struct FileMeta {
    path: PathBuf,
    file_name: String,
    mtime_ns: i64,
}

fn file_meta(conn: &Connection, file_id: i64) -> Result<Option<FileMeta>> {
    conn.query_row(
        "SELECT path, file_name, mtime_ns FROM files WHERE id = ?1",
        params![file_id],
        |row| {
            Ok(FileMeta {
                path: PathBuf::from(row.get::<_, String>(0)?),
                file_name: row.get(1)?,
                mtime_ns: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(crate::error::Error::Db)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Runs FTS5 and `vec_text` search, fuses them with Reciprocal Rank
/// Fusion, applies filename/recency boosts, and returns the top `limit`
/// files (SPEC.md §5.6).
///
/// ponytail: the two queries run sequentially against `conn` here, not on
/// separate parallel reader connections — SPEC.md's "run in parallel" is
/// about not blocking one query behind the other on the engine's reader
/// pool, which doesn't exist until M6. Fusion correctness doesn't depend
/// on query order, so this defers threading to when that pool lands.
pub fn hybrid_search(
    conn: &Connection,
    embedder: &dyn TextEmbedder,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let fts_hits = fts::search_fts(conn, query, FTS_FETCH_LIMIT)?;
    let query_embedding = embedder.embed_query(query)?;
    let vector_hits = vector::search_vector_text(conn, &query_embedding, VECTOR_FETCH_LIMIT)?;

    let fts_ids: Vec<i64> = fts_hits.iter().map(|h| h.file_id).collect();
    let vector_ids: Vec<i64> = vector_hits.iter().map(|h| h.file_id).collect();
    let fused = reciprocal_rank_fusion(&[
        RankedList {
            file_ids: &fts_ids,
            weight: 1.0,
        },
        RankedList {
            file_ids: &vector_ids,
            weight: 1.0,
        },
    ]);

    let query_tokens: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    let now = now_unix();

    let mut hits = Vec::new();
    for (file_id, base_score) in fused {
        let Some(meta) = file_meta(conn, file_id)? else {
            continue; // file removed between the two queries and here
        };
        let boost = filename_boost(&query_tokens, &meta.file_name)
            * recency_boost(meta.mtime_ns / 1_000_000_000, now);

        let mut match_sources = Vec::new();
        let mut snippet = String::new();
        if let Some(h) = fts_hits.iter().find(|h| h.file_id == file_id) {
            match_sources.push("keyword");
            snippet = h.snippet.clone();
        }
        if let Some(h) = vector_hits.iter().find(|h| h.file_id == file_id) {
            match_sources.push("semantic");
            if snippet.is_empty() {
                snippet = h.snippet.clone();
            }
        }

        hits.push(SearchHit {
            file_id,
            path: meta.path,
            score: base_score * boost,
            snippet,
            match_sources,
        });
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit as usize);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::files::{FileRecord, upsert_file};
    use crate::embed::FakeEmbedder;
    use crate::extract::RawChunk;
    use std::path::{Path, PathBuf};

    fn open_test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("magi.db")).unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        db::roots::add(&conn, root_dir.path()).unwrap();
        (dir, conn)
    }

    fn index_text(conn: &mut Connection, path: &str, body: &str, mtime_ns: i64) {
        let path_buf = PathBuf::from(path);
        let rel = PathBuf::from(Path::new(path).file_name().unwrap());
        let record = FileRecord {
            root_id: 1,
            path: &path_buf,
            rel_path: &rel,
            file_name: rel.to_str().unwrap(),
            ext: Some("txt"),
            kind: "text",
            size: body.len() as u64,
            mtime_ns,
            lang: None,
            state: "indexed",
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
        };
        let chunks = vec![RawChunk::body(body.to_string())];
        let embeddings = FakeEmbedder.embed_passages(&[body.to_string()]).unwrap();
        upsert_file(conn, &record, &chunks, &embeddings).unwrap();
    }

    #[test]
    fn empty_query_or_zero_limit_returns_no_results() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/notes.txt", "hello world", 0);
        let embedder = FakeEmbedder;

        assert!(hybrid_search(&conn, &embedder, "", 10).unwrap().is_empty());
        assert!(
            hybrid_search(&conn, &embedder, "hello", 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn keyword_only_match_is_found_via_fts() {
        let (_dir, mut conn) = open_test_db();
        index_text(
            &mut conn,
            "/roots/a/recipe.txt",
            "receta de arepas con queso",
            0,
        );
        let embedder = FakeEmbedder;

        let hits = hybrid_search(&conn, &embedder, "arepas", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/recipe.txt"));
        assert!(hits[0].match_sources.contains(&"keyword"));
    }

    #[test]
    fn file_matching_both_keyword_and_vector_ranks_first() {
        let (_dir, mut conn) = open_test_db();
        // Shares the query's exact words (keyword + vector match).
        index_text(&mut conn, "/roots/a/cats.txt", "cat sat on the mat", 0);
        // Unrelated content (neither list should surface it).
        index_text(
            &mut conn,
            "/roots/a/market.txt",
            "stock market rallied today",
            0,
        );
        let embedder = FakeEmbedder;

        let hits = hybrid_search(&conn, &embedder, "cat sat mat", 10).unwrap();
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/cats.txt"));
        assert!(hits[0].match_sources.contains(&"keyword"));
        assert!(hits[0].match_sources.contains(&"semantic"));
    }

    #[test]
    fn filename_overlap_boosts_ranking() {
        let (_dir, mut conn) = open_test_db();
        // Identical body text (so FTS and vector scores are exactly tied
        // pre-boost, up to KNN tie-break order) and only the filename
        // differs — isolates filename_boost's effect on the final order.
        // Neither body contains "budget", so FTS's implicit AND across
        // query terms can't match either file on it; only the vector list
        // and the filename boost are in play for that term.
        index_text(
            &mut conn,
            "/roots/a/budget_report.txt",
            "the quarterly numbers were reviewed",
            0,
        );
        index_text(
            &mut conn,
            "/roots/a/other.txt",
            "the quarterly numbers were reviewed",
            0,
        );
        let embedder = FakeEmbedder;

        let hits = hybrid_search(&conn, &embedder, "budget quarterly", 10).unwrap();
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/budget_report.txt"));
    }
}
