//! The single decode point for every image kind (SPEC.md §7 M4).
//!
//! Nothing else in the crate calls an image decoder: OCR, QR scanning,
//! visual embedding and thumbnails all consume the `RgbImage` this module
//! produces, so the decompression-bomb defence and the EXIF orientation fix
//! are applied exactly once, in one place.
//!
//! Dimensions are read from the header before anything is allocated, so an
//! image that declares more than `max_megapixels` is refused without a
//! decoder ever seeing it.

use std::io::Cursor;
use std::path::Path;

use image::{ImageDecoder, ImageReader, Limits};

use super::{ChunkSource, ExtractedDoc, RawChunk, heic, lang};
use crate::embed::ImageEmbedder;
use crate::error::{Error, Result};
use crate::ocr::OcrEngine;

/// Extra allocation a decoder may need beyond the output buffer: progressive
/// JPEG coefficient planes, PNG filter rows, and so on.
const DECODE_SLACK_BYTES: u64 = 64 * 1024 * 1024;

/// Long edge an image is downscaled to before OCR, per SPEC.md §5.3. Text
/// stops getting more legible well before a 12 MP photo's full resolution,
/// and the recognizer's cost scales with pixels.
const OCR_LONG_SIDE: u32 = 2048;

/// Prefixed to every decoded barcode payload so that SPEC.md §7 M4's two
/// required queries — `qr code` and `código QR` — both match the chunk,
/// whichever language the user searches in.
const QR_CHUNK_PREFIX: &str = "QR code / código QR: ";

/// Megapixels, rounded up, so the limit reads the way a user states it: a
/// 64 MP ceiling rejects anything over 64 million pixels.
pub fn megapixels(width: u32, height: u32) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    pixels.div_ceil(1_000_000).min(u64::from(u32::MAX)) as u32
}

/// Reads the image's dimensions from its header, decoding nothing.
pub fn probe_dimensions(path: &Path, bytes: &[u8]) -> Result<(u32, u32)> {
    if heic::is_heic(path, bytes) {
        return heic::probe_dimensions(bytes);
    }
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::Image(format!("unrecognized image format: {e}")))?
        .into_dimensions()
        .map_err(|e| Error::Image(e.to_string()))
}

/// Decodes to RGB8 with the image upright, refusing anything over
/// `max_megapixels`.
///
/// Orientation comes from wherever the format keeps it: the EXIF tag for
/// JPEG and TIFF, the container's `irot`/`imir` transform for HEIC. Callers
/// get an image they can hand straight to OCR or an embedder.
pub fn decode_bounded(path: &Path, bytes: &[u8], max_megapixels: u32) -> Result<image::RgbImage> {
    let (width, height) = probe_dimensions(path, bytes)?;
    let declared = megapixels(width, height);
    if declared > max_megapixels {
        return Err(Error::ImageTooLarge {
            megapixels: declared,
            limit: max_megapixels,
        });
    }

    if heic::is_heic(path, bytes) {
        return heic::decode_heic(bytes, max_megapixels);
    }

    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::Image(format!("unrecognized image format: {e}")))?;
    // A second bound, in case a container's header understates what the
    // decoder will actually ask for. Sized by the policy rather than by this
    // image, since the header has already been checked against the policy.
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(u64::from(max_megapixels) * 4_000_000 + DECODE_SLACK_BYTES);
    reader.limits(limits);

    let mut decoder = reader
        .into_decoder()
        .map_err(|e| Error::Image(e.to_string()))?;
    let orientation = decoder
        .orientation()
        .map_err(|e| Error::Image(e.to_string()))?;
    let mut decoded =
        image::DynamicImage::from_decoder(decoder).map_err(|e| Error::Image(e.to_string()))?;
    decoded.apply_orientation(orientation);
    Ok(decoded.into_rgb8())
}

/// OCR on a photo with no text still emits a stray glyph or two (`.`, `M`,
/// `00000`). Embedded alone, a chunk like that sits near almost any short
/// query and crowds real documents out of the text-vector list, so OCR output
/// with fewer letters and digits than this is dropped. Measured on the
/// fixture corpus, everything of 4-5 characters was junk but one partial read.
///
/// ponytail: real 4-5 letter text alone in a photo (a sign reading `EXIT`) is
/// dropped too. Lower this if that matters more than the crowding.
const MIN_OCR_ALNUM: usize = 6;

/// Everything derived from one image, with the full-resolution buffer
/// already dropped (SPEC.md §5.3).
pub struct ImageArtifacts {
    /// `ocr` and `qr` chunks, plus the language detected from the OCR text.
    pub doc: ExtractedDoc,
    /// Already downscaled to [`crate::thumbs::THUMB_LONG_SIDE`].
    pub thumbnail: image::RgbImage,
    /// The visual embedding, or `None` without an embedder or when embedding
    /// failed (the image is still searchable by its text and filename).
    pub image_embedding: Option<Vec<f32>>,
}

/// Decodes `bytes` once and derives everything from that single buffer:
/// barcode payloads, the visual embedding, recognized text, and the thumbnail.
///
/// The full-resolution image is dropped before OCR runs. Barcode
/// detection and the embedding run on it first ([`crate::qr::decode_barcodes`]
/// needs the detail, and the embedder resizes straight to its own input);
/// OCR gets a copy capped at [`OCR_LONG_SIDE`].
pub fn extract_image(
    path: &Path,
    bytes: &[u8],
    max_megapixels: u32,
    ocr: &dyn OcrEngine,
    embedder: Option<&dyn ImageEmbedder>,
) -> Result<ImageArtifacts> {
    let full = decode_bounded(path, bytes, max_megapixels)?;

    let mut chunks = Vec::new();
    for payload in crate::qr::decode_barcodes(&full) {
        chunks.push(RawChunk {
            source: ChunkSource::Qr,
            text: format!("{QR_CHUNK_PREFIX}{payload}"),
            page: None,
            line_start: None,
            line_end: None,
        });
    }

    // A failed embedding costs the file its visual match, never its entry.
    let image_embedding = embedder.and_then(|e| match e.embed_image(&full) {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "image embedding failed");
            None
        }
    });

    // One Lanczos pass over the full-resolution pixels (~0.55 s at 48 MP);
    // the thumbnail comes from that copy, and the full buffer is dropped
    // before OCR runs.
    let ocr_input = downscaled(&full, OCR_LONG_SIDE);
    let thumbnail = downscaled(&ocr_input, crate::thumbs::THUMB_LONG_SIDE);
    drop(full);

    // Same policy as the embedding: a failed OCR costs the file its text,
    // never the QR chunks and embedding already computed.
    let text = ocr.recognize(&ocr_input).unwrap_or_else(|err| {
        tracing::warn!(path = %path.display(), error = %err, "OCR failed");
        String::new()
    });
    let trimmed = text.trim();
    let trimmed = if trimmed.chars().filter(|c| c.is_alphanumeric()).count() < MIN_OCR_ALNUM {
        ""
    } else {
        trimmed
    };
    for piece in crate::chunk::chunk_text(trimmed) {
        chunks.push(RawChunk {
            source: ChunkSource::Ocr,
            text: piece,
            page: None,
            line_start: None,
            line_end: None,
        });
    }

    Ok(ImageArtifacts {
        doc: ExtractedDoc {
            chunks,
            lang: lang::detect_lang(trimmed),
        },
        thumbnail,
        image_embedding,
    })
}

/// A copy with its long edge at most `long_side`. Never upscales; an image
/// already small enough is copied as-is.
fn downscaled(image: &image::RgbImage, long_side: u32) -> image::RgbImage {
    let longest = image.width().max(image.height());
    if longest <= long_side {
        return image.clone();
    }
    let scale = f64::from(long_side) / f64::from(longest);
    image::imageops::resize(
        image,
        ((f64::from(image.width()) * scale).round() as u32).max(1),
        ((f64::from(image.height()) * scale).round() as u32).max(1),
        image::imageops::FilterType::Lanczos3,
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use super::*;

    fn corpus(rel: &str) -> (PathBuf, Vec<u8>) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus")
            .join(rel);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        (path, bytes)
    }

    #[test]
    fn megapixels_rounds_up_so_a_limit_reads_as_written() {
        assert_eq!(megapixels(4000, 3000), 12);
        assert_eq!(megapixels(1, 1), 1);
        assert_eq!(megapixels(8000, 8001), 65);
    }

    #[test]
    fn exif_orientation_is_applied_to_jpeg() {
        let (path, bytes) = corpus("edge/rotated_exif.jpg");
        // Stored landscape, tagged "rotate 90 CW", so it decodes portrait.
        assert_eq!(probe_dimensions(&path, &bytes).unwrap(), (64, 32));
        let image = decode_bounded(&path, &bytes, 64).unwrap();
        assert_eq!((image.width(), image.height()), (32, 64));
    }

    #[test]
    fn heic_is_routed_through_the_heic_decoder() {
        let (path, bytes) = corpus("images/phone_qr.heic");
        assert_eq!(probe_dimensions(&path, &bytes).unwrap(), (1834, 1546));
        let image = decode_bounded(&path, &bytes, 64).unwrap();
        assert_eq!((image.width(), image.height()), (1834, 1546));
    }

    #[test]
    fn decompression_bomb_is_refused_from_the_header() {
        let (path, bytes) = corpus("edge/bomb.png");
        let started = Instant::now();
        let error = decode_bounded(&path, &bytes, 64).unwrap_err();
        let elapsed = started.elapsed();

        assert!(
            matches!(error, Error::ImageTooLarge { limit: 64, .. }),
            "expected ImageTooLarge, got {error:?}"
        );
        // The header alone decides, so this cannot be slow. The RSS half of
        // SPEC.md §7 M4's budget follows from never allocating at all.
        assert!(
            elapsed.as_millis() < 250,
            "rejecting a bomb took {elapsed:?} — is it being decoded?"
        );
    }

    #[test]
    fn truncated_jpeg_is_a_clean_error_not_a_panic() {
        let (path, bytes) = corpus("edge/truncated.jpg");
        assert!(matches!(
            decode_bounded(&path, &bytes, 64),
            Err(Error::Image(_))
        ));
    }

    #[test]
    fn a_photographed_qr_becomes_a_chunk_both_required_queries_can_match() {
        let (path, bytes) = corpus("images/phone_qr.heic");
        let artifacts = extract_image(&path, &bytes, 64, &crate::ocr::NoOcr, None).unwrap();

        let qr: Vec<&str> = artifacts
            .doc
            .chunks
            .iter()
            .filter(|c| c.source == ChunkSource::Qr)
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(qr, ["QR code / código QR: https://www.cntindigena.org/"]);
        // SPEC.md §7 M4 requires both `qr code` and `código QR` to hit.
        assert!(qr[0].to_lowercase().contains("qr code"));
        assert!(qr[0].to_lowercase().contains("código qr"));

        // The thumbnail is already small: the full-resolution buffer must
        // not escape this function.
        assert_eq!(
            artifacts
                .thumbnail
                .width()
                .max(artifacts.thumbnail.height()),
            crate::thumbs::THUMB_LONG_SIDE
        );
    }

    #[test]
    fn without_an_ocr_engine_an_image_still_yields_its_thumbnail() {
        let (path, bytes) = corpus("images/mountain_sunset.jpg");
        let artifacts = extract_image(&path, &bytes, 64, &crate::ocr::NoOcr, None).unwrap();
        assert!(artifacts.doc.chunks.is_empty());
        assert!(artifacts.doc.lang.is_none());
        assert_eq!(artifacts.thumbnail.height(), 171); // 1920x1280 -> 256x171
    }

    #[test]
    fn stray_ocr_glyphs_from_a_photo_are_not_indexed_but_real_text_is() {
        struct FixedOcr(&'static str);
        impl OcrEngine for FixedOcr {
            fn engine_id(&self) -> &str {
                "fixed"
            }
            fn recognize(&self, _: &image::RgbImage) -> Result<String> {
                Ok(self.0.to_string())
            }
        }
        let (path, bytes) = corpus("images/mountain_sunset.jpg");
        let ocr_chunks = |text: &'static str| {
            extract_image(&path, &bytes, 64, &FixedOcr(text), None)
                .unwrap()
                .doc
                .chunks
                .iter()
                .filter(|c| c.source == ChunkSource::Ocr)
                .count()
        };
        // What OCR really returned for photos with no text in them (fewer than
        // six letters and digits); six or more is kept.
        for junk in [".", "M", "3 S59", "TUABO", "00000"] {
            assert_eq!(ocr_chunks(junk), 0, "{junk:?} should be dropped");
        }
        assert_eq!(ocr_chunks("Hello World!"), 1);
    }

    #[test]
    fn bytes_that_are_not_an_image_are_a_clean_error() {
        let path = Path::new("notes.png");
        assert!(matches!(
            probe_dimensions(path, b"just some text"),
            Err(Error::Image(_))
        ));
    }

    #[test]
    fn an_image_over_the_megapixel_limit_is_refused_through_the_heic_path_too() {
        let (path, bytes) = corpus("images/shelf_christmas.heic");
        // The fixture is 12 MP, so a 1 MP ceiling must reject it.
        assert!(matches!(
            decode_bounded(&path, &bytes, 1),
            Err(Error::ImageTooLarge {
                megapixels: 12,
                limit: 1
            })
        ));
    }

    #[test]
    fn a_failing_ocr_engine_keeps_the_qr_chunk_and_thumbnail() {
        struct FailingOcr;
        impl OcrEngine for FailingOcr {
            fn engine_id(&self) -> &str {
                "failing"
            }
            fn recognize(&self, _: &image::RgbImage) -> Result<String> {
                Err(Error::Model("boom".into()))
            }
        }
        let (path, bytes) = corpus("images/phone_qr.heic");
        let artifacts = extract_image(&path, &bytes, 64, &FailingOcr, None).unwrap();
        assert!(
            artifacts
                .doc
                .chunks
                .iter()
                .any(|c| c.source == ChunkSource::Qr)
        );
        assert_eq!(
            artifacts
                .thumbnail
                .width()
                .max(artifacts.thumbnail.height()),
            256
        );
    }
}
