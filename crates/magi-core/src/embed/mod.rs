//! `TextEmbedder` / `ImageEmbedder` traits and a deterministic `FakeEmbedder`
//! for tests. Implemented in M3 (text) and M4 (image) — see SPEC.md §7.

pub mod e5;
pub mod manager;
pub mod siglip;

pub use e5::E5Embedder;
pub use siglip::SigLipEmbedder;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::error::Result;

/// Dimensionality of `multilingual-e5-small` embeddings (SPEC.md §3).
pub const TEXT_EMBEDDING_DIM: usize = 384;

/// Dimensionality of SigLIP 2 base embeddings (SPEC.md section 3).
pub const IMAGE_EMBEDDING_DIM: usize = 768;

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
    /// Takes `&str` so callers never have to copy a file's text just to
    /// hand it over.
    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    /// Embeds a single search query.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}

/// Encodes images for `vec_image` and search queries into the same space
/// (SPEC.md section 5.6 step 2c). Both towers are lazy: an indexing run only
/// ever loads the image tower and a search only the text tower.
pub trait ImageEmbedder: Send + Sync {
    /// Stored in `meta.image_model_id`; includes the quantization so a
    /// variant change re-embeds, like a text model change does.
    fn model_id(&self) -> &str;
    fn dim(&self) -> usize;
    /// Embeds an upright decoded image. The caller may pass any size; the
    /// implementation resizes to the model's input.
    fn embed_image(&self, image: &image::RgbImage) -> Result<Vec<f32>>;
    /// Embeds a search query with the text tower.
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

/// Feature-hashes whitespace tokens into a unit vector of length `dim`.
fn hash_text(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for token in text.to_lowercase().split_whitespace() {
        let h = hash_token(token);
        let idx = (h % dim as u64) as usize;
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

/// Deterministic image embedder for tests: a 4x4 grid of coarse colours
/// hashed into the vector, so identical images match and different ones
/// don't. Queries are hashed text, so a query is *not* close to any image;
/// tests that need image-query ranking must plant vectors directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeImageEmbedder;

impl ImageEmbedder for FakeImageEmbedder {
    fn model_id(&self) -> &str {
        "fake-image-v1"
    }

    fn dim(&self) -> usize {
        IMAGE_EMBEDDING_DIM
    }

    fn embed_image(&self, image: &image::RgbImage) -> Result<Vec<f32>> {
        let (w, h) = image.dimensions();
        let mut cells = String::new();
        for gy in 0..4 {
            for gx in 0..4 {
                let px = image.get_pixel((gx * w / 4).min(w - 1), (gy * h / 4).min(h - 1));
                cells.push_str(&format!(
                    "{gx}{gy}{}{}{} ",
                    px[0] / 64,
                    px[1] / 64,
                    px[2] / 64
                ));
            }
        }
        Ok(hash_text(&cells, IMAGE_EMBEDDING_DIM))
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(hash_text(text, IMAGE_EMBEDDING_DIM))
    }
}

impl TextEmbedder for FakeEmbedder {
    fn model_id(&self) -> &str {
        "fake-v1"
    }

    fn dim(&self) -> usize {
        TEXT_EMBEDDING_DIM
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| hash_text(t, TEXT_EMBEDDING_DIM))
            .collect())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(hash_text(text, TEXT_EMBEDDING_DIM))
    }
}

/// Wraps a [`TextEmbedder`] and counts what goes through it, so a test can
/// prove that nothing was re-embedded (SPEC.md §7 M5's test instrumentation).
/// `with_model_id` overrides the reported id, to simulate a model change.
pub struct CountingEmbedder<E> {
    inner: E,
    model_id: Option<String>,
    calls: std::sync::atomic::AtomicUsize,
    chunks: std::sync::atomic::AtomicUsize,
}

impl<E> CountingEmbedder<E> {
    pub fn new(inner: E) -> Self {
        Self {
            inner,
            model_id: None,
            calls: Default::default(),
            chunks: Default::default(),
        }
    }

    pub fn with_model_id(mut self, id: impl Into<String>) -> Self {
        self.model_id = Some(id.into());
        self
    }

    /// `embed_passages` calls so far.
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Chunk texts embedded so far, across all calls.
    pub fn chunks(&self) -> usize {
        self.chunks.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl FakeEmbedder {
    /// A [`FakeEmbedder`] that counts its calls.
    pub fn counting() -> CountingEmbedder<FakeEmbedder> {
        CountingEmbedder::new(FakeEmbedder)
    }
}

impl<E: TextEmbedder> TextEmbedder for CountingEmbedder<E> {
    fn model_id(&self) -> &str {
        self.model_id
            .as_deref()
            .unwrap_or_else(|| self.inner.model_id())
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        use std::sync::atomic::Ordering::SeqCst;
        self.calls.fetch_add(1, SeqCst);
        self.chunks.fetch_add(texts.len(), SeqCst);
        self.inner.embed_passages(texts)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.inner.embed_query(text)
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
    fn counting_embedder_counts_calls_and_chunks_but_not_queries() {
        let counting = FakeEmbedder::counting();
        counting.embed_passages(&["a", "b"]).unwrap();
        counting.embed_passages(&["c"]).unwrap();
        counting.embed_query("q").unwrap();
        assert_eq!((counting.calls(), counting.chunks()), (2, 3));
        assert_eq!(counting.model_id(), "fake-v1");
        assert_eq!(FakeEmbedder::counting().with_model_id("x").model_id(), "x");
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
        let out = e.embed_passages(&["alpha", "beta"]).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], e.embed_query("alpha").unwrap());
        assert_eq!(out[1], e.embed_query("beta").unwrap());
    }

    #[test]
    fn fake_embedder_model_id_is_stable() {
        assert_eq!(FakeEmbedder.model_id(), "fake-v1");
    }
}
