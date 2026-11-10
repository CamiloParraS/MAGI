//! What the Ask window draws: the sign-in scene, the conversation (your
//! questions on the right, Magi's answers on the left with the files they
//! came from), and the dock with the chips and the question box.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{AskView, COLUMN};
use crate::ask::{Answer, Source, Turn, TurnState, ai_note, blocked_text, problem_text};
use crate::i18n::{Lang, Strings};
use crate::search::highlight::one_line;
use crate::theme::{self, Palette};
use crate::ui::{MARK, brand_tile, note, pressable, subheading, warn_note};

/// The glyph on a chip, and the room a chip's file name gets.
const CHIP_GLYPH: Pixels = px(14.);
const CHIP_THUMB: Pixels = px(20.);
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

/// A chip's icon: the file's thumbnail when it has one, else its glyph.
fn chip_icon(source: &Source) -> AnyElement {
    let (kind, name) = (source.kind, source.file_name.clone());
    match &source.thumb {
        Some(path) => img(path.clone())
            .flex_none()
            .size(CHIP_THUMB)
            .rounded(px(4.))
            .object_fit(ObjectFit::Cover)
            .with_fallback(move || theme::file_glyph(kind, &name, CHIP_GLYPH).into_any_element())
            .into_any_element(),
        None => theme::file_glyph(kind, &source.file_name, CHIP_GLYPH)
            .flex_none()
            .into_any_element(),
    }
}

/// The tooltip of a chip: the excerpt, under a larger look at the file when
/// it has a thumbnail.
fn source_tip(source: &Source) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let (text, thumb) = (excerpt_tip(source), source.thumb.clone());
    move |window, cx| match thumb.clone() {
        None => Tooltip::new(text.clone()).build(window, cx),
        Some(path) => {
            let text = text.clone();
            Tooltip::element(move |_, _| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .w(px(240.))
                    .child(
                        img(path.clone())
                            .w_full()
                            .h(px(180.))
                            .rounded(px(4.))
                            .object_fit(ObjectFit::Contain),
                    )
                    .child(div().text_xs().child(text.clone()))
            })
            .build(window, cx)
        }
    }
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
        .child(chip_icon(source))
        .child(
            div()
                .max_w(CHIP_NAME)
                .truncate()
                .child(source.file_name.clone()),
        )
}

/// The text color on an accent fill: black on the dark theme's light blue,
/// white on the light theme's deep one.
fn on_accent(p: &Palette) -> Hsla {
    if p.dark { black() } else { white() }
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

/// Your question, on the right in the accent: the one filled shape in the
/// chat, so who said what reads at a glance.
fn question_bubble(text: &str, p: &Palette) -> Div {
    div().flex().justify_end().child(
        div()
            .max_w(relative(0.8))
            .px(px(15.))
            .py(px(10.))
            .rounded(px(18.))
            .rounded_br(px(5.))
            .bg(p.accent)
            .text_color(on_accent(p))
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
        let (title, body) = match self.sign_in.read(cx).creating() {
            true => (s.ask_create_title, s.ask_create_body),
            false => (s.ask_sign_in_title, s.ask_sign_in_body),
        };
        let trust = [
            (IconName::HardDrive, s.ask_trust_local),
            (IconName::FileCheck, s.ask_trust_choose),
            (IconName::ShieldCheck, s.ask_trust_nokeep),
        ];
        div()
            .id("ask-sign-in")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(24.))
            .py(px(28.))
            .child(
                div()
                    .w_full()
                    .max_w(px(380.))
                    .mx_auto()
                    .my_auto()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(brand_tile(p, px(40.)).mb(px(14.)))
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_center()
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .max_w(px(340.))
                            .text_center()
                            .text_color(p.mute)
                            .line_height(relative(1.5))
                            .child(body),
                    )
                    .when(expired, |d| {
                        d.child(warn_note(p, s.ask_session_ended).mt(px(10.)))
                    })
                    .child(
                        div()
                            .w_full()
                            .mt(px(22.))
                            .p(px(16.))
                            .rounded(px(16.))
                            .border_1()
                            .border_color(p.line)
                            .bg(p.card)
                            .when(!p.dark, |d| d.shadow_sm())
                            .child(self.sign_in.clone()),
                    )
                    .child(
                        div()
                            .mt(px(20.))
                            .flex()
                            .flex_col()
                            .gap(px(7.))
                            .children(trust.map(|(icon, text)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_xs()
                                    .text_color(p.mute)
                                    .child(Icon::new(icon).size(px(13.)).text_color(p.accent))
                                    .child(text)
                            })),
                    ),
            )
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
            .px(px(24.))
            .pt(px(24.))
            .pb(px(12.))
            .child(content)
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
            .when(has_text, |d| {
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
                    .child(answer.text.clone()),
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
            let on = source.ticked;
            div()
                .id(("ask-source", ix))
                .debug_selector(move || format!("ask-source-{ix}"))
                .role(Role::CheckBox)
                .aria_label(source.file_name.clone())
                .aria_toggled(if on { Toggled::True } else { Toggled::False })
                .when(!busy, |d| d.cursor_pointer())
                .tooltip(source_tip(source))
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
                        .when(!sources.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div().px(px(2.)).child(subheading(s.ask_what_gets_sent, p)),
                                    )
                                    .child(div().flex().flex_wrap().gap(px(6.)).children(chips)),
                            )
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
