//! Experimental z-score + word-coverage fusion, measured against RRF by
//! `magi-cli eval` (GitHub issue #9, docs/eval.md). Not on the product path:
//! `hybrid_search` still fuses with RRF.
//!
//! The idea comes from DigUp (`Sources/DigUpKit/Searcher.swift`): turn each
//! cosine into a z-score against this query's own distribution, so "how far
//! does this stand out" replaces "which rank is it", and replace the keyword
//! rank with the share of the query's words the file contains. A file is a
//! candidate only if it stands out (z >= gate) or holds half the words, so a
//! nonsense query can return nothing, which RRF never does.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::error::Result;
use crate::search::vector::{MAX_KNN_K, image_cosines, text_cosines};
use crate::search::{FTS_FETCH_LIMIT, FileHit, QueryVectors, SearchHit, boost_and_assemble, fts};

/// The knobs of [`fuse`]. Defaults are DigUp's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZFusionParams {
    /// Least z that counts as a meaning match.
    pub z_gate: f64,
    pub z_weight: f64,
    pub word_weight: f64,
    /// Least share of the query's words that makes a file a candidate.
    pub coverage_gate: f64,
    /// Images below this cosine are no meaning match, whatever their z: the
    /// floor the RRF path puts on the visual list (`IMAGE_MIN_COSINE`).
    /// `None` is DigUp's behaviour. They still count toward the scale.
    pub image_min_cosine: Option<f32>,
    /// Coverage under `coverage_gate` adds nothing to the score (DigUp adds
    /// it to any candidate).
    pub words_need_gate: bool,
}

impl Default for ZFusionParams {
    fn default() -> Self {
        Self {
            z_gate: 2.0,
            z_weight: 1.0,
            word_weight: 2.0,
            coverage_gate: 0.5,
            image_min_cosine: None,
            words_need_gate: false,
        }
    }
}

/// Which vector table a cosine came from: their cosines sit at different
/// levels for the same query (e5 text ~0.8, SigLIP images ~0.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modality {
    Text,
    Visual,
}

/// A modality with fewer vectors than this is centered on the overall
/// average: its own would be noise.
const MIN_GROUP: usize = 20;
/// Floor on the deviation, so a query whose scores are all alike does not
/// blow tiny differences up into large z.
const MIN_DEVIATION: f64 = 0.02;

/// Each cosine minus its modality's average, in standard deviations of all
/// the centered scores together (one shared scale, so within a modality the
/// order is the model's own). DigUp's `Searcher.meaning`.
pub fn meaning_z(scores: &[(Modality, f64)]) -> Vec<f64> {
    if scores.is_empty() {
        return Vec::new();
    }
    let n = scores.len() as f64;
    let overall = scores.iter().map(|(_, s)| s).sum::<f64>() / n;
    let mut groups: HashMap<Modality, (f64, usize)> = HashMap::new();
    for &(modality, score) in scores {
        let group = groups.entry(modality).or_default();
        group.0 += score;
        group.1 += 1;
    }
    let mean_of = |modality: Modality| match groups.get(&modality) {
        Some(&(sum, count)) if count >= MIN_GROUP => sum / count as f64,
        _ => overall,
    };
    let centered: Vec<f64> = scores.iter().map(|&(m, s)| s - mean_of(m)).collect();
    let mean = centered.iter().sum::<f64>() / n;
    let variance = centered.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / n;
    let deviation = variance.sqrt().max(MIN_DEVIATION);
    centered.iter().map(|c| (c - mean) / deviation).collect()
}

/// Function words left out of word coverage: they are in nearly every file,
/// so "a dog on the beach" would cover half its words anywhere. English
/// from DigUp's list, Spanish to match. Kind words ("photo", "pdf") are
/// kept: DigUp drops them only because it scores the kind separately.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "at", "to", "for", "with", "and", "or", "my", "me", "that",
    "this", "these", "those", "from", "about", "where", "what", "which", "who", "when", "is",
    "was", "are", "were", "be", "it", "its", "some", "any", //
    "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del", "al", "en", "y", "o",
    "con", "para", "por", "mi", "mis", "que", "este", "esta", "estos", "estas", "ese", "esa",
    "donde", "cual", "quien", "cuando", "es", "era", "son", "lo", "su", "sus",
];

/// The query's content words as keyword search sees them: accents dropped,
/// lowercased, split on anything not alphanumeric (as FTS5's `unicode61`
/// does, so no camelCase split), stopwords and one-character words out,
/// each word once.
pub fn query_terms(query: &str) -> Vec<String> {
    let folded: String = query.nfd().filter(|&c| !is_combining_mark(c)).collect();
    let mut terms: Vec<String> = Vec::new();
    for word in folded.split(|c: char| !c.is_alphanumeric()) {
        let word = word.to_lowercase();
        if word.chars().count() >= 2
            && !STOPWORDS.contains(&word.as_str())
            && !terms.contains(&word)
        {
            terms.push(word);
        }
    }
    terms
}

/// A query made only of codes (invoice numbers, error codes, part numbers):
/// every term has a digit. Embeddings know nothing of these, so a lookup
/// weighs words up and meaning down.
pub fn is_lookup(terms: &[String]) -> bool {
    !terms.is_empty() && terms.iter().all(|t| t.chars().any(|c| c.is_numeric()))
}

/// Scores the candidates: `z_weight * max(z, 0) + word_weight * coverage^1.5`
/// (z weight halved and word weight doubled for a lookup). A file is a
/// candidate when its best z reaches `z_gate` or its coverage reaches
/// `coverage_gate`. Sorted best first, ties by file id.
pub fn fuse(
    z: &HashMap<i64, f64>,
    coverage: &HashMap<i64, f64>,
    lookup: bool,
    params: &ZFusionParams,
) -> Vec<(i64, f64)> {
    let (z_weight, word_weight) = if lookup {
        (params.z_weight * 0.5, params.word_weight * 2.0)
    } else {
        (params.z_weight, params.word_weight)
    };
    let mut candidates: Vec<i64> = z
        .iter()
        .filter(|(_, z)| **z >= params.z_gate)
        .map(|(id, _)| *id)
        .chain(
            coverage
                .iter()
                .filter(|(_, c)| **c >= params.coverage_gate)
                .map(|(id, _)| *id),
        )
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    let mut ranked: Vec<(i64, f64)> = candidates
        .into_iter()
        .map(|id| {
            let meaning = z.get(&id).copied().unwrap_or(0.0).max(0.0);
            let share = coverage.get(&id).copied().unwrap_or(0.0);
            let words = if params.words_need_gate && share < params.coverage_gate {
                0.0
            } else {
                share.powf(1.5)
            };
            (id, z_weight * meaning + word_weight * words)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

/// How many of `scores` (best first) score at least `ratio` times the
/// first: the rest would fold under "weaker matches". A non-positive top
/// score folds nothing.
pub fn strong_count(scores: &[f64], ratio: f64) -> usize {
    match scores.first() {
        Some(&top) if top > 0.0 => scores.iter().take_while(|&&s| s >= top * ratio).count(),
        _ => scores.len(),
    }
}

/// Vectors scored per table to build the distribution. sqlite-vec's KNN
/// cap, so a corpus over 4,096 chunks is scaled from its nearest 4,096
/// only; the eval corpora are far below it. A product version would need a
/// full scan, as DigUp does.
pub const SCAN_K: u32 = MAX_KNN_K;

/// The z-score counterpart of [`crate::search::search_with`]: same inputs,
/// same filename/recency boosts and hit shape, different fusion.
pub fn search_zscore(
    conn: &Connection,
    vectors: &QueryVectors,
    query: &str,
    limit: u32,
    params: &ZFusionParams,
) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let text = match &vectors.text {
        Some(embedding) => text_cosines(conn, embedding, SCAN_K)?,
        None => Vec::new(),
    };
    let image = match &vectors.image {
        Some(embedding) => image_cosines(conn, embedding, SCAN_K)?,
        None => Vec::new(),
    };
    let scores: Vec<(Modality, f64)> = text
        .iter()
        .map(|(_, c)| (Modality::Text, *c))
        .chain(image.iter().map(|(_, c)| (Modality::Visual, *c)))
        .collect();
    let z = meaning_z(&scores);
    let (text_z, image_z) = z.split_at(text.len());
    let best_text = best_per_file(text.iter().map(|(h, _)| h).zip(text_z));
    let floor = params.image_min_cosine.map_or(f64::NEG_INFINITY, f64::from);
    let best_image = best_per_file(
        image
            .iter()
            .zip(image_z)
            .filter(|((_, cosine), _)| *cosine >= floor)
            .map(|((hit, _), z)| (hit, z)),
    );

    let mut z_by_file: HashMap<i64, f64> = HashMap::new();
    for (id, (z, _)) in best_text.iter().chain(&best_image) {
        let best = z_by_file.entry(*id).or_insert(f64::NEG_INFINITY);
        *best = best.max(*z);
    }

    let terms = query_terms(query);
    let mut coverage: HashMap<i64, f64> = HashMap::new();
    for (i, term) in terms.iter().enumerate() {
        // The last word is a prefix, as in the keyword list.
        for id in fts::files_with_term(conn, term, i + 1 == terms.len())? {
            *coverage.entry(id).or_default() += 1.0 / terms.len() as f64;
        }
    }

    let fused = fuse(&z_by_file, &coverage, is_lookup(&terms), params);
    let fts_hits = fts::search_fts(conn, query, FTS_FETCH_LIMIT)?;
    let gated = |best: &HashMap<i64, (f64, &FileHit)>| -> Vec<FileHit> {
        best.values()
            .filter(|(z, _)| *z >= params.z_gate)
            .map(|(_, h)| (*h).clone())
            .collect()
    };
    let mut fallback: Vec<FileHit> = best_text
        .values()
        .chain(best_image.values())
        .map(|(_, h)| (*h).clone())
        .collect();
    // Found by its words alone and in no vector list (no text embedder, or
    // past `SCAN_K`): read its row so it is not dropped.
    for (id, _) in fused.iter().take(limit as usize) {
        let known = fts_hits.iter().chain(&fallback).any(|h| h.file_id == *id);
        if !known && let Some(hit) = file_hit(conn, *id)? {
            fallback.push(hit);
        }
    }
    Ok(boost_and_assemble(
        query,
        fused,
        &fts_hits,
        &gated(&best_text),
        &gated(&best_image),
        &fallback,
        limit,
    ))
}

/// `file_id`'s row as a hit with no matched chunk (the file name stands in
/// for the snippet, as for a visual match).
fn file_hit(conn: &Connection, file_id: i64) -> Result<Option<FileHit>> {
    let mut stmt = conn.prepare_cached(
        "SELECT path, file_name, mtime_ns, kind, thumb_key FROM files WHERE id = ?1",
    )?;
    let hit = stmt
        .query_row([file_id], |row| {
            let file_name: String = row.get(1)?;
            Ok(FileHit {
                file_id,
                path: PathBuf::from(row.get::<_, String>(0)?),
                snippet: file_name.clone(),
                file_name,
                kind: row.get(3)?,
                mtime_ns: row.get(2)?,
                thumb_key: row.get(4)?,
                page: None,
                source: None,
            })
        })
        .optional()?;
    Ok(hit)
}

/// Each file's highest-z row.
fn best_per_file<'a>(
    rows: impl Iterator<Item = (&'a FileHit, &'a f64)>,
) -> HashMap<i64, (f64, &'a FileHit)> {
    let mut best: HashMap<i64, (f64, &FileHit)> = HashMap::new();
    for (hit, &z) in rows {
        match best.get(&hit.file_id) {
            Some((current, _)) if *current >= z => {}
            _ => {
                best.insert(hit.file_id, (z, hit));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests;
