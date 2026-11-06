use std::collections::HashMap;

use super::*;

fn text(cosines: &[f64]) -> Vec<(Modality, f64)> {
    cosines.iter().map(|&c| (Modality::Text, c)).collect()
}

#[test]
fn a_vector_that_stands_out_gets_a_high_z() {
    let mut scores = text(&[0.80; 30]);
    scores[0].1 = 0.70;
    scores[1].1 = 0.90;
    scores.push((Modality::Text, 0.95));
    let z = meaning_z(&scores);
    assert!(z[scores.len() - 1] > 2.0, "{z:?}");
    assert!(z[2].abs() < 1.0, "{z:?}");
}

#[test]
fn each_modality_is_centered_on_its_own_average() {
    // e5 text cosines sit near 0.8, SigLIP image cosines near 0.1: the best
    // image must not read as far below every text vector.
    let mut scores = text(&[0.80; 25]);
    scores.extend((0..25).map(|_| (Modality::Visual, 0.10)));
    scores[0].1 = 0.90;
    scores[25].1 = 0.20;
    let z = meaning_z(&scores);
    assert!((z[0] - z[25]).abs() < 1e-9, "{} vs {}", z[0], z[25]);
    assert!(z[25] > 2.0, "{z:?}");
}

#[test]
fn a_small_modality_takes_the_overall_average() {
    // Under 20 vectors an average is noise: a lone image would be its own
    // mean and score 0 however good it was.
    let mut scores = text(&[0.80; 25]);
    scores.push((Modality::Visual, 0.95));
    let z = meaning_z(&scores);
    assert!(z[25] > 2.0, "{z:?}");
}

#[test]
fn identical_scores_give_zero_not_nan() {
    let z = meaning_z(&text(&[0.5; 5]));
    assert!(z.iter().all(|&v| v == 0.0), "{z:?}");
    assert!(meaning_z(&[]).is_empty());
}

#[test]
fn query_terms_drop_stopwords_single_letters_and_repeats() {
    assert_eq!(
        query_terms("The Canción de la Playa, playa y el mar"),
        vec!["cancion", "playa", "mar"]
    );
    assert_eq!(query_terms("a dog on the beach"), vec!["dog", "beach"]);
    assert_eq!(
        query_terms("struct Point origin"),
        vec!["struct", "point", "origin"]
    );
    assert!(query_terms("the of a").is_empty());
}

#[test]
fn a_query_of_codes_is_a_lookup() {
    let terms = |q: &str| query_terms(q);
    assert!(is_lookup(&terms("K7Q2LM")));
    assert!(is_lookup(&terms("4815162342 TRX50")));
    assert!(!is_lookup(&terms("invoice 2024")));
    assert!(!is_lookup(&terms("electrician invoice")));
    assert!(!is_lookup(&[]));
}

fn ids(ranked: &[(i64, f64)]) -> Vec<i64> {
    ranked.iter().map(|(id, _)| *id).collect()
}

#[test]
fn a_file_is_a_candidate_by_z_or_by_coverage() {
    let z = HashMap::from([(1, 2.0), (2, 1.9), (3, 0.0)]);
    let coverage = HashMap::from([(3, 0.5), (4, 0.49)]);
    let ranked = fuse(&z, &coverage, false, &ZFusionParams::default());
    let mut found = ids(&ranked);
    found.sort();
    assert_eq!(found, vec![1, 3]);
}

#[test]
fn the_score_is_weighted_z_plus_weighted_coverage_to_the_1_5() {
    let params = ZFusionParams::default();
    let z = HashMap::from([(1, 3.0)]);
    let coverage = HashMap::from([(1, 0.5)]);
    let ranked = fuse(&z, &coverage, false, &params);
    let expected = params.z_weight * 3.0 + params.word_weight * 0.5f64.powf(1.5);
    assert!((ranked[0].1 - expected).abs() < 1e-9);

    // A lookup halves meaning and doubles words.
    let ranked = fuse(&z, &coverage, true, &params);
    let expected = 0.5 * params.z_weight * 3.0 + 2.0 * params.word_weight * 0.5f64.powf(1.5);
    assert!((ranked[0].1 - expected).abs() < 1e-9);
}

#[test]
fn ranked_by_score_with_ties_broken_by_id() {
    // Every word (2.0) ranks below a z of 2.1 at the default weights.
    let z = HashMap::from([(1, 2.1), (5, 2.5), (2, 2.5)]);
    let coverage = HashMap::from([(9, 1.0)]);
    let ranked = fuse(&z, &coverage, false, &ZFusionParams::default());
    assert_eq!(ids(&ranked), vec![2, 5, 1, 9]);
}

#[test]
fn a_negative_z_adds_nothing() {
    let z = HashMap::from([(1, -3.0)]);
    let coverage = HashMap::from([(1, 1.0)]);
    let ranked = fuse(&z, &coverage, false, &ZFusionParams::default());
    assert!((ranked[0].1 - ZFusionParams::default().word_weight).abs() < 1e-9);
}

#[test]
fn strong_count_stops_at_half_the_top_score() {
    assert_eq!(strong_count(&[4.0, 3.0, 2.0, 1.9, 5.0], 0.5), 3);
    assert_eq!(strong_count(&[], 0.5), 0);
    assert_eq!(strong_count(&[0.0, 0.0], 0.5), 2);
}

#[test]
fn files_are_found_by_word_coverage_and_nonsense_finds_nothing() {
    use crate::search::tests::{index_text, open_test_db};
    let (_dir, mut conn) = open_test_db();
    index_text(
        &mut conn,
        "/roots/a/bill.txt",
        "electrician invoice for March",
        0,
    );
    index_text(
        &mut conn,
        "/roots/a/trip.txt",
        "notes from the beach trip",
        0,
    );
    let none = QueryVectors::default();
    let params = ZFusionParams::default();

    // Half the words is enough; the file with none of them is not a candidate.
    let hits = search_zscore(&conn, &none, "electrician plumber", 10, &params).unwrap();
    let paths: Vec<_> = hits.iter().map(|h| h.path.to_str().unwrap()).collect();
    assert_eq!(paths, vec!["/roots/a/bill.txt"]);
    assert_eq!(
        hits[0].match_sources,
        Vec::new(),
        "not every word: no keyword hit"
    );

    let hits = search_zscore(&conn, &none, "electrician invoice", 10, &params).unwrap();
    assert_eq!(
        hits[0].match_sources,
        vec![crate::dto::MatchSource::Keyword]
    );

    // RRF returns its nearest vectors for anything; this returns nothing.
    assert!(
        search_zscore(&conn, &none, "zxqv blorft", 10, &params)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn files_with_term_matches_any_chunk_and_prefixes_on_request() {
    use crate::search::tests::{index_text, open_test_db};
    let (_dir, mut conn) = open_test_db();
    index_text(&mut conn, "/roots/a/song.txt", "una canción de cuna", 0);
    let ids = |term, prefix| fts::files_with_term(&conn, term, prefix).unwrap().len();
    assert_eq!(ids("cancion", false), 1);
    assert_eq!(ids("canc", false), 0);
    assert_eq!(ids("canc", true), 1);
    assert_eq!(ids("\"x", false), 0);
}

#[test]
fn with_words_gated_a_minority_of_the_words_adds_nothing() {
    let z = HashMap::from([(1, 2.0), (2, 2.0)]);
    let coverage = HashMap::from([(1, 0.25), (2, 0.5)]);
    let params = ZFusionParams {
        words_need_gate: true,
        ..ZFusionParams::default()
    };
    let ranked: HashMap<i64, f64> = fuse(&z, &coverage, false, &params).into_iter().collect();
    assert!((ranked[&1] - 2.0).abs() < 1e-9);
    assert!((ranked[&2] - (2.0 + 2.0 * 0.5f64.powf(1.5))).abs() < 1e-9);
}

#[test]
fn a_reference_sets_the_scale_and_the_scores_are_placed_on_it() {
    let reference = text(&[0.4, 0.6].repeat(10));
    // Mean 0.5, deviation 0.1: 0.7 sits two deviations up, whatever the
    // scores themselves look like.
    let z = meaning_z_against(&text(&[0.7, 0.5]), &reference);
    assert!((z[0] - 2.0).abs() < 1e-9 && z[1].abs() < 1e-9, "{z:?}");
    // Against itself it is `meaning_z`.
    let scores = text(&[0.1, 0.2, 0.9]);
    assert_eq!(meaning_z_against(&scores, &scores), meaning_z(&scores));
    assert_eq!(meaning_z_against(&scores, &[]), vec![0.0; 3]);
}
