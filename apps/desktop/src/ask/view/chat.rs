//! What the Ask window draws: the sign-in scene, the conversation (your
//! questions on the right, Magi's answers on the left with the files they
//! came from), and the dock with the chips and the question box.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{AskView, COLUMN};
use crate::ask::{Answer, Source, Turn, TurnState, ai_note, blocked_text, problem_text};
use crate::i18n::{Lang, Strings};
use crate::search::highlight::one_line;
use crate::theme::{self, Palette};
use crate::ui::{note, warn_note};

/// The glyph on a chip, and the room a chip's file name gets.
const CHIP_GLYPH: Pixels = px(14.);
const CHIP_NAME: Pixels = px(170.);
/// What a chip's tooltip quotes of its excerpt.
const TIP_LEN: usize = 280;

/// The column everything sits in, centered when the window is wide.
fn column() -> Div {
    div().w_full().max_w(COLUMN).mx_auto()
}

/// The excerpt as a tooltip: a file's name says little about what goes.
fn excerpt_tip(source: &Source) -> SharedString {
    let text = one_line(&source.text);
    let mut tip: String = text.chars().take(TIP_LEN).collect();
    if text.chars().count() > TIP_LEN {
        tip.push('…');
    }
    tip.into()
}

/// A chip's face: its number when it has one, the file's glyph and name.
fn chip_face(number: Option<usize>, source: &Source, p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(26.))
        .pl(px(if number.is_some() { 4. } else { 8. }))
        .pr(px(10.))
        .rounded_full()
        .border_1()
        .text_size(px(12.))
        .when_some(number, |d, n| {
            d.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded_full()
                    .bg(p.selection)
                    .text_color(p.accent)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(n.to_string()),
            )
        })
        .child(theme::file_glyph(source.kind, &source.file_name, CHIP_GLYPH).flex_none())
        .child(
            div()
                .max_w(CHIP_NAME)
                .truncate()
                .child(source.file_name.clone()),
        )
}

/// Magi's mark beside each answer.
fn magi_mark(p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded_full()
                .bg(p.selection)
                .child(
                    Icon::new(IconName::Sparkles)
                        .size(px(13.))
                        .text_color(p.accent),
                ),
        )
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Magi"),
        )
}

/// Your question, on the right like any chat.
fn question_bubble(text: &str, p: &Palette) -> Div {
    div().flex().justify_end().child(
        div()
            .max_w(relative(0.8))
            .px(px(14.))
            .py(px(9.))
            .rounded(px(18.))
            .rounded_br(px(6.))
            .bg(p.selection)
            .line_height(relative(1.5))
            .child(text.to_owned()),
    )
}

impl AskView {
    pub(super) fn sign_in_scene(
        &self,
        expired: bool,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let s = self.live.read(cx).lang.strings();
        div()
            .id("ask-sign-in")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(24.))
            .py(px(32.))
            .child(
                div()
                    .w_full()
                    .max_w(px(380.))
                    .mx_auto()
                    .my_auto()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(magi_mark(p).mb(px(10.)))
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(s.ask_sign_in_title),
                    )
                    .child(
                        div()
                            .text_color(p.mute)
                            .line_height(relative(1.5))
                            .child(s.ask_sign_in_body),
                    )
                    .when(expired, |d| {
                        d.child(warn_note(p, s.ask_session_ended).mt(px(8.)))
                    })
                    .child(div().mt(px(16.)).child(self.sign_in.clone())),
            )
    }

    /// The chat so far, or before the first question what it is for.
    pub(super) fn conversation(&self, p: &Palette, cx: &Context<Self>) -> Stateful<Div> {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let turns = self.model.turns();
        let last = turns.len().saturating_sub(1);
        let content = if turns.is_empty() {
            self.empty_state(s, p).into_any_element()
        } else {
            column()
                .flex()
                .flex_col()
                .gap(px(22.))
                .children(turns.iter().enumerate().map(|(ix, turn)| {
                    theme::fade_in(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .child(question_bubble(&turn.question, p))
                            .child(self.reply(ix, turn, ix == last, lang, s, p, cx)),
                        ("ask-turn", ix),
                    )
                }))
                .into_any_element()
        };
        div()
            .id("ask-chat")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(24.))
            .pt(px(24.))
            .pb(px(12.))
            .child(content)
    }

    fn empty_state(&self, s: &Strings, p: &Palette) -> Div {
        let body = if self.model.sources().is_empty() {
            s.ask_no_text
        } else {
            s.ask_empty_body
        };
        column()
            .my_auto()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .px(px(24.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(44.))
                    .mb(px(6.))
                    .rounded_full()
                    .bg(p.selection)
                    .child(
                        Icon::new(IconName::Sparkles)
                            .size(px(20.))
                            .text_color(p.accent),
                    ),
            )
            .child(
                div()
                    .text_size(px(19.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s.ask_empty_title),
            )
            .child(
                div()
                    .max_w(px(380.))
                    .text_center()
                    .text_color(p.mute)
                    .line_height(relative(1.5))
                    .child(body),
            )
    }

    /// Magi's side of a turn: on its way, the answer, a failure or the limit.
    #[allow(clippy::too_many_arguments)]
    fn reply(
        &self,
        ix: usize,
        turn: &Turn,
        last: bool,
        lang: Lang,
        s: &Strings,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Div {
        let body = match &turn.state {
            TurnState::Asking => div()
                .id("ask-status")
                .flex()
                .items_center()
                .gap(px(8.))
                .text_color(p.mute)
                .role(Role::Status)
                .child(Spinner::new().xsmall().color(p.accent))
                .child(s.ask_asking)
                .into_any_element(),
            TurnState::Answered(answer) => {
                self.answer(ix, turn, answer, s, p, cx).into_any_element()
            }
            TurnState::Failed(problem) => div()
                .flex()
                .items_center()
                .gap_3()
                .child(warn_note(p, problem_text(*problem, s)).mt_0())
                .when(last, |d| {
                    d.child(
                        Button::new("ask-retry")
                            .cursor_pointer()
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCcw)
                            .label(s.try_again)
                            .on_click(cx.listener(|this, _, window, cx| this.retry(window, cx))),
                    )
                })
                .into_any_element(),
            TurnState::Limit { limit, everyone } => {
                let title = if *everyone {
                    s.ask_limit_everyone_title.to_owned()
                } else {
                    s.ask_limit_title
                        .replace("{n}", &lang.number(u64::from(*limit)))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(title))
                    .child(div().text_color(p.mute).child(s.ask_limit_body))
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(magi_mark(p))
            .child(div().pl(px(32.)).child(body))
    }

    /// The answer, then the files it came from as chips that open them.
    fn answer(
        &self,
        turn_ix: usize,
        turn: &Turn,
        answer: &Answer,
        s: &Strings,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Div {
        let chips = answer.cited.iter().map(|&ix| {
            let source = &turn.sent[ix];
            let (file_id, tip) = (source.file_id, excerpt_tip(source));
            div()
                .id(("ask-cite", turn_ix * 16 + ix))
                .debug_selector(move || format!("ask-cite-{turn_ix}-{ix}"))
                .cursor_pointer()
                .role(Role::Button)
                .aria_label(source.file_name.clone())
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| this.open_file(file_id, cx)))
                .child(
                    chip_face(Some(ix + 1), source, p)
                        .border_color(p.line)
                        .bg(p.card)
                        .hover(|d| d.bg(p.selection)),
                )
        });
        let gone = turn
            .sent
            .iter()
            .any(|source| self.gone == Some(source.file_id));
        div()
            .flex()
            .flex_col()
            .child(div().line_height(relative(1.6)).child(answer.text.clone()))
            .when(!answer.cited.is_empty(), |d| {
                d.child(
                    div()
                        .mt(px(12.))
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .children(chips),
                )
            })
            .when(gone, |d| d.child(warn_note(p, s.ask_file_gone).mt(px(8.))))
            .child(note(p).mt(px(10.)).child(ai_note(answer, s)))
    }

    /// Under the chat: the disclosure the first time, then the box with the
    /// chips that go with the next question.
    pub(super) fn dock(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let sources = self.model.sources();
        let question = self.question.read(cx).value();
        let blocked = self.model.blocked(&question);
        let ticked = sources.iter().filter(|source| source.ticked).count();
        let caption = blocked
            .and_then(|reason| blocked_text(reason, !sources.is_empty(), s))
            .map(|text| warn_note(p, text).mt_0().into_any_element())
            .or_else(|| {
                (!sources.is_empty()).then(|| {
                    note(p)
                        .mt_0()
                        .child(lang.plural(&s.ask_excerpts, ticked as u64))
                        .into_any_element()
                })
            });
        let used = self.model.used().map(|(used, limit)| {
            s.ask_used
                .replace("{used}", &lang.number(u64::from(used)))
                .replace("{limit}", &lang.number(u64::from(limit)))
        });
        let busy = self.model.is_asking();
        let chips = sources.iter().enumerate().map(|(ix, source)| {
            let tip = excerpt_tip(source);
            let on = source.ticked;
            div()
                .id(("ask-source", ix))
                .debug_selector(move || format!("ask-source-{ix}"))
                .role(Role::CheckBox)
                .aria_label(source.file_name.clone())
                .aria_toggled(if on { Toggled::True } else { Toggled::False })
                .when(!busy, |d| d.cursor_pointer())
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.toggle(ix);
                    cx.notify();
                }))
                .child(
                    chip_face(None, source, p)
                        .map(|chip| match on {
                            true => chip.border_color(p.accent.opacity(0.45)).bg(p.selection),
                            false => chip
                                .border_color(p.line)
                                .text_color(p.mute)
                                .line_through()
                                .opacity(0.6),
                        })
                        .child(
                            Icon::new(if on { IconName::Check } else { IconName::Plus })
                                .size(px(11.))
                                .text_color(if on { p.accent } else { p.mute }),
                        ),
                )
        });
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
        div().flex_none().px(px(20.)).pt(px(4.)).pb(px(16.)).child(
            column()
                .children(disclosure)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .px(px(12.))
                        .pt(px(10.))
                        .pb(px(8.))
                        .rounded(px(18.))
                        .border_1()
                        .border_color(p.line)
                        .bg(p.card)
                        .shadow_sm()
                        .when(!sources.is_empty(), |d| {
                            d.child(div().flex().flex_wrap().gap(px(6.)).children(chips))
                        })
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
                                .child(send),
                        ),
                )
                .children(used.map(|used| {
                    div()
                        .mt(px(6.))
                        .flex()
                        .justify_end()
                        .px(px(6.))
                        .child(note(p).mt_0().child(used))
                })),
        )
    }
}
