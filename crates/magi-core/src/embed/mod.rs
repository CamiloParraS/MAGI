//! `TextEmbedder` / `ImageEmbedder` traits and a deterministic `FakeEmbedder`
//! for tests. Implemented in M3 (text) and M4 (image) — see SPEC.md §7.

pub mod e5;
pub mod manager;
pub mod siglip;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::error::Result;

/// Dimensionality of `multilingual-e5-small` embeddings (SPEC.md §3).
pub const TEXT_EMBEDDING_DIM: usize = 384;

/// Serializes an embedding as the JSON array text `vec_f32()` parses
/// (sqlite-vec's documented insertion format — see
/// https://github.com/asg017/sqlite-vec/blob/v0.1.9/site/features/knn.md).
/// Shared by `db::files::upsert_file` (writes) and
/// `search::vector::search_vector_text` (reads the query embedding) so the
/// two sides of that round trip can't drift apart.
///
/// ponytail: text (de)serialization of ~384 floats per chunk is simpler
/// and safer than hand-packing the raw little-endian blob `vec0` expects,
/// at the cost of some CPU on inserts/queries. Switch to binding the raw
/// bytes directly if indexing/search throughput profiling ever points here.
pub(crate) fn embedding_to_json(embedding: &[f32]) -> String {
    let mut s = String::with_capacity(embedding.len() * 12 + 2);
    s.push('[');
    for (i, x) in embedding.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&x.to_string());
    }
    s.push(']');
    s
}

/// Encodes text into vectors for `vec_text` (SPEC.md §5.6). Implementations
/// apply their own query/passage prefixing internally (e5 requires
/// `"query: "` / `"passage: "`, per SPEC.md §3).
pub trait TextEmbedder: Send + Sync {
    /// Identifier stored in `meta.text_model_id`, used to detect a model
    /// change so affected files can be re-embedded (SPEC.md §7 M3). The
    /// re-embed *trigger* itself needs M5's scheduler; this slice only
    /// records the id.
    fn model_id(&self) -> &str;
    fn dim(&self) -> usize;
    /// Embeds chunk texts for storage. One vector per input, same order.
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    /// Embeds a single search query.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn hash_token(token: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

/// Deterministic embedder for tests (SPEC.md §4.5 `MAGI_FAKE_EMBEDDER`).
/// Uses feature hashing: each lowercased whitespace token hashes to a
/// dimension and sign, so texts sharing vocabulary land closer together
/// than unrelated texts — enough geometric structure for hybrid-search
/// tests to exercise real ranking behavior without a model.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeEmbedder;

impl FakeEmbedder {
    fn hash_text(text: &str) -> Vec<f32> {
        let mut v = vec![0f32; TEXT_EMBEDDING_DIM];
        for token in text.to_lowercase().split_whitespace() {
            let h = hash_token(token);
            let idx = (h % TEXT_EMBEDDING_DIM as u64) as usize;
            let sign = if (h >> 32).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            v[idx] += sign;
        }
        l2_normalize(&mut v);
        v
    }
}

impl TextEmbedder for FakeEmbedder {
    fn model_id(&self) -> &str {
        "fake-v1"
    }

    fn dim(&self) -> usize {
        TEXT_EMBEDDING_DIM
    }

    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| Self::hash_text(t)).collect())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(Self::hash_text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
    }

    #[test]
    fn fake_embedder_is_deterministic() {
        let e = FakeEmbedder;
        let a = e.embed_query("hello world").unwrap();
        let b = e.embed_query("hello world").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn fake_embedder_vectors_are_l2_normalized() {
        let e = FakeEmbedder;
        let v = e.embed_query("the quick brown fox").unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }

    #[test]
    fn fake_embedder_dim_matches_e5() {
        assert_eq!(FakeEmbedder.dim(), TEXT_EMBEDDING_DIM);
        assert_eq!(
            FakeEmbedder.embed_query("x").unwrap().len(),
            TEXT_EMBEDDING_DIM
        );
    }

    #[test]
    fn fake_embedder_similar_texts_are_closer_than_unrelated() {
        let e = FakeEmbedder;
        let a = e.embed_query("cat sat on the mat").unwrap();
        let b = e.embed_query("cat sat on a mat").unwrap();
        let c = e.embed_query("stock market rallied today").unwrap();
        assert!(cosine(&a, &b) > cosine(&a, &c));
    }

    #[test]
    fn fake_embedder_empty_text_yields_zero_vector_without_panic() {
        let v = FakeEmbedder.embed_query("").unwrap();
        assert_eq!(v, vec![0.0_f32; TEXT_EMBEDDING_DIM]);
    }

    #[test]
    fn fake_embedder_embeds_passages_in_order() {
        let e = FakeEmbedder;
        let texts = vec!["alpha".to_string(), "beta".to_string()];
        let out = e.embed_passages(&texts).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], e.embed_query("alpha").unwrap());
        assert_eq!(out[1], e.embed_query("beta").unwrap());
    }

    #[test]
    fn fake_embedder_model_id_is_stable() {
        assert_eq!(FakeEmbedder.model_id(), "fake-v1");
    }
}
