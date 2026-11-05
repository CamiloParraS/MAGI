//! Plain text for a result row: where its file is, when, and what a screen
//! reader says for it.

use std::path::Path;

use chrono::NaiveDate;

pub(in crate::search::view) fn folder_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// What a screen reader says for a row: the name, where it is, when, and why
/// it matched (the footer's reason for the mouse-less is the same fact).
pub(super) fn row_label(name: &str, folder: &str, date: &str, sources: &str) -> String {
    [name, folder, date, sources]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

/// The last two folders of the path, `…\Documents\Contratos`: enough to
/// tell two files of one name apart; the tooltip has the rest.
pub(in crate::search::view) fn short_folder(path: &str) -> String {
    let Some(parent) = Path::new(path).parent() else {
        return String::new();
    };
    let names: Vec<_> = parent
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if names.len() <= 2 {
        return parent.display().to_string();
    }
    let sep = std::path::MAIN_SEPARATOR.to_string();
    format!("\u{2026}{sep}{}", names[names.len() - 2..].join(&sep))
}

/// The name of the folder a file is in.
pub(super) fn folder_name(path: &str) -> String {
    Path::new(path)
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(in crate::search::view) fn local_date(unix_ms: i64) -> Option<NaiveDate> {
    chrono::DateTime::from_timestamp_millis(unix_ms)
        .map(|t| t.with_timezone(&chrono::Local).date_naive())
}

#[cfg(test)]
mod tests {
    use super::{row_label, short_folder};

    #[test]
    fn a_row_is_read_as_name_place_date_and_reason() {
        assert_eq!(
            row_label("a.pdf", "Contratos", "18 sept", "Related"),
            "a.pdf, Contratos, 18 sept, Related"
        );
        assert_eq!(row_label("a.pdf", "", "", ""), "a.pdf");
    }

    #[test]
    fn a_deep_path_keeps_its_last_two_folders() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = ["", "home", "ana", "Documents", "Contratos", "a.pdf"].join(&sep.to_string());
        assert_eq!(
            short_folder(&path),
            format!("\u{2026}{sep}Documents{sep}Contratos")
        );
        assert_eq!(short_folder("a.pdf"), "");
    }
}
