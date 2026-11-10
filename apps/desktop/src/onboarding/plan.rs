//! What the setup screen's choices change, and what the finish screen says.
//! Plain functions, tested without a window.

use magi_core::config::FeaturesConfig;
use magi_core::dto::{FeatureStatus, IndexState, IndexStatus, Install};
use magi_core::features::Feature;

/// The features checked when the step opens: the ones labelled Recommended
/// (ADR-0010's selection: meaning and image text), whatever the config says
/// until the user confirms.
pub fn preselected(features: &[FeatureStatus]) -> Vec<Feature> {
    let recommended = FeaturesConfig::default();
    features
        .iter()
        .filter(|f| recommended.enabled(f.feature))
        .map(|f| f.feature)
        .collect()
}

/// Not on disk and not on its way.
fn missing(f: &FeatureStatus) -> bool {
    matches!(f.install, Install::NotInstalled | Install::Failed { .. })
}

/// What confirming the features step changes: each feature to turn on
/// (enable, then download if missing) or off. One already as chosen is
/// left alone, so nothing restarts the engine for no change.
pub fn feature_changes(features: &[FeatureStatus], chosen: &[Feature]) -> Vec<(Feature, bool)> {
    features
        .iter()
        .map(|f| (f, chosen.contains(&f.feature)))
        .filter(|&(f, on)| f.enabled != on || (on && missing(f)))
        .map(|(f, on)| (f.feature, on))
        .collect()
}

/// The bytes the chosen features will download: the consent states it.
pub fn download_total(features: &[FeatureStatus], chosen: &[Feature]) -> u64 {
    features
        .iter()
        .filter(|f| chosen.contains(&f.feature) && missing(f))
        .map(|f| f.download_size)
        .sum()
}

/// What the finish screen says, in one plain sentence and no counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Readiness {
    /// A chosen feature still downloading: search works, but without it
    /// until it lands. `feature` is `None` while several are, with their
    /// bytes added up.
    Downloading {
        feature: Option<Feature>,
        bytes: u64,
        total: u64,
    },
    /// Files still being read; every feature is on disk.
    Preparing,
    Ready,
}

/// A failed download is not "preparing": it is reported on its own.
pub fn readiness(status: Option<&IndexStatus>, features: &[FeatureStatus]) -> Readiness {
    let downloading: Vec<_> = features
        .iter()
        .filter_map(|f| match f.install {
            Install::Downloading { bytes, total } => Some((f.feature, bytes, total)),
            _ => None,
        })
        .collect();
    if !downloading.is_empty() {
        return Readiness::Downloading {
            feature: match downloading.as_slice() {
                [(feature, ..)] => Some(*feature),
                _ => None,
            },
            bytes: downloading.iter().map(|d| d.1).sum(),
            total: downloading.iter().map(|d| d.2).sum(),
        };
    }
    match status {
        Some(st) if st.state == IndexState::Idle => Readiness::Ready,
        _ => Readiness::Preparing,
    }
}

/// A download's percentage; `None` (an indeterminate bar) before its size
/// is known.
pub fn download_progress(bytes: u64, total: u64) -> Option<f32> {
    (total > 0).then(|| bytes.min(total) as f32 * 100. / total as f32)
}

/// The first index's percentage; `None` (an indeterminate bar) while the
/// scan is still finding files, since each one found would push it back.
pub fn progress(st: &IndexStatus) -> Option<f32> {
    if st.state == IndexState::Scanning {
        return None;
    }
    let total = st.indexed + st.queued;
    Some(if total == 0 {
        100.
    } else {
        st.indexed as f32 * 100. / total as f32
    })
}
