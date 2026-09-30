//! The settings window (SPEC.md M6, FR-9), styled as variant A "Pane":
//! a sidebar plus grouped rows. The logic is here; [`view`] wires it to GPUI.

pub mod view;

use magi_core::db::roots::Health;
use magi_core::dto::{ErrorCode, RootStatus};

use crate::i18n::Strings;

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

/// Why adding, removing or switching a folder failed, for the user.
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
        _ => s.folders_failed.into(),
    }
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
            "Couldn't change the folders.",
            "internal details are for the log only"
        );
    }
}
