//! What indexing reads: file types and largest file (under What's
//! searched), exclusions (under Folders), and pause on battery (under
//! General).

use gpui_kit::assets::IconName;
use gpui_kit::component::Disableable as _;
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
use crate::ui::{grouped, note, row_icon, stack, warn_note};

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
        // One grouped list, as the search features above: a type's glyph, its
        // name and extensions, then the switch.
        let rows = FILE_TYPES.into_iter().map(|kind| {
            let on = types.contains(&kind);
            let name = kind_label(kind, s);
            self.row(Field::FileTypes, p)
                .py(px(10.))
                .child(div().flex_none().child(theme::kind_glyph(kind, px(26.))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::MEDIUM).child(name))
                        .children(kind_examples(kind).map(|e| note(p).child(e)))
                        // Why it reads nothing yet, in the muted tone: a state, not an alarm.
                        .children(
                            (kind == Kind::Image && images_needed)
                                .then(|| note(p).child(s.images_needed)),
                        ),
                )
                .child(
                    Switch::new(("kind", kind as usize))
                        .cursor_pointer()
                        .checked(on)
                        .color(p.accent)
                        .accessibility_label(name)
                        .on_click(cx.listener(move |this, &on: &bool, window, cx| {
                            let types = toggle_kind(&this.indexing.file_types, kind, on);
                            this.save(
                                Field::FileTypes,
                                json!({ "indexing": { "file_types": types } }),
                                window,
                                cx,
                            );
                        })),
                )
        });
        let none = types.is_empty().then(|| warn_note(p, s.kind_none).mt_0());
        let file_types = div()
            .child(note(p).mb(px(8.)).child(s.file_types_note))
            .child(grouped(p, rows))
            .children(none.map(|n| div().mt(px(8.)).child(n)))
            .children(self.problem(Field::FileTypes, s, p));
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
        stack().child(file_types).child(grouped(p, [max_size]))
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
        self.row(Field::Excludes, p)
            .flex_col()
            .items_stretch()
            .gap_3()
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(row_icon(IconName::FolderX, p))
                    .child(self.labelled(s.excludes, Some(s.excludes_note), Field::Excludes, s, p)),
            )
            // `rows` does not size the box; a height does.
            .child(Textarea::new(&self.excludes).h(px(150.)).bg(p.solid))
            .child(
                div().flex().justify_end().child(
                    Button::new("save-excludes")
                        .when(excludes_edited, |b| b.cursor_pointer())
                        .primary()
                        .label(if self.saved == Some(Field::Excludes) && !excludes_edited {
                            s.saved
                        } else {
                            s.save
                        })
                        .disabled(!excludes_edited)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.save_excludes(window, cx)),
                        ),
                ),
            )
    }
}
