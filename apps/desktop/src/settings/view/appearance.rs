//! Settings › Appearance: theme, window background, and the search
//! window's layout.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::Slider;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;
use std::time::Duration;

use super::{Field, SettingsView};
use crate::i18n::Strings;
use crate::settings::{intensity_from_slider, see_through_adjustable};
use crate::theme::Palette;
use crate::ui::{grouped, row_icon};

impl SettingsView {
    pub(super) fn save_intensity(
        &mut self,
        see_through: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let intensity = intensity_from_slider(see_through);
        self.save(
            Field::SeeThrough,
            json!({ "ui": { "transparency_intensity": intensity } }),
            window,
            cx,
        );
    }

    fn nudge_see_through(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let step = match event.keystroke.key.as_str() {
            "left" => -0.05,
            "right" => 0.05,
            _ => return cx.propagate(),
        };
        if !see_through_adjustable(self.ui.transparency_mode, self.support) {
            return;
        }
        let value = (self.see_through.read(cx).value().end() + step).clamp(0., 1.);
        self.see_through
            .update(cx, |slider, cx| slider.set_value(value, window, cx));
        self.save_intensity(value, window, cx);
    }

    pub(super) fn appearance(&self, p: &Palette, window: &Window, cx: &Context<Self>) -> Div {
        let s = self.live.read(cx).lang.strings();
        let adjustable = see_through_adjustable(self.ui.transparency_mode, self.support);
        grouped(
            p,
            [
                self.setting(
                    Field::Theme,
                    IconName::SunMoon,
                    s.theme,
                    None,
                    div().w(px(260.)).child(
                        Select::new(&self.theme_choice)
                            .cursor_pointer()
                            .bg(p.solid)
                            .accessibility_label(s.theme),
                    ),
                    s,
                    p,
                ),
                self.setting(
                    Field::Background,
                    IconName::AppWindow,
                    s.window_background,
                    None,
                    div().w(px(260.)).child(
                        Select::new(&self.background)
                            .cursor_pointer()
                            .bg(p.solid)
                            .accessibility_label(s.window_background),
                    ),
                    s,
                    p,
                ),
                self.setting(
                    Field::SeeThrough,
                    IconName::Blend,
                    s.see_through_amount,
                    (!adjustable).then_some(s.see_through_off),
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .text_xs()
                        .text_color(p.mute)
                        .child(s.more_solid)
                        .child(
                            div()
                                .id("see-through")
                                .track_focus(&self.see_through_focus)
                                .on_key_down(cx.listener(Self::nudge_see_through))
                                .w(px(172.))
                                .px(px(6.))
                                .py_1()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(if self.see_through_focus.is_focused(window) {
                                    p.accent
                                } else {
                                    transparent_black()
                                })
                                .child(
                                    Slider::new(&self.see_through)
                                        .when(adjustable, |s| s.cursor_pointer())
                                        .disabled(!adjustable),
                                ),
                        )
                        .child(s.more_transparent),
                    s,
                    p,
                ),
                self.search_layout(s, p, cx),
            ],
        )
    }

    /// `ui.detail_pane` as a choice between two pictures of the search
    /// window, since what it changes is easier seen than described.
    fn search_layout(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let on = self.ui.detail_pane;
        let choice = |split: bool, label: &'static str| {
            layout_choice(split, label, on == split, p).on_click(cx.listener(
                move |this, _, window, cx| {
                    if this.ui.detail_pane != split {
                        this.save(
                            Field::DetailPane,
                            json!({ "ui": { "detail_pane": split } }),
                            window,
                            cx,
                        );
                    }
                },
            ))
        };
        self.row(Field::DetailPane, p)
            .flex_col()
            .items_stretch()
            .gap_3()
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .w_full()
                    .child(row_icon(IconName::PanelRight, p))
                    .child(self.labelled(
                        s.search_layout,
                        Some(s.search_layout_note),
                        Field::DetailPane,
                        s,
                        p,
                    )),
            )
            .child(
                div()
                    .id("search-layout")
                    .role(Role::RadioGroup)
                    .aria_label(s.search_layout)
                    .flex()
                    .gap_4()
                    .w_full()
                    // Under the label's text, past the icon.
                    .pl(px(28.))
                    .child(choice(false, s.layout_list))
                    .child(choice(true, s.layout_preview)),
            )
    }
}

/// A layout picture's height; its width is the row's share.
const SKETCH_H: Pixels = px(164.);
/// How long the chosen picture's selection rests on a result before it
/// steps to the next.
const SKETCH_STEP: Duration = Duration::from_millis(1600);
/// The results in a picture; the selection walks them and starts over.
const SKETCH_ROWS: usize = 5;

/// One layout to pick: its picture, framed in the accent when chosen, and
/// its name under it. The two share the row's width.
fn layout_choice(split: bool, label: &'static str, chosen: bool, p: &Palette) -> Stateful<Div> {
    div()
        .id(if split {
            "layout-preview"
        } else {
            "layout-list"
        })
        .role(Role::RadioButton)
        .aria_label(label)
        .aria_toggled(if chosen {
            Toggled::True
        } else {
            Toggled::False
        })
        .cursor_pointer()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(8.))
        .child(
            div()
                .p(px(4.))
                .rounded(px(10.))
                .border_2()
                .border_color(if chosen { p.accent } else { p.line })
                .hover(|d| d.border_color(if chosen { p.accent } else { p.mute }))
                .child(sketch(split, chosen, p)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_sm()
                .text_color(if chosen { p.ink } else { p.mute })
                .when(chosen, |d| {
                    d.child(
                        Icon::new(IconName::Check)
                            .size(px(13.))
                            .text_color(p.accent),
                    )
                })
                .child(label),
        )
}

/// A search window in miniature: the input, a few results (one selected),
/// and with `split` the detail pane beside them: its preview, name, a
/// snippet, then the tags with the actions under them, right-aligned, as in
/// the window. Widths are shares of the picture, so it fills whatever room
/// the row gives it. As in the window, only a line between the list and
/// the pane.
///
/// The chosen picture is the live one: the selection walks down the list
/// and the pane's contents settle in again at each step, as a real
/// selection does. The other stays still (and so does everything, with
/// reduced motion), which also says which is on.
fn sketch(split: bool, chosen: bool, p: &Palette) -> Div {
    let bar = |w: f32, color: Hsla| div().h(px(4.)).w(relative(w)).rounded_full().bg(color);
    let strong = p.mute.opacity(0.45);
    let selection = p.selection;
    let result = |ix: usize| {
        let row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(6.))
            .py(px(4.))
            .rounded(px(3.))
            .child(div().flex_none().size(px(12.)).rounded(px(2.)).bg(p.chip))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .flex_1()
                    .child(bar(0.55, strong))
                    .child(bar(0.85, p.line)),
            );
        if chosen {
            // Each row lights while the walk is on it: a quick rise and a
            // slower fade, so the selection seems to travel.
            row.with_animation(
                (if split { "sk-split-row" } else { "sk-list-row" }, ix),
                Animation::new(SKETCH_STEP * SKETCH_ROWS as u32).repeat(),
                move |el, t| {
                    let d = (t * SKETCH_ROWS as f32 - ix as f32).rem_euclid(SKETCH_ROWS as f32);
                    let on = if d < 1. {
                        (d / 0.18).min(1.) * ((1. - d) / 0.3).min(1.)
                    } else {
                        0.
                    };
                    el.bg(selection.opacity(on))
                },
            )
            .into_any_element()
        } else {
            row.when(ix == 0, |d| d.bg(selection)).into_any_element()
        }
    };
    let list = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .p(px(4.))
        .when(split, |d| d.w(relative(0.42)).flex_none())
        .when(!split, |d| d.flex_1())
        .children((0..SKETCH_ROWS).map(result));
    let pane = split.then(|| {
        let tag = |w: f32| {
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .h(px(8.))
                .w(px(w))
                .px(px(2.))
                .rounded_full()
                .bg(p.chip)
                .child(div().size(px(3.)).rounded_full().bg(p.accent))
        };
        let content = div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .size_full()
            .child(div().h(px(34.)).w_full().rounded(px(3.)).bg(p.chip))
            .child(bar(0.6, strong))
            .child(bar(0.4, p.line))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .p(px(4.))
                    .rounded(px(3.))
                    .bg(p.card)
                    .child(bar(0.9, p.line))
                    .child(bar(0.7, p.line)),
            )
            // Why it matched, then the actions at the far end.
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .gap(px(3.))
                            .child(tag(20.))
                            .child(tag(26.))
                            .child(tag(16.)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(3.))
                            .child(
                                div()
                                    .h(px(8.))
                                    .w(px(26.))
                                    .rounded(px(2.))
                                    .border_1()
                                    .border_color(p.line),
                            )
                            .child(div().h(px(8.)).w(px(18.)).rounded(px(2.)).bg(p.accent)),
                    ),
            );
        let content = if chosen {
            content
                .with_animation(
                    "sk-pane",
                    Animation::new(SKETCH_STEP)
                        .repeat()
                        .with_easing(ease_out_quint()),
                    // The new result's pane rises into place a few pixels.
                    |el, t| el.opacity(0.3 + 0.7 * t).pt(px(4. * (1. - t))),
                )
                .into_any_element()
        } else {
            content.into_any_element()
        };
        div()
            .flex_1()
            .p(px(8.))
            .border_l_1()
            .border_color(p.line)
            .child(content)
    });
    div()
        .flex()
        .flex_col()
        .w_full()
        .h(SKETCH_H)
        .rounded(px(6.))
        .overflow_hidden()
        .bg(p.solid)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(20.))
                .px(px(8.))
                .child(
                    div()
                        .flex_none()
                        .size(px(7.))
                        .rounded_full()
                        .border_1()
                        .border_color(p.mute),
                )
                .child(bar(0.35, strong)),
        )
        .child(div().flex().flex_1().min_h_0().child(list).children(pane))
}
