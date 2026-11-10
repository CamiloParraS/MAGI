//! The picture Ask Magi sends for an image file (ADR-0014): a fresh copy, no
//! more than [`LONG_SIDE`] px on its longer side, re-encoded as a JPEG so no
//! EXIF (location included) goes with it. A cloud placeholder is never read.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::roots::searchable_sql;
use crate::error::{Error, Result};
use crate::extract::image::decode_bounded;
use crate::platform::{CloudPlaceholder, Os};

/// The longer side of a sent picture. Enough to read text in a screenshot.
pub const LONG_SIDE: u32 = 1024;
/// The most base64 one picture may take; the server refuses more.
pub const MAX_ENCODED: usize = 2_000_000;
/// The most pictures one question sends.
pub const MAX_IMAGES: usize = 3;
/// A picture over this is not decoded for Ask Magi.
const MAX_MEGAPIXELS: u32 = 64;
const QUALITY: u8 = 82;
const FALLBACK_QUALITY: u8 = 60;

/// The path of a searchable image file: nothing for a file that is gone, is
/// not an image, or is in a root that is paused or removed.
pub fn image_path(conn: &Connection, file_id: i64) -> Result<Option<PathBuf>> {
    let path = conn
        .query_row(
            concat!(
                "SELECT f.path FROM files f JOIN roots r ON r.id = f.root_id
                 WHERE f.id = ?1 AND f.kind = 'image' AND ",
                searchable_sql!()
            ),
            params![file_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(path.map(PathBuf::from))
}

/// `image` as a JPEG no larger than [`LONG_SIDE`], base64 encoded.
pub fn encode(image: &image::RgbImage) -> Result<String> {
    let (w, h) = image.dimensions();
    let resized;
    let image = if w.max(h) > LONG_SIDE {
        let scale = f64::from(LONG_SIDE) / f64::from(w.max(h));
        let (nw, nh) = (
            ((f64::from(w) * scale).round() as u32).max(1),
            ((f64::from(h) * scale).round() as u32).max(1),
        );
        resized = image::imageops::resize(image, nw, nh, FilterType::Triangle);
        &resized
    } else {
        image
    };
    for quality in [QUALITY, FALLBACK_QUALITY] {
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut Cursor::new(&mut bytes), quality)
            .encode_image(image)
            .map_err(|e| Error::Image(e.to_string()))?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        if data.len() <= MAX_ENCODED {
            return Ok(data);
        }
    }
    Err(Error::Image("picture too large to send".into()))
}

/// Reads the picture at `path` and encodes it for sending. Refuses a cloud
/// placeholder without opening it, since reading it would download it.
pub fn encode_file(path: &Path) -> Result<String> {
    let io = |source| Error::Io {
        path: path.to_owned(),
        source,
    };
    let meta = std::fs::metadata(path).map_err(io)?;
    if Os.is_cloud_only(&meta) {
        return Err(Error::Image("cloud placeholder; not read".into()));
    }
    let bytes = std::fs::read(path).map_err(io)?;
    encode(&decode_bounded(path, &bytes, MAX_MEGAPIXELS)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(data: &str) -> image::DynamicImage {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        image::load_from_memory(&bytes).unwrap()
    }

    #[test]
    fn a_large_picture_is_scaled_to_the_long_side_keeping_its_shape() {
        let big = image::RgbImage::from_pixel(3000, 1500, image::Rgb([10, 200, 30]));
        let sent = decoded(&encode(&big).unwrap());
        assert_eq!((sent.width(), sent.height()), (LONG_SIDE, 512));
    }

    #[test]
    fn a_small_picture_is_not_upscaled() {
        let small = image::RgbImage::from_pixel(40, 30, image::Rgb([0, 0, 0]));
        let sent = decoded(&encode(&small).unwrap());
        assert_eq!((sent.width(), sent.height()), (40, 30));
    }

    #[test]
    fn the_sent_jpeg_carries_no_exif() {
        let picture = image::RgbImage::from_pixel(64, 64, image::Rgb([1, 2, 3]));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encode(&picture).unwrap())
            .unwrap();
        // An APP1 segment (FF E1) is where EXIF lives.
        assert!(!bytes.windows(2).any(|w| w == [0xFF, 0xE1]));
    }

    #[test]
    fn a_file_that_is_not_there_is_an_error_not_a_panic() {
        assert!(encode_file(Path::new("definitely/not/here.jpg")).is_err());
    }
}
