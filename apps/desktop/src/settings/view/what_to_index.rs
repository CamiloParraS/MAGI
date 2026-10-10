//! Settings › Folders, under the list: file types, largest file, battery,
//! exclusions.

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::discovery::Kind;
use magi_core::dto::ErrorCode;
use serde_json::json;

use super::{Field, SettingsView};
use crate::i18n::Strings;
use crate::settings::{
    FILE_TYPES, MAX_SIZE_FIELD, globs_from_text, images_needed, kind_examples, kind_label,
    max_size_from_text, toggle_kind,
};
use crate::theme::{self, Palette};
use crate::ui::{note, stack};

impl SettingsView {
    pub(super) fn save_max_size(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match max_size_from_text(text) {
            Some(mb) if mb == self.indexing.max_file_size_mb => {}
            Some(mb) => self.save(
                Field::MaxSize,
                json!({ "indexing": { "max_file_size_mb": mb } }),
                window,
                cx,
            ),
            None => {
                self.error = Some((
                    Field::MaxSize,
                    ErrorCode::InvalidSetting {
                        field: MAX_SIZE_FIELD.into(),
                    },
                ));
                cx.notify();
            }
        }
    }

    fn save_excludes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let globs = globs_from_text(&self.excludes.read(cx).value());
        if globs != self.indexing.exclude_globs {
            self.save(
                Field::Excludes,
                json!({ "indexing": { "exclude_globs": globs } }),
                window,
                cx,
            );
        }
    }

    pub(super) fn what_to_index(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let types = &self.indexing.file_types;
        let images_needed = images_needed(types, &self.live.read(cx).features);
        let kinds = FILE_TYPES.into_iter().map(|kind| {
            let on = types.contains(&kind);
            let checkbox = Checkbox::new(("kind", kind as usize))
                .cursor_pointer()
                .label(kind_label(kind, s))
                .checked(on)
                .on_click(cx.listener(move |this, &on: &bool, window, cx| {
                    let types = toggle_kind(&this.indexing.file_types, kind, on);
                    this.save(
                        Field::FileTypes,
                        json!({ "indexing": { "file_types": types } }),
                        window,
                        cx,
                    );
                }));
            let warning = (kind == Kind::Image && images_needed)
                .then(|| note(p).text_color(p.warn).child(s.images_needed));
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(theme::kind_glyph(kind, px(22.)))
                .child(
                    div()
                        .min_w_0()
                        .child(checkbox)
                        .children(kind_examples(kind).map(|e| note(p).child(e)))
                        .children(warning),
                )
        });
        let none = types
            .is_empty()
            .then(|| note(p).mt_0().text_color(p.warn).child(s.kind_none));
        let battery = self.indexing.pause_on_battery;
        let excludes_edited =
            globs_from_text(&self.excludes.read(cx).value()) != self.indexing.exclude_globs;
        stack()
            .child(
                self.card(Field::FileTypes, p)
                    .flex_col()
                    .items_start()
                    .gap_3()
                    .child(self.labelled(
                        s.file_types,
                        Some(s.file_types_note),
                        Field::FileTypes,
                        s,
                        p,
                    ))
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .w_full()
                            .gap_x_6()
                            .gap_y_3()
                            .children(kinds),
                    )
                    .children(none),
            )
            .child(
                self.setting(
                    Field::MaxSize,
                    None,
                    s.max_size,
                    Some(s.max_size_note),
                    div().w(px(140.)).child(
                        Input::new(&self.max_size)
                            .bg(p.solid)
                            .suffix(div().text_color(p.mute).child("MB")),
                    ),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Battery,
                    None,
                    s.pause_on_battery,
                    Some(s.pause_on_battery_note),
                    Switch::new("battery")
                        .cursor_pointer()
                        .checked(battery)
                        .color(p.accent)
                        .accessibility_label(s.pause_on_battery)
                        .on_click(cx.listener(|this, &on: &bool, window, cx| {
                            this.save(
                                Field::Battery,
                                json!({ "indexing": { "pause_on_battery": on } }),
                                window,
                                cx,
                            );
                        })),
                    s,
                    p,
                ),
            )
            .child(
                self.card(Field::Excludes, p)
                    .flex_col()
                    .items_stretch()
                    .gap_3()
                    .child(self.labelled(s.excludes, Some(s.excludes_note), Field::Excludes, s, p))
                    // `rows` does not size the box; a height does.
                    .child(Textarea::new(&self.excludes).h(px(150.)).bg(p.solid))
                    .child(
                        div().flex().justify_end().child(
                            Button::new("save-excludes")
                                .when(excludes_edited, |b| b.cursor_pointer())
                                .primary()
                                .label(s.save)
                                .disabled(!excludes_edited)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_excludes(window, cx)
                                })),
                        ),
                    ),
            )
    }
}
