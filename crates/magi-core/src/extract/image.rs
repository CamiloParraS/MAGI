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

/// The declared size of an image, from its header. A decompression bomb is
/// caught here, before any decoder allocates for it.
pub fn probe_megapixels(path: &Path, bytes: &[u8]) -> Result<u32> {
    let (width, height) = probe_dimensions(path, bytes)?;
    Ok(megapixels(width, height))
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

/// Everything derived from one image, with the full-resolution buffer
/// already dropped (SPEC.md §5.3).
pub struct ImageArtifacts {
    /// `ocr` and `qr` chunks, plus the language detected from the OCR text.
    pub doc: ExtractedDoc,
    /// Already downscaled to [`crate::thumbs::THUMB_LONG_SIDE`].
    pub thumbnail: image::RgbImage,
}

/// Decodes `bytes` once and derives everything from that single buffer:
/// barcode payloads, recognized text, and the thumbnail.
///
/// The full-resolution image is dropped before this returns. Barcode
/// detection runs on it first, since [`crate::qr::decode_barcodes`] needs
/// the detail; OCR gets a copy capped at [`OCR_LONG_SIDE`].
pub fn extract_image(
    path: &Path,
    bytes: &[u8],
    max_megapixels: u32,
    ocr: &dyn OcrEngine,
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

    let text = ocr.recognize(&downscaled(&full, OCR_LONG_SIDE))?;
    let trimmed = text.trim();
    for piece in crate::chunk::chunk_text(trimmed) {
        chunks.push(RawChunk {
            source: ChunkSource::Ocr,
            text: piece,
            page: None,
            line_start: None,
            line_end: None,
        });
    }

    let thumbnail = downscaled(&full, crate::thumbs::THUMB_LONG_SIDE);
    drop(full);

    Ok(ImageArtifacts {
        doc: ExtractedDoc {
            chunks,
            lang: lang::detect_lang(trimmed),
        },
        thumbnail,
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
        let artifacts = extract_image(&path, &bytes, 64, &crate::ocr::NoOcr).unwrap();

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
        let artifacts = extract_image(&path, &bytes, 64, &crate::ocr::NoOcr).unwrap();
        assert!(artifacts.doc.chunks.is_empty());
        assert!(artifacts.doc.lang.is_none());
        assert_eq!(artifacts.thumbnail.height(), 171); // 1920x1280 -> 256x171
    }

    #[test]
    fn bytes_that_are_not_an_image_are_a_clean_error() {
        let path = Path::new("notes.png");
        assert!(matches!(
            probe_megapixels(path, b"just some text"),
            Err(Error::Image(_))
        ));
    }
}
