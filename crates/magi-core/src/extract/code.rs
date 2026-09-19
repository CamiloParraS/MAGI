//! Code extraction via tree-sitter: chunk by top-level symbol for the
//! languages with a grammar, falling back to line windows otherwise (see
//! SPEC.md §7 M2, §3 tech stack).

use std::path::Path;

use super::{ChunkSource, ExtractedDoc, Extractor, RawChunk};
use crate::error::{Error, Result};

const LINE_WINDOW_SIZE: usize = 100;
const LINE_WINDOW_OVERLAP: usize = 10;

fn language_for_extension(ext: &str) -> Option<tree_sitter::Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "js" | "jsx" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        _ => None,
    }
}

pub struct CodeExtractor;

impl Extractor for CodeExtractor {
    fn extract(&self, path: &Path, bytes: &[u8]) -> Result<ExtractedDoc> {
        let source = super::text::decode_text(bytes);
        if source.trim().is_empty() {
            return Ok(ExtractedDoc::default());
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let raw = match ext.as_deref().and_then(language_for_extension) {
            Some(language) => symbol_chunks(&source, language)?,
            None => line_window_chunks(&source),
        };
        // Symbols/windows can exceed the embedder's token limit; re-split so
        // no tail is truncated out of vector search. Pieces keep the parent's
        // line range (a bounding range, not exact).
        let chunks = raw
            .into_iter()
            .flat_map(|c| {
                crate::chunk::chunk_text(&c.text)
                    .into_iter()
                    .map(move |text| RawChunk { text, ..c.clone() })
            })
            .collect();
        let lang = super::lang::detect_lang(&source);
        Ok(ExtractedDoc { chunks, lang })
    }
}

/// One chunk per top-level named node in the parse tree (function, struct,
/// class, impl block, etc. — whatever the grammar groups at file scope).
fn symbol_chunks(source: &str, language: tree_sitter::Language) -> Result<Vec<RawChunk>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| Error::Code(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Code("parser produced no tree".to_string()))?;

    let mut chunks = Vec::new();
    let mut cursor = tree.root_node().walk();
    for child in tree.root_node().named_children(&mut cursor) {
        let text = &source[child.byte_range()];
        if text.trim().is_empty() {
            continue;
        }
        chunks.push(RawChunk {
            source: ChunkSource::CodeSymbol,
            text: text.to_string(),
            page: None,
            line_start: Some(child.start_position().row as i64 + 1),
            line_end: Some(child.end_position().row as i64 + 1),
        });
    }
    // No meaningful top-level grouping (e.g. a file that's one big
    // expression) still needs to be searchable.
    if chunks.is_empty() {
        return Ok(line_window_chunks(source));
    }
    Ok(chunks)
}

/// Fallback for `Kind::Code` extensions without a tree-sitter grammar (see
/// SPEC.md §3: "fallback to line windows").
fn line_window_chunks(text: &str) -> Vec<RawChunk> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let step = LINE_WINDOW_SIZE - LINE_WINDOW_OVERLAP;
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + LINE_WINDOW_SIZE).min(lines.len());
        let window = lines[start..end].join("\n");
        if !window.trim().is_empty() {
            chunks.push(RawChunk {
                source: ChunkSource::Body,
                text: window,
                page: None,
                line_start: Some(start as i64 + 1),
                line_end: Some(end as i64),
            });
        }
        if end >= lines.len() {
            break;
        }
        start += step;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_source_is_chunked_by_top_level_symbol() {
        let source = r#"
use std::fmt;

fn add(a: i32, b: i32) -> i32 {
    a + b
}

struct Point {
    x: i32,
    y: i32,
}
"#;
        let doc = CodeExtractor
            .extract(Path::new("lib.rs"), source.as_bytes())
            .unwrap();

        assert!(
            doc.chunks
                .iter()
                .any(|c| c.text.contains("fn add") && c.source == ChunkSource::CodeSymbol)
        );
        assert!(doc.chunks.iter().any(|c| c.text.contains("struct Point")));
        // "use std::fmt;" is its own top-level chunk too, which is fine —
        // harmless noise for keyword search.
        for chunk in &doc.chunks {
            assert!(chunk.line_start.is_some());
            assert!(chunk.line_end.is_some());
        }
    }

    #[test]
    fn python_source_is_chunked_by_top_level_symbol() {
        let source =
            "def greet(name):\n    return f\"hola {name}\"\n\n\nclass Greeter:\n    pass\n";
        let doc = CodeExtractor
            .extract(Path::new("greet.py"), source.as_bytes())
            .unwrap();

        assert!(doc.chunks.iter().any(|c| c.text.contains("def greet")));
        assert!(doc.chunks.iter().any(|c| c.text.contains("class Greeter")));
    }

    #[test]
    fn typescript_react_component_is_chunked() {
        let source = "export function Hello() {\n  return <div>hi</div>;\n}\n";
        let doc = CodeExtractor
            .extract(Path::new("Hello.tsx"), source.as_bytes())
            .unwrap();

        assert!(!doc.chunks.is_empty());
        assert!(doc.chunks.iter().any(|c| c.text.contains("Hello")));
    }

    #[test]
    fn language_without_a_grammar_falls_back_to_line_windows() {
        let source = "def greet(name)\n  puts \"hola #{name}\"\nend\n";
        let doc = CodeExtractor
            .extract(Path::new("greet.rb"), source.as_bytes())
            .unwrap();

        assert_eq!(doc.chunks.len(), 1);
        assert_eq!(doc.chunks[0].source, ChunkSource::Body);
        assert_eq!(doc.chunks[0].line_start, Some(1));
        assert!(doc.chunks[0].text.contains("puts"));
    }

    #[test]
    fn oversized_symbol_is_split_below_token_limit() {
        let body = (0..1500)
            .map(|i| format!("x{i}"))
            .collect::<Vec<_>>()
            .join(" + ");
        let source = format!("fn big() -> i32 {{ {body} }}\n");
        let doc = CodeExtractor
            .extract(Path::new("big.rs"), source.as_bytes())
            .unwrap();

        assert!(doc.chunks.len() > 1);
        assert!(doc.chunks.iter().all(|c| c.line_start == Some(1)));
        // Word count is a lower bound on token count, so this holds whether
        // `chunk_text` used the real tokenizer or the word-count fallback.
        assert!(
            doc.chunks
                .iter()
                .all(|c| c.text.split_whitespace().count() <= crate::chunk::TARGET_MAX_TOKENS),
            "a piece is still over the chunker's target"
        );
    }

    #[test]
    fn empty_source_yields_no_chunks() {
        let doc = CodeExtractor.extract(Path::new("empty.rs"), b"").unwrap();
        assert!(doc.chunks.is_empty());
    }

    #[test]
    fn real_repo_rust_file_parses_without_error() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/chunk.rs");
        let bytes = std::fs::read(&path).unwrap();
        let doc = CodeExtractor
            .extract(Path::new("chunk.rs"), &bytes)
            .unwrap();

        assert!(
            doc.chunks
                .iter()
                .any(|c| c.text.contains("pub fn chunk_text"))
        );
        assert_eq!(doc.lang.as_deref(), Some("en"));
    }
}
