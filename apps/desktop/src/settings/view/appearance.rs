//! Settings › Appearance: theme and window background.

use gpui_kit::assets::IconName;
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::Slider;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

use super::{Field, SettingsView};
use crate::settings::{intensity_from_slider, see_through_adjustable};
use crate::theme::Palette;
use crate::ui::grouped;

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
            ],
        )
    }
}
