//! Hybrid search orchestration (FTS + text vector + image vector, fused
//! with RRF), per SPEC.md §5.6.

mod copies;
pub mod fts;
pub mod fuse;
mod opaque;
pub mod rerank;
pub mod strength;
pub mod vector;
pub mod zfusion;

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::Connection;

use crate::discovery::Kind;
use crate::dto::MatchSource;
use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::Result;
use crate::extract::ChunkSource;
use crate::search::fuse::{
    RankedList, filename_boost, match_words, recency_boost, reciprocal_rank_fusion,
};

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
/// A visual match already passed `IMAGE_MIN_COSINE`, so its list counts a
/// touch more than text: at 0.8 a rank-1 image lost to text-vector rank ~17
/// and fell out of the top 10. Swept on realistic_names: 1.1 recovers all
/// of the gain (hybrid recall@5 0.908 -> 0.939); 1.5+ adds nothing and
/// pushes unrelated images above text hits (en/es MRR drops).
const IMAGE_WEIGHT: f64 = 1.1;

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
    /// Cosine from the text-vector list, when it found the file.
    pub text_similarity: Option<f64>,
    /// Cosine from the visual list, when it found the file.
    pub image_similarity: Option<f64>,
    /// Other indexed files with the same bytes, folded into this hit.
    pub copies: Vec<PathBuf>,
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
    /// Cosine of the query against the best chunk (text list) or the image
    /// (visual list); `None` for a keyword hit.
    pub similarity: Option<f64>,
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

    boost_and_assemble(query, fused, fts_hits, vector_hits, image_hits, &[], limit)
}

/// SPEC.md §5.6 step 5 and the hit assembly, after any fusion: applies the
/// filename/recency boosts to each `(file_id, score)` in `fused`, builds its
/// [`SearchHit`] from the list(s) it came from, and returns the top `limit`.
/// `fallback` supplies the file row for a fused id in none of the three
/// lists (it adds no match source); `fused` ids found nowhere are dropped.
pub(crate) fn boost_and_assemble(
    query: &str,
    fused: Vec<(i64, f64)>,
    fts_hits: &[FileHit],
    vector_hits: &[FileHit],
    image_hits: &[FileHit],
    fallback: &[FileHit],
    limit: u32,
) -> Vec<SearchHit> {
    let query_tokens = match_words(query);
    let now = now_unix();
    let fallback: HashMap<i64, &FileHit> = fallback.iter().map(|h| (h.file_id, h)).collect();

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

    // Under RRF `fused`'s ids are the union of the lists, so every lookup
    // here resolves. Preferring the FTS side keeps the snippet that carries
    // the match highlights, and the visual side (a bare file name) is the
    // last resort before `fallback`.
    let mut hits: Vec<SearchHit> = fused
        .into_iter()
        .filter_map(|(file_id, base_score)| {
            let [fts, vector, image] = by_id.get(&file_id).copied().unwrap_or_default();
            let hit = fts
                .or(vector)
                .or(image)
                .or_else(|| fallback.get(&file_id).copied())?;
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
                // The name chunk only says the name matched when the keyword
                // list found it; by vector alone it is just similar, and
                // "Related" already says so.
                Some(ChunkSource::Filename) if fts.is_some() => {
                    match_sources.push(MatchSource::Filename)
                }
                Some(ChunkSource::Filename | ChunkSource::Body | ChunkSource::CodeSymbol)
                | None => {}
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
                text_similarity: vector.and_then(|h| h.similarity),
                image_similarity: image.and_then(|h| h.similarity),
                copies: Vec::new(),
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
    /// Embeds `query` with the text tower, then the visual one. A missing or
    /// broken visual model degrades to text-only search rather than failing
    /// the query.
    ///
    /// One after the other on purpose (NFR-12): a cold query builds each
    /// tower's session, and SigLIP's briefly takes ~800 MB. In parallel a
    /// cold search peaked at 1,206 MB instead of 844 and saved ~7 ms warm
    /// (docs/benchmarks.md).
    pub fn embed(
        text: Option<&dyn TextEmbedder>,
        image: Option<&dyn ImageEmbedder>,
        query: &str,
    ) -> Result<Self> {
        if query.trim().is_empty() {
            return Ok(Self::default());
        }
        let text = text.map(|e| e.embed_query(query)).transpose()?;
        let image = match image.map(|e| e.embed_query(query)) {
            Some(Ok(embedding)) => Some(embedding),
            Some(Err(e)) => {
                tracing::warn!(error = %e, "visual search unavailable");
                None
            }
            None => None,
        };
        Ok(Self { text, image })
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
/// ponytail: the SQL runs sequentially on one connection, after the query
/// is embedded. A second reader connection
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

    // Extra hits, so collapsing copies still fills the list.
    let ranked = rank_and_boost(
        query,
        &fts_hits,
        &vector_hits,
        &image_hits,
        limit.saturating_mul(2),
    );
    copies::collapse(conn, ranked, limit)
}

#[cfg(test)]
mod tests;
