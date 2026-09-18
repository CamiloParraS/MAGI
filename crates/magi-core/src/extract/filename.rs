//! Every file gets one filename chunk, built from its name and parent
//! folder names, so content-less files are still findable (see SPEC.md
//! §5.5).

use std::path::{Component, Path};

use super::{ChunkSource, RawChunk};

/// Builds the filename chunk for `rel_path` (a file's path relative to its
/// indexed root): every path component split on `_ - . space` and
/// camelCase boundaries, joined with spaces.
pub fn filename_chunk(rel_path: &Path) -> RawChunk {
    let mut words = Vec::new();
    for component in rel_path.components() {
        if let Component::Normal(part) = component
            && let Some(part) = part.to_str()
        {
            words.extend(split_words(part));
        }
    }
    RawChunk {
        source: ChunkSource::Filename,
        text: words.join(" "),
        page: None,
        line_start: None,
        line_end: None,
    }
}

fn split_words(s: &str) -> Vec<String> {
    s.split(|c: char| c == '_' || c == '-' || c == '.' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .flat_map(split_camel_case)
        .map(str::to_string)
        .collect()
}

fn split_camel_case(s: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut result = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let (idx, c) = chars[i];
        let (_, prev) = chars[i - 1];
        if prev.is_lowercase() && c.is_uppercase() {
            result.push(&s[start..idx]);
            start = idx;
        }
    }
    result.push(&s[start..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_delimiters() {
        let chunk = filename_chunk(Path::new("invoice_final-v2.report.txt"));
        assert_eq!(chunk.text, "invoice final v2 report txt");
        assert_eq!(chunk.source, ChunkSource::Filename);
    }

    #[test]
    fn splits_camel_case() {
        let chunk = filename_chunk(Path::new("myScreenShot2024.png"));
        assert_eq!(chunk.text, "my Screen Shot2024 png");
    }

    #[test]
    fn includes_parent_folder_names() {
        let chunk = filename_chunk(Path::new("Tax Documents/2023_electrician_invoice.pdf"));
        assert_eq!(chunk.text, "Tax Documents 2023 electrician invoice pdf");
    }
}
