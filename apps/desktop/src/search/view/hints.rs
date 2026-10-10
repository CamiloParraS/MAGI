//! The search window's hints: the shortcut, and a feature to turn on.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{IconName, Sizable as _};
use gpui_kit::*;
use magi_core::features::Feature;

use super::SearchView;
use crate::search::hint::{Hint, teach_shortcut};
use crate::settings::{hotkey_keystroke, turn_on};
use crate::theme::{self, Palette};

impl SearchView {
    /// The hint reports the progress through [`Live::features`].
    fn turn_on(&mut self, feature: Feature, cx: &mut Context<Self>) {
        let host = self.host.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = turn_on(&host, feature) {
                    tracing::warn!(%error, ?feature, "could not turn the feature on");
                }
            })
            .detach();
    }

    fn dismiss(&mut self, feature: Feature, cx: &mut Context<Self>) {
        self.live.update(cx, |live, cx| {
            live.dismissed.push(feature);
            cx.notify();
        });
    }

    /// The shortcut, under the input, so next time search is one keypress
    /// away (see [`teach_shortcut`]).
    pub(super) fn shortcut_hint(
        &self,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let show = teach_shortcut(
            self.teach.is_none(),
            live.shortcut_dismissed,
            live.hotkey_conflict.is_some(),
        );
        let keystroke = self
            .teach
            .as_ref()
            .filter(|_| show)
            .and_then(|hotkey| Keystroke::parse(&hotkey_keystroke(hotkey)).ok())?;
        let hint = div()
            .flex()
            .items_center()
            .gap_2()
            .mx(px(12.))
            .mb(px(10.))
            .pl(px(12.))
            .pr(px(6.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(p.selection)
            .text_sm()
            .child(s.teach_shortcut)
            .child(Kbd::new(keystroke))
            .child(div().flex_1())
            .child(
                Button::new("dismiss-shortcut")
                    .cursor_pointer()
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(s.dismiss)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.live.update(cx, |live, cx| {
                            live.shortcut_dismissed = true;
                            cx.notify();
                        });
                    })),
            );
        Some(theme::fade_in(hint, "shortcut-hint"))
    }

    pub(super) fn hint(
        &self,
        hint: Hint,
        p: &Palette,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let feature = hint.feature();
        let text = s.feature(feature);
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let (message, name_at, action) = match hint {
            Hint::Offer { size, .. } => {
                let off = s.feature_off.replace("{name}", text.name);
                let name_at = s.feature_off.find("{name}");
                let button = Button::new("turn-on")
                    .cursor_pointer()
                    .primary()
                    .small()
                    .label(s.turn_on.replace("{size}", &lang.size(size)))
                    .on_click(cx.listener(move |this, _, _, cx| this.turn_on(feature, cx)));
                (format!("{off} {}", text.pitch), name_at, Some(button))
            }
            Hint::Downloading { done, total, .. } => (
                s.downloading
                    .replace("{name}", text.name)
                    .replace("{done}", &lang.size(done))
                    .replace("{total}", &lang.size(total)),
                None,
                None,
            ),
            Hint::Updating { left, .. } => (
                lang.plural(&s.updating, left).replace("{name}", text.name),
                None,
                None,
            ),
        };
        let highlights = name_at
            .map(|at| vec![(at..at + text.name.len(), bold)])
            .unwrap_or_default();
        // A new stage (offer, downloading, updating) fades in again.
        let stage = match hint {
            Hint::Offer { .. } => 0u64,
            Hint::Downloading { .. } => 1,
            Hint::Updating { .. } => 2,
        };
        let hint = div()
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .px(px(12.))
            .py(px(10.))
            .rounded(px(6.))
            .bg(p.selection)
            .text_color(p.ink)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(StyledText::new(message).with_highlights(highlights)),
            )
            .children(action)
            .child(
                Button::new("dismiss")
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .tooltip(s.dismiss)
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss(feature, cx))),
            );
        theme::fade_in(hint, ("hint", stage))
    }
}
