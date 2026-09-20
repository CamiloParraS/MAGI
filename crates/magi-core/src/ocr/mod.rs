//! The OCR seam (SPEC.md §7 M4).
//!
//! The engine is PaddleOCR-ONNX (`paddle`, ADR-0006), chosen by the M4 OCR
//! spike and ADR-0006. Everything upstream of that decision codes against
//! this trait, so the losing candidate is never implemented and the winner
//! drops in without touching the pipeline.

pub mod paddle;

/// Recognizes text in an already-decoded, upright image.
///
/// Implementations are shared across indexing threads, so they must be
/// `Sync`; the model itself lives behind whatever locking the engine needs.
pub trait OcrEngine: Send + Sync {
    /// Stored in `meta.ocr_engine_id`. A change re-queues image files in M5,
    /// the same way a changed embedding model does.
    fn engine_id(&self) -> &str;

    /// Recognized text, one line per detected line, in reading order. An
    /// image with no text returns an empty string — that is not an error.
    fn recognize(&self, image: &image::RgbImage) -> crate::error::Result<String>;
}

/// Used until ADR-0006's engine lands, and afterwards whenever the OCR
/// models are absent — an image still gets its QR payloads, thumbnail and
/// filename chunk, it just has no recognized text.
pub struct NoOcr;

impl OcrEngine for NoOcr {
    fn engine_id(&self) -> &str {
        "none"
    }

    fn recognize(&self, _image: &image::RgbImage) -> crate::error::Result<String> {
        Ok(String::new())
    }
}
