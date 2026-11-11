//! What Ask Magi sends for a file (ADR-0014). By default, the passages of that
//! file that best match the question, not the snippet the search happened to
//! show: the file's own chunks are ranked by keywords and, when a text
//! embedder is running, by meaning, and the best ones are sent up to a
//! character budget. A file the person switched to "whole" sends its indexed
//! text in order, up to a larger budget; a picture sends itself ([`image`]).
//! Never the path. The server counts in UTF-16 units, so the budgets do too.

use rusqlite::{Connection, OptionalExtension, params};

use super::image;
use super::{AskFile, AskImage, AskPayload, AskSource, SendAs};
use crate::db::roots::searchable_sql;
use crate::error::Result;

/// The most text sent for all files of one question.
pub const TOTAL_BUDGET: usize = 8000;
/// The most one file gets, and the least, however many files are ticked.
pub const MAX_PER_FILE: usize = 4000;
pub const MIN_PER_FILE: usize = 1200;
/// What a file sent whole may take, and all text of one question together.
pub const WHOLE_PER_FILE: usize = 60_000;
pub const WHOLE_TOTAL: usize = 120_000;
/// How many bytes at the start of a chunk may repeat the end of the one
/// before (the chunker's 50-token overlap, with room for long words).
const MAX_OVERLAP_BYTES: usize = 2000;
/// A file's chunks past this many are not looked at (a 15 MB text file has
/// thousands).
const MAX_CANDIDATES: i64 = 5000;
/// Below this much budget left, another passage is not worth its own gap.
const MIN_PASSAGE: usize = 200;
const SEPARATOR: &str = "\n\n";
/// Reciprocal rank fusion's constant, as in the search itself.
const RRF_K: f64 = 60.0;

/// One chunk of a file, as it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub ordinal: i64,
    pub text: String,
    pub embedding: Option<Vec<f32>>,
}

/// What each file may send when `files` are ticked.
pub fn per_file_budget(files: usize) -> usize {
    (TOTAL_BUDGET / files.max(1)).clamp(MIN_PER_FILE, MAX_PER_FILE)
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text` cut to at most `max` UTF-16 units, never inside a character.
fn clip(text: &str, max: usize) -> &str {
    let mut used = 0;
    for (ix, c) in text.char_indices() {
        used += c.len_utf16();
        if used > max {
            return &text[..ix];
        }
    }
    text
}

fn terms(question: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for word in question.split(|c: char| !c.is_alphanumeric()) {
        let word = word.to_lowercase();
        if word.chars().count() >= 3 && !found.contains(&word) {
            found.push(word);
        }
    }
    found
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() {
        return 0.0;
    }
    // The embeddings are unit vectors, so the dot product is the cosine.
    a.iter().zip(b).map(|(x, y)| f64::from(x * y)).sum()
}

/// The passages of `candidates` that best answer `question`, in the file's
/// order, at most `budget` units long. Empty when there is nothing to send.
pub fn select(
    question: &str,
    query: Option<&[f32]>,
    candidates: &[Candidate],
    budget: usize,
) -> String {
    let terms = terms(question);
    let keyword: Vec<usize> = candidates
        .iter()
        .map(|c| {
            let text = c.text.to_lowercase();
            terms.iter().filter(|t| text.contains(t.as_str())).count()
        })
        .collect();
    let meaning: Vec<Option<f64>> = candidates
        .iter()
        .map(|c| match (query, &c.embedding) {
            (Some(q), Some(e)) => Some(cosine(q, e)),
            _ => None,
        })
        .collect();

    let mut score = vec![0.0_f64; candidates.len()];
    let mut by_keyword: Vec<usize> = (0..candidates.len()).filter(|&i| keyword[i] > 0).collect();
    by_keyword.sort_by(|&a, &b| keyword[b].cmp(&keyword[a]).then(a.cmp(&b)));
    for (rank, &i) in by_keyword.iter().enumerate() {
        score[i] += 1.0 / (RRF_K + rank as f64);
    }
    let mut by_meaning: Vec<usize> = (0..candidates.len())
        .filter(|&i| meaning[i].is_some())
        .collect();
    by_meaning.sort_by(|&a, &b| {
        let (a_score, b_score) = (meaning[a].unwrap_or(0.0), meaning[b].unwrap_or(0.0));
        b_score.total_cmp(&a_score).then(a.cmp(&b))
    });
    for (rank, &i) in by_meaning.iter().enumerate() {
        score[i] += 1.0 / (RRF_K + rank as f64);
    }

    // Best first; with nothing to rank by, the start of the file.
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by(|&a, &b| score[b].total_cmp(&score[a]).then(a.cmp(&b)));

    let mut chosen: Vec<(i64, &str)> = Vec::new();
    let mut used = 0;
    for i in order {
        let gap = if chosen.is_empty() {
            0
        } else {
            SEPARATOR.len()
        };
        let left = budget.saturating_sub(used + gap);
        let whole = candidates[i].text.trim();
        // A passage that would be cut to a stub is not worth its gap.
        if left < MIN_PASSAGE && utf16_len(whole) > left && !chosen.is_empty() {
            break;
        }
        let text = clip(whole, left);
        if text.is_empty() {
            continue;
        }
        used += gap + utf16_len(text);
        chosen.push((candidates[i].ordinal, text));
        if used >= budget {
            break;
        }
    }
    chosen.sort_by_key(|(ordinal, _)| *ordinal);
    chosen
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_ne_bytes(*b))
        .collect()
}

/// A searchable file's text chunks in order, with their vectors when it has
/// them. Nothing for a file that is gone or in a root that is paused or
/// removed, so Ask Magi cannot send what search would not show.
pub fn load(conn: &Connection, file_id: i64) -> Result<Vec<Candidate>> {
    let mut stmt = conn.prepare(concat!(
        "SELECT c.id, c.ordinal, c.text
         FROM chunks c
         JOIN files f ON f.id = c.file_id
         JOIN roots r ON r.id = f.root_id
         WHERE c.file_id = ?1 AND c.source IN ('body', 'ocr', 'code_symbol') AND ",
        searchable_sql!(),
        "
         ORDER BY c.ordinal
         LIMIT ?2"
    ))?;
    let rows = stmt
        .query_map(params![file_id, MAX_CANDIDATES], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut vector = conn.prepare("SELECT embedding FROM vec_text WHERE chunk_id = ?1")?;
    let mut out = Vec::with_capacity(rows.len());
    for (chunk_id, ordinal, text) in rows {
        let embedding = vector
            .query_row(params![chunk_id], |row| row.get::<_, Vec<u8>>(0))
            .optional()?
            .map(|blob| blob_to_embedding(&blob));
        out.push(Candidate {
            ordinal,
            text,
            embedding,
        });
    }
    Ok(out)
}

/// The first of a searchable file's text chunks, read alone: [`load`] reads
/// them all with their vectors, too much for a chip's preview.
pub fn first_text(conn: &Connection, file_id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            concat!(
                "SELECT c.text
                 FROM chunks c
                 JOIN files f ON f.id = c.file_id
                 JOIN roots r ON r.id = f.root_id
                 WHERE c.file_id = ?1 AND c.source IN ('body', 'ocr', 'code_symbol')
                   AND trim(c.text) <> '' AND ",
                searchable_sql!(),
                "
                 ORDER BY c.ordinal
                 LIMIT 1"
            ),
            params![file_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

/// The longest start of `next` that `prev` ends with, cut at a word end:
/// the overlap the chunker carried from one chunk into the next.
fn overlap(prev: &str, next: &str) -> usize {
    let limit = next.len().min(prev.len()).min(MAX_OVERLAP_BYTES);
    let mut ends: Vec<usize> = next
        .char_indices()
        .take_while(|(ix, _)| *ix <= limit)
        .filter(|(_, c)| c.is_whitespace())
        .map(|(ix, _)| ix)
        .collect();
    if next.len() <= limit {
        ends.push(next.len());
    }
    let word_start = |end: usize| {
        let before = &prev[..prev.len() - end];
        before.is_empty() || before.ends_with(char::is_whitespace)
    };
    ends.into_iter()
        .rev()
        .find(|&end| end > 0 && prev.ends_with(&next[..end]) && word_start(end))
        .unwrap_or(0)
}

/// The file's chunks in order, with the overlap between neighbours taken out,
/// at most `budget` units. The second value says whether it was cut short.
pub fn whole(candidates: &[Candidate], budget: usize) -> (String, bool) {
    let mut out = String::new();
    let mut used = 0;
    let mut prev: Option<&str> = None;
    for candidate in candidates {
        let text = candidate.text.trim();
        if text.is_empty() {
            continue;
        }
        let piece = match prev {
            Some(p) => match overlap(p, text) {
                0 => format!("{SEPARATOR}{text}"),
                n => text[n..].to_owned(),
            },
            None => text.to_owned(),
        };
        let left = budget.saturating_sub(used);
        let size = utf16_len(&piece);
        if size > left {
            out.push_str(clip(&piece, left));
            return (out, true);
        }
        out.push_str(&piece);
        used += size;
        prev = Some(text);
    }
    (out, false)
}

/// One source per ticked file, numbered as the chips are, each with the
/// passages of that file that best match the question.
pub fn sources_for(
    conn: &Connection,
    query: Option<&[f32]>,
    question: &str,
    files: &[AskFile],
) -> Result<Vec<AskSource>> {
    let passages: Vec<AskFile> = files
        .iter()
        .cloned()
        .map(|f| AskFile {
            send: SendAs::Passages,
            ..f
        })
        .collect();
    Ok(payload_for(conn, query, question, &passages)?.sources)
}

/// The picture for `file`, if it should and can go as one.
fn picture(conn: &Connection, file: &AskFile) -> Result<Option<String>> {
    let Some(path) = image::image_path(conn, file.file_id)? else {
        return Ok(None);
    };
    match image::encode_file(&path) {
        Ok(data) => Ok(Some(data)),
        Err(reason) => {
            tracing::warn!(file_id = file.file_id, %reason, "picture not sent; sending its text");
            Ok(None)
        }
    }
}

/// What one question sends: for each ticked file, numbered as the chips are,
/// its passages, its whole text or its picture, as the person chose. A
/// picture that cannot be read, or past [`image::MAX_IMAGES`], goes as text.
pub fn payload_for(
    conn: &Connection,
    query: Option<&[f32]>,
    question: &str,
    files: &[AskFile],
) -> Result<AskPayload> {
    let mut images: Vec<AskImage> = Vec::new();
    let mut as_text: Vec<(usize, &AskFile, SendAs)> = Vec::new();
    for (ix, file) in files.iter().enumerate() {
        if file.send == SendAs::Image
            && images.len() < image::MAX_IMAGES
            && let Some(data) = picture(conn, file)?
        {
            images.push(AskImage {
                id: (ix + 1).to_string(),
                file_name: file.file_name.clone(),
                data,
            });
            continue;
        }
        let send = match file.send {
            SendAs::Whole => SendAs::Whole,
            _ => SendAs::Passages,
        };
        as_text.push((ix, file, send));
    }

    let passage_files = as_text
        .iter()
        .filter(|(_, _, send)| *send == SendAs::Passages)
        .count();
    let passage_budget = per_file_budget(passage_files);
    // Passages keep their own share; whole files get what is left.
    let mut whole_left = WHOLE_TOTAL.saturating_sub(passage_budget * passage_files);
    let mut sources = Vec::with_capacity(as_text.len());
    for (ix, file, send) in as_text {
        let candidates = load(conn, file.file_id)?;
        let mut text = match send {
            SendAs::Whole => {
                let (text, _cut) = whole(&candidates, WHOLE_PER_FILE.min(whole_left));
                whole_left = whole_left.saturating_sub(utf16_len(&text));
                text
            }
            _ => select(question, query, &candidates, passage_budget),
        };
        if text.is_empty() {
            text = clip(file.fallback.trim(), passage_budget).to_owned();
        }
        sources.push(AskSource {
            id: (ix + 1).to_string(),
            file_name: file.file_name.clone(),
            text,
        });
    }
    Ok(AskPayload { sources, images })
}

/// How much "whole" would send for a file, from the index alone: its text in
/// UTF-16 units once the overlap is out, and whether [`WHOLE_PER_FILE`] cuts
/// it. For the chip, before anything is sent.
pub fn whole_size(conn: &Connection, file_id: i64) -> Result<(usize, bool)> {
    let (text, cut) = whole(&load(conn, file_id)?, WHOLE_PER_FILE);
    Ok((utf16_len(&text), cut))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(ordinal: i64, text: &str) -> Candidate {
        Candidate {
            ordinal,
            text: text.to_owned(),
            embedding: None,
        }
    }

    #[test]
    fn the_budget_shrinks_with_more_files_but_stays_in_bounds() {
        assert_eq!(per_file_budget(1), MAX_PER_FILE);
        assert_eq!(per_file_budget(2), 4000);
        assert_eq!(per_file_budget(5), 1600);
        assert_eq!(per_file_budget(20), MIN_PER_FILE);
        assert_eq!(per_file_budget(0), MAX_PER_FILE);
    }

    #[test]
    fn the_chunk_with_the_question_words_is_sent_not_the_first_one() {
        let chunks = [
            chunk(0, "Cover page and table of contents"),
            chunk(1, "More front matter"),
            chunk(
                2,
                "A buyer persona is a semi-fictional profile of an ideal customer.",
            ),
        ];
        let text = select("what is a buyer persona?", None, &chunks, 70);
        assert!(text.starts_with("A buyer persona"), "{text}");
    }

    #[test]
    fn passages_come_back_in_the_files_order_and_fit_the_budget() {
        let chunks = [
            chunk(0, "alpha one"),
            chunk(1, "filler"),
            chunk(2, "alpha beta two"),
        ];
        let text = select("alpha beta", None, &chunks, 28);
        assert_eq!(text, "alpha one\n\nalpha beta two");
        let tight = select("alpha beta", None, &chunks, 20);
        assert!(utf16_len(&tight) <= 20, "{tight}");
    }

    #[test]
    fn meaning_ranks_a_chunk_that_shares_no_word_with_the_question() {
        let mut near = chunk(5, "Customers we want to reach, described as a person.");
        near.embedding = Some(vec![1.0, 0.0]);
        let mut far = chunk(1, "Shipping costs and delivery times.");
        far.embedding = Some(vec![0.0, 1.0]);
        let text = select(
            "who is the ideal client",
            Some(&[1.0, 0.0]),
            &[far, near],
            60,
        );
        assert!(text.starts_with("Customers we want"), "{text}");
    }

    #[test]
    fn with_nothing_to_rank_by_the_start_of_the_file_is_sent() {
        let chunks = [chunk(0, "first part"), chunk(1, "second part")];
        assert_eq!(select("??", None, &chunks, 12), "first part");
    }

    #[test]
    fn a_cut_never_lands_inside_a_character_and_counts_utf16_units() {
        let chunks = [chunk(0, "ñandú 😀😀😀😀")];
        let text = select("", None, &chunks, 8);
        assert!(utf16_len(&text) <= 8);
        assert!(!text.is_empty());
    }

    #[test]
    fn whole_takes_out_the_overlap_between_chunks() {
        let chunks = [
            chunk(0, "one two three four five"),
            chunk(1, "four five six seven"),
            chunk(2, "seven eight"),
        ];
        assert_eq!(
            whole(&chunks, 1000),
            ("one two three four five six seven eight".to_owned(), false)
        );
    }

    #[test]
    fn whole_keeps_chunks_that_do_not_overlap_apart() {
        let chunks = [chunk(0, "Page one."), chunk(1, "Page two.")];
        assert_eq!(whole(&chunks, 1000).0, "Page one.\n\nPage two.");
    }

    #[test]
    fn a_word_that_only_ends_the_same_is_not_overlap() {
        // "concat" ends with "cat", but "cat" is not a word of it.
        assert_eq!(overlap("go concat", "cat food"), 0);
    }

    #[test]
    fn whole_stops_at_its_budget_and_says_so() {
        let chunks = [chunk(0, "aaaa bbbb"), chunk(1, "cccc dddd")];
        let (text, cut) = whole(&chunks, 12);
        assert!(cut);
        assert_eq!(utf16_len(&text), 12);
    }

    #[test]
    fn nothing_to_send_is_empty() {
        assert_eq!(select("anything", None, &[], 100), "");
    }
}
