//! QR decoding (SPEC.md §7 M4).
//!
//! Takes pixels rather than a path: `extract::image` has already decoded and
//! straightened the image, and decoding it a second time inside `rxing`
//! would double both the work and the memory.

use image::buffer::ConvertBuffer;
use rxing::common::HybridBinarizer;
use rxing::multi::{GenericMultipleBarcodeReader, MultipleBarcodeReader};
use rxing::{BinaryBitmap, DecodeHints, Luma8Source, MultiUseMultiFormatReader};

/// Downscale factors tried, in order, until a QR decodes.
///
/// A photographed QR fails to detect at full phone resolution and decodes
/// once shrunk: the committed JPEG fixture needs 1/2, the HEIC 1/4. Native
/// resolution comes first because screenshots and generated QR images — the
/// common case — decode there immediately, at zero extra cost.
///
/// ponytail: a retry ladder, not a fix. The real cause is that the detector
/// and binarizer are tuned for a module size far smaller than a 12 MP photo
/// produces; estimating the module size once and resampling to it would be
/// one pass instead of up to four. Worth doing if QR decoding shows up in
/// the indexing profile.
const SCALE_LADDER: &[u32] = &[1, 2, 4, 8];

/// Below this, a downscale has destroyed whatever it was going to find.
const MIN_SCANNED_EDGE: u32 = 100;

/// Every QR payload in the image, deduplicated, in detection order.
pub fn decode_barcodes(image: &image::RgbImage) -> Vec<String> {
    for &scale in SCALE_LADDER {
        let (width, height) = (image.width() / scale, image.height() / scale);
        if width.min(height) < MIN_SCANNED_EDGE {
            break;
        }
        let payloads = if scale == 1 {
            decode_at(image)
        } else {
            decode_at(&image::imageops::resize(
                image,
                width,
                height,
                image::imageops::FilterType::Triangle,
            ))
        };
        // The same code at a different scale yields the same payload, so the
        // first scale that reads anything is the answer.
        if !payloads.is_empty() {
            return payloads;
        }
    }
    Vec::new()
}

fn decode_at(image: &image::RgbImage) -> Vec<String> {
    let luma: image::GrayImage = image.convert();
    let Ok(source) = Luma8Source::new_with_slice(luma.as_raw(), image.width(), image.height())
    else {
        return Vec::new();
    };
    let mut bitmap = BinaryBitmap::new(HybridBinarizer::new(source));
    let mut reader = GenericMultipleBarcodeReader::new(MultiUseMultiFormatReader::default());

    // No barcode is the common case, not an error.
    let Ok(results) = reader.decode_multiple_with_hints(&mut bitmap, &DecodeHints::default())
    else {
        return Vec::new();
    };

    let mut payloads: Vec<String> = Vec::new();
    for result in results {
        let text = result.getText().to_string();
        if !text.is_empty() && !payloads.contains(&text) {
            payloads.push(text);
        }
    }
    payloads
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn decode_fixture(rel: &str) -> Vec<String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus")
            .join(rel);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let image = crate::extract::image::decode_bounded(&path, &bytes, 64).unwrap();
        decode_barcodes(&image)
    }

    #[test]
    fn generated_qr_codes_decode_to_their_exact_payloads() {
        assert_eq!(
            decode_fixture("qr/qr_url.png"),
            vec!["https://example.com/magi-test".to_string()]
        );
        assert_eq!(
            decode_fixture("qr/qr_text_es.png"),
            vec!["código QR de prueba".to_string()]
        );
    }

    #[test]
    fn a_photographed_qr_code_decodes_the_same_from_heic_and_jpeg() {
        let from_heic = decode_fixture("images/iphone_qr.heic");
        let from_jpeg = decode_fixture("images/iphone_qr.jpg");
        assert_eq!(from_heic, vec!["https://www.cntindigena.org/".to_string()]);
        assert_eq!(from_heic, from_jpeg);
    }

    #[test]
    fn an_image_with_no_barcode_yields_nothing() {
        assert!(decode_fixture("images/mountain_sunset.jpg").is_empty());
    }
}
