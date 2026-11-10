//! The dock under the chat: the chips (what goes with the next question and
//! how), the picker and the drop target that add to them, and the question
//! box.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::ask::SendAs;
use magi_core::dto::WholeSize;

use super::chat::column;
use super::chip::{CHIP_NAME, chip_icon, chip_shell, source_tip, text_tip};
use super::{AskView, DOCK_OVERLAP};
use crate::ask::attach::{Notice, next_send, notice_text, send_text, send_tip, whole_note};
use crate::ask::{Source, blocked_text};
use crate::i18n::{Lang, Strings};
use crate::theme::Palette;
use crate::ui::{note, subheading, warn_note};

/// How long a notice stays, and how many dropped files are looked at.
const NOTICE_FOR: Duration = Duration::from_secs(5);

impl AskView {
    /// Says `notice` under the box for a few seconds.
    pub(super) fn say(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notice_generation += 1;
        let generation = self.notice_generation;
        self.notice = Some(notice);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_FOR).await;
            let _ = this.update(cx, |view, cx| {
                if view.notice_generation == generation {
                    view.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Ticks or unticks chip `ix`; a picture past the limit says why not.
    fn toggle_chip(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.model.is_asking() && !self.model.tick_allowed(ix) {
            self.say(Notice::TooManyPictures, cx);
            return;
        }
        self.model.toggle(ix);
        cx.notify();
    }

    /// Changes how chip `ix` is sent. A whole file also learns how much that
    /// is.
    pub fn switch_send(&mut self, ix: usize, send: SendAs, cx: &mut Context<Self>) {
        let Some(source) = self.model.sources().get(ix) else {
            return;
        };
        let file_id = source.file_id;
        if source.send == send {
            return;
        }
        if self.model.set_send(ix, send) {
            if send == SendAs::Whole {
                self.fetch_whole_size(file_id, cx);
            }
        } else if send == SendAs::Image && !self.model.is_asking() {
            self.say(Notice::TooManyPictures, cx);
        }
        cx.notify();
    }

    /// How much "whole file" sends, from the index, off the main thread.
    fn fetch_whole_size(&mut self, file_id: i64, cx: &mut Context<Self>) {
        if self.whole.contains_key(&file_id) {
            return;
        }
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let size = cx
                .background_executor()
                .spawn(async move { host.ask_whole_size(file_id) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match size {
                    Ok(size) => {
                        view.whole.insert(file_id, size);
                    }
                    Err(error) => tracing::warn!(file_id, %error, "no size for a whole file"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// many files go.
    fn caption(
        &self,
        s: &Strings,
        lang: Lang,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(notice) = self.notice {
            return Some(
                div()
                    .id("ask-notice")
                    .role(Role::Status)
                    .child(warn_note(p, notice_text(notice, s)).mt_0())
                    .into_any_element(),
            );
        }
        let sources = self.model.sources();
        let question = self.question.read(cx).value();
        let blocked = self.model.blocked(&question);
        blocked
            .and_then(|reason| blocked_text(reason, !sources.is_empty(), s))
            .map(|text| warn_note(p, text).mt_0().into_any_element())
            .or_else(|| {
                let ticked = sources.iter().filter(|source| source.ticked).count();
                (!sources.is_empty()).then(|| {
                    note(p)
                        .mt_0()
                        .child(lang.plural(&s.ask_excerpts, ticked as u64))
                        .into_any_element()
                })
            })
    }

    /// One chip, all in one pill: the file (hover to preview it, click to
    /// send it or not), how it is sent (click to switch when it can go more
    /// than one way), and a way to take it off.
    fn dock_chip(&self, ix: usize, source: &Source, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let (on, busy) = (source.ticked, self.model.is_asking());
        let (line, ink, accent, selection, card) = (p.line, p.ink, p.accent, p.selection, p.card);
        let toggle = div()
            .id(("ask-source", ix))
            .debug_selector(move || format!("ask-source-{ix}"))
            .focusable()
            .focus_visible(move |d| d.bg(selection))
            .flex()
            .items_center()
            .gap(px(6.))
            .h_full()
            .pl(px(4.))
            .pr(px(8.))
            .rounded_full()
            .role(Role::CheckBox)
            .aria_label(source.file_name.clone())
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .when(!busy, |d| d.cursor_pointer())
            // Hovering shows the preview; the lift says there is one.
            .hover(move |d| d.bg(if on { card } else { selection }))
            .tooltip(source_tip(source))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_chip(ix, cx)))
            .child(
                div()
                    .flex_none()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(if on { accent.opacity(0.35) } else { line })
                    .child(chip_icon(source)),
            )
            .child(
                div()
                    .max_w(CHIP_NAME)
                    .truncate()
                    .when(!on, |d| d.text_color(p.mute).line_through())
                    .child(source.file_name.clone()),
            )
            .child(
                Icon::new(if on { IconName::Check } else { IconName::Plus })
                    .size(px(11.))
                    .text_color(if on { accent } else { p.mute }),
            );
        let remove = div()
            .id(("ask-remove", ix))
            .focusable()
            .focus_visible(move |d| d.bg(selection))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(22.))
            .mr(px(3.))
            .rounded_full()
            .cursor_pointer()
            .text_color(p.mute)
            .hover(move |d| d.bg(line).text_color(ink))
            .role(Role::Button)
            .aria_label(format!("{}: {}", s.ask_remove_file, source.file_name))
            .tooltip(text_tip(s.ask_remove_file))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.model.remove(ix);
                cx.notify();
            }))
            .child(Icon::new(IconName::Close).size(px(10.)));
        chip_shell()
            .h(px(32.))
            .gap(px(0.))
            .pl(px(3.))
            .text_size(px(12.5))
            .child(toggle)
            .child(div().w(px(1.)).h(px(16.)).bg(line))
            .child(self.send_pill(ix, source, lang, s, p, cx))
            .when(!busy, |d| d.child(remove))
            .when(busy, |d| d.pr(px(4.)))
            .map(|d| match on {
                true => d.border_color(accent.opacity(0.45)).bg(selection),
                false => d.border_color(line).opacity(0.6),
            })
    }

    /// How a chip's file goes, inside the chip: an icon and a word, and for a
    /// whole file how much. A file that can go another way shows the
    /// up-down chevrons and switches on click.
    fn send_pill(
        &self,
        ix: usize,
        source: &Source,
        lang: Lang,
        s: &Strings,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let size: Option<WholeSize> = self.whole.get(&source.file_id).copied();
        let busy = self.model.is_asking();
        let next = next_send(source).filter(|_| !busy);
        let (mute, ink, line, selection) = (p.mute, p.ink, p.line, p.selection);
        let icon = match source.send {
            SendAs::Passages => IconName::TextQuote,
            SendAs::Whole => IconName::FileText,
            SendAs::Image => IconName::Image,
        };
        let size_note = (source.send == SendAs::Whole)
            .then_some(size)
            .flatten()
            .map(|size| {
                div()
                    .text_size(px(11.))
                    .text_color(if size.cut { p.warn } else { mute })
                    .child(format!("· {}", whole_note(size, lang, s)))
            });
        let mut tip = send_tip(source.send, size, s);
        if next.is_some() {
            tip = format!("{tip} {}", s.ask_send_switch);
        }
        div()
            .id(("ask-send-as", ix))
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(24.))
            .mx(px(3.))
            .px(px(7.))
            .rounded_full()
            .text_size(px(11.5))
            .text_color(mute)
            .when(!source.ticked, |d| d.opacity(0.6))
            .tooltip(text_tip(tip))
            .child(Icon::new(icon).size(px(12.)))
            .child(send_text(source.send, s))
            .children(size_note)
            .when_some(next, |d, next| {
                d.focusable()
                    .focus_visible(move |d| d.bg(selection))
                    .cursor_pointer()
                    .role(Role::Button)
                    .aria_label(format!(
                        "{}: {}. {}",
                        source.file_name,
                        send_text(source.send, s),
                        s.ask_send_switch
                    ))
                    .hover(move |d| d.bg(line).text_color(ink))
                    .on_click(cx.listener(move |this, _, _, cx| this.switch_send(ix, next, cx)))
                    .child(Icon::new(IconName::ChevronsUpDown).size(px(11.)))
            })
    }

    /// Under the chat: the disclosure the first time, then the box with the
    /// chips that go with the next question.
    pub(super) fn dock(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let sources = self.model.sources();
        let question = self.question.read(cx).value();
        let blocked = self.model.blocked(&question);
        let caption = self.caption(s, lang, p, cx);
        let used = self.model.used().map(|(used, limit)| {
            s.ask_used
                .replace("{used}", &lang.number(u64::from(used)))
                .replace("{limit}", &lang.number(u64::from(limit)))
        });
        let busy = self.model.is_asking();
        let chips = sources
            .iter()
            .enumerate()
            .map(|(ix, source)| self.dock_chip(ix, source, p, cx));
        let send = Button::new("ask-send")
            .cursor_pointer()
            .primary()
            .small()
            .icon(IconName::ArrowUp)
            .tooltip(s.ask_send)
            .loading(busy)
            .disabled(blocked.is_some())
            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)));
        let disclosure = self.model.needs_disclosure().then(|| {
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .mb(px(10.))
                .px(px(12.))
                .py(px(10.))
                .rounded(px(12.))
                .border_1()
                .border_color(p.line)
                .bg(p.card)
                .child(
                    div().pt(px(1.)).child(
                        Icon::new(IconName::ShieldCheck)
                            .size(px(15.))
                            .text_color(p.accent),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.5))
                        .line_height(relative(1.45))
                        .text_color(p.mute)
                        .child(s.ask_disclosure_body),
                )
                .child(
                    Button::new("ask-got-it")
                        .cursor_pointer()
                        .ghost()
                        .xsmall()
                        .label(s.ask_got_it)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.accept_disclosure(window, cx);
                        })),
                )
        });
        div()
            .flex_none()
            .mt(-DOCK_OVERLAP)
            .px(px(20.))
            .pb(px(14.))
            .child(
                column().children(disclosure).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .px(px(12.))
                        .pt(px(10.))
                        .pb(px(8.))
                        .rounded(px(20.))
                        .border_1()
                        // Lights up once there is something to send.
                        .border_color(if question.is_empty() {
                            p.line
                        } else {
                            p.accent.opacity(0.5)
                        })
                        .bg(p.card)
                        .when(!p.dark, |d| d.shadow_sm())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .when(!sources.is_empty(), |d| {
                                    d.child(
                                        div().px(px(2.)).child(subheading(s.ask_what_gets_sent, p)),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(6.))
                                        .children(chips)
                                        .child(self.add_chip(p, s, cx))
                                        .when(!busy, |d| {
                                            d.child(
                                                div()
                                                    .pl(px(2.))
                                                    .text_size(px(12.))
                                                    .text_color(p.mute)
                                                    .child(s.ask_drag_hint),
                                            )
                                        }),
                                )
                                .children(
                                    self.picker
                                        .as_ref()
                                        .map(|picker| self.picker_panel(picker, p, s, cx)),
                                ),
                        )
                        .child(
                            div()
                                .px(px(2.))
                                .child(Textarea::new(&self.question).appearance(false)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().min_w_0().px(px(2.)).children(caption))
                                .children(used.map(|used| {
                                    div()
                                        .flex_none()
                                        .text_size(px(12.))
                                        .text_color(p.mute)
                                        .child(used)
                                }))
                                .child(send),
                        ),
                ),
            )
    }
}
