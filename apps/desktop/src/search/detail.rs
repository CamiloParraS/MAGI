//! What the search window's detail pane says about the selected result
//! (`ui.detail_pane`): plain functions, so `cargo test` covers the wording.

use std::path::Path;

use chrono::NaiveDate;
use magi_core::dto::{FileDetails, MatchSource};

use super::state::shown_sources;
use crate::i18n::{Lang, Strings};

/// The file's type as its extension in capitals ("PDF", "DOCX"), the name
/// people already know it by; empty for a file without one.
pub fn type_label(file_name: &str) -> String {
    Path::new(file_name)
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_default()
}

/// "PDF · Aug 18 · 8 pages · 196 KB": type, modified date, page count (when
/// the index has one) and size (once [`FileDetails`] has loaded).
pub fn details_line(
    file_name: &str,
    modified: Option<NaiveDate>,
    details: Option<FileDetails>,
    lang: Lang,
    today: NaiveDate,
) -> String {
    let s = lang.strings();
    let pages = details
        .and_then(|d| d.pages)
        .filter(|&n| n > 0)
        .map(|n| lang.plural(&s.pages, n as u64));
    let size = details.map(|d| lang.file_size(d.size));
    [
        Some(type_label(file_name)),
        modified.map(|d| lang.date(d, today)),
        pages,
        size,
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

/// Why it matched, as the pane's chips: each source the row names, short
/// ("Words") with its sentence ("Has your exact words") for the tooltip.
pub fn reasons(sources: &[MatchSource], s: &Strings) -> Vec<(&'static str, &'static str)> {
    shown_sources(sources)
        .into_iter()
        .map(|m| (s.source(m), s.source_tip(m)))
        .collect()
}

/// The snippet only repeats where the file is: a match on its name quotes
/// the path's words ("en lease agreement txt"), which the pane already
/// shows. Every word of it is then a word of the path.
pub fn snippet_is_the_path(snippet: &str, path: &str) -> bool {
    let words = |text: &str| -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let path = words(path);
    let snippet = words(snippet);
    !snippet.is_empty() && snippet.iter().all(|w| path.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn the_type_is_the_extension_in_capitals() {
        assert_eq!(type_label("Lease - 14 Elm Street.pdf"), "PDF");
        assert_eq!(type_label("notes.Md"), "MD");
        assert_eq!(type_label("Makefile"), "");
    }

    #[test]
    fn the_line_has_type_date_pages_and_size() {
        let details = FileDetails {
            size: 196_000,
            pages: Some(8),
        };
        assert_eq!(
            details_line(
                "lease.pdf",
                Some(day(2025, 8, 18)),
                Some(details),
                Lang::En,
                day(2026, 11, 4)
            ),
            "PDF · Aug 18, 2025 · 8 pages · 196 KB"
        );
    }

    #[test]
    fn what_is_unknown_is_left_out() {
        let today = day(2026, 11, 4);
        assert_eq!(
            details_line("Makefile", Some(today), None, Lang::En, today),
            "today"
        );
        let one_page = FileDetails {
            size: 900,
            pages: Some(1),
        };
        assert_eq!(
            details_line("a.pdf", None, Some(one_page), Lang::En, today),
            "PDF · 1 page · 900 B"
        );
        let no_pages = FileDetails {
            size: 2_400_000,
            pages: None,
        };
        assert_eq!(
            details_line("a.txt", None, Some(no_pages), Lang::En, today),
            "TXT · 2.4 MB"
        );
    }

    #[test]
    fn reasons_skip_related_beside_a_more_specific_source() {
        let s = Lang::En.strings();
        assert_eq!(
            reasons(&[MatchSource::Keyword, MatchSource::Semantic], s),
            vec![(s.source_keyword, s.source_keyword_tip)]
        );
        assert_eq!(
            reasons(&[MatchSource::Semantic], s),
            vec![(s.source_semantic, s.source_semantic_tip)]
        );
    }

    #[test]
    fn a_snippet_of_the_path_words_is_the_path() {
        let path = "C:/TETO/fixtures/corpus/en/lease_agreement.txt";
        assert!(snippet_is_the_path("en lease agreement txt", path));
        assert!(snippet_is_the_path("en Lease agreement…", path));
        assert!(!snippet_is_the_path("the tenant shall pay rent", path));
        assert!(!snippet_is_the_path("", path));
    }
}
