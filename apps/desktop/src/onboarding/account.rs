//! The account screen: sign in or create an account for Ask Magi. Shown only
//! when setup turned Ask Magi on and nobody is signed in; once signed in it
//! says so, and Continue moves on. Search needs no account, so it can be
//! skipped.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::*;

use super::OnboardingView;
use crate::ask::view::sign_in::sign_in_panel;
use crate::i18n::Strings;
use crate::theme::Palette;
use crate::ui::note;

impl OnboardingView {
    pub(super) fn account(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let body = match self.live.read(cx).account.signed_in_as.clone() {
            None => sign_in_panel(
                &self.sign_in,
                self.sign_in.read(cx).creating(),
                false,
                false,
                p,
                s,
            ),
            Some(email) => signed_in(&email, s, p),
        };
        div().my_auto().flex().flex_col().child(body)
    }

    /// Whether the account screen shows the signed-in confirmation.
    pub(super) fn is_signed_in(&self, cx: &App) -> bool {
        self.live.read(cx).account.signed_in_as.is_some()
    }
}

fn signed_in(email: &str, s: &Strings, p: &Palette) -> Div {
    let on_ok: Hsla = if p.dark { black() } else { white() };
    div()
        .w_full()
        .max_w(px(380.))
        .mx_auto()
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .p(px(6.))
                .rounded(px(18.))
                .bg(p.ok.opacity(if p.dark { 0.16 } else { 0.12 }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(40.))
                        .rounded(px(12.))
                        .bg(p.ok)
                        .child(Icon::new(IconName::Check).size(px(22.)).text_color(on_ok)),
                ),
        )
        .child(
            div()
                .mt(px(14.))
                .text_size(px(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_center()
                .child(s.ob_signed_in_title),
        )
        .child(
            div()
                .mt(px(6.))
                .font_weight(FontWeight::MEDIUM)
                .child(email.to_owned()),
        )
        .child(
            note(p)
                .mt(px(6.))
                .max_w(px(320.))
                .text_center()
                .child(s.ob_signed_in_note),
        )
}
