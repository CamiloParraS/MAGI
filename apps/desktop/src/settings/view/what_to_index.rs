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
use crate::ui::{grouped, note, row_icon, stack};

/// `el` styled for `on` (`style` takes 0 for off, 1 for on). Right after a
/// click it plays there from the other state, over [`theme::FADE`].
fn toggled(
    el: Div,
    touched: bool,
    on: bool,
    id: impl Into<ElementId>,
    style: impl Fn(Div, f32) -> Div + 'static,
) -> AnyElement {
    if !touched {
        return style(el, f32::from(u8::from(on))).into_any_element();
    }
    el.with_animation(
        id,
        Animation::new(theme::FADE).with_easing(ease_out_quint()),
        move |el, t| style(el, if on { t } else { 1. - t }),
    )
    .into_any_element()
}

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
            let (accent, line) = (p.accent, p.line);
            // Only a tile just clicked animates (the frame, the tick and the
            // glyph, both ways); opening the section shows each as it is.
            let touched = self.kinds_touched.contains(&kind);
            let key = kind as usize * 2 + usize::from(on);
            let frame = (on || touched).then(|| {
                toggled(
                    div().absolute().inset_0().rounded(px(8.)).border_1(),
                    touched,
                    on,
                    ("kind-frame", key),
                    move |el, a| {
                        el.border_color(accent.opacity(a))
                            .bg(accent.opacity(0.08 * a))
                    },
                )
            });
            let tick = toggled(
                div()
                    .flex_none()
                    .size(px(18.))
                    .rounded_full()
                    .border_1()
                    .flex()
                    .items_center()
                    .justify_center(),
                touched,
                on,
                ("kind-tick", key),
                move |el, a| {
                    el.border_color(line.blend(accent.opacity(a)))
                        .bg(accent.opacity(a))
                        .when(a > 0., |d| {
                            d.child(
                                Icon::new(IconName::Check)
                                    .size(px(8. + 4. * a))
                                    .text_color(white().opacity(a)),
                            )
                        })
                },
            );
            let glyph = toggled(
                div().child(theme::kind_glyph(kind, px(26.))),
                touched,
                on,
                ("kind-glyph", key),
                |el, a| el.opacity(0.45 + 0.55 * a),
            );
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
                                .child(glyph)
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
                    if !this.kinds_touched.contains(&kind) {
                        this.kinds_touched.push(kind);
                    }
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(row_icon(IconName::Files, p))
                    .child(self.labelled(
                        s.file_types,
                        Some(s.file_types_note),
                        Field::FileTypes,
                        s,
                        p,
                    )),
            )
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
                        .label(s.save)
                        .disabled(!excludes_edited)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.save_excludes(window, cx)),
                        ),
                ),
            )
    }
}
