//! The settings window (SPEC.md M6, FR-9), styled as variant A "Pane":
//! a sidebar plus grouped rows. The logic is here; [`view`] wires it to GPUI.

pub mod view;

use magi_core::config::{TRANSPARENCY_INTENSITY, TransparencyMode};
use magi_core::db::roots::Health;
use magi_core::discovery::Kind;
use magi_core::dto::{ErrorCode, RootStatus};
use magi_core::platform::BackdropSupport;

use crate::i18n::{Lang, Strings};
use crate::theme::{self, Backdrop};

/// A root's second line, and whether it is a problem to fix (FR-11).
pub fn root_line(root: &RootStatus, s: &Strings) -> (&'static str, bool) {
    if !root.enabled {
        return (s.root_disabled, false);
    }
    match root.status {
        Health::Ok => (s.root_watching, false),
        Health::WatchFailed => (s.root_polling, false),
        Health::Missing => (s.root_missing, true),
        Health::PermissionDenied => (s.root_denied, true),
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

    fn root(enabled: bool, status: Health) -> RootStatus {
        RootStatus {
            id: 1,
            path: "D:\\Photos".into(),
            enabled,
            status,
        }
    }

    #[test]
    fn a_root_says_whether_it_is_watched_and_flags_problems() {
        let s = Lang::En.strings();
        let line = |enabled, status| root_line(&root(enabled, status), s);
        assert_eq!(line(true, Health::Ok), ("Watching for changes", false));
        assert_eq!(
            line(true, Health::WatchFailed),
            (
                "Can't watch for changes here; checking now and then instead",
                false
            )
        );
        assert_eq!(
            line(true, Health::Missing),
            (
                "Folder not found. Reconnect the drive; the index is kept.",
                true
            )
        );
        assert_eq!(
            line(true, Health::PermissionDenied),
            ("Magi isn't allowed to read this folder.", true)
        );
        // A switched-off root is not probed, so its last status is stale.
        assert_eq!(
            line(false, Health::Missing),
            ("Paused, not searched", false)
        );
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
