//! The detail pane beside the results (`ui.detail_pane`): the selected
//! result's preview, name, folder, details, why it matched, and its actions.
//! The wording is in [`detail`](crate::search::detail).

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::*;
use magi_core::dto::SearchResult;

use super::SearchView;
use super::results::{folder_of, local_date, short_folder};
use crate::search::{detail, highlight};
use crate::theme::{self, Palette};

/// The thumbnail box's least height; it grows into the room the text
/// leaves, and the thumbnail is fitted inside, never cropped.
const PREVIEW_HEIGHT: Pixels = px(144.);
/// The file's glyph beside its name, when it has no thumbnail.
const GLYPH: Pixels = px(40.);

impl SearchView {
    pub(super) fn pane(&self, r: &SearchResult, p: &Palette, cx: &Context<Self>) -> Stateful<Div> {
        let lang = self.live.read(cx).lang;
        let s = lang.strings();
        let today = chrono::Local::now().date_naive();
        let details = self
            .details
            .filter(|(id, _)| *id == r.file_id)
            .and_then(|(_, d)| d);
        let line = detail::details_line(
            &r.file_name,
            local_date(r.modified_at),
            details,
            lang,
            today,
        );
        let full: SharedString = folder_of(&r.path).into();
        let title = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .min_w_0()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_clamp(2)
                    .child(r.file_name.clone()),
            )
            // Where it is; a click shows it there, like Ctrl+Enter.
            .child(
                div()
                    .id("pane-folder")
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(p.mute)
                    .hover(|d| d.text_color(p.ink))
                    .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.act(true, window, cx)))
                    .child(Icon::new(IconName::Folder).size(px(12.)).flex_none())
                    .child(div().truncate().child(short_folder(&r.path))),
            )
            .child(div().text_xs().text_color(p.mute).child(line));
        // A thumbnail earns the space above; a file without one keeps its
        // glyph small, beside the name.
        let head = match &r.thumb_path {
            // The picture takes the height the text leaves.
            Some(path) => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .gap_3()
                .child(preview(PathBuf::from(path), r, p))
                .child(title),
            None => div()
                .flex()
                .items_start()
                .gap_3()
                .child(theme::file_glyph(r.kind, &r.file_name, GLYPH).flex_none())
                .child(title),
        };
        let page = r.page.map(|n| {
            chip(
                p,
                IconName::File,
                s.page.replace("{n}", &lang.number(n.max(0) as u64)).into(),
            )
            .into_any_element()
        });
        let chips = detail::reasons(&r.match_sources, s)
            .into_iter()
            .enumerate()
            .map(|(ix, (label, tip))| {
                chip(p, IconName::Check, label.into())
                    .id(("reason", ix))
                    .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                    .into_any_element()
            });
        // Whole lines that fit under a thumbnail, more without one: a clamp
        // the body would cut mid-line otherwise.
        let lines = if r.thumb_path.is_some() { 4 } else { 10 };
        let snippet = r
            .snippet
            .as_ref()
            .filter(|snippet| !detail::snippet_is_the_path(&snippet.text, &r.path))
            .map(|snippet| {
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .bg(p.card)
                    .text_size(px(12.5))
                    .line_height(relative(1.5))
                    .text_color(p.mute)
                    .line_clamp(lines)
                    .child(
                        StyledText::new(highlight::one_line(&snippet.text)).with_highlights(
                            highlight::runs(
                                &snippet.text,
                                &snippet.highlights,
                                HighlightStyle {
                                    background_color: Some(p.mark),
                                    color: Some(p.ink),
                                    ..Default::default()
                                },
                            ),
                        ),
                    )
            });
        div()
            .id("detail-pane")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .border_l_1()
            .border_color(p.line)
            .child(
                div()
                    .id("detail-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .p(px(16.))
                    .overflow_y_scroll()
                    .child(head)
                    .children(snippet),
            )
            // Always in view, whatever the body holds: the actions, and why
            // it matched at the other end.
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px(px(16.))
                    .pt(px(4.))
                    .pb(px(12.))
                    .child(
                        Button::new("pane-open")
                            .cursor_pointer()
                            .primary()
                            .small()
                            .label(s.open)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.act(false, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("pane-reveal")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(s.reveal)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.act(true, window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .flex_wrap()
                            .justify_end()
                            .gap(px(6.))
                            .min_w_0()
                            .children(chips)
                            .children(page),
                    ),
            )
    }
}

/// A small label for why it matched or where: an icon and a word or two.
fn chip(p: &Palette, icon: IconName, label: SharedString) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .bg(p.chip)
        .text_xs()
        .text_color(p.ink)
        .child(Icon::new(icon).size(px(12.)).text_color(p.accent))
        .child(label)
}

/// The thumbnail fitted in its box. The thumbnail is a PDF's first page,
/// so the matched page is a chip below, not a badge on it.
fn preview(path: PathBuf, r: &SearchResult, p: &Palette) -> Div {
    let (kind, name) = (r.kind, r.file_name.clone());
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w_full()
        .flex_1()
        .min_h(PREVIEW_HEIGHT)
        .rounded(px(6.))
        .bg(p.card)
        .overflow_hidden()
        .child(
            img(path)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .with_fallback(move || theme::file_glyph(kind, &name, GLYPH).into_any_element()),
        )
}
