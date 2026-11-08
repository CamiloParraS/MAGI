//! The real or fake models, chosen by `MAGI_FAKE_EMBEDDER`, for `eval`.

use magi_core::embed::{E5Embedder, FakeEmbedder, TextEmbedder};
use magi_core::embed::{FakeImageEmbedder, ImageEmbedder, SigLipEmbedder};
use magi_core::ocr::{NoOcr, OcrEngine, paddle::PaddleOcr};

pub(crate) fn embedder_from_env() -> anyhow::Result<std::sync::Arc<dyn TextEmbedder>> {
    if std::env::var("MAGI_FAKE_EMBEDDER").as_deref() == Ok("1") {
        Ok(std::sync::Arc::new(FakeEmbedder))
    } else {
        Ok(std::sync::Arc::new(E5Embedder::load().map_err(|e| {
            anyhow::anyhow!(
                "loading the real text embedder failed: {e}\n\
                 run `just models` and `cargo xtask fetch-onnxruntime` first, \
                 or set MAGI_FAKE_EMBEDDER=1 to index/search with the fake one"
            )
        })?))
    }
}

/// Real OCR unless the fake embedder is on (tests stay deterministic and
/// model-free). Missing OCR models are not fatal: images just get no
/// recognized text.
pub(crate) fn ocr_from_env() -> std::sync::Arc<dyn OcrEngine> {
    if std::env::var("MAGI_FAKE_EMBEDDER").as_deref() == Ok("1") {
        return std::sync::Arc::new(NoOcr);
    }
    match PaddleOcr::load() {
        Ok(ocr) => std::sync::Arc::new(ocr),
        Err(e) => {
            eprintln!("warning: OCR unavailable ({e}); images will be indexed without text");
            std::sync::Arc::new(NoOcr)
        }
    }
}

/// The fake one under `MAGI_FAKE_EMBEDDER=1`; otherwise SigLIP, which loads
/// nothing until the first image or query (and degrades if its models are
/// missing).
pub(crate) fn image_embedder_from_env() -> std::sync::Arc<dyn ImageEmbedder> {
    if std::env::var("MAGI_FAKE_EMBEDDER").as_deref() == Ok("1") {
        std::sync::Arc::new(FakeImageEmbedder)
    } else {
        std::sync::Arc::new(SigLipEmbedder::new())
    }
}
