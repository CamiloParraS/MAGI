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
    blake3::Hash::from(*content_hash).to_hex().to_string()
}

/// Root of the thumbnail cache. This is the one directory the webview's asset
/// protocol may read (SPEC.md §9); user files are never exposed to it.
pub fn thumbs_dir() -> PathBuf {
    crate::paths::cache_dir().join("thumbs")
}

/// `<cache_dir>/thumbs/<first two hex chars>/<key>.jpg`.
///
/// The two-character shard keeps any one directory to a few thousand
/// entries, which matters on filesystems that scan a directory linearly.
pub fn thumb_path(key: &str) -> PathBuf {
    let shard = key.get(..2).unwrap_or("00");
    thumbs_dir().join(shard).join(format!("{key}.jpg"))
}

/// Writes a JPEG, creating the parent directory. The caller passes an image
/// already at [`THUMB_LONG_SIDE`] (`extract_image` and the PDF renderer both
/// do); it is stored as-is. Written to a temp name then renamed, so a crash
/// never leaves a truncated file at `path`.
pub fn write_thumbnail(path: &Path, image: &image::RgbImage) -> Result<()> {
    if image.width() == 0 || image.height() == 0 {
        return Err(Error::Image("cannot thumbnail an empty image".into()));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| Error::Image(e.to_string()))?;

    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &encoded)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|source| Error::Io {
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
    fn a_leftover_partial_write_never_shows_up_as_the_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shard").join("x.jpg");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // What a crash mid-write leaves behind.
        std::fs::write(path.with_extension("tmp"), b"\xff\xd8truncated").unwrap();
        assert!(!path.exists());

        write_thumbnail(&path, &gradient(256, 128)).unwrap();

        let written = image::open(&path).unwrap();
        assert_eq!((written.width(), written.height()), (256, 128));
        assert!(!path.with_extension("tmp").exists());
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
