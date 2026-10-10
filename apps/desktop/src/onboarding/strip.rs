//! The setup screen's strip: the folders, through the features turned on.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{FOLDERS_3, OnboardingView};
use crate::i18n::Strings;
use crate::theme::Palette;
use crate::ui::feature_icon;

/// The setup strip's folder icon: how many folders it stacks (none to
/// three), and whether a plus says there are more.
pub fn folder_glyphs(n: usize) -> (usize, bool) {
    (n.min(3), n > 3)
}

impl OnboardingView {
    /// A faint picture of what Start sets up: the folders, through the
    /// features turned on. It only moves when one of them changes.
    pub(super) fn strip(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let chosen = self.chosen(cx);
        let n = self.roots(cx).map_or(0, |roots| roots.len());
        let (stacked, more) = folder_glyphs(n);
        let lit = p.ink.opacity(STRIP_LIT);
        let folders =
            if n == 0 {
                // Where the first folder will go.
                div()
                    .size(px(18.))
                    .rounded(px(4.))
                    .border_1()
                    .border_dashed()
                    .border_color(lit)
                    .into_any_element()
            } else {
                let icon = match stacked {
                    1 => Icon::new(IconName::Folder),
                    2 => Icon::new(IconName::Folders),
                    _ => Icon::empty().path(FOLDERS_3),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .relative()
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            // Keyed by the stack, so only a new icon grows in
                            // as it fades in; a fourth folder only adds the plus.
                            .child(icon.text_color(lit).with_animation(
                                ("strip-folders", stacked),
                                Animation::new(STRIP_IN).with_easing(ease_out_quint()),
                                |icon, t| icon.size(px(20. * (0.9 + 0.1 * t))).opacity(t),
                            ))
                            .when(more, |d| {
                                d.child(
                                    div().absolute().top(px(-4.)).right(px(-6.)).child(
                                        Icon::new(IconName::Plus).size(px(10.)).text_color(lit),
                                    ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.mute.opacity(0.5))
                            .child(live.lang.plural(&s.ob_folder_count, n as u64)),
                    )
                    .into_any_element()
            };
        let features = live.features.iter().map(|f| {
            let on = chosen.contains(&f.feature);
            let (from, to) = if on {
                (STRIP_OFF, STRIP_LIT)
            } else {
                (STRIP_LIT, STRIP_OFF)
            };
            let ink = p.ink;
            // Keyed by the state, so a toggle replays it from the other end.
            Icon::new(feature_icon(f.feature))
                .size(px(18.))
                .with_animation(
                    SharedString::from(format!("strip-feature-{}-{on}", f.feature as usize)),
                    Animation::new(STRIP_TOGGLE).with_easing(ease_out_quint()),
                    move |icon, t| icon.text_color(ink.opacity(from + (to - from) * t)),
                )
        });
        div()
            .mt(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .child(folders)
            .child(
                div()
                    .flex_1()
                    .max_w(px(96.))
                    .border_t_1()
                    .border_dashed()
                    .border_color(lit),
            )
            .child(div().flex().gap(px(10.)).children(features))
    }
}

/// The setup strip: a new folder arriving, a feature lighting up.
const STRIP_IN: Duration = Duration::from_millis(200);
const STRIP_TOGGLE: Duration = Duration::from_millis(150);
/// The strip's ink: barely there, and a feature turned off fainter still.
const STRIP_LIT: f32 = 0.12;
const STRIP_OFF: f32 = 0.04;

#[cfg(test)]
mod tests {
    use super::folder_glyphs;

    #[test]
    fn the_strip_stacks_up_to_three_folders_then_adds_a_plus() {
        assert_eq!(folder_glyphs(0), (0, false));
        assert_eq!(folder_glyphs(1), (1, false));
        assert_eq!(folder_glyphs(3), (3, false));
        assert_eq!(folder_glyphs(12), (3, true));
    }
}
