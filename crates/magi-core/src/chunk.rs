//! Chunking. M2 adds a provisional character-based chunker; M3 replaces it
//! with a token-aware chunker (see SPEC.md §7 M2, M3).

/// Target chunk size, in `char`s (not bytes, so multi-byte UTF-8 sequences
/// are never split mid-codepoint).
pub const CHUNK_SIZE: usize = 1500;
/// Overlap between consecutive chunks, in `char`s.
pub const CHUNK_OVERLAP: usize = 200;

/// Splits `text` into overlapping chunks of roughly [`CHUNK_SIZE`] chars,
/// each starting [`CHUNK_OVERLAP`] chars before the previous one ended.
/// Returns no chunks for empty or whitespace-only input.
pub fn chunk_text(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let byte_offsets: Vec<usize> = text
        .char_indices()
        .map(|(byte_offset, _)| byte_offset)
        .collect();
    let char_len = byte_offsets.len();
    if char_len <= CHUNK_SIZE {
        return vec![text.to_string()];
    }

    let step = CHUNK_SIZE - CHUNK_OVERLAP;
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + CHUNK_SIZE).min(char_len);
        let start_byte = byte_offsets[start];
        let end_byte = byte_offsets.get(end).copied().unwrap_or(text.len());
        chunks.push(text[start_byte..end_byte].to_string());
        if end >= char_len {
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
    fn empty_text_yields_no_chunks() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   \n\t  ").is_empty());
    }

    #[test]
    fn short_text_is_a_single_chunk() {
        let text = "hello world";
        assert_eq!(chunk_text(text), vec![text.to_string()]);
    }

    #[test]
    fn text_at_exact_chunk_size_is_a_single_chunk() {
        let text = "a".repeat(CHUNK_SIZE);
        assert_eq!(chunk_text(&text), vec![text]);
    }

    #[test]
    fn long_text_is_split_with_overlap() {
        let text = "a".repeat(3000);
        let chunks = chunk_text(&text);

        // step = 1500 - 200 = 1300, so starts are 0, 1300, 2600.
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chars().count(), CHUNK_SIZE);
        assert_eq!(chunks[1].chars().count(), CHUNK_SIZE);
        assert_eq!(chunks[2].chars().count(), 3000 - 2600);

        // The overlap region is identical content.
        let overlap_from_first = &chunks[0][chunks[0].len() - 200..];
        let overlap_from_second = &chunks[1][..200];
        assert_eq!(overlap_from_first, overlap_from_second);
    }

    #[test]
    fn unicode_chars_are_never_split_mid_codepoint() {
        // Spanish accented text repeated well past CHUNK_SIZE in *chars*.
        let sentence = "La canción tiene una página con ñ, á, é, í, ó, ú. ";
        let text = sentence.repeat(100);
        assert!(text.chars().count() > CHUNK_SIZE);

        let chunks = chunk_text(&text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            // Round-tripping through String guarantees valid UTF-8; this
            // just documents the invariant under test.
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
        // Every accented word must survive intact somewhere in some chunk.
        let rejoined: String = chunks.join("");
        assert!(rejoined.contains("canción"));
        assert!(rejoined.contains("página"));
    }
}
