// Not `super::*`: GPUI's prelude has its own `test` attribute.
use super::{
    Readiness, download_progress, download_total, feature_changes, preselected, progress, readiness,
};
use magi_core::dto::{DownloadError, FeatureStatus, IndexState, IndexStatus, Install};
use magi_core::features::Feature;

fn feature(feature: Feature, enabled: bool, install: Install) -> FeatureStatus {
    FeatureStatus {
        feature,
        enabled,
        download_size: 100,
        install,
        backfill: None,
    }
}

/// The first run: the config's defaults, nothing downloaded.
fn first_run() -> Vec<FeatureStatus> {
    vec![
        feature(Feature::Meaning, true, Install::NotInstalled),
        feature(Feature::ImageText, true, Install::NotInstalled),
        feature(Feature::ImageVisual, false, Install::NotInstalled),
    ]
}

#[test]
fn the_config_defaults_are_preselected() {
    assert_eq!(
        preselected(&first_run()),
        vec![Feature::Meaning, Feature::ImageText]
    );
}

#[test]
fn confirming_changes_only_what_differs() {
    use Feature::*;
    // Wanted but not downloaded: turned on again, which downloads it.
    assert_eq!(
        feature_changes(&first_run(), &[Meaning, ImageText]),
        vec![(Meaning, true), (ImageText, true)]
    );
    // Unchecking a default turns it off; checking the third turns it on.
    assert_eq!(
        feature_changes(&first_run(), &[Meaning, ImageVisual]),
        vec![(Meaning, true), (ImageText, false), (ImageVisual, true)]
    );
    // None: keyword and file name search only.
    assert_eq!(
        feature_changes(&first_run(), &[]),
        vec![(Meaning, false), (ImageText, false)]
    );
    let settled = [
        feature(Meaning, true, Install::Installed { size_bytes: 1 }),
        feature(ImageText, true, Install::Downloading { bytes: 0, total: 1 }),
        feature(ImageVisual, false, Install::Installed { size_bytes: 1 }),
    ];
    assert!(feature_changes(&settled, &[Meaning, ImageText]).is_empty());
    // A failed download is retried.
    let failed = [feature(
        Meaning,
        true,
        Install::Failed {
            code: DownloadError::DownloadNetworkError,
        },
    )];
    assert_eq!(feature_changes(&failed, &[Meaning]), vec![(Meaning, true)]);
}

#[test]
fn the_download_total_counts_what_is_still_missing() {
    use Feature::*;
    assert_eq!(download_total(&first_run(), &[Meaning, ImageText]), 200);
    assert_eq!(download_total(&first_run(), &[]), 0);
    let some = [
        feature(Meaning, true, Install::Installed { size_bytes: 100 }),
        feature(
            ImageText,
            true,
            Install::Downloading {
                bytes: 0,
                total: 100,
            },
        ),
        feature(
            ImageVisual,
            false,
            Install::Failed {
                code: DownloadError::DiskFull,
            },
        ),
    ];
    assert_eq!(
        download_total(&some, &[Meaning, ImageText, ImageVisual]),
        100
    );
}

fn status(state: IndexState, indexed: u64, queued: u64) -> IndexStatus {
    IndexStatus {
        state,
        paused_for: None,
        scanned: 0,
        queued,
        indexed,
        skipped: 0,
        errors: 0,
        current_file: None,
        roots: vec![],
    }
}

#[test]
fn the_bar_has_no_value_while_the_total_is_still_growing() {
    // Scanning keeps adding to `queued`: a percentage would go backwards.
    assert_eq!(progress(&status(IndexState::Scanning, 10, 5)), None);
    assert_eq!(progress(&status(IndexState::Indexing, 25, 75)), Some(25.));
    assert_eq!(progress(&status(IndexState::Idle, 0, 0)), Some(100.));
    assert_eq!(progress(&status(IndexState::Paused, 50, 50)), Some(50.));
}

#[test]
fn the_last_step_says_ready_only_when_nothing_is_left() {
    let idle = status(IndexState::Idle, 10, 0);
    assert_eq!(readiness(Some(&idle), &first_run()), Readiness::Ready);
    // Before the first status, and while scanning or reading.
    assert_eq!(readiness(None, &[]), Readiness::Preparing);
    let busy = status(IndexState::Indexing, 5, 5);
    assert_eq!(readiness(Some(&busy), &[]), Readiness::Preparing);
    // A download names its feature and how far it is, even with the files
    // done: it is what "all set" would hide.
    let downloading = [feature(
        Feature::Meaning,
        true,
        Install::Downloading { bytes: 1, total: 2 },
    )];
    assert_eq!(
        readiness(Some(&idle), &downloading),
        Readiness::Downloading {
            feature: Some(Feature::Meaning),
            bytes: 1,
            total: 2
        }
    );
    // A failed download is reported on its own, not as preparing.
    let failed = [feature(
        Feature::Meaning,
        true,
        Install::Failed {
            code: DownloadError::DownloadNetworkError,
        },
    )];
    assert_eq!(readiness(Some(&idle), &failed), Readiness::Ready);
}

#[test]
fn a_download_bar_waits_for_its_size() {
    assert_eq!(download_progress(0, 0), None);
    assert_eq!(download_progress(25, 100), Some(25.));
    assert_eq!(download_progress(120, 100), Some(100.));
}

#[test]
fn several_downloads_add_up_under_no_one_name() {
    let idle = status(IndexState::Idle, 10, 0);
    let both = [
        feature(
            Feature::Meaning,
            true,
            Install::Downloading {
                bytes: 10,
                total: 40,
            },
        ),
        feature(
            Feature::ImageText,
            true,
            Install::Downloading {
                bytes: 5,
                total: 60,
            },
        ),
        feature(Feature::ImageVisual, false, Install::NotInstalled),
    ];
    assert_eq!(
        readiness(Some(&idle), &both),
        Readiness::Downloading {
            feature: None,
            bytes: 15,
            total: 100
        }
    );
}
