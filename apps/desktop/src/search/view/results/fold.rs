//! The line between the results that match and the weaker ones behind it.

use gpui_kit::component::{Icon, IconName};
use gpui_kit::*;

use crate::theme::Palette;

/// A hairline section break the whole width of the list: a turned chevron
/// and the count, so it reads as where the good results end, not as one more
/// row. Stays put open or closed, with only the chevron changing.
pub(super) fn divider(open: bool, label: String, p: &Palette) -> Stateful<Div> {
    let pressed = Hsla {
        a: p.selection.a * 2.,
        ..p.selection
    };
    let quarter = std::f32::consts::FRAC_PI_2;
    div()
        .id("weaker-matches")
        .role(Role::Button)
        .aria_label(label.clone())
        .cursor_pointer()
        .flex()
        .items_center()
        .gap_2()
        .mt(px(6.))
        .pl(px(13.))
        .pr(px(10.))
        .py(px(6.))
        .rounded(px(4.))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.mute)
        .hover(|d| d.bg(p.selection))
        .active(move |d| d.bg(pressed))
        .child(
            Icon::new(IconName::ChevronRight)
                .size(px(14.))
                .flex_none()
                .rotate(radians(if open { quarter } else { 0. })),
        )
        .child(div().flex_none().child(label))
        .child(div().flex_1().h(px(1.)).bg(p.line))
}
