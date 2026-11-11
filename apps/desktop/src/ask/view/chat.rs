//! What the Ask window draws: the sign-in scene, the conversation (your
//! questions on the right, Magi's answers on the left with the files they
//! came from), and the dock with the chips and the question box.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::chip::{chip_face, source_tip};
use super::sign_in::sign_in_panel;
use super::{AskView, COLUMN, DOCK_OVERLAP};
use crate::ask::{Answer, Turn, TurnState, ai_note, problem_text};
use crate::i18n::{Lang, Strings};
use crate::theme::{self, Palette};
use crate::ui::{MARK, brand_tile, note, pressable, subheading, warn_note};

/// The column everything sits in, centered when the window is wide.
pub(super) fn column() -> Div {
    div().w_full().max_w(COLUMN).mx_auto()
}

/// Magi's avatar in the gutter beside each answer: the mark on the accent
/// tint.
fn magi_avatar(p: &Palette) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(9.))
        .bg(p.selection)
        .child(svg().path(MARK).size(px(17.)).text_color(p.accent))
}

/// Your question, on the right in an accent tint with an accent hairline:
/// who said what reads at a glance without a large accent fill.
fn question_bubble(text: &str, p: &Palette) -> Div {
    div().flex().justify_end().child(
        div()
            .max_w(relative(0.8))
            .px(px(15.))
            .py(px(10.))
            .rounded(px(18.))
            .rounded_br(px(5.))
            .bg(p.selection)
            .border_1()
            .border_color(p.accent.opacity(0.4))
            .text_color(p.ink)
            .line_height(relative(1.5))
            .child(text.to_owned()),
    )
}

/// Three dots that rise and fall in turn while Magi works.
fn thinking_dots(p: &Palette) -> Div {
    let still = magi_core::platform::reduce_motion();
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .children((0..3usize).map(|i| {
            let dot = div().size(px(6.)).rounded_full().bg(p.accent);
            if still {
                return dot.opacity(0.6).into_any_element();
            }
            dot.with_animation(
                ("ask-dot", i),
                Animation::new(Duration::from_millis(1300))
                    .repeat()
                    .with_easing(ease_in_out),
                move |d, t| {
                    let phase = (t + 1. - i as f32 * 0.16) % 1.;
                    d.opacity(0.22 + 0.78 * (1. - (phase * 2. - 1.).abs()))
                },
            )
            .into_any_element()
        }))
}

/// A pill that puts a starter question in the box.
fn suggestion(
    id: usize,
    text: &'static str,
    p: &Palette,
    cx: &Context<AskView>,
) -> impl IntoElement {
    pressable(
        div()
            .flex()
            .items_center()
            .gap(px(7.))
            .h(px(32.))
            .px(px(13.))
            .rounded_full()
            .border_1()
            .border_color(p.line)
            .bg(p.card)
            .text_size(px(13.))
            .hover(|d| d.border_color(p.accent.opacity(0.5)).bg(p.selection))
            .child(
                Icon::new(IconName::Sparkles)
                    .size(px(12.))
                    .text_color(p.accent),
            )
            .child(text),
        ("ask-suggest", id),
    )
    .cursor_pointer()
    .role(Role::Button)
    .aria_label(text)
    .on_click(cx.listener(move |this, _, window, cx| this.set_question(text, window, cx)))
}

impl AskView {
    pub(super) fn sign_in_scene(
        &self,
        expired: bool,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let s = self.live.read(cx).lang.strings();
        let creating = self.sign_in.read(cx).creating();
        div()
            .id("ask-sign-in")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(24.))
            .py(px(28.))
            .child(sign_in_panel(&self.sign_in, creating, expired, false, p, s).my_auto())
    }

    /// The chat so far, or before the first question what it is for.
    pub(super) fn conversation(&self, p: &Palette, cx: &Context<Self>) -> Stateful<Div> {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let turns = self.model.turns();
        let last = turns.len().saturating_sub(1);
        let content = if turns.is_empty() {
            self.empty_state(s, p, cx).into_any_element()
        } else {
            column()
                .flex()
                .flex_col()
                .gap(px(24.))
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
            // Redraw on scroll, so the arrow to the latest comes and goes.
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .px(px(24.))
            .pt(px(24.))
            // Room for the end to clear the dock, which overlaps this.
            .pb(DOCK_OVERLAP + px(16.))
            .child(content)
    }

    /// The chat is scrolled up from its end (with a little slack).
    fn above_the_end(&self) -> bool {
        let max = self.scroll.max_offset().y;
        max > px(1.) && -self.scroll.offset().y < max - px(24.)
    }

    /// The conversation, with an arrow back to the latest answer in the
    /// right margin while it is scrolled up. Its end runs under the dock.
    pub(super) fn chat_area(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let s = self.live.read(cx).lang.strings();
        let (card, line, ink, selection) = (p.card, p.line, p.ink, p.selection);
        let arrow = self.above_the_end().then(|| {
            div()
                .absolute()
                .bottom(DOCK_OVERLAP + px(10.))
                .right(px(10.))
                .child(
                    div()
                        .id("ask-scroll-down")
                        .focusable()
                        .focus_visible(move |d| d.bg(selection))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(32.))
                        .rounded_full()
                        .border_1()
                        .border_color(line)
                        .bg(card)
                        .shadow_md()
                        .cursor_pointer()
                        .text_color(ink)
                        .hover(move |d| d.bg(selection))
                        .role(Role::Button)
                        .aria_label(s.ask_scroll_down)
                        .tooltip(super::chip::text_tip(s.ask_scroll_down))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.scroll.scroll_to_bottom();
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::ArrowDown).size(px(15.))),
                )
        });
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.conversation(p, cx))
            .children(arrow)
    }

    fn empty_state(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let has_text = !self.model.sources().is_empty();
        let body = if has_text {
            s.ask_empty_body
        } else {
            s.ask_no_text
        };
        let ideas = [
            s.ask_suggest_summary,
            s.ask_suggest_common,
            s.ask_suggest_details,
        ];
        column()
            .my_auto()
            .flex()
            .flex_col()
            .items_center()
            .px(px(24.))
            .child(brand_tile(p, px(36.)).mb(px(10.)))
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s.ask_empty_title),
            )
            .child(
                div()
                    .mt(px(6.))
                    .max_w(px(380.))
                    .text_center()
                    .text_color(p.mute)
                    .line_height(relative(1.5))
                    .child(body),
            )
            // The open picker makes the dock taller; the ideas would run
            // under it, and while picking they are not what is wanted.
            .when(has_text && self.picker.is_none(), |d| {
                d.child(
                    div()
                        .mt(px(22.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(10.))
                        .child(subheading(s.ask_suggest_title, p))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .justify_center()
                                .gap(px(8.))
                                .children(
                                    ideas
                                        .into_iter()
                                        .enumerate()
                                        .map(|(ix, text)| suggestion(ix, text, p, cx)),
                                ),
                        ),
                )
            })
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
                .gap(px(10.))
                .h(px(22.))
                .text_color(p.mute)
                .role(Role::Status)
                .child(thinking_dots(p))
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
                    .items_start()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(12.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(p.line)
                    .bg(p.card)
                    .child(
                        div().pt(px(2.)).child(
                            Icon::new(IconName::Clock)
                                .size(px(15.))
                                .text_color(p.accent),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(div().font_weight(FontWeight::MEDIUM).child(title))
                            .child(div().text_color(p.mute).child(s.ask_limit_body)),
                    )
                    .into_any_element()
            }
        };
        // The avatar hangs in the gutter; the name sits level with it and the
        // answer starts under the name, so long answers keep one left edge.
        div()
            .flex()
            .items_start()
            .gap(px(12.))
            .child(magi_avatar(p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Magi"),
                    )
                    .child(body),
            )
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
            let file_id = source.file_id;
            div()
                .id(("ask-cite", turn_ix * 16 + ix))
                .debug_selector(move || format!("ask-cite-{turn_ix}-{ix}"))
                .cursor_pointer()
                .role(Role::Button)
                .aria_label(source.file_name.clone())
                .tooltip(source_tip(source))
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
            .px(px(15.))
            .py(px(12.))
            .rounded(px(18.))
            .rounded_tl(px(5.))
            .border_1()
            .border_color(p.line)
            .bg(p.card)
            .when(!p.dark, |d| d.shadow_sm())
            .child(
                div()
                    .text_size(px(14.5))
                    .line_height(relative(1.65))
                    // Selectable and copyable, like any chat's answer.
                    .child(SelectableText::new(
                        ("ask-answer", turn_ix),
                        answer.text.clone(),
                    )),
            )
            .when(!answer.cited.is_empty(), |d| {
                d.child(
                    div()
                        .mt(px(14.))
                        .pt(px(12.))
                        .border_t_1()
                        .border_color(p.line)
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(subheading(s.ask_sources, p))
                        .child(div().flex().flex_wrap().gap(px(6.)).children(chips)),
                )
            })
            .when(gone, |d| d.child(warn_note(p, s.ask_file_gone).mt(px(8.))))
            .child(note(p).mt(px(10.)).child(ai_note(answer, s)))
    }
}
