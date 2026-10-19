//! The search window's footer: settings, the index status, the keys.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{CONTEXT, COPY_FLASH, COPY_KEY, OpenSettings, SearchView};
use crate::search::state::{footer_status, hidden_reason};
use crate::theme::{Palette, Tone};
use crate::tray::{status_line, status_tone};

impl SearchView {
    /// Always there: the settings gear, the index status (as in the tray)
    /// and the keys that apply now.
    pub(super) fn footer(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        // A selected result whose match the row does not show says why, for
        // the keyboard as the tooltip does for the mouse. The index status
        // gives way only when the results are complete (idle, with or
        // without unreadable files); "results may be incomplete" (scanning,
        // indexing, paused) is never hidden.
        let why = self
            .state
            .selected()
            .and_then(|r| hidden_reason(&r.match_sources, s));
        let status = live.status.as_ref().map(footer_status).map(|st| {
            let st = &st;
            let line = div().flex().items_center().gap_2().min_w_0();
            match why.filter(|_| matches!(status_tone(st), Tone::Ok | Tone::Err)) {
                Some(why) => line.child(div().truncate().child(why)),
                None => line
                    .child(p.dot(status_tone(st)))
                    .child(div().truncate().child(status_line(st, lang))),
            }
        });
        // Always shown, so they are learned in the empty window; dimmed while
        // there is no result to act on.
        let idle = self.state.results().is_empty();
        let pairs: &[(&str, &'static str)] = &[
            ("enter", s.open),
            ("secondary-enter", s.reveal),
            (
                COPY_KEY,
                if self.copied.is_some() {
                    s.copied
                } else {
                    s.copy_path
                },
            ),
        ];
        div()
            .flex()
            .items_center()
            .gap_3()
            .pl(px(8.))
            .pr(px(16.))
            .py(px(6.))
            .text_xs()
            .text_color(p.mute)
            .border_t_1()
            .border_color(p.line)
            .child(
                Button::new("settings")
                    .cursor_pointer()
                    .ghost()
                    .xsmall()
                    .icon(IconName::Settings)
                    .tooltip_with_action(s.settings, &OpenSettings, Some(CONTEXT))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(&OpenSettings, window, cx)
                    })),
            )
            .child(div().flex_1().min_w_0().children(status))
            .child(
                keys(pairs, self.copied.is_some().then_some((self.copies, p)))
                    .when(idle, |keys| keys.opacity(0.7))
                    .child(esc_key(p)),
            )
    }
}

/// Escape closes the window; a plain `esc` keycap says so without a label.
fn esc_key(p: &Palette) -> Div {
    div()
        .px(px(5.))
        .rounded(px(3.))
        .border_1()
        .border_color(p.line)
        .child("esc")
}

/// Key hints; `keys` are GPUI keystrokes (`secondary` is Ctrl, or Cmd on
/// macOS), shown the platform's way.
///
/// `copied` is `Some((copies, palette))` while "Path copied" shows: that
/// label starts in the accent color and settles to the others' with the
/// row flash; a new copy replays it.
fn keys(pairs: &[(&str, &'static str)], copied: Option<(u64, &Palette)>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .children(pairs.iter().flat_map(|&(key, label)| {
            let kbd = Keystroke::parse(key)
                .ok()
                .map(|k| Kbd::new(k).into_any_element());
            let label = (!label.is_empty()).then(|| {
                let label = div().mr(px(6.)).child(label);
                match copied {
                    Some((n, p)) if key == COPY_KEY => {
                        let (accent, mute) = (p.accent, p.mute);
                        label
                            .with_animation(
                                ("copied-label", n),
                                Animation::new(COPY_FLASH).with_easing(ease_out_quint()),
                                move |label, t| {
                                    label.text_color(mute.blend(accent.opacity(1. - t)))
                                },
                            )
                            .into_any_element()
                    }
                    _ => label.into_any_element(),
                }
            });
            kbd.into_iter().chain(label)
        }))
}
