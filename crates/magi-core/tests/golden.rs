use std::path::{Path, PathBuf};

use magi_core::extract::code::CodeExtractor;
use magi_core::extract::office::OfficeExtractor;
use magi_core::extract::pdf::PdfExtractor;
use magi_core::extract::text::TextExtractor;
use magi_core::extract::{ExtractedDoc, Extractor};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden")
}

fn assert_matches_golden(name: &str, doc: &ExtractedDoc) {
    let rendered = format!("{doc:#?}\n");
    let golden_path = golden_dir().join(format!("{name}.txt"));
    if std::env::var("MAGI_BLESS_GOLDEN").is_ok() {
        std::fs::write(&golden_path, &rendered).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("reading golden {golden_path:?}: {e}"));
    assert_eq!(
        rendered, expected,
        "extraction output drifted from {golden_path:?}"
    );
}

fn check(name: &str, rel_path: &str, extractor: &dyn Extractor) {
    let path = corpus_dir().join(rel_path);
    let bytes = std::fs::read(&path).unwrap();
    let doc = extractor.extract(&path, &bytes).unwrap();
    assert_matches_golden(name, &doc);
}

#[test]
fn text_en_onboarding_notes_matches_golden() {
    check(
        "text_en_onboarding_notes",
        "en/onboarding_notes.txt",
        &TextExtractor,
    );
}

#[test]
fn text_es_notas_incorporacion_matches_golden() {
    check(
        "text_es_notas_incorporacion",
        "es/notas_incorporacion.txt",
        &TextExtractor,
    );
}

#[test]
fn code_sample_rs_matches_golden() {
    check("code_sample_rs", "code/sample.rs", &CodeExtractor);
}

#[test]
fn code_muestra_py_matches_golden() {
    check("code_muestra_py", "code/muestra.py", &CodeExtractor);
}

#[test]
fn pdf_report_matches_golden() {
    check("pdf_report", "pdf/report.pdf", &PdfExtractor);
}

#[test]
fn pdf_factura_electricista_matches_golden() {
    check(
        "pdf_factura_electricista",
        "pdf/factura_electricista.pdf",
        &PdfExtractor,
    );
}

#[test]
fn office_notes_docx_matches_golden() {
    check("office_notes_docx", "office/notes.docx", &OfficeExtractor);
}

#[test]
fn office_kickoff_pptx_matches_golden() {
    check(
        "office_kickoff_pptx",
        "office/kickoff.pptx",
        &OfficeExtractor,
    );
}

#[test]
fn office_inventory_xlsx_matches_golden() {
    check(
        "office_inventory_xlsx",
        "office/inventory.xlsx",
        &OfficeExtractor,
    );
}
