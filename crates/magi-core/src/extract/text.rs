//! Plain text and Markdown extraction: encoding detection + NFC
//! normalization, then the provisional character chunker (see SPEC.md §7
//! M2). Legacy Spanish files are often Windows-1252 / Latin-1.

use std::path::Path;

use unicode_normalization::UnicodeNormalization;

use super::{ExtractedDoc, Extractor, RawChunk};
use crate::error::Result;

/// Decodes `bytes` using a sniffed encoding (falling back to UTF-8) and
/// normalizes the result to Unicode NFC.
pub fn decode_text(bytes: &[u8]) -> String {
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, chardetng::Utf8Detection::Allow);
    let (decoded, _, _) = encoding.decode(bytes);
    decoded.nfc().collect()
}

pub struct TextExtractor;

impl Extractor for TextExtractor {
    fn extract(&self, _path: &Path, bytes: &[u8]) -> Result<ExtractedDoc> {
        let decoded = decode_text(bytes);
        let body = decoded.trim();
        if body.is_empty() {
            return Ok(ExtractedDoc::default());
        }
        let lang = super::lang::detect_lang(body);
        let chunks = crate::chunk::chunk_text(body)
            .into_iter()
            .map(RawChunk::body)
            .collect();
        Ok(ExtractedDoc {
            chunks,
            lang,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn decodes_utf8_and_normalizes_to_nfc() {
        // "café" with a combining acute accent (NFD) must come back as the
        // precomposed (NFC) form.
        let nfd = "cafe\u{0301}";
        let decoded = decode_text(nfd.as_bytes());
        assert_eq!(decoded, "café");
    }

    #[test]
    fn decodes_windows_1252_spanish_text() {
        // "canción" encoded as Windows-1252 (ñ = 0xF1).
        let (encoded, _, _) = encoding_rs::WINDOWS_1252.encode("La canción del día");
        let decoded = decode_text(&encoded);
        assert_eq!(decoded, "La canción del día");
    }

    #[test]
    fn empty_bytes_yield_no_chunks() {
        let doc = TextExtractor
            .extract(&PathBuf::from("empty.txt"), b"")
            .unwrap();
        assert!(doc.chunks.is_empty());
        assert_eq!(doc.lang, None);
    }

    #[test]
    fn extracts_body_chunks_and_language() {
        let text = "The quick brown fox jumps over the lazy dog near the riverbank.";
        let doc = TextExtractor
            .extract(&PathBuf::from("notes.txt"), text.as_bytes())
            .unwrap();
        assert_eq!(doc.chunks.len(), 1);
        assert_eq!(doc.chunks[0].text, text);
        assert_eq!(doc.lang.as_deref(), Some("en"));
    }
}
