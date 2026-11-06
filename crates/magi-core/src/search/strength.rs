//! Strong and weak results, as proposed by docs/issues/0003: a weak result
//! would be folded under "Weaker matches", and a list with no strong result
//! headed "Nothing closely related found". Measured by `magi-cli eval`, not
//! on the product path: no threshold separated the eval's `neg` queries from
//! weak right answers (docs/eval.md, 2026-11-05 "Precision").
//!
//! A result is strong when keyword search found it (every query word is in
//! the file, or its name), or when its cosine stands out from this query's
//! own distribution: `z = (cosine - mean) / deviation` over a fixed sample of
//! the table (the scale), with a stricter threshold for images, because
//! SigLIP puts a few photos above z = 2 for any text.

use rusqlite::{Connection, params};

use crate::dto::MatchSource;
use crate::embed::embedding_to_blob;
use crate::error::Result;
use crate::search::vector::{image_cosines, text_cosines};
use crate::search::{IMAGE_FETCH_LIMIT, QueryVectors, SearchHit, VECTOR_FETCH_LIMIT};

/// Vectors read per table to build a query's scale: every `step`-th row by
/// id, so the sample is fixed and spread over the whole table.
pub const SCALE_SAMPLE: u32 = 500;
/// A table with fewer sampled vectors than this has no scale: its results
/// count as strong, since nothing says they are not.
const MIN_SCALE_SAMPLE: usize = 20;
/// Floor on the deviation, so a table whose vectors all score alike does not
/// turn tiny differences into large z (as in `search::zfusion`).
const MIN_DEVIATION: f64 = 0.02;

/// Mean and deviation of one table's cosines for one query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub mean: f64,
    pub deviation: f64,
}

impl Scale {
    /// The scale of `cosines`, or `None` for fewer than
    /// [`MIN_SCALE_SAMPLE`].
    pub fn from_cosines(cosines: &[f64]) -> Option<Self> {
        if cosines.len() < MIN_SCALE_SAMPLE {
            return None;
        }
        let n = cosines.len() as f64;
        let mean = cosines.iter().sum::<f64>() / n;
        let variance = cosines.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / n;
        Some(Self {
            mean,
            deviation: variance.sqrt().max(MIN_DEVIATION),
        })
    }

    pub fn z(&self, cosine: f64) -> f64 {
        (cosine - self.mean) / self.deviation
    }
}

/// The query's scale on each vector table; `None` where the table is too
/// small or the query has no embedding for it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Scales {
    pub text: Option<Scale>,
    pub image: Option<Scale>,
}

impl Scales {
    /// The scale from a fixed sample of each table ([`SCALE_SAMPLE`] rows).
    pub fn sample(conn: &Connection, vectors: &QueryVectors) -> Result<Self> {
        let mut scales = Self::default();
        if let Some(e) = &vectors.text {
            scales.text = Scale::from_cosines(&sample_text_cosines(conn, e, SCALE_SAMPLE)?);
        }
        if let Some(e) = &vectors.image {
            scales.image = Scale::from_cosines(&sample_image_cosines(conn, e, SCALE_SAMPLE)?);
        }
        Ok(scales)
    }

    /// The scale from the nearest rows the RRF lists fetch (100 chunks, 50
    /// images): the alternative `magi-cli eval` measures it against.
    pub fn fetched(conn: &Connection, vectors: &QueryVectors) -> Result<Self> {
        let cosines = |rows: Vec<(crate::search::FileHit, f64)>| {
            rows.into_iter().map(|(_, c)| c).collect::<Vec<_>>()
        };
        let mut scales = Self::default();
        if let Some(e) = &vectors.text {
            scales.text = Scale::from_cosines(&cosines(text_cosines(conn, e, VECTOR_FETCH_LIMIT)?));
        }
        if let Some(e) = &vectors.image {
            scales.image =
                Scale::from_cosines(&cosines(image_cosines(conn, e, IMAGE_FETCH_LIMIT)?));
        }
        Ok(scales)
    }
}

/// Least z that makes a meaning-only result strong.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub text_z: f64,
    pub image_z: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Strong,
    Weak,
}

/// `hit`'s strength. Keyword evidence is always strong; otherwise the best
/// of its text and visual z decides. A list without a scale cannot judge,
/// so it does not call anything weak.
pub fn strength(hit: &SearchHit, scales: &Scales, thresholds: &Thresholds) -> Strength {
    if hit.match_sources.contains(&MatchSource::Keyword) {
        return Strength::Strong;
    }
    let clears = |cosine: Option<f64>, scale: Option<Scale>, threshold: f64| match scale {
        Some(scale) => cosine.is_some_and(|c| scale.z(c) >= threshold),
        None => cosine.is_some(),
    };
    if clears(hit.text_similarity, scales.text, thresholds.text_z)
        || clears(hit.image_similarity, scales.image, thresholds.image_z)
    {
        Strength::Strong
    } else {
        Strength::Weak
    }
}

/// Cosines of `embedding` against a fixed sample of about `n` rows of
/// `vec_text`.
pub fn sample_text_cosines(conn: &Connection, embedding: &[f32], n: u32) -> Result<Vec<f64>> {
    sample_cosines(conn, embedding, n, "vec_text", "chunk_id", "chunks")
}

/// Cosines of `embedding` against a fixed sample of about `n` rows of
/// `vec_image`.
pub fn sample_image_cosines(conn: &Connection, embedding: &[f32], n: u32) -> Result<Vec<f64>> {
    sample_cosines(conn, embedding, n, "vec_image", "file_id", "files")
}

/// Every `step`-th row by id, `step` chosen from the owning table's largest
/// id so about `n` rows are read. Not cheap: `vec0` still walks the whole
/// table, so on the 100k-chunk bench this adds ~125 ms per query
/// (docs/eval.md, 2026-11-05 "Precision"). A product version would cache
/// the sampled vectors instead of reading them per query.
fn sample_cosines(
    conn: &Connection,
    embedding: &[f32],
    n: u32,
    table: &str,
    id: &str,
    owner: &str,
) -> Result<Vec<f64>> {
    if embedding.is_empty() || n == 0 {
        return Ok(Vec::new());
    }
    let max_id: Option<i64> =
        conn.query_row(&format!("SELECT max(id) FROM {owner}"), [], |row| {
            row.get(0)
        })?;
    let Some(max_id) = max_id else {
        return Ok(Vec::new());
    };
    let step = (max_id / i64::from(n)).max(1);
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT vec_distance_l2(embedding, vec_f32(?1)) FROM {table}
         WHERE {id} % ?2 = 0 LIMIT ?3"
    ))?;
    let cosines = stmt
        .query_map(
            params![embedding_to_blob(embedding), step, i64::from(n) * 2],
            |row| row.get::<_, f64>(0),
        )?
        .map(|d| d.map(|d| 1.0 - d * d / 2.0))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(cosines)
}

#[cfg(test)]
mod tests;
