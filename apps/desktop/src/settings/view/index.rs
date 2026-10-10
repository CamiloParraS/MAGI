//! Settings › Index: counts, the files that could not be read, clearing.

use gpui_kit::assets::IconName;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Field, SettingsView};
use crate::settings::file_error_label;
use crate::theme::{Palette, Tone};
use crate::ui::{card, note, stack, subheading};

/// How many unreadable files the Index section lists.
const ERRORS_SHOWN: u32 = 50;

impl SettingsView {
    /// A failed read leaves `None`, which shows no list rather than "every
    /// file was read".
    pub(super) fn load_unreadable(&mut self, cx: &mut Context<Self>) {
        self.errors_listed = self.live.read(cx).status.as_ref().map(|st| st.errors);
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move { host.list_errors(ERRORS_SHOWN) })
                .await
                .inspect_err(|error| tracing::warn!(%error, "could not list the unreadable files"))
                .ok();
            let _ = this.update(cx, |view, cx| {
                view.unreadable = listed;
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks first: clearing forgets every indexed file.
    fn confirm_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let indexed = live.status.as_ref().map_or(0, |st| st.indexed);
        let body: SharedString = lang.plural(&s.clear_confirm_body, indexed).into();
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title(s.clear_confirm_title)
                .description(body.clone())
                .ok_text(s.clear_index)
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(s.cancel)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = view.update(cx, |view, cx| {
                        view.run(Field::Clear, |host| host.clear_index(), cx);
                    });
                    true
                })
        });
    }

    pub(super) fn index(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let stats = live.status.as_ref().map(|st| {
            let stat = |value: u64, label: &'static str, tone: Option<Tone>| {
                card(p)
                    .flex_1()
                    .flex_col()
                    .items_start()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .when_some(tone, |d, t| d.text_color(p.tone(t)))
                            .child(lang.number(value)),
                    )
                    .child(div().text_xs().text_color(p.mute).child(label))
            };
            div()
                .flex()
                .gap(px(8.))
                .child(stat(st.indexed, s.stat_indexed, None))
                .child(stat(st.queued, s.stat_waiting, None))
                .child(stat(st.skipped, s.stat_skipped, None))
                .child(stat(
                    st.errors,
                    s.stat_errors,
                    (st.errors > 0).then_some(Tone::Err),
                ))
        });
        let unreadable = self.unreadable.as_deref().unwrap_or_default();
        let files = match self.unreadable.as_deref() {
            None => stack(),
            Some([]) => stack().child(card(p).text_color(p.mute).child(s.all_read)),
            Some(unreadable) => stack().children(unreadable.iter().map(|e| {
                let path = std::path::Path::new(&e.path);
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let folder = path
                    .parent()
                    .map(std::path::Path::display)
                    .map(|d| d.to_string());
                let folder = folder.unwrap_or_default();
                card(p)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(16.))
                            .text_color(p.err),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(name.to_string()))
                            .child(
                                note(p)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis_middle()
                                    .child(format!("{} · {folder}", file_error_label(e.code, s))),
                            ),
                    )
            })),
        };
        div()
            .children(stats)
            .child(
                div()
                    .mt(px(22.))
                    .mb(px(8.))
                    .child(subheading(s.unreadable, p)),
            )
            .child(files)
            .children(self.problem(Field::Unreadable, s, p))
            .when(!unreadable.is_empty(), |d| {
                d.child(
                    div().mt(px(8.)).child(
                        Button::new("retry-all")
                            .cursor_pointer()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(s.retry_all)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run(
                                    Field::Unreadable,
                                    |host| host.engine()?.retry_errors().map(drop),
                                    cx,
                                );
                            })),
                    ),
                )
            })
            .child(
                div()
                    .mt(px(22.))
                    .mb(px(8.))
                    .child(subheading(s.danger_zone, p)),
            )
            .child(
                self.card(Field::Clear, p)
                    .border_color(p.err)
                    .child(self.labelled(
                        s.clear_index,
                        Some(s.clear_index_note),
                        Field::Clear,
                        s,
                        p,
                    ))
                    .child(
                        Button::new("clear-index")
                            .cursor_pointer()
                            .danger()
                            .small()
                            .label(s.clear_index_button)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_clear(window, cx)),
                            ),
                    ),
            )
    }
}
