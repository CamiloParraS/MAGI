//! `Extractor` trait plus implementations for text, code, PDF, Office,
//! image, HEIC, and filename extraction. Implemented across M2 and M4
//! (see SPEC.md §7 M2, M4).

pub mod code;
pub mod filename;
pub mod lang;
pub mod office;
pub mod pdf;
pub mod text;

use std::path::Path;

/// Matches the `chunks.source` column in `migrations/0001_init.sql`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkSource {
    Body,
    Ocr,
    Qr,
    Filename,
    CodeSymbol,
}

impl ChunkSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ChunkSource::Body => "body",
            ChunkSource::Ocr => "ocr",
            ChunkSource::Qr => "qr",
            ChunkSource::Filename => "filename",
            ChunkSource::CodeSymbol => "code_symbol",
        }
    }
}

/// One extracted chunk, ready for the chunker/writer. Mirrors the `chunks`
/// table's columns (see SPEC.md §5.5).
#[derive(Debug, Clone, PartialEq)]
pub struct RawChunk {
    pub source: ChunkSource,
    pub text: String,
    pub page: Option<i64>,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
}

impl RawChunk {
    pub fn body(text: String) -> Self {
        Self {
            source: ChunkSource::Body,
            text,
            page: None,
            line_start: None,
            line_end: None,
        }
    }
}

/// Output of extracting one file's content: chunks plus the detected
/// language (from the concatenated body text, where detectable). Does not
/// include the filename chunk — the pipeline adds that for every file
/// regardless of kind (see [`filename::filename_chunk`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtractedDoc {
    pub chunks: Vec<RawChunk>,
    pub lang: Option<String>,
}

pub trait Extractor {
    fn extract(&self, path: &Path, bytes: &[u8]) -> crate::error::Result<ExtractedDoc>;
}

/// Builds an [`ExtractedDoc`] from a sequence of page/slide/sheet texts:
/// each non-empty page is chunked (via [`crate::chunk::chunk_text`]) and
/// tagged with its 1-based page number, and the language is detected from
/// the whole document's concatenated text. Shared by the PDF, PPTX, XLSX,
/// and DOCX extractors, which differ only in how they produce page texts.
///
/// ponytail: `full_text` accumulates the entire document just to feed
/// `detect_lang` once at the end, which only needs a modest sample — on a
/// very large document this is 2-3x the allocation/copying strictly
/// necessary. Upgrade by detecting language from the first non-empty page
/// (or first N chars) if this ever shows up in a profile.
pub fn paginated_doc(pages: impl IntoIterator<Item = String>) -> ExtractedDoc {
    let mut chunks = Vec::new();
    let mut full_text = String::new();
    for (index, text) in pages.into_iter().enumerate() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        full_text.push_str(trimmed);
        full_text.push('\n');
        for piece in crate::chunk::chunk_text(trimmed) {
            chunks.push(RawChunk {
                source: ChunkSource::Body,
                text: piece,
                page: Some(index as i64 + 1),
                line_start: None,
                line_end: None,
            });
        }
    }
    let lang = lang::detect_lang(full_text.trim());
    ExtractedDoc { chunks, lang }
}
