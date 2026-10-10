//! Settings › General: the shortcut, startup, language, results, quit.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::Button;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

use super::{Field, SettingsView};
use crate::app::AppEvent;
use crate::settings::hotkey_keystroke;
use crate::theme::Palette;
use crate::ui::{card, note, stack};

impl SettingsView {
    /// The recorder's keys: Esc cancels; a usable shortcut is registered by
    /// the shell first, and saved only if that worked. Anything else (a
    /// modifier still held, plain typing) keeps waiting.
    fn record_hotkey(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.record(&event.keystroke, window, cx);
    }

    pub fn record(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        if k.key == "escape" && !k.modifiers.modified() {
            self.recording = false;
            return cx.notify();
        }
        let Some(spec) = crate::hotkey::from_keystroke(k) else {
            return;
        };
        self.recording = false;
        cx.notify();
        let events = self.events.clone();
        cx.spawn_in(window, async move |this, cx| {
            // ponytail: a failed save leaves the new hotkey registered until
            // restart; re-register the old one if that ever matters.
            if crate::app::register_hotkey(&events, spec.clone()).await {
                let _ = this.update_in(cx, |view, window, cx| {
                    if spec != view.ui.hotkey {
                        view.save(
                            Field::Hotkey,
                            json!({ "ui": { "hotkey": spec } }),
                            window,
                            cx,
                        );
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn general(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let kbd = |spec: &str| Keystroke::parse(&hotkey_keystroke(spec)).ok().map(Kbd::new);
        let recording = self.recording;
        let conflict = live.hotkey_conflict.as_deref().map(|taken| {
            card(p)
                .border_color(p.warn)
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(16.))
                        .text_color(p.warn),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_color(p.warn).child(s.shortcut_taken))
                        .child(note(p).child(s.shortcut_taken_note)),
                )
                .children(kbd(taken))
        });
        stack()
            .child(
                self.card(Field::Hotkey, p)
                    .track_focus(&self.recorder)
                    .when(recording, |d| {
                        d.border_color(p.accent)
                            .on_key_down(cx.listener(Self::record_hotkey))
                    })
                    .child(
                        Icon::new(IconName::Keyboard)
                            .size(px(16.))
                            .text_color(p.mute),
                    )
                    .child(self.labelled(
                        s.shortcut,
                        Some(if recording {
                            s.shortcut_recording
                        } else {
                            s.shortcut_note
                        }),
                        Field::Hotkey,
                        s,
                        p,
                    ))
                    .children((!recording).then(|| kbd(&self.ui.hotkey)).flatten())
                    .child(
                        Button::new("change-hotkey")
                            .cursor_pointer()
                            .label(if recording {
                                s.cancel
                            } else {
                                s.shortcut_change
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.recording = !this.recording;
                                if this.recording {
                                    this.error = None;
                                    window.focus(&this.recorder, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .children(conflict)
            .child(
                self.setting(
                    Field::LaunchAtLogin,
                    Some(IconName::Power),
                    s.launch_at_login,
                    Some(s.launch_at_login_note),
                    Switch::new("launch-at-login")
                        .cursor_pointer()
                        .checked(self.ui.launch_at_login)
                        .color(p.accent)
                        .accessibility_label(s.launch_at_login)
                        .on_click(cx.listener(|this, &on: &bool, window, cx| {
                            this.save(
                                Field::LaunchAtLogin,
                                json!({ "ui": { "launch_at_login": on } }),
                                window,
                                cx,
                            );
                        })),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Language,
                    Some(IconName::Globe),
                    s.language,
                    None,
                    div().w(px(260.)).child(
                        Select::new(&self.language)
                            .cursor_pointer()
                            .bg(p.solid)
                            .accessibility_label(s.language),
                    ),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Results,
                    Some(IconName::Search),
                    s.results_shown,
                    Some(s.results_shown_note),
                    div().w(px(120.)).child(
                        Select::new(&self.results)
                            .cursor_pointer()
                            .bg(p.solid)
                            .accessibility_label(s.results_shown),
                    ),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Quit,
                    None,
                    s.quit_app,
                    Some(s.quit_app_note),
                    Button::new("quit")
                        .cursor_pointer()
                        .label(s.tray_quit)
                        .on_click(cx.listener(|this, _, _, _| {
                            let _ = this.events.try_send(AppEvent::Quit);
                        })),
                    s,
                    p,
                ),
            )
    }
}
