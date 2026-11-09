//! Settings › Account: who is signed in to Ask Magi, signing out, and
//! deleting the account.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _};
use gpui_kit::*;

use super::{Field, SettingsView};
use crate::theme::Palette;
use crate::ui::{group, grouped, note, row_icon};

impl SettingsView {
    /// Tells every window who is signed in now; an open Ask window goes
    /// back to sign-in.
    pub(super) fn refresh_account(&self, cx: &mut Context<Self>) {
        match self.host.account_state() {
            Ok(account) => self.live.update(cx, |live, cx| {
                live.account = account;
                cx.notify();
            }),
            Err(error) => tracing::warn!(%error, "could not read the account state"),
        }
    }

    /// Asks first: this removes the account from the server for good.
    fn confirm_delete_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.live.read(cx).lang.strings();
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title(s.account_delete_confirm_title)
                .description(s.account_delete_confirm_body)
                .ok_text(s.account_delete)
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(s.cancel)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = view.update(cx, |view, cx| {
                        view.run(
                            Field::DeleteAccount,
                            |host| host.delete_account().map(drop),
                            cx,
                        );
                    });
                    true
                })
        });
    }

    pub(super) fn account(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        // Signed out: the Ask window's own form, so either place works.
        let Some(email) = live.account.signed_in_as.clone() else {
            return group(p).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .px_4()
                    .py(px(14.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(row_icon(IconName::CircleUser, p))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(s.ask_sign_in_title),
                            ),
                    )
                    .child(note(p).pl(px(28.)).child(s.ask_sign_in_body))
                    .child(
                        div()
                            .pl(px(28.))
                            .pt(px(10.))
                            .max_w(px(420.))
                            .child(self.sign_in.clone()),
                    ),
            );
        };
        let busy = self.is_busy(Field::SignOut) || self.is_busy(Field::DeleteAccount);
        div()
            .child(grouped(
                p,
                [self
                    .row(Field::SignOut, p)
                    .child(row_icon(IconName::CircleUser, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(s.account_signed_in_as)
                            .child(note(p).truncate().child(email))
                            .children(self.problem(Field::SignOut, s, p)),
                    )
                    .child(
                        Button::new("sign-out")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            .label(s.account_sign_out)
                            .loading(self.is_busy(Field::SignOut))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run(Field::SignOut, |host| host.sign_out().map(drop), cx);
                            })),
                    )],
            ))
            .child(
                group(p).mt(px(16.)).border_color(p.err).child(
                    self.row(Field::DeleteAccount, p)
                        .child(row_icon(IconName::Trash, p).text_color(p.err))
                        .child(self.labelled(
                            s.account_delete,
                            Some(s.account_delete_note),
                            Field::DeleteAccount,
                            s,
                            p,
                        ))
                        .child(
                            Button::new("delete-account")
                                .cursor_pointer()
                                .danger()
                                .small()
                                .label(s.account_delete)
                                .loading(self.is_busy(Field::DeleteAccount))
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_delete_account(window, cx)
                                })),
                        ),
                ),
            )
    }
}
