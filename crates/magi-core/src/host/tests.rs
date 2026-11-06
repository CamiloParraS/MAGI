use super::*;
use crate::discovery::Kind;
use crate::dto::{Backfill, Install};

fn status(install: Install, backfill: Option<Backfill>) -> Vec<FeatureStatus> {
    vec![FeatureStatus {
        feature: Feature::Meaning,
        enabled: true,
        download_size: 100,
        install,
        backfill,
    }]
}

#[test]
fn a_hit_becomes_a_result_without_the_database() {
    let hit = |match_sources| SearchHit {
        file_id: 42,
        path: PathBuf::from("/photos/IMG_1.jpg"),
        file_name: "IMG_1.jpg".into(),
        kind: Kind::Image,
        mtime_ns: 3_000_000_000,
        thumb_key: None,
        score: 1.0,
        snippet: "IMG_1.jpg".into(),
        match_sources,
        page: None,
        text_similarity: None,
        image_similarity: None,
    };
    let visual = SearchResult::from(hit(vec![MatchSource::Visual]));
    assert_eq!(
        (
            visual.kind,
            visual.modified_at,
            visual.snippet,
            visual.thumb_path
        ),
        (Kind::Image, 3_000, None, None)
    );
    let keyword = SearchResult::from(hit(vec![MatchSource::Keyword]));
    assert!(keyword.snippet.is_some(), "only a visual-only hit loses it");
}

#[test]
fn queued_controls_fold_into_the_strongest() {
    let (a, _a) = crossbeam_channel::bounded(1);
    let (b, _b) = crossbeam_channel::bounded(1);
    let (next, folded) = coalesce(
        Control::Restart,
        [Control::Clear(a), Control::Restart, Control::Clear(b)].into_iter(),
    );
    assert!(matches!(next, Control::Clear(_)));
    assert_eq!(folded.len(), 1, "the other clear still gets a reply");

    let (c, _c) = crossbeam_channel::bounded(1);
    let (next, folded) = coalesce(
        Control::Clear(c),
        [Control::Restart, Control::Stop, Control::Restart].into_iter(),
    );
    assert!(matches!(next, Control::Stop));
    assert_eq!(folded.len(), 1);

    let (next, folded) = coalesce(
        Control::Restart,
        [Control::Restart, Control::Restart].into_iter(),
    );
    assert!(matches!(next, Control::Restart));
    assert!(folded.is_empty());
}

#[test]
fn progress_and_backfill_do_not_change_engine_inputs() {
    let base = engine_inputs(&status(
        Install::Downloading {
            bytes: 1,
            total: 100,
        },
        None,
    ));
    assert_eq!(
        base,
        engine_inputs(&status(
            Install::Downloading {
                bytes: 99,
                total: 100
            },
            None
        ))
    );
    let installed = engine_inputs(&status(Install::Installed { size_bytes: 100 }, None));
    assert_ne!(base, installed, "finishing a download restarts");
    assert_eq!(
        installed,
        engine_inputs(&status(
            Install::Installed { size_bytes: 100 },
            Some(Backfill { done: 3, total: 9 })
        ))
    );
}
