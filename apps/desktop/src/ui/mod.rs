//! Small building blocks every window draws with.

pub mod folders;
pub mod keycaps;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::features::Feature;

use magi_core::dto::{IndexState, IndexStatus};

use std::f32::consts::TAU;
use std::time::Duration;

use crate::i18n::{Lang, Strings};
use crate::live::Live;
use crate::theme::{self, Palette};
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

/// A problem line: the alert icon and the text in the warning color, so it
/// reads without the color.
pub(crate) fn warn_note(p: &Palette, text: impl IntoElement) -> Div {
    note(p)
        .flex()
        .items_start()
        .gap(px(4.))
        .text_color(p.warn)
        .child(
            div()
                .flex_none()
                .pt(px(1.))
                .child(Icon::new(IconName::TriangleAlert).size(px(12.))),
        )
        .child(div().min_w_0().child(text))
}

/// "Saved", with a check, in a note's place.
pub(crate) fn saved_note(s: &Strings, p: &Palette) -> Div {
    note(p)
        .flex()
        .items_center()
        .gap(px(4.))
        .child(Icon::new(IconName::Check).size(px(12.)).text_color(p.mute))
        .child(s.saved)
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
pub(crate) fn status_row(status: &IndexStatus, lang: Lang, p: &Palette) -> impl IntoElement {
    status_content(status, lang, p, true)
        // Settles in as the advanced patterns do, instead of popping in.
        .relative()
        .with_animation(
            "status-in",
            Animation::new(theme::FADE).with_easing(ease_out_quint()),
            |el, t| el.opacity(t).top(px(-6. * (1. - t))),
        )
}

/// How long the busy icon turns, and how many turns, when the row appears or
/// the work changes (walking to reading): a flick that winds down says "on
/// it" without a spinner to stare at through an hour of indexing.
const SPIN: Duration = Duration::from_millis(2400);
const SPIN_TURNS: f32 = 2.;

/// How long a scan row takes to leave once the work is done.
pub(crate) const SCAN_EXIT: Duration = Duration::from_millis(200);

/// Walking the folders or reading their files.
pub(crate) fn busy(status: &IndexStatus) -> bool {
    matches!(status.state, IndexState::Scanning | IndexState::Indexing)
}

fn status_content(status: &IndexStatus, lang: Lang, p: &Palette, spin: bool) -> Div {
    let icon = if busy(status) {
        let icon = row_icon(IconName::RefreshCw, p);
        if spin {
            icon.with_animation(
                ("status-spin", status.state as u64),
                Animation::new(SPIN).with_easing(ease_out_quint()),
                |icon, t| icon.rotate(radians(TAU * SPIN_TURNS * t)),
            )
            .into_any_element()
        } else {
            icon.into_any_element()
        }
    } else {
        row_icon(IconName::Clock, p).into_any_element()
    };
    grouped(
        p,
        [row()
            .child(icon)
            .text_color(p.mute)
            .child(status_line(status, lang))],
    )
}

/// [`status_row`] while the folders are being walked or their files read, for
/// the list where a folder was just added: a big one takes a while before any
/// file shows up, and silence there reads as an app that does nothing. Once
/// the work is done it fades and folds away over [`SCAN_EXIT`], faster than it
/// came.
pub(crate) fn scan_row(live: &Live, p: &Palette) -> Option<AnyElement> {
    if let Some(status) = live.status.as_ref().filter(|st| busy(st)) {
        return Some(status_row(status, live.lang, p).into_any_element());
    }
    live.scan_left.as_ref().map(|status| {
        status_content(status, live.lang, p, false)
            .overflow_hidden()
            .with_animation(
                "status-out",
                Animation::new(SCAN_EXIT).with_easing(ease_out_quint()),
                // ponytail: folds from a fixed height a little over one row;
                // measure the row if its text ever wraps.
                |el, t| el.opacity(1. - t).max_h(px(48. * (1. - t))),
            )
            .into_any_element()
    })
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
