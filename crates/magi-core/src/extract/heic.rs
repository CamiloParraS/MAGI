//! HEIC/HEIF decoding (SPEC.md §7 M4).
//!
//! Backed by `heic-rs`, a pure-Rust decoder, per ADR-0003. This module is
//! the only place in the crate that knows which decoder is in use; swapping
//! back to `libheif-rs` means rewriting this file and nothing else.
//!
//! iPhone photos are stored as tile grids and carry an `irot`/`imir`
//! transform, both of which the decoder composes and applies here, so
//! callers get an upright, full-frame image.

use std::path::Path;

use heic_rs::{DecodeOptions, PixelLayout};

use crate::error::{Error, Result};

/// Extensions `discovery::classify` routes here.
const HEIC_EXTENSIONS: &[&str] = &["heic", "heif"];

/// True when the bytes are a HEIF-family still image, or — for an empty or
/// unreadable head — when the extension says so.
pub fn is_heic(path: &Path, bytes: &[u8]) -> bool {
    if heic_rs::io::looks_like_heif(bytes) {
        return true;
    }
    bytes.is_empty()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| HEIC_EXTENSIONS.iter().any(|k| k.eq_ignore_ascii_case(e)))
}

/// Reads the primary image's display dimensions from the container,
/// decoding nothing. Rotation is already applied, so these are the
/// dimensions [`decode_heic`] will return.
pub fn probe_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let info = heic_rs::probe(bytes).map_err(|e| Error::Heic(e.to_string()))?;
    Ok((info.width, info.height))
}

/// Decodes the primary image to RGB8, with the container's rotation and
/// mirror transforms applied.
///
/// Auxiliary images (depth maps, alpha) and the `.MOV` half of a Live Photo
/// are ignored: only the primary item is decoded. Callers go through
/// `image::decode_bounded`, which rejects oversize images from the header.
pub fn decode_heic(bytes: &[u8], max_megapixels: u32) -> Result<image::RgbImage> {
    let max_pixels = u64::from(max_megapixels) * 1_000_000;

    // `decode_bounded` has already checked the declared size; `max_pixels`
    // here stops a container that lies about its dimensions from making the
    // decoder allocate past the budget.
    //
    // ponytail: the thread count is left to the rayon default. Peak RSS
    // scales with it (ADR-0003 measured 45 MB and 116 MB for two 12 MP
    // photos on the same machine); bound it with `with_threads` if the
    // 48 MP fixture misses SPEC.md §7 M4's 400 MB budget.
    let options = DecodeOptions::default()
        .with_layout(PixelLayout::Rgb8)
        .with_max_pixels(Some(max_pixels))
        .with_transforms(true);
    let decoded = heic_rs::decode(bytes, &options).map_err(|e| Error::Heic(e.to_string()))?;

    image::RgbImage::from_raw(decoded.width, decoded.height, decoded.data).ok_or_else(|| {
        Error::Heic(format!(
            "decoded buffer does not match {}x{} RGB8",
            decoded.width, decoded.height
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn fixture(name: &str) -> Option<(PathBuf, Vec<u8>)> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/images")
            .join(name);
        // Some image fixtures are local-only (fixtures/README.md), so a
        // missing one is a skip, never a failure.
        let bytes = std::fs::read(&path).ok()?;
        Some((path, bytes))
    }

    /// The two committed iPhone fixtures cover both orientations at 12 MP.
    const COMMITTED: &[(&str, u32, u32)] = &[
        ("shelf_christmas.heic", 4000, 3000),
        ("phone_text_es.heic", 3000, 4000),
        ("phone_qr.heic", 1834, 1546),
    ];

    #[test]
    fn decodes_committed_fixtures_upright() {
        for (name, width, height) in COMMITTED {
            let Some((path, bytes)) = fixture(name) else {
                panic!("committed fixture {name} is missing");
            };
            assert!(is_heic(&path, &bytes), "{name} not recognized as HEIC");

            let image = decode_heic(&bytes, 64).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                (image.width(), image.height()),
                (*width, *height),
                "{name} decoded to the wrong size — orientation transform not applied?"
            );
            // A photograph is never a single flat colour; catches a decode
            // that silently produces an empty or zeroed buffer.
            let first = image.get_pixel(0, 0);
            assert!(
                image.pixels().any(|p| p != first),
                "{name} decoded to a uniform image"
            );
        }
    }

    #[test]
    fn rejects_bytes_that_are_not_heif() {
        let path = Path::new("not_really.heic");
        assert!(!is_heic(path, b"P6\n1 1\n255\n"));
        assert!(matches!(
            decode_heic(b"P6\n1 1\n255\n", 64),
            Err(Error::Heic(_))
        ));
    }
}
