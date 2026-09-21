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

/// Committed fixtures whose upright orientation is not the stored one. By the
/// `irot` box (quarter turns counter-clockwise): `phone_text_es` 3,
/// `Frontphoto` 1, `Upsidedown` 1; `shelf_christmas` and `phone_qr` need none.
/// `Portrait_photo.jpg` is a JPEG whose only rotation is EXIF `Orientation = 6`.
/// No committed file has a 180-degree `irot` or an `imir` mirror box, which is
/// what `every_irot_angle_decodes_as_the_same_picture_turned` covers for 180.
const FIXTURES: &[&str] = &[
    "phone_text_es.heic",
    "shelf_christmas.heic",
    "phone_qr.heic",
    "Frontphoto.heic",
    "Upsidedown.heic",
    "Portrait_photo.jpg",
];

/// Mean absolute per-channel difference (0-255) allowed between a fresh
/// thumbnail and its golden. The golden is a JPEG, so it is not bit-exact;
/// a wrong orientation lands far above this (see the discrimination check).
const TOLERANCE: f64 = 4.0;

fn corpus_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/images")
        .join(name)
}

fn golden_path(name: &str) -> PathBuf {
    let stem = Path::new(name).file_stem().unwrap().to_str().unwrap();
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/golden/thumbs")
        .join(format!("{stem}.jpg"))
}

fn thumbnail_of(name: &str) -> RgbImage {
    let path = corpus_path(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    extract_image(&path, &bytes, 64, &NoOcr, None)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .thumbnail
        .expect("an image always yields a thumbnail")
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
fn thumbnails_are_upright_and_match_their_goldens() {
    for stem in FIXTURES {
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

/// A HEIC `irot` box is a single byte after its type, whose low two bits are
/// the angle. Patching it makes the other angles from one real photo, and a
/// correct decoder must return the same picture turned a further quarter each
/// time, exactly (a rotation loses no pixels). This is the only test of the
/// 180-degree case; mirroring (`imir`) is still untested.
#[test]
fn every_irot_angle_decodes_as_the_same_picture_turned() {
    let path = corpus_path("Upsidedown.heic");
    let original = std::fs::read(&path).unwrap();
    let decode = |angle: u8| {
        let mut bytes = original.clone();
        let mut at = 0;
        while let Some(i) = bytes[at..].windows(4).position(|w| w == b"irot") {
            let angle_byte = at + i + 4;
            // Leave the thumbnail's own `irot 0` alone; the primary image's is 1.
            if bytes[angle_byte] & 3 == 1 {
                bytes[angle_byte] = angle;
            }
            at += i + 4;
        }
        magi_core::extract::image::decode_bounded(&path, &bytes, 64).unwrap()
    };

    let mut previous = decode(0);
    for angle in 1..4u8 {
        let current = decode(angle);
        // One more quarter turn counter-clockwise.
        assert_eq!(
            imageops::rotate270(&previous),
            current,
            "irot {angle} is not irot {} turned a quarter",
            angle - 1
        );
        previous = current;
    }
}
