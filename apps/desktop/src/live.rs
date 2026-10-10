//! App state every window shows besides its own: the shell keeps it
//! current; an open window re-renders on every change.

use magi_core::dto::{FeatureStatus, IndexStatus};
use magi_core::features::Feature;

use crate::i18n::Lang;

pub struct Live {
    pub lang: Lang,
    /// `ui.theme`.
    pub theme: String,
    pub status: Option<IndexStatus>,
    pub features: Vec<FeatureStatus>,
    /// ponytail: hints closed this session only; persist in config if a
    /// hint that returns after a restart annoys anyone.
    pub dismissed: Vec<Feature>,
    /// The shortcut line was closed (this session, as the hints).
    pub shortcut_dismissed: bool,
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
            features: Vec::new(),
            dismissed: Vec::new(),
            shortcut_dismissed: false,
            hotkey_presses: 0,
            hotkey_conflict: None,
            search_open: false,
            last_query: String::new(),
        }
    }
}
