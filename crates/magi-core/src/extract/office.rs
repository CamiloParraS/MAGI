//! DOCX/PPTX text extraction (zip + `quick-xml`) and XLSX extraction
//! (`calamine`), dispatched by extension since all three share the
//! `office` [`super::super::discovery::Kind`] (see SPEC.md §7 M2).

use std::io::{Cursor, Read};

use calamine::Reader as _;
use quick_xml::events::Event;

use super::{ExtractedDoc, Extractor};
use crate::error::{Error, Result};

pub struct OfficeExtractor;

impl Extractor for OfficeExtractor {
    fn extract(&self, path: &std::path::Path, bytes: &[u8]) -> Result<ExtractedDoc> {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("docx") => extract_docx(bytes),
            Some("pptx") => extract_pptx(bytes),
            Some("xlsx") => extract_xlsx(bytes),
            _ => Ok(ExtractedDoc::default()),
        }
    }
}

fn open_zip(bytes: &[u8]) -> Result<zip::ZipArchive<Cursor<&[u8]>>> {
    zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| Error::Office(e.to_string()))
}

fn read_zip_entry(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<String> {
    let mut file = archive
        .by_name(name)
        .map_err(|e| Error::Office(e.to_string()))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .map_err(|e| Error::Office(e.to_string()))?;
    Ok(buf)
}

/// Extracts the text of every `<.../t>` run inside `<.../p>` paragraphs,
/// regardless of XML namespace prefix (WordprocessingML uses `w:t`/`w:p`,
/// DrawingML used by PPTX slides uses `a:t`/`a:p` — both have the local
/// names `t`/`p`). Paragraph boundaries become newlines.
fn extract_paragraph_text(xml: &str) -> String {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut in_text_run = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = e.local_name();
                if local.as_ref() == "p" && !out.is_empty() {
                    out.push('\n');
                } else if local.as_ref() == "t" {
                    in_text_run = true;
                }
            }
            Ok(Event::Text(e)) => {
                if in_text_run {
                    out.push_str(e.as_ref());
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == "t" {
                    in_text_run = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn extract_docx(bytes: &[u8]) -> Result<ExtractedDoc> {
    let mut archive = open_zip(bytes)?;
    let xml = read_zip_entry(&mut archive, "word/document.xml")?;
    Ok(super::paginated_doc(std::iter::once(
        extract_paragraph_text(&xml),
    )))
}

/// ponytail: slides are ordered by their `slideN.xml` filename number, not
/// by walking `ppt/presentation.xml`'s real slide order; upgrade if a
/// fixture with reordered slides ever surfaces.
fn extract_pptx(bytes: &[u8]) -> Result<ExtractedDoc> {
    let mut archive = open_zip(bytes)?;
    let mut slide_names: Vec<(u32, String)> = archive
        .file_names()
        .filter_map(|name| {
            let suffix = name
                .strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?;
            suffix.parse::<u32>().ok().map(|n| (n, name.to_string()))
        })
        .collect();
    slide_names.sort_by_key(|(n, _)| *n);

    let mut slide_texts = Vec::with_capacity(slide_names.len());
    for (_, name) in &slide_names {
        let xml = read_zip_entry(&mut archive, name)?;
        slide_texts.push(extract_paragraph_text(&xml));
    }
    Ok(super::paginated_doc(slide_texts))
}

fn extract_xlsx(bytes: &[u8]) -> Result<ExtractedDoc> {
    let mut workbook: calamine::Xlsx<Cursor<&[u8]>> =
        calamine::open_workbook_from_rs(Cursor::new(bytes))
            .map_err(|e: calamine::XlsxError| Error::Office(e.to_string()))?;
    let sheet_names = workbook.sheet_names().to_owned();

    let mut sheet_texts = Vec::with_capacity(sheet_names.len());
    for name in &sheet_names {
        // A sheet that fails to load still occupies its position in the
        // page numbering, same as an empty sheet would.
        let Ok(range) = workbook.worksheet_range(name) else {
            sheet_texts.push(String::new());
            continue;
        };
        let mut sheet_text = String::new();
        for row in range.rows() {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| cell.to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if cells.is_empty() {
                continue;
            }
            sheet_text.push_str(&cells.join(" "));
            sheet_text.push('\n');
        }
        sheet_texts.push(sheet_text);
    }
    Ok(super::paginated_doc(sheet_texts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/office")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
    }

    #[test]
    fn extracts_docx_paragraphs_and_detects_spanish() {
        let bytes = fixture("notes.docx");
        let doc = OfficeExtractor
            .extract(Path::new("notes.docx"), &bytes)
            .unwrap();

        let joined: String = doc.chunks.iter().map(|c| c.text.as_str()).collect();
        assert!(joined.contains("Project Notes"));
        assert!(joined.contains("migration to the new warehouse"));
        assert!(joined.contains("reunion sera el martes"));
    }

    #[test]
    fn extracts_pptx_slides_in_order_with_slide_numbers() {
        let bytes = fixture("kickoff.pptx");
        let doc = OfficeExtractor
            .extract(Path::new("kickoff.pptx"), &bytes)
            .unwrap();

        assert_eq!(doc.chunks.len(), 2, "one chunk per slide");
        assert_eq!(doc.chunks[0].page, Some(1));
        assert!(doc.chunks[0].text.contains("Quarterly Kickoff"));
        assert_eq!(doc.chunks[1].page, Some(2));
        assert!(doc.chunks[1].text.contains("Presupuesto"));
    }

    #[test]
    fn extracts_xlsx_sheets_including_named_second_sheet() {
        let bytes = fixture("inventory.xlsx");
        let doc = OfficeExtractor
            .extract(Path::new("inventory.xlsx"), &bytes)
            .unwrap();

        assert_eq!(doc.chunks.len(), 2, "one chunk per sheet");
        assert!(doc.chunks[0].text.contains("Widget"));
        assert!(doc.chunks[0].text.contains("Warehouse A"));
        assert!(doc.chunks[1].text.contains("Electricista Lopez"));
    }

    #[test]
    fn non_office_extension_yields_empty_doc() {
        let doc = OfficeExtractor
            .extract(Path::new("plain.txt"), b"hi")
            .unwrap();
        assert!(doc.chunks.is_empty());
    }
}
