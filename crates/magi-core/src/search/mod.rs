//! Hybrid search orchestration (FTS + text vector + image vector, fused
//! with RRF). Implemented in M3 (text) and M4 (image) — see SPEC.md §5.6.

pub mod fts;
pub mod fuse;
pub mod vector;

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::Connection;

use crate::discovery::Kind;
use crate::dto::MatchSource;
use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::Result;
use crate::extract::ChunkSource;
use crate::search::fuse::{RankedList, filename_boost, recency_boost, reciprocal_rank_fusion};

pub const FTS_FETCH_LIMIT: u32 = 100;
pub const VECTOR_FETCH_LIMIT: u32 = 100;
/// SPEC.md §5.6 step 2c: the visual list is shorter than the text ones.
pub const IMAGE_FETCH_LIMIT: u32 = 50;
/// Least query-image cosine that counts as a visual match for SigLIP 2
/// (ADR-0007). Measured on the fixture eval: text-only queries never exceed
/// 0.117 against any image, genuine visual matches have a median of 0.139 and
/// the weakest non-OCR one is ~0.105. Tied to the model: recalibrate when the
/// image model changes.
pub const IMAGE_MIN_COSINE: f32 = 0.10;
/// SPEC.md §5.6 step 4: the visual list counts for less than text.
const IMAGE_WEIGHT: f64 = 0.8;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub file_id: i64,
    pub path: PathBuf,
    pub file_name: String,
    pub kind: Kind,
    pub mtime_ns: i64,
    pub thumb_key: Option<String>,
    pub score: f64,
    pub snippet: String,
    pub match_sources: Vec<MatchSource>,
    /// Page of the best chunk (PDF page, slide), when it has one.
    pub page: Option<i64>,
}

/// One file's best-matching chunk from a single ranked source. Every ranked
/// list returns these with the file's row already joined in, so neither
/// `hybrid_search` nor the Host goes back to the database per result.
#[derive(Debug, Clone, PartialEq)]
pub struct FileHit {
    pub file_id: i64,
    pub path: PathBuf,
    pub file_name: String,
    pub kind: Kind,
    pub mtime_ns: i64,
    pub thumb_key: Option<String>,
    pub snippet: String,
    /// Page of the best chunk (PDF page, slide), when it has one.
    pub page: Option<i64>,
    /// The best chunk's source; `None` for a visual match, which has no
    /// chunk.
    pub source: Option<ChunkSource>,
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
    // match highlights, and the visual side (a bare file name) is the last
    // resort.
    let mut hits: Vec<SearchHit> = fused
        .into_iter()
        .filter_map(|(file_id, base_score)| {
            let [fts, vector, image] = *by_id.get(&file_id)?;
            let hit = fts.or(vector).or(image)?;
            let boost = filename_boost(&query_tokens, &hit.file_name)
                * recency_boost(hit.mtime_ns / 1_000_000_000, now);
            let mut match_sources = Vec::new();
            if fts.is_some() {
                match_sources.push(MatchSource::Keyword);
            }
            if vector.is_some() {
                match_sources.push(MatchSource::Semantic);
            }
            if image.is_some() {
                match_sources.push(MatchSource::Visual);
            }
            match hit.source {
                Some(ChunkSource::Ocr) => match_sources.push(MatchSource::Ocr),
                Some(ChunkSource::Qr) => match_sources.push(MatchSource::Qr),
                Some(ChunkSource::Filename) => match_sources.push(MatchSource::Filename),
                Some(ChunkSource::Body | ChunkSource::CodeSymbol) | None => {}
            }
            Some(SearchHit {
                file_id,
                path: hit.path.clone(),
                file_name: hit.file_name.clone(),
                kind: hit.kind,
                mtime_ns: hit.mtime_ns,
                thumb_key: hit.thumb_key.clone(),
                score: base_score * boost,
                snippet: hit.snippet.clone(),
                match_sources,
                page: hit.page,
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

/// The query's embeddings, one per tower that is loaded. Computed without
/// the database, so a caller sharing a connection embeds before locking it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryVectors {
    pub text: Option<Vec<f32>>,
    pub image: Option<Vec<f32>>,
}

impl QueryVectors {
    /// Embeds `query` with both towers at once: they are separate ONNX
    /// sessions, so the query waits for the slower one, not for both. A
    /// missing or broken visual model degrades to text-only search rather
    /// than failing the query.
    pub fn embed(
        text: Option<&dyn TextEmbedder>,
        image: Option<&dyn ImageEmbedder>,
        query: &str,
    ) -> Result<Self> {
        if query.trim().is_empty() {
            return Ok(Self::default());
        }
        std::thread::scope(|scope| {
            let image = image.map(|e| scope.spawn(|| e.embed_query(query)));
            let text = text.map(|e| e.embed_query(query)).transpose()?;
            let image =
                match image.map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))) {
                    Some(Ok(embedding)) => Some(embedding),
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "visual search unavailable");
                        None
                    }
                    None => None,
                };
            Ok(Self { text, image })
        })
    }
}

/// Embeds the query ([`QueryVectors::embed`]), then [`search_with`].
pub fn hybrid_search(
    conn: &Connection,
    embedder: Option<&dyn TextEmbedder>,
    image_embedder: Option<&dyn ImageEmbedder>,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchHit>> {
    let vectors = QueryVectors::embed(embedder, image_embedder, query)?;
    search_with(conn, &vectors, query, limit)
}

/// Runs FTS5, `vec_text` and `vec_image` search for the vectors given,
/// fuses them with Reciprocal Rank Fusion, applies filename/recency boosts,
/// and returns the top `limit` files (SPEC.md §5.6).
///
/// ponytail: the SQL runs sequentially on one connection; the embeddings,
/// the expensive part, already run in parallel. A second reader connection
/// would let FTS overlap the KNN scans if `vec_text` grows into the budget.
pub fn search_with(
    conn: &Connection,
    vectors: &QueryVectors,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let fts_hits = fts::search_fts(conn, query, FTS_FETCH_LIMIT)?;
    // Without meaning, old `vec_text` rows stay unused.
    let vector_hits = match &vectors.text {
        Some(embedding) => vector::search_vector_text(conn, embedding, VECTOR_FETCH_LIMIT)?,
        None => Vec::new(),
    };
    let image_hits = match &vectors.image {
        Some(embedding) => {
            vector::search_vector_image(conn, embedding, IMAGE_FETCH_LIMIT, IMAGE_MIN_COSINE)?
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
            state: crate::db::files::FileState::Indexed,
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
            features_missing: 0,
        };
        let chunks = vec![RawChunk::body(body.to_string())];
        let embeddings = FakeEmbedder.embed_passages(&[body]).unwrap();
        upsert_file(conn, &record, &chunks, Some(&embeddings), None).unwrap();
    }

    #[test]
    fn hybrid_search_without_a_text_embedder_uses_keywords_only() {
        // vec_text rows from when meaning was on stay unused, and nothing
        // tries to embed the query.
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/port.txt", "the harbor at dawn", 0);
        let hits = hybrid_search(&conn, None, None, "harbor", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(
            conn.query_row("SELECT COUNT(*) FROM vec_text", [], |r| r.get::<_, i64>(0))
                .unwrap()
                > 0
        );
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
            state: crate::db::files::FileState::Indexed,
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
            features_missing: 0,
        };
        let name_chunk = vec![RawChunk::body("IMG 0042 jpg".to_string())];
        let embeddings = FakeEmbedder.embed_passages(&["IMG 0042 jpg"]).unwrap();
        let visual = FakeImageEmbedder.embed_query("dog on the beach").unwrap();
        upsert_file(
            &mut conn,
            &record,
            &name_chunk,
            Some(&embeddings),
            Some(&visual),
        )
        .unwrap();
        index_text(&mut conn, "/roots/a/tax.txt", "quarterly tax filing", 0);

        let hits = hybrid_search(
            &conn,
            Some(&FakeEmbedder),
            Some(&FakeImageEmbedder),
            "dog on the beach",
            10,
        )
        .unwrap();
        let photo = hits.iter().find(|h| h.path == path).expect("photo found");
        assert!(photo.match_sources.contains(&MatchSource::Visual));
        // An unrelated query is below the cosine floor: the photo is not
        // dragged into every search.
        let unrelated = hybrid_search(
            &conn,
            Some(&FakeEmbedder),
            Some(&FakeImageEmbedder),
            "quarterly tax filing",
            10,
        )
        .unwrap();
        assert!(
            !unrelated
                .iter()
                .any(|h| h.match_sources.contains(&MatchSource::Visual)),
            "{unrelated:?}"
        );
        // Without the image embedder the same query cannot reach it.
        let text_only =
            hybrid_search(&conn, Some(&FakeEmbedder), None, "dog on the beach", 10).unwrap();
        assert!(
            !text_only
                .iter()
                .any(|h| h.match_sources.contains(&MatchSource::Visual))
        );
    }

    /// A tower that, asked for a query vector, signals it started and waits
    /// for the other tower to start too: embedded one after the other, the
    /// first gives up and the query fails.
    struct Rendezvous {
        mine: crossbeam_channel::Sender<()>,
        theirs: crossbeam_channel::Receiver<()>,
    }

    impl Rendezvous {
        fn pair() -> (Self, Self) {
            let (a_tx, a_rx) = crossbeam_channel::bounded(1);
            let (b_tx, b_rx) = crossbeam_channel::bounded(1);
            let text = Rendezvous {
                mine: a_tx,
                theirs: b_rx,
            };
            let image = Rendezvous {
                mine: b_tx,
                theirs: a_rx,
            };
            (text, image)
        }

        fn meet(&self, dim: usize) -> Result<Vec<f32>> {
            let _ = self.mine.send(());
            self.theirs
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|_| crate::error::Error::Engine("embedded one after the other".into()))?;
            Ok(vec![1.0; dim])
        }
    }

    impl TextEmbedder for Rendezvous {
        fn model_id(&self) -> &str {
            "rendezvous"
        }
        fn dim(&self) -> usize {
            crate::embed::TEXT_EMBEDDING_DIM
        }
        fn embed_passages(&self, _: &[&str]) -> Result<Vec<Vec<f32>>> {
            unreachable!("only queries are embedded")
        }
        fn embed_query(&self, _: &str) -> Result<Vec<f32>> {
            self.meet(crate::embed::TEXT_EMBEDDING_DIM)
        }
    }

    impl ImageEmbedder for Rendezvous {
        fn model_id(&self) -> &str {
            "rendezvous"
        }
        fn dim(&self) -> usize {
            crate::embed::IMAGE_EMBEDDING_DIM
        }
        fn embed_image(&self, _: &image::RgbImage) -> Result<Vec<f32>> {
            unreachable!("only queries are embedded")
        }
        fn embed_query(&self, _: &str) -> Result<Vec<f32>> {
            self.meet(crate::embed::IMAGE_EMBEDDING_DIM)
        }
    }

    #[test]
    fn both_towers_embed_the_query_at_once() {
        let (text, image) = Rendezvous::pair();
        let vectors = QueryVectors::embed(Some(&text), Some(&image), "harbor").unwrap();
        assert_eq!(
            (
                vectors.text.map(|v| v.len()),
                vectors.image.map(|v| v.len())
            ),
            (
                Some(crate::embed::TEXT_EMBEDDING_DIM),
                Some(crate::embed::IMAGE_EMBEDDING_DIM)
            )
        );
    }

    #[test]
    fn a_broken_visual_tower_degrades_to_text_only() {
        struct Broken;
        impl ImageEmbedder for Broken {
            fn model_id(&self) -> &str {
                "broken"
            }
            fn dim(&self) -> usize {
                crate::embed::IMAGE_EMBEDDING_DIM
            }
            fn embed_image(&self, _: &image::RgbImage) -> Result<Vec<f32>> {
                unreachable!()
            }
            fn embed_query(&self, _: &str) -> Result<Vec<f32>> {
                Err(crate::error::Error::Engine("no session".into()))
            }
        }
        let vectors = QueryVectors::embed(Some(&FakeEmbedder), Some(&Broken), "harbor").unwrap();
        assert!(vectors.text.is_some() && vectors.image.is_none());
    }

    #[test]
    fn a_blank_query_is_not_embedded() {
        let (text, image) = Rendezvous::pair();
        // Its partner gone, the text tower fails at once if it is called.
        drop(image);
        let vectors = QueryVectors::embed(Some(&text), None, "   ").unwrap();
        assert_eq!(vectors, QueryVectors::default());
    }

    #[test]
    fn empty_query_or_zero_limit_returns_no_results() {
        let (_dir, mut conn) = open_test_db();
        index_text(&mut conn, "/roots/a/notes.txt", "hello world", 0);
        let embedder = FakeEmbedder;

        assert!(
            hybrid_search(&conn, Some(&embedder), None, "", 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            hybrid_search(&conn, Some(&embedder), None, "hello", 0)
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

        let hits = hybrid_search(&conn, Some(&embedder), None, "arepas", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/recipe.txt"));
        assert!(hits[0].match_sources.contains(&MatchSource::Keyword));
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

        let hits = hybrid_search(&conn, Some(&embedder), None, "cat sat mat", 10).unwrap();
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/cats.txt"));
        assert!(hits[0].match_sources.contains(&MatchSource::Keyword));
        assert!(hits[0].match_sources.contains(&MatchSource::Semantic));
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

        let hits = hybrid_search(&conn, Some(&embedder), None, "budget quarterly", 10).unwrap();
        assert_eq!(hits[0].path, PathBuf::from("/roots/a/budget_report.txt"));
    }

    #[test]
    fn the_best_chunks_source_and_page_reach_the_hit() {
        use MatchSource::*;
        let hit = |id: i64, source, page| FileHit {
            file_id: id,
            path: PathBuf::from(format!("/r/{id}")),
            file_name: format!("f{id}"),
            kind: Kind::Text,
            mtime_ns: 0,
            thumb_key: None,
            snippet: "s".into(),
            page,
            source: Some(source),
        };
        let hits = rank_and_boost(
            "zzz",
            &[
                hit(1, ChunkSource::Ocr, None),
                hit(2, ChunkSource::Body, Some(3)),
            ],
            &[hit(3, ChunkSource::Qr, None)],
            &[],
            10,
        );
        let by_id = |id| hits.iter().find(|h| h.file_id == id).unwrap();
        assert_eq!(by_id(1).match_sources, vec![Keyword, Ocr]);
        assert_eq!(
            (by_id(2).match_sources.clone(), by_id(2).page),
            (vec![Keyword], Some(3))
        );
        assert_eq!(by_id(3).match_sources, vec![Semantic, Qr]);
    }

    /// Every ranked list joins the file's row, so the Host never goes back
    /// to the database per result for what the UI shows.
    #[test]
    fn each_ranked_list_carries_the_files_kind_and_thumbnail() {
        use crate::embed::FakeImageEmbedder;
        let (_dir, mut conn) = open_test_db();
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
            mtime_ns: 7,
            lang: None,
            state: crate::db::files::FileState::Indexed,
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: Some("thumb42"),
            features_missing: 0,
        };
        let chunks = vec![RawChunk::body("harbor at dawn".to_string())];
        let text = FakeEmbedder.embed_passages(&["harbor at dawn"]).unwrap();
        let visual = FakeImageEmbedder.embed_query("harbor at dawn").unwrap();
        upsert_file(&mut conn, &record, &chunks, Some(&text), Some(&visual)).unwrap();

        let query = FakeEmbedder.embed_query("harbor at dawn").unwrap();
        let lists = [
            fts::search_fts(&conn, "harbor", 10).unwrap(),
            vector::search_vector_text(&conn, &query, 10).unwrap(),
            vector::search_vector_image(&conn, &visual, 10, IMAGE_MIN_COSINE).unwrap(),
        ];
        for hits in lists {
            let hit = &hits[0];
            assert_eq!(
                (hit.kind, hit.thumb_key.as_deref(), hit.mtime_ns),
                (Kind::Image, Some("thumb42"), 7),
                "{hit:?}"
            );
        }
    }
}
