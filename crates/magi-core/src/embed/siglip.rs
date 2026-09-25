//! SigLIP 2 image and text towers (`ort`), ADR-0007: 256 px, q4f16.
//!
//! Each tower is its own lazily loaded, idle-unloadable [`ModelSlot`]: an
//! indexing run only ever loads the vision tower (~80 MB) and a search only
//! the text tower (~450 MB plus its tokenizer), so they never share memory.

use std::sync::Mutex;
use std::time::Duration;

use image::RgbImage;
use image::imageops::{self, FilterType};
use ort::session::Session;
use ort::value::TensorRef;
use tokenizers::Tokenizer;

use super::manager::{ModelSlot, model_dir};
use super::{IMAGE_EMBEDDING_DIM, ImageEmbedder, l2_normalize};
use crate::error::{Error, Result};

/// The quantization is part of the id: vectors from another variant are close
/// but not identical, so changing it must trigger a re-embed.
const MODEL_ID: &str = "google/siglip2-base-patch16-256@q4f16";
/// Model input side. SigLIP resizes straight to a square (no crop, no aspect
/// preservation), so a wide photo is squashed, exactly as in training.
const INPUT_SIDE: u32 = 256;
/// The text tower's fixed sequence length; shorter queries are padded with
/// the pad id, longer ones are cut with the EOS kept on the end.
const TEXT_LEN: usize = 64;
const PAD_ID: i64 = 0;
const EOS_ID: i64 = 1;

struct TextTower {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

/// Real SigLIP 2 embedder. Constructing it loads nothing.
#[derive(Default)]
pub struct SigLipEmbedder {
    vision: ModelSlot<Mutex<Session>>,
    text: ModelSlot<TextTower>,
}

impl SigLipEmbedder {
    pub fn new() -> Self {
        Self::default()
    }
}

fn load_vision() -> Result<Mutex<Session>> {
    crate::onnx::init()?;
    let path = model_dir("image").join("vision_model.onnx");
    Ok(Mutex::new(crate::onnx::session(
        &path,
        crate::onnx::indexing_threads(),
        false,
    )?))
}

fn load_text() -> Result<TextTower> {
    crate::onnx::init()?;
    let dir = model_dir("image");
    // Session first: building it briefly takes ~2x the model file (~800 MB),
    // and the Gemma tokenizer (~64 MB resident) shouldn't sit on top of that
    // peak (NFR-12, docs/benchmarks.md).
    let session = Mutex::new(crate::onnx::session(
        &dir.join("text_model.onnx"),
        1,
        false,
    )?);
    let tokenizer_path = dir.join("tokenizer.json");
    let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
        Error::Model(format!(
            "loading tokenizer {} (run `just models` first): {e}",
            tokenizer_path.display()
        ))
    })?;
    Ok(TextTower { session, tokenizer })
}

/// Resizes to the model's square input and lays the pixels out as an NCHW
/// float tensor, RGB, scaled to [-1, 1] (mean = std = 0.5).
fn preprocess(image: &RgbImage) -> Vec<f32> {
    // `Triangle` is a bilinear filter that widens its support when shrinking,
    // which is what PIL's `BILINEAR` (the reference's resample = 2) does.
    let small = imageops::resize(image, INPUT_SIDE, INPUT_SIDE, FilterType::Triangle);
    let plane = (INPUT_SIDE * INPUT_SIDE) as usize;
    let mut data = vec![0f32; 3 * plane];
    for (i, px) in small.pixels().enumerate() {
        for c in 0..3 {
            data[c * plane + i] = px[c] as f32 / 127.5 - 1.0;
        }
    }
    data
}

/// Token ids for the text tower: the tokenizer's own output (which appends
/// EOS), cut to [`TEXT_LEN`] keeping EOS last, then padded.
fn text_ids(tokenizer: &Tokenizer, text: &str) -> Result<Vec<i64>> {
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|e| Error::Model(format!("tokenizing query: {e}")))?;
    let mut ids: Vec<i64> = encoding.get_ids().iter().map(|&i| i as i64).collect();
    if ids.len() > TEXT_LEN {
        ids.truncate(TEXT_LEN);
        ids[TEXT_LEN - 1] = EOS_ID;
    }
    ids.resize(TEXT_LEN, PAD_ID);
    Ok(ids)
}

/// Reads the L2-normalized `pooler_output` row out of a tower's outputs.
fn pooled(outputs: &ort::session::SessionOutputs<'_>) -> Result<Vec<f32>> {
    let pooled = outputs
        .get("pooler_output")
        .ok_or_else(|| Error::Model("model produced no pooler_output".to_string()))?
        .try_extract_array::<f32>()
        .map_err(|e| Error::Model(format!("extracting pooler_output: {e}")))?;
    let mut v: Vec<f32> = pooled.iter().copied().collect();
    if v.len() != IMAGE_EMBEDDING_DIM {
        return Err(Error::Model(format!(
            "expected a {IMAGE_EMBEDDING_DIM}-d embedding, got {}",
            v.len()
        )));
    }
    l2_normalize(&mut v);
    Ok(v)
}

fn lock(session: &Mutex<Session>) -> Result<std::sync::MutexGuard<'_, Session>> {
    session
        .lock()
        .map_err(|_| Error::Model("ONNX session lock poisoned".to_string()))
}

impl ImageEmbedder for SigLipEmbedder {
    fn model_id(&self) -> &str {
        MODEL_ID
    }

    fn dim(&self) -> usize {
        IMAGE_EMBEDDING_DIM
    }

    /// Drops whichever towers have been idle at least `idle`.
    fn unload_if_idle(&self, idle: Duration) {
        self.vision.unload_if_idle(idle);
        self.text.unload_if_idle(idle);
    }

    fn embed_image(&self, image: &RgbImage) -> Result<Vec<f32>> {
        let data = preprocess(image);
        let tower = self.vision.get_or_load(load_vision)?;
        let input = TensorRef::from_array_view((
            [1usize, 3, INPUT_SIDE as usize, INPUT_SIDE as usize],
            &*data,
        ))
        .map_err(|e| Error::Model(format!("building pixel_values tensor: {e}")))?;
        let mut session = lock(&tower)?;
        let outputs = session
            .run(ort::inputs! { "pixel_values" => input })
            .map_err(|e| Error::Model(format!("running vision tower: {e}")))?;
        pooled(&outputs)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let tower = self.text.get_or_load(load_text)?;
        let ids = text_ids(&tower.tokenizer, text)?;
        let input = TensorRef::from_array_view(([1usize, TEXT_LEN], &*ids))
            .map_err(|e| Error::Model(format!("building input_ids tensor: {e}")))?;
        let mut session = lock(&tower.session)?;
        let outputs = session
            .run(ort::inputs! { "input_ids" => input })
            .map_err(|e| Error::Model(format!("running text tower: {e}")))?;
        pooled(&outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn preprocess_scales_to_minus_one_one_in_nchw() {
        let img = RgbImage::from_pixel(10, 5, image::Rgb([255, 0, 128]));
        let data = preprocess(&img);
        let plane = (INPUT_SIDE * INPUT_SIDE) as usize;
        assert_eq!(data.len(), 3 * plane);
        assert!((data[0] - 1.0).abs() < 1e-6, "red plane");
        assert!((data[plane] + 1.0).abs() < 1e-6, "green plane");
        assert!(
            (data[2 * plane] - (128.0 / 127.5 - 1.0)).abs() < 1e-6,
            "blue plane"
        );
    }

    #[derive(serde::Deserialize)]
    struct Reference {
        images: Vec<NamedVec>,
        texts: Vec<TextVec>,
    }
    #[derive(serde::Deserialize)]
    struct NamedVec {
        name: String,
        embedding: Vec<f32>,
    }
    #[derive(serde::Deserialize)]
    struct TextVec {
        text: String,
        embedding: Vec<f32>,
    }

    /// ADR-0007 parity: both towers against the PyTorch fp32 reference.
    /// Needs `just models` and the vendored ONNX Runtime.
    #[test]
    #[ignore = "requires `just models` and `cargo xtask fetch-onnxruntime`"]
    fn towers_match_the_pytorch_reference() {
        let reference: Reference = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/reference_embeddings/siglip2.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let embedder = SigLipEmbedder::new();

        let mut worst_text = 1f32;
        for t in &reference.texts {
            let got = embedder.embed_query(&t.text).unwrap();
            let c = cosine(&got, &t.embedding);
            eprintln!("text  {c:.4}  {}", t.text);
            worst_text = worst_text.min(c);
        }
        let mut worst_image = 1f32;
        for i in &reference.images {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/corpus/images")
                .join(&i.name);
            let bytes = std::fs::read(&path).unwrap();
            let img = crate::extract::image::decode_bounded(&path, &bytes, 64).unwrap();
            let c = cosine(&embedder.embed_image(&img).unwrap(), &i.embedding);
            eprintln!("image {c:.4}  {}", i.name);
            worst_image = worst_image.min(c);
        }
        // ADR-0007: q4f16 measured 0.952 / 0.994 (image / text) minimum in
        // Python; the Rust preprocessing must not lose more than a hair.
        assert!(worst_text >= 0.99, "text parity {worst_text}");
        assert!(worst_image >= 0.95, "image parity {worst_image}");
    }
}
