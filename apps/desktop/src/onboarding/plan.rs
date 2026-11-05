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
#[derive(Debug, PartialEq)]
pub enum Readiness {
    /// Files still being read, or this feature downloading.
    Preparing(Option<Feature>),
    Ready,
}

/// A failed download is not "preparing": it is reported on its own.
pub fn readiness(status: Option<&IndexStatus>, features: &[FeatureStatus]) -> Readiness {
    if let Some(f) = features
        .iter()
        .find(|f| matches!(f.install, Install::Downloading { .. }))
    {
        return Readiness::Preparing(Some(f.feature));
    }
    match status {
        Some(st) if st.state == IndexState::Idle => Readiness::Ready,
        _ => Readiness::Preparing(None),
    }
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
