//! The search hint (ADR-0010): with fewer than 3 results, offer a search
//! feature that is off, or report the one being set up.

use magi_core::dto::{FeatureStatus, Install};
use magi_core::features::Feature;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// Off, or on but not downloaded: offer to turn it on.
    Offer { feature: Feature, size: u64 },
    Downloading {
        feature: Feature,
        done: u64,
        total: u64,
    },
    /// On and installed, still backfilling files indexed without it.
    Updating { feature: Feature, left: u64 },
}

impl Hint {
    pub fn feature(self) -> Feature {
        match self {
            Self::Offer { feature, .. }
            | Self::Downloading { feature, .. }
            | Self::Updating { feature, .. } => feature,
        }
    }
}

/// Work in progress wins over an offer; otherwise features in `Feature::ALL`
/// order.
pub fn hint(results: usize, features: &[FeatureStatus], dismissed: &[Feature]) -> Option<Hint> {
    if results >= 3 {
        return None;
    }
    let candidates = || {
        Feature::ALL
            .into_iter()
            .filter(|f| !dismissed.contains(f))
            .filter_map(|f| features.iter().find(|s| s.feature == f))
    };
    let in_progress = candidates().find_map(|s| match (&s.install, s.backfill) {
        (Install::Downloading { bytes, total }, _) => Some(Hint::Downloading {
            feature: s.feature,
            done: *bytes,
            total: *total,
        }),
        (Install::Installed { .. }, Some(b)) if s.enabled && b.done < b.total => {
            Some(Hint::Updating {
                feature: s.feature,
                left: b.total - b.done,
            })
        }
        _ => None,
    });
    in_progress.or_else(|| {
        candidates()
            .find(|s| !(s.enabled && matches!(s.install, Install::Installed { .. })))
            .map(|s| Hint::Offer {
                feature: s.feature,
                size: s.download_size,
            })
    })
}

/// The shortcut line under the input, for whoever skipped onboarding or
/// forgot the shortcut: only when search was opened some other way (tray,
/// launching Magi), not closed this session, and the shortcut works.
pub fn teach_shortcut(by_hotkey: bool, dismissed: bool, conflict: bool) -> bool {
    !by_hotkey && !dismissed && !conflict
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_core::dto::Backfill;

    fn status(feature: Feature, enabled: bool, install: Install) -> FeatureStatus {
        FeatureStatus {
            feature,
            enabled,
            download_size: 812_000_000,
            install,
            backfill: None,
        }
    }

    const INSTALLED: Install = Install::Installed { size_bytes: 1 };

    fn all_on() -> Vec<FeatureStatus> {
        Feature::ALL
            .into_iter()
            .map(|f| status(f, true, INSTALLED))
            .collect()
    }

    #[test]
    fn the_shortcut_is_taught_when_search_was_opened_another_way() {
        assert!(teach_shortcut(false, false, false));
        // Opened with it: they know it.
        assert!(!teach_shortcut(true, false, false));
        // Closed this session.
        assert!(!teach_shortcut(false, true, false));
        // Not registered: it would not work.
        assert!(!teach_shortcut(false, false, true));
    }

    #[test]
    fn three_results_or_everything_on_means_no_hint() {
        let mut features = all_on();
        features[2].enabled = false;
        assert_eq!(hint(3, &features, &[]), None);
        assert_eq!(hint(0, &all_on(), &[]), None);
    }

    #[test]
    fn an_off_or_undownloaded_feature_is_offered_unless_dismissed() {
        let mut features = all_on();
        features[2].enabled = false;
        let offer = Hint::Offer {
            feature: Feature::ImageVisual,
            size: 812_000_000,
        };
        assert_eq!(hint(2, &features, &[]), Some(offer));
        assert_eq!(hint(0, &features, &[Feature::ImageVisual]), None);

        let mut features = all_on();
        features[1].install = Install::NotInstalled;
        assert_eq!(
            hint(0, &features, &[]).map(Hint::feature),
            Some(Feature::ImageText)
        );
    }

    #[test]
    fn a_download_or_backfill_in_progress_wins_over_an_offer() {
        let mut features = all_on();
        features[0].enabled = false;
        features[2].install = Install::Downloading {
            bytes: 10,
            total: 20,
        };
        assert_eq!(
            hint(0, &features, &[]),
            Some(Hint::Downloading {
                feature: Feature::ImageVisual,
                done: 10,
                total: 20
            })
        );
        features[2].install = INSTALLED;
        features[2].backfill = Some(Backfill { done: 5, total: 12 });
        assert_eq!(
            hint(0, &features, &[]),
            Some(Hint::Updating {
                feature: Feature::ImageVisual,
                left: 7
            })
        );
    }
}
