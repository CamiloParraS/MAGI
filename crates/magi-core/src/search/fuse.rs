//! Reciprocal Rank Fusion plus filename/recency boosts. Implemented in M3
//! (see SPEC.md §5.6).

use std::collections::HashMap;

/// RRF's rank-damping constant (SPEC.md §5.6 step 4: `score = Σ w_i / (60 + rank_i)`).
const RRF_K: f64 = 60.0;

/// One ranked list contributing to fusion: `file_id`s in rank order
/// (best/rank-1 first), plus that list's weight.
pub struct RankedList<'a> {
    pub file_ids: &'a [i64],
    pub weight: f64,
}

/// Fuses ranked lists with Reciprocal Rank Fusion (SPEC.md §5.6 step 4). A
/// file absent from a list contributes 0 for that list, not an
/// infinite-rank penalty. Returns `(file_id, score)` sorted descending.
pub fn reciprocal_rank_fusion(lists: &[RankedList]) -> Vec<(i64, f64)> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in lists {
        for (index, &file_id) in list.file_ids.iter().enumerate() {
            let rank = (index + 1) as f64;
            *scores.entry(file_id).or_insert(0.0) += list.weight / (RRF_K + rank);
        }
    }
    let mut ranked: Vec<(i64, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked
}

fn tokenize_filename(file_name: &str) -> std::collections::HashSet<String> {
    file_name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Filename-token overlap boost (SPEC.md §5.6 step 5): up to ×1.2,
/// scaled by the fraction of query tokens present in the file name.
pub fn filename_boost(query_tokens: &[String], file_name: &str) -> f64 {
    if query_tokens.is_empty() {
        return 1.0;
    }
    let name_tokens = tokenize_filename(file_name);
    let matched = query_tokens
        .iter()
        .filter(|t| name_tokens.contains(&t.to_lowercase()))
        .count();
    let overlap = matched as f64 / query_tokens.len() as f64;
    1.0 + 0.2 * overlap
}

/// Recency boost (SPEC.md §5.6 step 5): up to ×1.1 for a file modified
/// just now, decaying linearly to ×1.0 at 30 days old and beyond.
pub fn recency_boost(modified_at_unix: i64, now_unix: i64) -> f64 {
    const WINDOW_SECS: i64 = 30 * 24 * 60 * 60;
    let age = (now_unix - modified_at_unix).max(0);
    if age >= WINDOW_SECS {
        return 1.0;
    }
    let fraction = 1.0 - (age as f64 / WINDOW_SECS as f64);
    1.0 + 0.1 * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_present_in_both_lists_ranks_above_single_list_file() {
        let fts = [1, 2];
        let vector = [2, 3];
        let ranked = reciprocal_rank_fusion(&[
            RankedList {
                file_ids: &fts,
                weight: 1.0,
            },
            RankedList {
                file_ids: &vector,
                weight: 1.0,
            },
        ]);
        assert_eq!(ranked[0].0, 2, "file 2 is in both lists, must rank first");
    }

    #[test]
    fn missing_from_a_list_contributes_zero_not_a_penalty() {
        let fts = [1];
        let vector: [i64; 0] = [];
        let ranked = reciprocal_rank_fusion(&[
            RankedList {
                file_ids: &fts,
                weight: 1.0,
            },
            RankedList {
                file_ids: &vector,
                weight: 1.0,
            },
        ]);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0], (1, 1.0 / 61.0));
    }

    #[test]
    fn empty_lists_yield_no_results() {
        let empty: [i64; 0] = [];
        let ranked = reciprocal_rank_fusion(&[RankedList {
            file_ids: &empty,
            weight: 1.0,
        }]);
        assert!(ranked.is_empty());
    }

    #[test]
    fn filename_boost_full_overlap_is_max() {
        let tokens = vec!["invoice".to_string(), "electrician".to_string()];
        let boost = filename_boost(&tokens, "electrician_invoice.pdf");
        assert!((boost - 1.2).abs() < 1e-9);
    }

    #[test]
    fn filename_boost_no_overlap_is_one() {
        let tokens = vec!["recipe".to_string()];
        assert_eq!(filename_boost(&tokens, "quarterly_report.pdf"), 1.0);
    }

    #[test]
    fn filename_boost_empty_query_is_one() {
        assert_eq!(filename_boost(&[], "anything.pdf"), 1.0);
    }

    #[test]
    fn recency_boost_is_max_for_just_modified() {
        let now = 1_700_000_000;
        assert!((recency_boost(now, now) - 1.1).abs() < 1e-9);
    }

    #[test]
    fn recency_boost_decays_to_one_after_30_days() {
        let now = 1_700_000_000;
        let thirty_days_ago = now - 30 * 24 * 60 * 60;
        assert_eq!(recency_boost(thirty_days_ago, now), 1.0);
        let sixty_days_ago = now - 60 * 24 * 60 * 60;
        assert_eq!(recency_boost(sixty_days_ago, now), 1.0);
    }

    #[test]
    fn tied_scores_break_deterministically_by_file_id() {
        let fts = [5, 2];
        let vector = [2, 5];
        let ranked = reciprocal_rank_fusion(&[
            RankedList {
                file_ids: &fts,
                weight: 1.0,
            },
            RankedList {
                file_ids: &vector,
                weight: 1.0,
            },
        ]);
        // Both files 2 and 5 get 1/61 + 1/62 — exactly tied. Lower file_id wins the tie.
        assert_eq!(ranked[0].0, 2);
        assert_eq!(ranked[1].0, 5);
    }
}
