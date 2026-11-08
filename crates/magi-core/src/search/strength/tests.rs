use std::path::PathBuf;

use super::*;
use crate::db;
use crate::db::files::{FileRecord, FileState, upsert_file};
use crate::discovery::Kind;
use crate::embed::{FakeEmbedder, TextEmbedder};
use crate::extract::RawChunk;
use crate::search::vector::text_cosines;

fn hit(match_sources: Vec<MatchSource>, text: Option<f64>, image: Option<f64>) -> SearchHit {
    SearchHit {
        file_id: 1,
        path: PathBuf::from("/r/a.txt"),
        file_name: "a.txt".into(),
        kind: Kind::Text,
        mtime_ns: 0,
        thumb_key: None,
        score: 1.0,
        snippet: String::new(),
        match_sources,
        page: None,
        text_similarity: text,
        image_similarity: image,
        copies: Vec::new(),
    }
}

/// Mean 0.5, deviation 0.1.
fn scale() -> Scale {
    Scale {
        mean: 0.5,
        deviation: 0.1,
    }
}

const THRESHOLDS: Thresholds = Thresholds {
    text_z: 2.0,
    image_z: 3.0,
};

#[test]
fn a_scale_needs_enough_vectors_and_a_least_deviation() {
    assert_eq!(Scale::from_cosines(&[0.5; 19]), None);
    let flat = Scale::from_cosines(&[0.5; 20]).unwrap();
    assert_eq!((flat.mean, flat.deviation), (0.5, MIN_DEVIATION));
    let spread: Vec<f64> = (0..20)
        .map(|i| if i % 2 == 0 { 0.4 } else { 0.6 })
        .collect();
    let s = Scale::from_cosines(&spread).unwrap();
    assert!((s.mean - 0.5).abs() < 1e-9 && (s.deviation - 0.1).abs() < 1e-9);
    assert!((s.z(0.8) - 3.0).abs() < 1e-9);
}

#[test]
fn keyword_evidence_is_strong_whatever_the_cosine() {
    let scales = Scales {
        text: Some(scale()),
        image: Some(scale()),
    };
    let keyword = hit(
        vec![MatchSource::Keyword, MatchSource::Semantic],
        Some(0.1),
        None,
    );
    assert_eq!(strength(&keyword, &scales, &THRESHOLDS), Strength::Strong);
}

#[test]
fn a_meaning_match_is_strong_only_when_it_stands_out() {
    let scales = Scales {
        text: Some(scale()),
        image: Some(scale()),
    };
    let text = |c| hit(vec![MatchSource::Semantic], Some(c), None);
    assert_eq!(
        strength(&text(0.71), &scales, &THRESHOLDS),
        Strength::Strong
    );
    assert_eq!(strength(&text(0.69), &scales, &THRESHOLDS), Strength::Weak);
    // Images need z 3: the same cosine that makes text strong does not.
    let image = |c| hit(vec![MatchSource::Visual], None, Some(c));
    assert_eq!(strength(&image(0.70), &scales, &THRESHOLDS), Strength::Weak);
    assert_eq!(
        strength(&image(0.81), &scales, &THRESHOLDS),
        Strength::Strong
    );
    // Either list clearing its threshold is enough.
    let both = hit(vec![MatchSource::Semantic], Some(0.6), Some(0.81));
    assert_eq!(strength(&both, &scales, &THRESHOLDS), Strength::Strong);
}

#[test]
fn without_a_scale_nothing_is_called_weak() {
    let none = Scales::default();
    let text = hit(vec![MatchSource::Semantic], Some(0.0), None);
    assert_eq!(strength(&text, &none, &THRESHOLDS), Strength::Strong);
    // A name match with no keyword and no vector has nothing to judge by.
    let bare = hit(vec![], None, None);
    assert_eq!(strength(&bare, &none, &THRESHOLDS), Strength::Weak);
}

fn index(conn: &mut rusqlite::Connection, i: usize) {
    let path = PathBuf::from(format!("/roots/a/{i}.txt"));
    let rel = PathBuf::from(format!("{i}.txt"));
    let body = format!("document number {i} about topic {}", i % 7);
    let record = FileRecord {
        root_id: 1,
        path: &path,
        rel_path: &rel,
        file_name: rel.to_str().unwrap(),
        ext: Some("txt"),
        kind: "text",
        size: body.len() as u64,
        mtime_ns: 0,
        lang: None,
        state: FileState::Indexed,
        skip_reason: None,
        error: None,
        seen_scan_id: 1,
        content_hash: None,
        thumb_key: None,
        features_missing: 0,
    };
    let embeddings = FakeEmbedder.embed_passages(&[body.as_str()]).unwrap();
    upsert_file(
        conn,
        &record,
        &[RawChunk::body(body)],
        Some(&embeddings),
        None,
    )
    .unwrap();
}

#[test]
fn the_sample_is_spread_over_the_table_and_scores_like_the_knn() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open(&dir.path().join("magi.db")).unwrap();
    let root = tempfile::tempdir().unwrap();
    db::roots::add(&conn, root.path()).unwrap();
    for i in 0..60 {
        index(&mut conn, i);
    }
    let query = FakeEmbedder.embed_query("topic 3").unwrap();

    let all = sample_text_cosines(&conn, &query, 1000).unwrap();
    let mut knn: Vec<f64> = text_cosines(&conn, &query, 1000)
        .unwrap()
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    let mut sorted = all.clone();
    sorted.sort_by(f64::total_cmp);
    knn.sort_by(f64::total_cmp);
    assert_eq!(sorted.len(), knn.len());
    assert!(sorted.iter().zip(&knn).all(|(a, b)| (a - b).abs() < 1e-6));

    let some = sample_text_cosines(&conn, &query, 10).unwrap();
    assert!((8..=20).contains(&some.len()), "{} sampled", some.len());

    assert!(
        sample_image_cosines(&conn, &[0.1; 768], 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn only_a_meaning_match_below_the_floor_folds() {
    let at = |sources, text, image| fold_strength(&hit(sources, text, image));
    let semantic = || vec![MatchSource::Semantic];
    assert_eq!(at(semantic(), Some(0.79), None), Strength::Weak);
    assert_eq!(at(semantic(), Some(0.80), None), Strength::Strong);
    // Its words, or a visual match that passed IMAGE_MIN_COSINE, keep it up.
    let words = vec![MatchSource::Keyword, MatchSource::Semantic];
    assert_eq!(at(words, Some(0.1), None), Strength::Strong);
    assert_eq!(at(semantic(), Some(0.1), Some(0.11)), Strength::Strong);
    // Nothing to judge by is not a reason to hide it.
    assert_eq!(at(vec![], None, None), Strength::Strong);
}

#[test]
fn strong_results_move_ahead_and_each_group_keeps_its_order() {
    let at = |id, cosine| SearchHit {
        file_id: id,
        ..hit(vec![MatchSource::Semantic], Some(cosine), None)
    };
    let mut hits = vec![at(1, 0.70), at(2, 0.90), at(3, 0.75), at(4, 0.85)];
    strong_first(&mut hits);
    let order: Vec<i64> = hits.iter().map(|h| h.file_id).collect();
    assert_eq!(order, vec![2, 4, 1, 3]);
}
