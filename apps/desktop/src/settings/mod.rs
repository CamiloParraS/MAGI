//! The settings window (SPEC.md M6, FR-9), styled as variant A "Pane":
//! a sidebar plus grouped rows. The logic is here; [`view`] wires it to GPUI.

pub mod view;

use magi_core::config::{TRANSPARENCY_INTENSITY, TransparencyMode};
use magi_core::db::roots::Health;
use magi_core::discovery::Kind;
use magi_core::dto::{DownloadError, ErrorCode, FeatureStatus, FileErrorCode, Install, RootStatus};
use magi_core::platform::BackdropSupport;

use crate::i18n::{Lang, Strings};
use crate::theme::{self, Backdrop, Tone};

/// A root's badge, and a line explaining anything but plain watching
/// (FR-11).
pub fn root_state(root: &RootStatus, s: &Strings) -> (&'static str, Tone, Option<&'static str>) {
    if !root.enabled {
        return (s.badge_paused, Tone::Neutral, Some(s.root_disabled));
    }
    match root.status {
        Health::Ok => (s.root_watching, Tone::Ok, None),
        Health::WatchFailed => (s.badge_polling, Tone::Neutral, Some(s.root_polling)),
        Health::Missing => (s.badge_missing, Tone::Warn, Some(s.root_missing)),
        Health::PermissionDenied => (s.badge_denied, Tone::Err, Some(s.root_denied)),
    }
}

/// The button a search feature's box offers besides its switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureAction {
    /// Enable and download at once (the stated size is the consent,
    /// ADR-0010); replaces the switch, which could not work yet.
    TurnOn,
    /// Frees the disk space of a switched-off feature.
    Remove,
    Retry,
    Nothing,
}

pub fn feature_action(f: &FeatureStatus) -> FeatureAction {
    match f.install {
        Install::NotInstalled => FeatureAction::TurnOn,
        Install::Installed { .. } if !f.enabled => FeatureAction::Remove,
        Install::Failed { .. } => FeatureAction::Retry,
        _ => FeatureAction::Nothing,
    }
}

/// `ui.hotkey` (global-hotkey's `CmdOrCtrl+Shift+Space`) as a GPUI
/// keystroke, which [`Kbd`](gpui_kit::component::kbd::Kbd) shows the
/// platform's way.
pub fn hotkey_keystroke(hotkey: &str) -> String {
    hotkey
        .split('+')
        .map(|key| {
            let key = key.to_lowercase();
            match key.as_str() {
                "cmdorctrl" | "cmdorcontrol" | "commandorctrl" | "commandorcontrol" => {
                    "secondary".into()
                }
                "control" => "ctrl".into(),
                "option" => "alt".into(),
                "command" | "super" => "cmd".into(),
                _ => key
                    .strip_prefix("key")
                    .or_else(|| key.strip_prefix("digit"))
                    .filter(|rest| rest.len() == 1)
                    .map_or_else(|| key.clone(), String::from),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// The "Results shown" choices, with the saved one even if it is not a
/// preset.
pub fn result_counts(current: u32) -> Vec<u32> {
    let mut counts = vec![10, 20, 30, 50, 100];
    if !counts.contains(&current) {
        counts.push(current);
        counts.sort_unstable();
    }
    counts
}

pub fn file_error_label(code: FileErrorCode, s: &Strings) -> &'static str {
    match code {
        FileErrorCode::PermissionDenied => s.read_denied,
        FileErrorCode::Locked => s.read_locked,
        FileErrorCode::ReadFailed => s.read_failed,
        FileErrorCode::ExtractFailed => s.read_damaged,
        FileErrorCode::TimedOut => s.read_timed_out,
        FileErrorCode::Crashed => s.read_crashed,
        FileErrorCode::EmbedFailed => s.read_embed_failed,
        FileErrorCode::WriteFailed => s.read_write_failed,
        FileErrorCode::Other => s.read_other,
    }
}

pub fn download_error_label(code: DownloadError, s: &Strings) -> &'static str {
    match code {
        DownloadError::DownloadNetworkError => s.dl_network,
        DownloadError::ChecksumMismatch => s.dl_checksum,
        DownloadError::DiskFull => s.dl_disk_full,
        DownloadError::PermissionDenied => s.dl_denied,
        DownloadError::WriteFailed => s.dl_write_failed,
    }
}

/// Why a settings change failed, for the user.
pub fn error_text(error: &ErrorCode, s: &Strings) -> String {
    match error {
        ErrorCode::RootNotFound { path } => s.root_not_found.replace("{path}", path),
        ErrorCode::RootAlreadyExists { path } => s.root_exists.replace("{path}", path),
        ErrorCode::NestedRoot {
            path,
            conflicts_with,
        } => s
            .root_nested
            .replace("{path}", path)
            .replace("{other}", conflicts_with),
        ErrorCode::EngineStarting => s.engine_starting.into(),
        ErrorCode::InvalidGlob { glob } => s.invalid_glob.replace("{glob}", glob),
        ErrorCode::InvalidSetting { field } if field == MAX_SIZE_FIELD => s.invalid_size.into(),
        _ => s.change_failed.into(),
    }
}

/// The kinds that can have their content read, in the order they are shown.
pub const FILE_TYPES: [Kind; 5] = [Kind::Text, Kind::Code, Kind::Pdf, Kind::Office, Kind::Image];

/// The field a bad max-size entry is reported under.
pub const MAX_SIZE_FIELD: &str = "indexing.max_file_size_mb";

pub fn kind_label(kind: Kind, s: &Strings) -> &'static str {
    match kind {
        Kind::Text => s.kind_text,
        Kind::Code => s.kind_code,
        Kind::Pdf => s.kind_pdf,
        Kind::Office => s.kind_office,
        Kind::Image | Kind::Other => s.kind_image,
    }
}

/// `types` with `kind` switched on or off, in [`FILE_TYPES`] order.
pub fn toggle_kind(types: &[Kind], kind: Kind, on: bool) -> Vec<Kind> {
    FILE_TYPES
        .into_iter()
        .filter(|&k| if k == kind { on } else { types.contains(&k) })
        .collect()
}

/// The largest file to read, from what the user typed: whole megabytes, 1
/// or more.
pub fn max_size_from_text(text: &str) -> Option<u64> {
    text.trim().parse().ok().filter(|&mb| mb >= 1)
}

/// Exclusion patterns, one per line; blank lines and surrounding spaces
/// are dropped.
pub fn globs_from_text(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect()
}

/// "Same as system (English)": the `system` choice names what it resolves to.
pub fn system_language(s: &Strings, resolved: Lang) -> String {
    s.language_system.replace("{lang}", resolved.name())
}

/// A background choice as the user sees it.
pub fn background_label(mode: TransparencyMode, s: &Strings) -> &'static str {
    match mode {
        TransparencyMode::MatchSystem => s.background_match_system,
        TransparencyMode::Always => s.background_always,
        TransparencyMode::Never => s.background_never,
    }
}

/// The amount slider only matters when the search window would be see-through.
pub fn see_through_adjustable(mode: TransparencyMode, support: BackdropSupport) -> bool {
    matches!(
        theme::backdrop(mode, *TRANSPARENCY_INTENSITY.end(), support),
        Backdrop::Blurred { .. }
    )
}

/// The slider runs from "More solid" to "More transparent"; the config keeps
/// the tint's alpha (`ui.transparency_intensity`), always inside its allowed
/// range and to two decimals, so float error never fails validation.
pub fn intensity_from_slider(see_through: f32) -> f32 {
    let (lo, hi) = (
        *TRANSPARENCY_INTENSITY.start(),
        *TRANSPARENCY_INTENSITY.end(),
    );
    let alpha = hi - see_through.clamp(0., 1.) * (hi - lo);
    ((alpha * 100.).round() / 100.).clamp(lo, hi)
}

pub fn slider_from_intensity(intensity: f32) -> f32 {
    let (lo, hi) = (
        *TRANSPARENCY_INTENSITY.start(),
        *TRANSPARENCY_INTENSITY.end(),
    );
    ((hi - intensity) / (hi - lo)).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;
    use magi_core::features::Feature;

    fn root(enabled: bool, status: Health) -> RootStatus {
        RootStatus {
            id: 1,
            path: "D:\\Photos".into(),
            enabled,
            status,
        }
    }

    #[test]
    fn a_root_has_a_badge_and_explains_anything_but_watching() {
        let s = Lang::En.strings();
        let state = |enabled, status| root_state(&root(enabled, status), s);
        assert_eq!(state(true, Health::Ok), ("Watching", Tone::Ok, None));
        assert_eq!(
            state(true, Health::WatchFailed),
            (
                "Checking",
                Tone::Neutral,
                Some("Can't watch for changes here; checking now and then instead")
            )
        );
        assert_eq!(
            state(true, Health::Missing),
            (
                "Not found",
                Tone::Warn,
                Some("Folder not found. Reconnect the drive; the index is kept.")
            )
        );
        assert_eq!(
            state(true, Health::PermissionDenied),
            (
                "No access",
                Tone::Err,
                Some("Magi isn't allowed to read this folder.")
            )
        );
        // A switched-off root is not probed, so its last status is stale.
        assert_eq!(
            state(false, Health::Missing),
            ("Paused", Tone::Neutral, Some("Paused, not searched"))
        );
    }

    fn feature(enabled: bool, install: Install) -> FeatureStatus {
        FeatureStatus {
            feature: Feature::Meaning,
            enabled,
            download_size: 1,
            install,
            backfill: None,
        }
    }

    #[test]
    fn a_feature_offers_the_one_action_that_fits() {
        use FeatureAction::*;
        let installed = Install::Installed { size_bytes: 1 };
        assert_eq!(
            feature_action(&feature(false, Install::NotInstalled)),
            TurnOn
        );
        assert_eq!(
            feature_action(&feature(true, Install::NotInstalled)),
            TurnOn,
            "wanted but never downloaded (a cancelled download)"
        );
        assert_eq!(feature_action(&feature(false, installed.clone())), Remove);
        assert_eq!(feature_action(&feature(true, installed)), Nothing);
        assert_eq!(
            feature_action(&feature(true, Install::Downloading { bytes: 0, total: 1 })),
            Nothing
        );
        assert_eq!(
            feature_action(&feature(
                true,
                Install::Failed {
                    code: DownloadError::ChecksumMismatch
                }
            )),
            Retry
        );
    }

    #[test]
    fn the_hotkey_setting_becomes_a_gpui_keystroke() {
        assert_eq!(
            hotkey_keystroke("CmdOrCtrl+Shift+Space"),
            "secondary-shift-space"
        );
        assert_eq!(hotkey_keystroke("Alt+K"), "alt-k");
        assert_eq!(
            hotkey_keystroke("Control+Option+KeyK"),
            "ctrl-alt-k",
            "global-hotkey's other spellings"
        );
        assert_eq!(hotkey_keystroke("CommandOrControl+Digit1"), "secondary-1");
        assert_eq!(hotkey_keystroke("Command+Space"), "cmd-space");
    }

    #[test]
    fn result_counts_keep_the_saved_one() {
        assert_eq!(result_counts(30), vec![10, 20, 30, 50, 100]);
        assert_eq!(result_counts(42), vec![10, 20, 30, 42, 50, 100]);
    }

    #[test]
    fn every_read_error_has_its_own_words() {
        let s = Lang::En.strings();
        let mut labels: Vec<_> = FileErrorCode::ALL
            .into_iter()
            .map(|code| file_error_label(code, s))
            .collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), FileErrorCode::ALL.len());
    }

    #[test]
    fn folder_errors_name_the_paths_involved() {
        let s = Lang::En.strings();
        let path = || "C:\\Users\\ana\\Documents\\notes".to_string();
        assert_eq!(
            error_text(
                &ErrorCode::NestedRoot {
                    path: path(),
                    conflicts_with: "C:\\Users\\ana\\Documents".into()
                },
                s
            ),
            "C:\\Users\\ana\\Documents\\notes is already searched as part of C:\\Users\\ana\\Documents."
        );
        assert_eq!(
            error_text(&ErrorCode::RootAlreadyExists { path: path() }, s),
            "C:\\Users\\ana\\Documents\\notes is already in the list."
        );
        assert_eq!(
            error_text(&ErrorCode::RootNotFound { path: path() }, s),
            "C:\\Users\\ana\\Documents\\notes doesn't exist."
        );
        assert_eq!(
            error_text(&ErrorCode::EngineStarting, s),
            "Magi is still starting. Try again in a moment."
        );
        assert_eq!(
            error_text(
                &ErrorCode::Internal {
                    detail: "disk I/O error".into()
                },
                s
            ),
            "Couldn't save the change.",
            "internal details are for the log only"
        );
    }

    #[test]
    fn index_setting_errors_say_what_to_fix() {
        let s = Lang::En.strings();
        assert_eq!(
            error_text(
                &ErrorCode::InvalidGlob {
                    glob: "**/[bad".into()
                },
                s
            ),
            "“**/[bad” isn't a valid pattern."
        );
        assert_eq!(
            error_text(
                &ErrorCode::InvalidSetting {
                    field: MAX_SIZE_FIELD.into()
                },
                s
            ),
            "Enter a whole number of megabytes, 1 or more."
        );
    }

    #[test]
    fn toggling_a_kind_keeps_the_display_order() {
        use Kind::*;
        assert_eq!(
            toggle_kind(&[Image, Text], Pdf, true),
            vec![Text, Pdf, Image]
        );
        assert_eq!(toggle_kind(&[Text, Pdf], Pdf, false), vec![Text]);
        assert_eq!(
            toggle_kind(&[Text], Text, true),
            vec![Text],
            "no duplicates"
        );
        assert_eq!(toggle_kind(&[Text], Code, false), vec![Text]);
        assert_eq!(
            toggle_kind(&[Text, Other], Code, true),
            vec![Text, Code],
            "kinds the window doesn't show are dropped"
        );
    }

    #[test]
    fn the_max_size_is_whole_megabytes() {
        assert_eq!(max_size_from_text(" 100 "), Some(100));
        assert_eq!(max_size_from_text("1"), Some(1));
        for bad in ["0", "", "-5", "1.5", "ten", "99999999999999999999999"] {
            assert_eq!(max_size_from_text(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn globs_are_one_per_line() {
        assert_eq!(
            globs_from_text("**/node_modules/**\r\n\n  **/*.tmp  \n\n"),
            vec!["**/node_modules/**", "**/*.tmp"]
        );
        assert!(globs_from_text("  \n").is_empty());
    }

    #[test]
    fn the_system_language_names_what_it_resolves_to() {
        assert_eq!(
            system_language(Lang::En.strings(), Lang::En),
            "Same as system (English)"
        );
        assert_eq!(
            system_language(Lang::Es.strings(), Lang::Es),
            "Igual que el sistema (Español)"
        );
    }

    #[test]
    fn the_amount_is_adjustable_only_when_see_through() {
        use TransparencyMode::*;
        let blur = BackdropSupport {
            mica: true,
            reduce_transparency: false,
        };
        assert!(see_through_adjustable(Always, blur));
        assert!(see_through_adjustable(MatchSystem, blur));
        assert!(!see_through_adjustable(Never, blur));
        let reduced = BackdropSupport {
            reduce_transparency: true,
            ..blur
        };
        assert!(!see_through_adjustable(MatchSystem, reduced));
        assert!(!see_through_adjustable(Always, BackdropSupport::default()));
    }

    #[test]
    fn the_slider_maps_to_an_intensity_the_config_accepts() {
        // Ends of the slider are the ends of the allowed range, reversed.
        assert_eq!(intensity_from_slider(0.0), 0.95);
        assert_eq!(intensity_from_slider(1.0), 0.40);
        assert_eq!(intensity_from_slider(0.2), 0.84);
        // Out of range or float noise still lands inside the range.
        assert_eq!(intensity_from_slider(1.3), 0.40);
        assert_eq!(intensity_from_slider(-0.2), 0.95);
        for step in 0..=20 {
            let i = intensity_from_slider(step as f32 * 0.05);
            assert!(TRANSPARENCY_INTENSITY.contains(&i), "{i}");
            assert!(
                (intensity_from_slider(slider_from_intensity(i)) - i).abs() < 1e-6,
                "round trip {i}"
            );
        }
        assert_eq!(slider_from_intensity(0.95), 0.0);
        assert!((slider_from_intensity(0.40) - 1.0).abs() < 1e-6);
    }
}
