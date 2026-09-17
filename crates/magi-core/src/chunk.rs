//! Token-aware chunking (SPEC.md §7 M3). Splits body text on word
//! boundaries, growing each chunk up to [`TARGET_MAX_TOKENS`] tokens (well
//! under [`MAX_TOKENS`] including the embedder's `"query: "`/`"passage: "`
//! prefix), with an [`OVERLAP_TOKENS`]-token overlap between consecutive
//! chunks. Token counts come from the real e5 tokenizer once it's been
//! downloaded (`embed::manager::shared_text_tokenizer`); before that
//! (tests, first run before `just models`) they fall back to a whitespace
//! word-count approximation. Replaces M2's provisional character-based
//! chunker.

/// Hard ceiling per SPEC.md §7 M3, including the embedder's query/passage
/// prefix (2-4 tokens for e5's `"query: "` / `"passage: "`).
pub const MAX_TOKENS: usize = 512;
/// Tokens reserved for the prefix a `TextEmbedder` adds before embedding.
pub const PREFIX_RESERVE_TOKENS: usize = 8;
/// Upper end of SPEC.md §7 M3's "target 256-400 tokens" — chunks grow up
/// to this many tokens before starting a new one.
pub const TARGET_MAX_TOKENS: usize = 400;
/// Tokens of overlap carried into the next chunk.
pub const OVERLAP_TOKENS: usize = 50;

const _: () = assert!(TARGET_MAX_TOKENS + PREFIX_RESERVE_TOKENS <= MAX_TOKENS);

/// Splits `text` into word units, each holding one word plus all
/// whitespace up to the start of the next word (or the end of the text).
/// Concatenating a contiguous run of units reproduces that span of `text`
/// byte-for-byte — chunking never rewrites line breaks or collapses
/// spacing the way a naive `split_whitespace().join(" ")` would.
fn split_units_preserving_whitespace(text: &str) -> Vec<&str> {
    let mut word_starts = Vec::new();
    let mut prev_was_whitespace = true;
    for (i, ch) in text.char_indices() {
        let is_whitespace = ch.is_whitespace();
        if !is_whitespace && prev_was_whitespace {
            word_starts.push(i);
        }
        prev_was_whitespace = is_whitespace;
    }
    word_starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = word_starts.get(i + 1).copied().unwrap_or(text.len());
            &text[start..end]
        })
        .collect()
}

/// Word-level greedy chunker parameterized on a token-counting function,
/// so the boundary logic is testable without a real tokenizer.
///
/// ponytail: sums each unit's own token count as a proxy for the joined
/// chunk's real token count, rather than re-tokenizing the growing
/// substring on every candidate word. BPE/Unigram merges essentially never
/// cross a whitespace boundary (the space itself is part of the next
/// token), so this is exact in practice for e5's tokenizer; upgrade to
/// re-tokenizing the candidate substring if eval ever shows drift. A
/// single word whose own token count exceeds `TARGET_MAX_TOKENS` (a huge
/// URL/hash) still becomes its own one-word chunk rather than looping
/// forever or splitting mid-token.
fn chunk_by_token_counter(text: &str, count_tokens: impl Fn(&str) -> usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let units = split_units_preserving_whitespace(text);
    let unit_tokens: Vec<usize> = units.iter().map(|u| count_tokens(u).max(1)).collect();

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < units.len() {
        let mut end = start;
        let mut total = 0usize;
        while end < units.len() && (end == start || total + unit_tokens[end] <= TARGET_MAX_TOKENS) {
            total += unit_tokens[end];
            end += 1;
        }
        chunks.push(units[start..end].concat().trim().to_string());
        if end >= units.len() {
            break;
        }
        let mut back = end;
        let mut overlap = 0usize;
        while back > start && overlap < OVERLAP_TOKENS {
            back -= 1;
            overlap += unit_tokens[back];
        }
        start = back.max(start + 1);
    }
    chunks
}

/// Whitespace word count as a token-count proxy when the real e5
/// tokenizer isn't available yet. Subword tokenization usually yields
/// somewhat more tokens than words, so this undercounts — acceptable
/// because it only affects chunk-size precision, never correctness, and
/// never applies once a model is downloaded.
pub(crate) fn approx_token_count(text: &str) -> usize {
    text.split_whitespace().count().max(1)
}

fn count_tokens(text: &str) -> usize {
    match crate::embed::manager::shared_text_tokenizer() {
        Some(tokenizer) => tokenizer
            .encode(text, false)
            .map(|encoding| encoding.len())
            .unwrap_or_else(|_| approx_token_count(text)),
        None => approx_token_count(text),
    }
}

/// Splits `text` into overlapping, token-bounded chunks. Returns no chunks
/// for empty or whitespace-only input.
pub fn chunk_text(text: &str) -> Vec<String> {
    chunk_by_token_counter(text, count_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One token per word — isolates the boundary/overlap logic from
    /// tokenizer specifics.
    fn word_counter(text: &str) -> usize {
        text.split_whitespace().count().max(1)
    }

    #[test]
    fn empty_text_yields_no_chunks() {
        assert!(chunk_by_token_counter("", word_counter).is_empty());
        assert!(chunk_by_token_counter("   \n\t  ", word_counter).is_empty());
    }

    #[test]
    fn short_text_is_a_single_chunk() {
        let text = "hello world";
        assert_eq!(
            chunk_by_token_counter(text, word_counter),
            vec![text.to_string()]
        );
    }

    #[test]
    fn whitespace_and_line_breaks_are_preserved_within_a_chunk() {
        let text = "Title\n\nFirst paragraph line one.\nLine two.\n\nSecond paragraph.";
        let chunks = chunk_by_token_counter(text, word_counter);
        assert_eq!(chunks, vec![text.to_string()]);
    }

    #[test]
    fn text_at_exact_target_is_a_single_chunk() {
        let words = vec!["w"; TARGET_MAX_TOKENS].join(" ");
        let chunks = chunk_by_token_counter(&words, word_counter);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn long_text_is_split_with_token_overlap() {
        // One token per word, so this is well past TARGET_MAX_TOKENS.
        let words: Vec<String> = (0..1000).map(|i| format!("w{i}")).collect();
        let text = words.join(" ");
        let chunks = chunk_by_token_counter(&text, word_counter);

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(word_counter(chunk) <= TARGET_MAX_TOKENS);
        }
        // Consecutive chunks share their overlap region verbatim.
        let first_words: Vec<&str> = chunks[0].split_whitespace().collect();
        let second_words: Vec<&str> = chunks[1].split_whitespace().collect();
        let overlap_start = &first_words[first_words.len() - OVERLAP_TOKENS..];
        let overlap_in_second = &second_words[..OVERLAP_TOKENS];
        assert_eq!(overlap_start, overlap_in_second);
    }

    #[test]
    fn chunking_always_makes_forward_progress() {
        // Every "word" alone exceeds TARGET_MAX_TOKENS: each becomes its
        // own chunk instead of looping forever or splitting mid-token.
        let huge_counter = |_: &str| TARGET_MAX_TOKENS * 2;
        let text = "aaaa bbbb cccc dddd";
        let chunks = chunk_by_token_counter(text, huge_counter);
        assert_eq!(chunks, vec!["aaaa", "bbbb", "cccc", "dddd"]);
    }

    #[test]
    fn unicode_chars_are_never_split_mid_codepoint() {
        let sentence = "La canción tiene una página con ñ, á, é, í, ó, ú.";
        let text = std::iter::repeat_n(sentence, 100)
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_by_token_counter(&text, word_counter);

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
        let rejoined: String = chunks.join(" ");
        assert!(rejoined.contains("canción"));
        assert!(rejoined.contains("página"));
    }

    #[test]
    fn public_chunk_text_falls_back_without_a_downloaded_tokenizer() {
        // No `just models` run in the test environment, so this exercises
        // the approx-token-count fallback path end to end.
        let text = "The quick brown fox jumps over the lazy dog.";
        assert_eq!(chunk_text(text), vec![text.to_string()]);
    }

    #[test]
    fn approx_token_count_is_at_least_one_for_nonempty_text() {
        assert_eq!(approx_token_count("word"), 1);
        assert!(approx_token_count("several words here") >= 1);
    }
}
