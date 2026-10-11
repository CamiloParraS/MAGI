//! Small building blocks every window draws with.

pub mod folders;
pub mod keycaps;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::features::Feature;

use magi_core::dto::{IndexState, IndexStatus};

use crate::i18n::Lang;
use crate::theme::Palette;
use crate::tray::status_line;

/// How far a pressed control moves down. GPUI scales only SVGs, so a press is
/// a push, with the button's own darker background.
pub(crate) const PRESS: Pixels = px(1.);

/// `el` pushed [`PRESS`] down while it is held. For controls that take their
/// own size (chips, small buttons); a full-width row pushes its content with
/// `group_active` instead, so its background stays put.
pub(crate) fn pressable(el: impl IntoElement, id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).relative().active(|s| s.top(PRESS)).child(el)
}

/// The space between groups.
const STACK_GAP: Pixels = px(10.);

pub(crate) fn stack() -> Div {
    div().flex().flex_col().gap(STACK_GAP)
}

/// A settings box, as in the variant A mockup. Fields inside it take
/// `p.solid` so they stand out from the box. In light mode a white box on a
/// near-white window needs a faint shadow to read as a layer.
pub(crate) fn card(p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(14.))
        .px_4()
        .py_3()
        .rounded(px(10.))
        .border_1()
        .border_color(p.line)
        .bg(p.card)
        .when(!p.dark, |d| d.shadow_xs())
}

/// A setting's second line.
pub(crate) fn note(p: &Palette) -> Div {
    div().mt(px(2.)).text_xs().text_color(p.mute)
}

pub(crate) fn heading(text: &'static str) -> Div {
    div()
        .mb(px(18.))
        .text_size(px(26.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// A group's name inside a section, small caps as in the mockup.
pub(crate) fn subheading(text: &'static str, p: &Palette) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.mute)
        .child(text)
}

/// Rows grouped in one rounded box, as in system settings.
pub(crate) fn group(p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .border_1()
        .border_color(p.line)
        .bg(p.card)
        .when(!p.dark, |d| d.shadow_xs())
        .overflow_hidden()
}

/// `rows` in one [`group`], a [`divider`] between each.
pub(crate) fn grouped(p: &Palette, rows: impl IntoIterator<Item = impl IntoElement>) -> Div {
    group(p).children(
        rows.into_iter()
            .enumerate()
            .map(|(ix, row)| div().children((ix > 0).then(|| divider(p))).child(row)),
    )
}

/// A line between grouped rows, starting where the text does.
pub(crate) fn divider(p: &Palette) -> Div {
    div().h(px(1.)).ml(px(42.)).bg(p.line)
}

/// A grouped row: its icon, then its text, then its controls.
pub(crate) fn row() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(6.))
        .min_h(px(44.))
}

/// What the engine is doing when it is not simply idle or indexing: walking
/// the folders ("N files found") or paused (and why). One grouped row.
pub(crate) fn status_row(status: &IndexStatus, lang: Lang, p: &Palette) -> Div {
    let icon = if matches!(status.state, IndexState::Scanning | IndexState::Indexing) {
        IconName::RefreshCw
    } else {
        IconName::Clock
    };
    grouped(
        p,
        [row()
            .child(row_icon(icon, p))
            .text_color(p.mute)
            .child(status_line(status, lang))],
    )
}

/// [`status_row`] while the folders are being walked or their files read, for
/// the list where a folder was just added: a big one takes a while before any
/// file shows up, and silence there reads as an app that does nothing.
pub(crate) fn scan_row(status: Option<&IndexStatus>, lang: Lang, p: &Palette) -> Option<Div> {
    status
        .filter(|st| matches!(st.state, IndexState::Scanning | IndexState::Indexing))
        .map(|st| status_row(st, lang, p))
}

/// A grouped row's icon, in the accent.
pub(crate) fn row_icon(icon: IconName, p: &Palette) -> Icon {
    Icon::new(icon)
        .size(px(18.))
        .flex_none()
        .text_color(p.accent)
}

/// What each feature does, as a picture.
pub(crate) fn feature_icon(feature: Feature) -> IconName {
    match feature {
        Feature::Meaning => IconName::TextSearch,
        Feature::ImageText => IconName::ScanText,
        Feature::ImageVisual => IconName::ScanEye,
    }
}
