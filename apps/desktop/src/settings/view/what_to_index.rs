//! What indexing reads: file types and largest file (under What's
//! searched), exclusions (under Folders), and pause on battery (under
//! General).

use gpui_kit::assets::IconName;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
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
use crate::ui::{grouped, note, stack};

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
            // The frame fades in when the type is switched on; switching off
            // is instant, like the rest of the window.
            let frame = on.then(|| {
                theme::fade_in(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(p.accent)
                        .bg(p.accent.opacity(0.08)),
                    ("kind-frame", kind as usize),
                )
            });
            let tick = div()
                .flex_none()
                .size(px(18.))
                .rounded_full()
                .border_1()
                .border_color(if on { p.accent } else { p.line })
                .when(on, |d| {
                    d.bg(p.accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Check).size(px(12.)).text_color(white()))
                });
            let warning = (kind == Kind::Image && images_needed)
                .then(|| note(p).text_color(p.warn).child(s.images_needed));
            // A button, so Tab and Enter reach every type; the cell makes
            // tiles in a row the same height.
            let tile = Button::new(("kind", kind as usize))
                .cursor_pointer()
                .ghost()
                .w_full()
                .h_auto()
                .text_size(px(14.))
                .bg(p.solid)
                .border_1()
                .border_color(p.line)
                .p_0()
                .rounded(px(8.))
                .accessibility_label(kind_label(kind, s))
                .child(
                    div()
                        .relative()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_2()
                        .p_3()
                        .children(frame)
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .opacity(if on { 1. } else { 0.45 })
                                        .child(theme::kind_glyph(kind, px(26.))),
                                )
                                .child(tick),
                        )
                        .child(
                            div()
                                .text_left()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(kind_label(kind, s)),
                                )
                                .children(kind_examples(kind).map(|e| note(p).child(e)))
                                .children(warning),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let on = !this.indexing.file_types.contains(&kind);
                    let types = toggle_kind(&this.indexing.file_types, kind, on);
                    this.save(
                        Field::FileTypes,
                        json!({ "indexing": { "file_types": types } }),
                        window,
                        cx,
                    );
                }));
            // An odd last tile takes the whole row.
            div()
                .flex()
                .when(
                    kind == FILE_TYPES[FILE_TYPES.len() - 1] && FILE_TYPES.len() % 2 == 1,
                    |d| d.col_span(2),
                )
                .child(tile)
        });
        let none = types
            .is_empty()
            .then(|| note(p).mt_0().text_color(p.warn).child(s.kind_none));
        let file_types = self
            .row(Field::FileTypes, p)
            .flex_col()
            .items_start()
            .gap_3()
            .py(px(10.))
            .child(self.labelled(
                s.file_types,
                Some(s.file_types_note),
                Field::FileTypes,
                s,
                p,
            ))
            .child(div().grid().grid_cols(2).w_full().gap_2().children(kinds))
            .children(none);
        let max_size = self.setting(
            Field::MaxSize,
            IconName::FileUp,
            s.max_size,
            Some(s.max_size_note),
            div().w(px(140.)).child(
                Input::new(&self.max_size)
                    .bg(p.solid)
                    .suffix(div().text_color(p.mute).child("MB")),
            ),
            s,
            p,
        );
        stack()
            .child(grouped(p, [file_types]))
            .child(grouped(p, [max_size]))
    }

    /// Under General: when indexing runs, not what it reads.
    pub(super) fn battery(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let battery = self.indexing.pause_on_battery;
        self.setting(
            Field::Battery,
            IconName::BatteryCharging,
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
        )
    }

    /// Under Folders: path patterns, read with the folders they cut into.
    pub(super) fn excludes(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let excludes_edited =
            globs_from_text(&self.excludes.read(cx).value()) != self.indexing.exclude_globs;
        let excludes = self
            .row(Field::Excludes, p)
            .flex_col()
            .items_stretch()
            .gap_3()
            .py(px(10.))
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
                        .on_click(
                            cx.listener(|this, _, window, cx| this.save_excludes(window, cx)),
                        ),
                ),
            );
        grouped(p, [excludes])
    }
}
