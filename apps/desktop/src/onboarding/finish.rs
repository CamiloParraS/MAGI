//! The finish screen: the shortcut to try, and how far the files are.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::dto::{ErrorCode, Install};
use serde_json::json;

use super::{OnboardingView, Readiness, progress, readiness, save_step};
use crate::app::AppEvent;
use crate::i18n::Strings;
use crate::settings::hotkey_keystroke;
use crate::theme::{self, Palette, Tone};
use crate::ui::keycaps::{key_caps, keycap};
use crate::ui::{card, heading, note};

impl OnboardingView {
    fn hotkey_tried(&self, cx: &App) -> bool {
        self.live.read(cx).hotkey_presses > self.hotkey_presses
    }

    /// "Press it again to close" only while search is open.
    fn works(&self, s: &Strings, cx: &App) -> &'static str {
        if self.live.read(cx).search_open {
            s.ob_hotkey_works
        } else {
            s.ob_hotkey_worked
        }
    }

    /// The finish step's Change: the recorder takes the next shortcut, as
    /// in settings; the shell registers it before it is saved (FR-7).
    pub fn record(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = crate::hotkey::from_keystroke(k) else {
            return;
        };
        self.recording = false;
        cx.notify();
        let (host, events) = (self.host.clone(), self.events.clone());
        cx.spawn_in(window, async move |this, cx| {
            if !crate::app::register_hotkey(&events, spec.clone()).await {
                return;
            }
            let saved = cx
                .background_executor()
                .spawn(async move { save_step(&host, json!({ "hotkey": spec })) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match saved {
                    Ok(ui) => {
                        view.hotkey = ui.hotkey.clone();
                        let _ = view.events.try_send(AppEvent::Ui(ui));
                    }
                    Err(error) => view.error = Some(ErrorCode::from(&error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn toggle_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = !self.recording;
        if self.recording {
            self.error = None;
            window.focus(&self.recorder, cx);
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// The finish line, centered: setup is done, so a check, the title, the
    /// shortcut as keycaps, and one plain sentence on how far the files are,
    /// no counts (Settings › Index has those).
    pub(super) fn indexing(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let ready = readiness(live.status.as_ref(), &live.features);
        // "Results get better" only while they still will.
        let intro = match ready {
            Readiness::Ready => s.ob_done_note_ready,
            Readiness::Preparing(_) => s.ob_done_note,
        };
        // Seen once, so it may arrive with a little life: it grows into its
        // fixed 52px slot (nothing around it moves) as it fades in.
        let accent = p.accent;
        let check = div()
            .size(px(52.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(accent.opacity(0.14))
                    .child(Icon::new(IconName::Check).size(px(26.)).text_color(accent))
                    .with_animation(
                        "done-check",
                        Animation::new(CHECK_IN).with_easing(ease_out_quint()),
                        |el, t| el.size(px(44. + 8. * t)).opacity(t),
                    ),
            );
        let recording = self.recording;
        let caps = Keystroke::parse(&hotkey_keystroke(&self.hotkey))
            .map(|k| key_caps(&k, s, magi_core::platform::SYMBOL_MODIFIERS, &self.held))
            .unwrap_or_default();
        let plus = !magi_core::platform::SYMBOL_MODIFIERS;
        let keys =
            div()
                .flex()
                .items_center()
                .gap_2()
                .children(caps.into_iter().enumerate().flat_map(|(ix, c)| {
                    let sep = (plus && ix > 0)
                        .then(|| div().text_color(p.mute).child("+").into_any_element());
                    sep.into_iter()
                        .chain([keycap(c, recording, p).into_any_element()])
                }));
        let tried = self.hotkey_tried(cx);
        let try_it = if recording {
            // In the try-it line's place, so the card keeps its height.
            note(p)
                .mt_0()
                .child(s.shortcut_recording)
                .into_any_element()
        } else if tried {
            theme::fade_in(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(p.tone(Tone::Ok))
                    .child(Icon::new(IconName::Check).size(px(14.)))
                    .child(self.works(s, cx)),
                "done-hotkey-works",
            )
            .into_any_element()
        } else if live.hotkey_conflict.is_some() {
            // Taken: said right under the keys, with Change just below it;
            // "Try it now" would be wrong.
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(p.warn)
                .child(Icon::new(IconName::TriangleAlert).size(px(14.)))
                .child(format!("{}. {}", s.shortcut_taken, s.shortcut_taken_note))
                .into_any_element()
        } else {
            // A hint, not a link: the keys themselves answer it.
            div()
                .text_sm()
                .text_color(p.mute)
                .child(s.ob_try_it)
                .into_any_element()
        };
        let shortcut = card(p)
            .w_full()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .py(px(24.))
            .track_focus(&self.recorder)
            .when(recording, |d| {
                d.border_color(p.accent).on_key_down(cx.listener(
                    |this, e: &KeyDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.record(&e.keystroke, window, cx);
                    },
                ))
            })
            .child(div().text_color(p.mute).child(s.ob_try_hotkey))
            .child(keys)
            // One height for every line in this slot, so swapping them
            // moves nothing.
            .child(div().min_h(px(20.)).flex().items_center().child(try_it))
            .child(
                // Outlined: it is a control, so it looks like one.
                Button::new("change-hotkey")
                    .cursor_pointer()
                    .small()
                    .outline()
                    .label(if recording {
                        s.cancel
                    } else {
                        s.shortcut_change
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_recording(window, cx))),
            );
        // Only while preparing: once ready, the intro already says so. A
        // thin bar under it, with no numbers.
        let preparing = match ready {
            Readiness::Ready => None,
            Readiness::Preparing(feature) => Some(feature),
        }
        .map(|feature| {
            let sentence = match feature {
                Some(feature) => s
                    .ob_preparing_feature
                    .replace("{name}", s.feature(feature).name),
                None => s.ob_preparing.to_string(),
            };
            let bar = Progress::new("preparing").color(p.accent);
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(p.dot(Tone::Busy))
                        .child(sentence),
                )
                .child(
                    div()
                        .w(px(260.))
                        .child(match live.status.as_ref().and_then(progress) {
                            Some(value) => bar.value(value),
                            None => bar.loading(true),
                        }),
                )
        });
        let failed = live
            .features
            .iter()
            .filter(|f| matches!(f.install, Install::Failed { .. }))
            .map(|f| {
                note(p).text_color(p.warn).child(
                    s.ob_download_failed
                        .replace("{name}", s.feature(f.feature).name),
                )
            });
        div()
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .pt(px(8.))
            .child(check)
            .child(heading(s.ob_done).mt(px(14.)).mb(px(4.)))
            .child(div().text_color(p.mute).max_w(px(460.)).child(intro))
            .child(div().mt(px(22.)).w_full().child(shortcut))
            .child(
                div()
                    .mt(px(18.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .children(preparing)
                    .children(failed)
                    .child(
                        // A quiet way out: muted, full ink on hover.
                        Button::new("open-settings")
                            .cursor_pointer()
                            .text()
                            .small()
                            .text_color(p.mute)
                            .label(s.open_settings)
                            .on_click(cx.listener(|this, _, window, _| {
                                this.leave(Some(AppEvent::Settings), window)
                            })),
                    ),
            )
    }
}

/// The finish screen's check growing in.
const CHECK_IN: Duration = Duration::from_millis(320);
