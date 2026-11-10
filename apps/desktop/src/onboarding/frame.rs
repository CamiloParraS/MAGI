//! The onboarding window's frame: the current screen sliding in, its error,
//! and the bar with the way forward.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Back, CONTEXT, Next, OnboardingView, Step, download_total};
use crate::settings::error_text;
use crate::theme::Palette;
use crate::ui::{note, warn_note};

impl Render for OnboardingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        if self.focus_account {
            self.focus_account = false;
            if !self.is_signed_in(cx) {
                self.sign_in.update(cx, |form, cx| form.focus(window, cx));
            }
        }
        let body = match self.step {
            Step::Setup => self.setup(s, &p, cx),
            Step::Account => self.account(s, &p, cx),
            Step::Indexing => self.indexing(s, &p, cx),
        };
        let signed_in = self.is_signed_in(cx);
        let can_continue = self.can_continue(cx);
        let no_folder = self.roots(cx).is_none_or(|roots| roots.is_empty());
        let folder_count = self.roots(cx).map_or(0, |roots| roots.len());
        let total = download_total(&self.live.read(cx).features, &self.chosen(cx));
        // The consent is the button: it says what agreeing downloads.
        let next_label: SharedString = match self.step {
            Step::Setup if total > 0 => s
                .ob_download_and_start
                .replace("{size}", &self.live.read(cx).lang.size(total))
                .into(),
            Step::Setup => s.ob_start.into(),
            Step::Account if signed_in => s.ob_continue.into(),
            Step::Account => s.ob_account_skip.into(),
            Step::Indexing => s.ob_start_searching.into(),
        };
        // What Start sets up, in one line: the folders and the download.
        let summary: SharedString = if no_folder {
            s.ob_need_folder.into()
        } else {
            let lang = self.live.read(cx).lang;
            let folders = lang.plural(&s.ob_folder_count, folder_count as u64);
            if total > 0 {
                format!(
                    "{folders} · {}",
                    s.ob_download_size.replace("{size}", &lang.size(total))
                )
            } else {
                format!("{folders} · {}", s.ob_nothing_to_download)
            }
            .into()
        };
        // A soft wash of the accent behind the top of the window: light mode
        // otherwise has no color but the button.
        let wash = p.wash(px(260.));
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            // Bindings run before the recorder's keys: while it waits, Enter
            // does nothing and Esc cancels it. On setup Enter does nothing
            // either: a stray key must not start a download; the focused
            // button still takes it.
            .on_action(cx.listener(|this, _: &Next, window, cx| {
                if !this.recording && this.step == Step::Indexing {
                    this.next(window, cx);
                }
            }))
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| {
                this.held = e.modifiers;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Back, window, cx| {
                if this.recording {
                    this.toggle_recording(window, cx);
                } else {
                    this.back(cx);
                }
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.solid)
            .text_color(p.ink)
            .text_size(px(14.))
            .child(wash)
            .child(
                div()
                    .id("onboarding")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(32.))
                    .pt(px(16.))
                    .pb(px(16.))
                    // The finish screen is short: centered in the space,
                    // not stacked at the top.
                    .flex()
                    .flex_col()
                    .when(self.step == Step::Indexing, |d| d.justify_center())
                    .child(slide_in(body, self.step, self.forward))
                    .children(
                        self.error
                            .as_ref()
                            .map(|error| warn_note(&p, error_text(error, s)).mt(px(10.))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(32.))
                    .py(px(16.))
                    .border_t_1()
                    .border_color(p.line)
                    // Why Start is off, or that nothing downloads.
                    .when(self.step == Step::Setup, |bar| {
                        bar.child(
                            note(&p)
                                .mt_0()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(summary),
                        )
                    })
                    // Back across from the way forward, where wizards keep it.
                    .when(self.step != Step::Setup, |bar| {
                        bar.child(
                            Button::new("back")
                                .cursor_pointer()
                                .ghost()
                                .icon(IconName::ArrowLeft)
                                .label(s.back)
                                .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                        )
                    })
                    .when(self.step != Step::Setup, |bar| bar.child(div().flex_1()))
                    .child(
                        Button::new("next")
                            .when(can_continue, |b| b.cursor_pointer())
                            // Skipping sign-in is quiet; signed in, Continue leads.
                            .map(|b| {
                                if self.step == Step::Account && !signed_in {
                                    b.ghost()
                                } else {
                                    b.primary()
                                }
                            })
                            .label(next_label)
                            .loading(self.busy)
                            .disabled(!can_continue && !self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            )
    }
}

/// How long a screen takes to slide in; onboarding is seen once, so a
/// little longer than [`theme::FADE`].
pub(super) const STEP_IN: Duration = Duration::from_millis(220);

/// A screen's body fades in from 8px toward where it came from: forward
/// from the right, back from the left. Under reduced motion GPUI shows the
/// end state.
fn slide_in(body: Div, step: Step, forward: bool) -> impl IntoElement {
    let from = if forward { 8. } else { -8. };
    body.relative().with_animation(
        ("step", step as usize),
        Animation::new(STEP_IN).with_easing(ease_out_quint()),
        move |el, t| el.opacity(t).left(px(from * (1. - t))),
    )
}
