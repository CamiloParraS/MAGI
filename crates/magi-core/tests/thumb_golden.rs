//! SPEC.md §7 M4: "Orientation is correct in thumbnails (golden thumbnail
//! comparison)". The HEIC decoder applies the container's `irot`/`imir`
//! transform, and a unit test already checks the decoded *dimensions*, but a
//! 180-degree rotation or a mirror keeps the dimensions. These goldens are
//! the pixels.
//!
//! Goldens live in `fixtures/golden/thumbs/` as JPEGs and were checked by eye
//! to be upright when blessed. Regenerate with `MAGI_BLESS_GOLDEN=1`.

use std::path::{Path, PathBuf};

use image::RgbImage;
use image::imageops;
use magi_core::extract::image::extract_image;
use magi_core::ocr::NoOcr;

/// The committed HEIC fixtures: portrait, landscape, and a small landscape.
const HEIC_FIXTURES: &[&str] = &["phone_text_es", "shelf_christmas", "phone_qr"];

/// Mean absolute per-channel difference (0-255) allowed between a fresh
/// thumbnail and its golden. The golden is a JPEG, so it is not bit-exact;
/// a wrong orientation lands far above this (see the discrimination check).
const TOLERANCE: f64 = 4.0;

fn corpus_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/images")
        .join(name)
}

fn golden_path(stem: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/golden/thumbs")
        .join(format!("{stem}.jpg"))
}

fn thumbnail_of(stem: &str) -> RgbImage {
    let path = corpus_path(&format!("{stem}.heic"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    extract_image(&path, &bytes, 64, &NoOcr, None)
        .unwrap_or_else(|e| panic!("{stem}: {e}"))
        .thumbnail
}

fn mean_abs_diff(a: &RgbImage, b: &RgbImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions(), "thumbnail size changed");
    let total: u64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    total as f64 / a.as_raw().len() as f64
}

#[test]
fn heic_thumbnails_are_upright_and_match_their_goldens() {
    for stem in HEIC_FIXTURES {
        let thumb = thumbnail_of(stem);
        let golden = golden_path(stem);

        if std::env::var("MAGI_BLESS_GOLDEN").is_ok() {
            std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
            magi_core::thumbs::write_thumbnail(&golden, &thumb).unwrap();
            continue;
        }

        let expected = image::open(&golden)
            .unwrap_or_else(|e| panic!("reading golden {golden:?}: {e}"))
            .to_rgb8();
        let diff = mean_abs_diff(&thumb, &expected);
        assert!(
            diff <= TOLERANCE,
            "{stem}: thumbnail drifted from its golden (mean abs diff {diff:.2})"
        );

        // The metric must be able to tell a wrong orientation from a right
        // one, or the comparison above proves nothing. Rotated and mirrored
        // copies of an upright thumbnail must land well outside the
        // tolerance (a square-cropped rotation is compared on the common
        // area; here only same-size variants are checked).
        for (what, wrong) in [
            ("rotated 180", imageops::rotate180(&thumb)),
            ("mirrored", imageops::flip_horizontal(&thumb)),
            ("flipped", imageops::flip_vertical(&thumb)),
        ] {
            let wrong_diff = mean_abs_diff(&wrong, &expected);
            assert!(
                wrong_diff > TOLERANCE * 2.0,
                "{stem}: {what} is indistinguishable from the golden (diff {wrong_diff:.2}); \
                 the comparison cannot catch an orientation bug on this image"
            );
        }
    }
}
