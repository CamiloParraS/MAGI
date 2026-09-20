//! Hybrid search orchestration (FTS + text vector + image vector, fused
//! with RRF). Implemented in M3 (text) and M4 (image) — see SPEC.md §5.6.

pub mod fts;
pub mod fuse;
pub mod vector;

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::Connection;

use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::Result;
use crate::search::fuse::{RankedList, filename_boost, recency_boost, reciprocal_rank_fusion};

pub const FTS_FETCH_LIMIT: u32 = 100;
pub const VECTOR_FETCH_LIMIT: u32 = 100;
/// SPEC.md §5.6 step 2c: the visual list is shorter than the text ones.
pub const IMAGE_FETCH_LIMIT: u32 = 50;
/// SPEC.md §5.6 step 4: the visual list counts for less than text.
const IMAGE_WEIGHT: f64 = 0.8;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub file_id: i64,
    pub path: PathBuf,
    pub score: f64,
    pub snippet: String,
    pub match_sources: Vec<&'static str>,
}

/// One file's best-matching chunk from a single ranked source. Both
/// [`fts::search_fts`] and [`vector::search_vector_text`] return these with
/// the file's metadata already joined in, so `hybrid_search` never has to
/// go back to the database per result.
#[derive(Debug, Clone, PartialEq)]
pub struct FileHit {
    pub file_id: i64,
    pub path: PathBuf,
    pub file_name: String,
    pub mtime_ns: i64,
    pub snippet: String,
}

/// How many chunk rows to read before per-file dedup: one file can
/// contribute several matching chunks.
pub(crate) fn chunk_fetch_limit(limit: u32) -> i64 {
    (limit as i64).saturating_mul(5).max(50)
}

/// Keeps each file's first hit, in the given order, up to `limit` files.
pub(crate) fn first_hit_per_file(
    rows: impl IntoIterator<Item = FileHit>,
    limit: u32,
) -> Vec<FileHit> {
    let mut seen = std::collections::HashSet::new();
    let mut hits = Vec::new();
    for hit in rows {
        if seen.insert(hit.file_id) {
            hits.push(hit);
            if hits.len() == limit as usize {
                break;
            }
        }
    }
    hits
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Fuses the given ranked lists with RRF, applies the filename/recency
/// boosts (SPEC.md §5.6 steps 4-5) and returns the top `limit` files.
///
/// Passing an empty list for one side yields that single mode's ranking
/// under the *same* boost path as hybrid. RRF over one non-empty list is
/// order-preserving (`weight / (60 + rank)` is strictly decreasing in
/// rank), so a single-mode call re-ranks only by the boosts — which is
/// what makes `magi-cli eval`'s baselines comparable to hybrid instead of
/// comparing two different ranking functions (SPEC.md §7 M3 item 4).
pub fn rank_and_boost(
    query: &str,
    fts_hits: &[FileHit],
    vector_hits: &[FileHit],
    image_hits: &[FileHit],
    limit: u32,
) -> Vec<SearchHit> {
    let fts_ids: Vec<i64> = fts_hits.iter().map(|h| h.file_id).collect();
    let vector_ids: Vec<i64> = vector_hits.iter().map(|h| h.file_id).collect();
    let image_ids: Vec<i64> = image_hits.iter().map(|h| h.file_id).collect();
    let fused = reciprocal_rank_fusion(&[
        RankedList {
            file_ids: &fts_ids,
            weight: 1.0,
        },
        RankedList {
            file_ids: &vector_ids,
            weight: 1.0,
        },
        RankedList {
            file_ids: &image_ids,
            weight: IMAGE_WEIGHT,
        },
    ]);

    let query_tokens: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    let now = now_unix();

    let mut by_id: HashMap<i64, [Option<&FileHit>; 3]> = HashMap::new();
    for hit in fts_hits {
        by_id.entry(hit.file_id).or_default()[0] = Some(hit);
    }
    for hit in vector_hits {
        by_id.entry(hit.file_id).or_default()[1] = Some(hit);
    }
    for hit in image_hits {
        by_id.entry(hit.file_id).or_default()[2] = Some(hit);
    }

    // `fused`'s ids are the union of the lists, so every lookup here
    // resolves. Preferring the FTS side keeps the snippet that carries the
    // `[...]` match highlights, and the visual side (a bare file name) is
    // the last resort.
    let mut hits: Vec<SearchHit> = fused
        .into_iter()
        .filter_map(|(file_id, base_score)| {
            let [fts, vector, image] = *by_id.get(&file_id)?;
            let hit = fts.or(vector).or(image)?;
            let boost = filename_boost(&query_tokens, &hit.file_name)
                * recency_boost(hit.mtime_ns / 1_000_000_000, now);
            let mut match_sources = Vec::new();
            if fts.is_some() {
                match_sources.push("keyword");
            }
            if vector.is_some() {
                match_sources.push("semantic");
            }
            if image.is_some() {
                match_sources.push("visual");
            }
            Some(SearchHit {
                file_id,
                path: hit.path.clone(),
                score: base_score * boost,
                snippet: hit.snippet.clone(),
                match_sources,
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit as usize);
    hits
}

/// Runs FTS5, `vec_text` and (when `image_embedder` is given) `vec_image`
/// search, fuses them with Reciprocal Rank
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
    image_embedder: Option<&dyn ImageEmbedder>,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let fts_hits = fts::search_fts(conn, query, FTS_FETCH_LIMIT)?;
    let query_embedding = embedder.embed_query(query)?;
    let vector_hits = vector::search_vector_text(conn, &query_embedding, VECTOR_FETCH_LIMIT)?;

    // A missing or broken visual model degrades to text-only search rather
    // than failing the query.
    let image_hits = match image_embedder.map(|e| e.embed_query(query)) {
        Some(Ok(embedding)) => vector::search_vector_image(conn, &embedding, IMAGE_FETCH_LIMIT)?,
        Some(Err(e)) => {
            tracing::warn!(error = %e, "visual search unavailable");
            Vec::new()
        }
        None => Vec::new(),
    };

    Ok(rank_and_boost(
        query,
        &fts_hits,
        &vector_hits,
        &image_hits,
        limit,
    ))
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
            content_hash: None,
            thumb_key: None,
        };
        let chunks = vec![RawChunk::body(body.to_string())];
        let embeddings = FakeEmbedder.embed_passages(&[body]).unwrap();
        upsert_file(conn, &record, &chunks, &embeddings, None).unwrap();
    }

    #[test]
    fn visual_match_is_found_through_vec_image_and_labelled() {
        use crate::embed::FakeImageEmbedder;
        let (_dir, mut conn) = open_test_db();
        // A photo with no text at all: only its planted vector can match.
        let path = PathBuf::from("/roots/a/IMG_0042.jpg");
        let rel = PathBuf::from("IMG_0042.jpg");
        let record = FileRecord {
            root_id: 1,
            path: &path,
            rel_path: &rel,
            file_name: "IMG_0042.jpg",
            ext: Some("jpg"),
            kind: "image",
            size: 1,
            mtime_ns: 0,
            lang: None,
            state: "indexed",
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
        };
        let name_chunk = vec![RawChunk::body("IMG 0042 jpg".to_string())];
        let embeddings = FakeEmbedder.embed_passages(&["IMG 0042 jpg"]).unwrap();
        let visual = FakeImageEmbedder.embed_query("dog on the beach").unwrap();
        upsert_file(&mut conn, &record, &name_chunk, &embeddings, Some(&visual)).unwrap();
        index_text(&mut conn, "/roots/a/tax.txt", "quarterly tax filing", 0);

        let hits = hybrid_search(
            &conn,
            &FakeEmbedder,
            Some(&FakeImageEmbedder),
            "dog on the beach",
            10,
        )
        .unwrap();
        let photo = hits.iter().find(|h| h.path == path).expect("photo found");
        assert!(photo.match_sources.contains(&"visual"));
        // Without the image embedder the same query cannot reach it.
        let text_only = hybrid_search(&conn, &FakeEmbedder, None, "dog on the beach", 10).unwrap();
        assert!(
            !text_only
                .iter()
                .any(|h| h.match_sources.contains(&"visual"))
        );
    }

    #[test]
    fn empty_query_or_zero_limit_returns_no_results() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/notes.txt", "hello world", 0);
        let embedder = FakeEmbedder;

        assert!(
            hybrid_search(&conn, &embedder, None, "", 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            hybrid_search(&conn, &embedder, None, "hello", 0)
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

        let hits = hybrid_search(&conn, &embedder, None, "arepas", 10).unwrap();
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

        let hits = hybrid_search(&conn, &embedder, None, "cat sat mat", 10).unwrap();
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

        let hits = hybrid_search(&conn, &embedder, None, "budget quarterly", 10).unwrap();
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/budget_report.txt"));
    }
}
