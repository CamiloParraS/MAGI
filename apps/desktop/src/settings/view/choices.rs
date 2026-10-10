//! The dropdowns' entries.

use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::SelectState;
use gpui_kit::*;
use magi_core::config::{Language, TransparencyMode};

use crate::i18n::{Lang, Strings};
use crate::settings::{background_label, result_counts, system_language};

const LANGUAGES: [Language; 3] = [Language::System, Language::En, Language::Es];
const BACKGROUNDS: [TransparencyMode; 3] = [
    TransparencyMode::MatchSystem,
    TransparencyMode::Always,
    TransparencyMode::Never,
];
/// `ui.theme` values; anything else follows the OS like `system`.
pub(super) const THEMES: [&str; 3] = ["system", "light", "dark"];

/// A dropdown entry: the label shown and the setting it stands for.
#[derive(Clone)]
pub(super) struct Choice<T> {
    label: SharedString,
    value: T,
}

impl<T: Clone + PartialEq + 'static> SearchableListItem for Choice<T> {
    type Value = T;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &T {
        &self.value
    }
}

pub(super) type Dropdown<T> = Entity<SelectState<Vec<Choice<T>>>>;

pub(super) fn language_choices(s: &Strings) -> Vec<Choice<Language>> {
    LANGUAGES
        .iter()
        .map(|&value| Choice {
            label: match value {
                Language::System => system_language(s, Lang::current(value)).into(),
                _ => Lang::current(value).name().into(),
            },
            value,
        })
        .collect()
}

pub(super) fn background_choices(s: &Strings) -> Vec<Choice<TransparencyMode>> {
    BACKGROUNDS
        .iter()
        .map(|&value| Choice {
            label: background_label(value, s).into(),
            value,
        })
        .collect()
}

pub(super) fn theme_choices(s: &Strings) -> Vec<Choice<&'static str>> {
    THEMES
        .iter()
        .zip([s.background_match_system, s.theme_light, s.theme_dark])
        .map(|(&value, label)| Choice {
            label: label.into(),
            value,
        })
        .collect()
}

pub(super) fn count_choices(lang: Lang, current: u32) -> Vec<Choice<u32>> {
    result_counts(current)
        .into_iter()
        .map(|n| Choice {
            label: lang.number(n.into()).into(),
            value: n,
        })
        .collect()
}
