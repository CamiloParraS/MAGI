//! App state every window shows besides its own: the shell keeps it
//! current; an open window re-renders on every change.

use magi_core::dto::{FeatureStatus, IndexStatus, PermissionIssue};
use magi_core::features::Feature;

use crate::i18n::Lang;
use crate::ui::busy;

pub struct Live {
    pub lang: Lang,
    /// `ui.theme`.
    pub theme: String,
    pub status: Option<IndexStatus>,
    /// The last busy status while the scan row leaves, after the work ended.
    pub scan_left: Option<IndexStatus>,
    pub features: Vec<FeatureStatus>,
    /// Roots that need the user, with what to tell them (M7).
    pub permissions: Vec<PermissionIssue>,
    /// ponytail: hints closed this session only; persist in config if a
    /// hint that returns after a restart annoys anyone.
    pub dismissed: Vec<Feature>,
    /// The shortcut line was closed (this session, as the hints).
    pub shortcut_dismissed: bool,
    /// The search footer asked for Settings on its Index section, where the
    /// unreadable files are listed; Settings takes the request when it sees it.
    pub show_unreadable: bool,
    /// Hotkey presses this run; onboarding confirms one it sees.
    pub hotkey_presses: u32,
    /// A shortcut that could not be registered (another app or the OS has
    /// it); settings asks for a different one.
    pub hotkey_conflict: Option<String>,
    /// The search window is open; onboarding says the hotkey closes it.
    pub search_open: bool,
    /// The last query typed; the next window opens with it selected.
    pub last_query: String,
}

impl Live {
    pub fn new(lang: Lang, theme: String) -> Self {
        Self {
            lang,
            theme,
            status: None,
            scan_left: None,
            features: Vec::new(),
            permissions: Vec::new(),
            dismissed: Vec::new(),
            shortcut_dismissed: false,
            show_unreadable: false,
            hotkey_presses: 0,
            hotkey_conflict: None,
            search_open: false,
            last_query: String::new(),
        }
    }

    /// Takes a new status. True when the work just ended: the scan row then
    /// leaves with the last busy status, and the shell clears `scan_left`
    /// once it has.
    pub fn set_status(&mut self, status: IndexStatus) -> bool {
        let ended = !busy(&status) && self.status.as_ref().is_some_and(busy);
        if ended {
            self.scan_left = self.status.take();
        } else if busy(&status) {
            self.scan_left = None;
        }
        self.status = Some(status);
        ended
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_core::dto::IndexState;

    fn status(state: IndexState) -> IndexStatus {
        IndexStatus {
            state,
            paused_for: None,
            scanned: 0,
            queued: 0,
            indexed: 0,
            skipped: 0,
            errors: 0,
            current_file: None,
            roots: Vec::new(),
        }
    }

    #[test]
    fn the_scan_row_leaves_when_the_work_ends_and_not_otherwise() {
        let mut live = Live::new(Lang::En, String::new());
        assert!(!live.set_status(status(IndexState::Idle)));
        assert!(!live.set_status(status(IndexState::Scanning)));
        assert!(!live.set_status(status(IndexState::Indexing)));
        // Work ended: the last busy status stays for the row to leave with.
        assert!(live.set_status(status(IndexState::Idle)));
        let left = live.scan_left.as_ref().map(|st| st.state);
        assert_eq!(left, Some(IndexState::Indexing));
        // Another idle update does not cut the leaving short.
        assert!(!live.set_status(status(IndexState::Idle)));
        assert!(live.scan_left.is_some());
        // New work brings the row back; nothing is leaving.
        assert!(!live.set_status(status(IndexState::Scanning)));
        assert!(live.scan_left.is_none());
    }
}
