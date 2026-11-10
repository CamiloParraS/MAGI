//! The chip: a file's glyph or thumbnail and name, which the dock shows for
//! what goes with the next question and an answer shows for what it cited.

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::discovery::Kind;
use std::path::PathBuf;

use crate::ask::Source;
use crate::search::highlight::one_line;
use crate::theme::{self, Palette};

/// The glyph on a chip, and the room a chip's file name gets.
pub(super) const CHIP_GLYPH: Pixels = px(14.);
pub(super) const CHIP_THUMB: Pixels = px(20.);
pub(super) const CHIP_NAME: Pixels = px(170.);
/// What a chip's tooltip quotes of its excerpt.
pub(super) const TIP_LEN: usize = 280;

/// The excerpt as a tooltip: a file's name says little about what goes.
pub(super) fn excerpt_tip(source: &Source) -> SharedString {
    let text = one_line(&source.text);
    let mut tip: String = text.chars().take(TIP_LEN).collect();
    if text.chars().count() > TIP_LEN {
        tip.push('…');
    }
    tip.into()
}

/// A file's icon: its thumbnail when it has one, else its glyph.
pub(super) fn file_icon(kind: Kind, file_name: &str, thumb: Option<&PathBuf>) -> AnyElement {
    let name = file_name.to_owned();
    match thumb {
        Some(path) => img(path.clone())
            .flex_none()
            .size(CHIP_THUMB)
            .rounded(px(4.))
            .object_fit(ObjectFit::Cover)
            .with_fallback(move || theme::file_glyph(kind, &name, CHIP_GLYPH).into_any_element())
            .into_any_element(),
        None => theme::file_glyph(kind, file_name, CHIP_GLYPH)
            .flex_none()
            .into_any_element(),
    }
}

/// A chip's icon.
pub(super) fn chip_icon(source: &Source) -> AnyElement {
    file_icon(source.kind, &source.file_name, source.thumb.as_ref())
}

/// The tooltip of a chip: the excerpt, under a larger look at the file when
/// it has a thumbnail.
pub(super) fn source_tip(source: &Source) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    preview_tip(excerpt_tip(source), source.thumb.clone())
}

/// A file's preview on hover: its picture, large, over the start of its text.
pub(super) fn preview_tip(
    text: SharedString,
    thumb: Option<PathBuf>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
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
                    .when(!text.is_empty(), |d| {
                        d.child(div().text_xs().child(text.clone()))
                    })
            })
            .build(window, cx)
        }
    }
}

/// The pill every chip is drawn in.
pub(super) fn chip_shell() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(26.))
        .rounded_full()
        .border_1()
        .text_size(px(12.))
}

/// A one-line tooltip.
pub(super) fn text_tip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

/// A chip's face: its number when it has one, the file's glyph and name.
pub(super) fn chip_face(number: Option<usize>, source: &Source, p: &Palette) -> Div {
    chip_shell()
        .pl(px(if number.is_some() { 4. } else { 8. }))
        .pr(px(10.))
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
