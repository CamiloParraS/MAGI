//! The grouped rows every section is built from, and how a failed change
//! shows in its row.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Field, SettingsView};
use crate::i18n::Strings;
use crate::settings::failure_text;
use crate::theme::{self, Palette};
use crate::ui::{note, row, row_icon, saved_note, warn_note};

impl SettingsView {
    pub(super) fn has_problem(&self, field: Field) -> bool {
        self.error.as_ref().is_some_and(|(f, _)| *f == field)
    }

    /// The failed change's message, for the last line of `field`'s row. It
    /// fades in, again for a different message in the same row.
    pub(super) fn problem(
        &self,
        field: Field,
        s: &Strings,
        p: &Palette,
    ) -> Option<impl IntoElement> {
        self.error
            .as_ref()
            .filter(|(f, _)| *f == field)
            .map(|(_, error)| {
                let text = failure_text(error, s);
                let id = SharedString::from(format!("problem-{field:?}-{text}"));
                theme::fade_in(warn_note(p, text), id)
            })
    }

    /// A grouped row; a failed change in it tints it.
    pub(super) fn row(&self, field: Field, p: &Palette) -> Div {
        row().when(self.has_problem(field), |d| d.bg(p.warn.opacity(0.08)))
    }

    /// A setting's name and note, and its problem if it has one.
    pub(super) fn labelled(
        &self,
        label: &'static str,
        note_text: Option<&'static str>,
        field: Field,
        s: &Strings,
        p: &Palette,
    ) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .child(label)
            .children(note_text.map(|text| {
                if self.saved == Some(field) {
                    saved_note(s, p)
                } else {
                    note(p).child(text)
                }
            }))
            .children(self.problem(field, s, p))
    }

    /// One setting's row: its icon, its name and note, and its control.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn setting(
        &self,
        field: Field,
        icon: IconName,
        label: &'static str,
        note_text: Option<&'static str>,
        control: impl IntoElement,
        s: &Strings,
        p: &Palette,
    ) -> Div {
        self.row(field, p)
            .child(row_icon(icon, p))
            .child(self.labelled(label, note_text, field, s, p))
            .child(div().flex_none().child(control))
    }
}
