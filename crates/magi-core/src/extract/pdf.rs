//! PDF text extraction via `pdfium-render`, one chunk group per page (see
//! SPEC.md §7 M2).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdfium_render::prelude::*;

use super::{ExtractedDoc, Extractor};
use crate::error::{Error, Result};

/// Resolves the vendored PDFium shared library: `MAGI_PDFIUM_PATH` first
/// (an explicit override), then the dev-time `vendor/pdfium/<target>/`
/// layout that `xtask fetch-pdfium` populates.
///
/// ponytail: dev-time resolution only; bundling the library next to the
/// installed app is an M6 packaging decision, not yet made.
fn resolve_library_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MAGI_PDFIUM_PATH") {
        return Some(PathBuf::from(path));
    }
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?.parent()?;
    let candidate = workspace_root
        .join("vendor/pdfium")
        .join(crate::platform::pdfium_vendor_dir())
        .join(crate::platform::pdfium_library_subdir())
        .join(crate::platform::pdfium_library_filename());
    candidate.exists().then_some(candidate)
}

/// PDFium's C library "makes no guarantees about thread safety" (per the
/// pdfium-render docs) — the `thread_safe` feature makes one bound
/// `Pdfium` instance safe to share across threads by mutexing every call,
/// but two independent `bind_to_library` calls loading the same DLL
/// concurrently is still a data race. So there must be exactly one
/// process-wide instance, created lazily on first use.
static PDFIUM: OnceLock<std::result::Result<Pdfium, String>> = OnceLock::new();

fn shared_pdfium() -> Result<&'static Pdfium> {
    let result = PDFIUM.get_or_init(|| {
        let path = resolve_library_path().ok_or_else(|| {
            "PDFium library not found (set MAGI_PDFIUM_PATH or run `cargo xtask fetch-pdfium`)"
                .to_string()
        })?;
        let bindings = Pdfium::bind_to_library(&path).map_err(|e| format!("load library: {e}"))?;
        Ok(Pdfium::new(bindings))
    });
    result
        .as_ref()
        .map_err(|message| Error::Pdf(message.clone()))
}

pub struct PdfExtractor;

impl Extractor for PdfExtractor {
    fn extract(&self, _path: &Path, bytes: &[u8]) -> Result<ExtractedDoc> {
        let pdfium = shared_pdfium()?;
        let document = pdfium
            .load_pdf_from_byte_slice(bytes, None)
            .map_err(|e| Error::Pdf(e.to_string()))?;

        let mut pages = Vec::new();
        for page in document.pages().iter() {
            pages.push(page.text().map_err(|e| Error::Pdf(e.to_string()))?.all());
        }
        Ok(super::paginated_doc(pages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
    }

    #[test]
    fn extracts_text_per_page_with_page_numbers() {
        let bytes = fixture("pdf/report.pdf");
        let doc = PdfExtractor
            .extract(Path::new("report.pdf"), &bytes)
            .unwrap();

        assert_eq!(doc.chunks.len(), 3, "one chunk per non-empty page");
        assert_eq!(doc.chunks[0].page, Some(1));
        assert!(doc.chunks[0].text.contains("Page One"));
        assert_eq!(doc.chunks[1].page, Some(2));
        assert!(doc.chunks[1].text.contains("Page Two"));
        assert_eq!(doc.chunks[2].page, Some(3));
        assert!(doc.chunks[2].text.contains("Page Three"));
        assert_eq!(doc.lang.as_deref(), Some("en"));
    }

    #[test]
    fn extracts_spanish_text_and_detects_language() {
        let bytes = fixture("pdf/factura_electricista.pdf");
        let doc = PdfExtractor
            .extract(Path::new("factura_electricista.pdf"), &bytes)
            .unwrap();

        assert_eq!(doc.chunks.len(), 1);
        assert!(doc.chunks[0].text.contains("Factura"));
        assert_eq!(doc.lang.as_deref(), Some("es"));
    }

    #[test]
    fn truncated_pdf_is_a_clean_error_not_a_panic() {
        let bytes = fixture("edge/truncated.pdf");
        let result = PdfExtractor.extract(Path::new("truncated.pdf"), &bytes);
        assert!(result.is_err());
    }

    #[test]
    fn password_protected_pdf_without_password_is_a_clean_error() {
        let bytes = fixture("edge/password_protected.pdf");
        let result = PdfExtractor.extract(Path::new("password_protected.pdf"), &bytes);
        assert!(result.is_err());
    }
}
