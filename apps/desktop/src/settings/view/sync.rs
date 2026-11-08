//! Keeping the window's controls and background on the saved settings.

use gpui_kit::*;

use super::SettingsView;
use super::choices::{THEMES, background_choices, count_choices, language_choices, theme_choices};
use crate::i18n::Lang;
use crate::settings::slider_from_intensity;
use crate::theme::{self, Backdrop};

impl SettingsView {
    /// The window's background, from the saved settings (like the search
    /// window's).
    pub(super) fn backdrop(&self) -> Backdrop {
        theme::backdrop(
            self.ui.transparency_mode,
            self.ui.transparency_intensity,
            self.support,
        )
    }

    /// Makes the General and Appearance controls and the window background
    /// show the saved settings.
    pub(super) fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_background_appearance(theme::window_background(self.backdrop()));
        let (language, mode, count) = (
            self.ui.language,
            self.ui.transparency_mode,
            self.ui.max_results,
        );
        let theme = THEMES
            .into_iter()
            .find(|&t| t == self.ui.theme)
            .unwrap_or(THEMES[0]);
        let see_through = slider_from_intensity(self.ui.transparency_intensity);
        let lang = self.lang;
        self.language
            .update(cx, |d, cx| d.set_selected_value(&language, window, cx));
        self.results.update(cx, |d, cx| {
            d.set_items(count_choices(lang, count), window, cx);
            d.set_selected_value(&count, window, cx);
        });
        self.theme_choice
            .update(cx, |d, cx| d.set_selected_value(&theme, window, cx));
        self.background
            .update(cx, |d, cx| d.set_selected_value(&mode, window, cx));
        self.see_through
            .update(cx, |slider, cx| slider.set_value(see_through, window, cx));
    }

    /// A live language switch: the window title and the dropdown labels.
    pub(super) fn relabel(&mut self, lang: Lang, window: &mut Window, cx: &mut Context<Self>) {
        let s = lang.strings();
        self.lang = lang;
        window.set_window_title(s.settings);
        self.language
            .update(cx, |d, cx| d.set_items(language_choices(s), window, cx));
        self.theme_choice
            .update(cx, |d, cx| d.set_items(theme_choices(s), window, cx));
        self.background
            .update(cx, |d, cx| d.set_items(background_choices(s), window, cx));
        self.sync_controls(window, cx);
    }

    pub(super) fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme = self.live.read(cx).theme.clone();
        self.dark = theme::sync(&self.theme, window, cx);
        cx.notify();
    }
}
