//! Thumbnail cache for images and PDF first pages (SPEC.md §7 M4).
//!
//! Keyed by the file's blake3 content hash, not its path, so two copies of
//! the same photo share one thumbnail and a renamed file keeps its own. The
//! desktop app serves these through the scoped asset protocol in M6.

use std::path::{Path, PathBuf};

use image::ImageEncoder;
use image::codecs::jpeg::JpegEncoder;

use crate::error::{Error, Result};

/// Long edge of a stored thumbnail, per SPEC.md §7 M4.
pub const THUMB_LONG_SIDE: u32 = 256;

/// Enough for a thumbnail; the file is 256 px on its long side.
const JPEG_QUALITY: u8 = 80;

/// Lowercase hex of the content hash.
pub fn thumb_key(content_hash: &[u8; 32]) -> String {
    let mut key = String::with_capacity(64);
    for byte in content_hash {
        use std::fmt::Write;
        // Writing to a String is infallible.
        let _ = write!(key, "{byte:02x}");
    }
    key
}

/// `<cache_dir>/thumbs/<first two hex chars>/<key>.jpg`.
///
/// The two-character shard keeps any one directory to a few thousand
/// entries, which matters on filesystems that scan a directory linearly.
pub fn thumb_path(key: &str) -> PathBuf {
    let shard = key.get(..2).unwrap_or("00");
    crate::paths::cache_dir()
        .join("thumbs")
        .join(shard)
        .join(format!("{key}.jpg"))
}

/// Downscales to [`THUMB_LONG_SIDE`] and writes a JPEG, creating the parent
/// directory. An image already at or below that size is stored as-is rather
/// than upscaled.
pub fn write_thumbnail(path: &Path, image: &image::RgbImage) -> Result<()> {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Err(Error::Image("cannot thumbnail an empty image".into()));
    }

    let longest = width.max(height);
    let resized;
    let thumbnail = if longest <= THUMB_LONG_SIDE {
        image
    } else {
        let scale = f64::from(THUMB_LONG_SIDE) / f64::from(longest);
        resized = image::imageops::resize(
            image,
            ((f64::from(width) * scale).round() as u32).max(1),
            ((f64::from(height) * scale).round() as u32).max(1),
            image::imageops::FilterType::Lanczos3,
        );
        &resized
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
        .write_image(
            thumbnail.as_raw(),
            thumbnail.width(),
            thumbnail.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| Error::Image(e.to_string()))?;

    std::fs::write(path, &encoded).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> image::RgbImage {
        image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        })
    }

    #[test]
    fn key_is_lowercase_hex_and_shards_by_its_first_two_characters() {
        let mut hash = [0u8; 32];
        hash[0] = 0xAB;
        hash[31] = 0x0F;
        let key = thumb_key(&hash);
        assert_eq!(key.len(), 64);
        assert!(key.starts_with("ab00"));
        assert!(key.ends_with("0f"));
        assert!(thumb_path(&key).ends_with(format!("ab/{key}.jpg")));
    }

    #[test]
    fn thumbnail_fits_the_long_side_and_keeps_its_aspect_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shard").join("wide.jpg");
        write_thumbnail(&path, &gradient(1024, 512)).unwrap();

        let written = image::open(&path).unwrap();
        assert_eq!((written.width(), written.height()), (256, 128));
    }

    #[test]
    fn a_small_image_is_not_upscaled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.jpg");
        write_thumbnail(&path, &gradient(80, 40)).unwrap();

        let written = image::open(&path).unwrap();
        assert_eq!((written.width(), written.height()), (80, 40));
    }
}
