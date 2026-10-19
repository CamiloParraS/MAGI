//! The search window's body: the intro, no match, or the results.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::NaiveDate;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::dto::{IndexState, SearchResult};

use super::{COPY_FLASH, LIST_MAX_HEIGHT, SearchView};
use crate::i18n::Lang;
use crate::search::highlight;
use crate::search::hint::hint;
use crate::search::state::{cause, explain_sources};
use crate::theme::{self, Palette};

impl SearchView {
    pub(super) fn body(&self, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let state = &self.state;
        let block = || {
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap_3()
                .px(px(18.))
                .pt(px(20.))
                .pb(px(22.))
                .border_t_1()
                .border_color(p.line)
                .text_color(p.mute)
        };
        if state.error().is_some() {
            // The first failure retries by itself and only says so; the
            // second leaves ways out, the input still taking new text.
            if state.failures() < 2 {
                // A pulsing dot: the wait is the engine working, not a hang.
                let dot = p.dot(theme::Tone::Busy).with_animation(
                    "retry-dot",
                    Animation::new(Duration::from_millis(900))
                        .repeat()
                        .with_easing(pulsating_between(0.3, 1.)),
                    |dot, t| dot.opacity(t),
                );
                return Some(
                    block()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(dot)
                                .child(s.search_failed),
                        )
                        .into_any_element(),
                );
            }
            let actions = if self.confirm_rebuild {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("rebuild-confirm")
                            .cursor_pointer()
                            .danger()
                            .small()
                            .label(s.rebuild_index)
                            .on_click(cx.listener(|this, _, _, cx| this.rebuild_index(cx))),
                    )
                    .child(
                        Button::new("rebuild-cancel")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            .label(s.cancel)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_rebuild = false;
                                cx.notify();
                            })),
                    )
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("retry")
                            .cursor_pointer()
                            .label(s.try_again)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                    .child(
                        Button::new("rebuild")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            .label(s.rebuild_index)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_rebuild = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("open-logs")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            .label(s.open_logs)
                            .on_click(|_, _, cx| cx.reveal_path(&crate::logging::path())),
                    )
            };
            let message = if self.confirm_rebuild {
                s.rebuild_index_confirm
            } else {
                s.search_still_failing
            };
            let why = state
                .error()
                .and_then(|e| cause(e, s))
                .filter(|_| !self.confirm_rebuild);
            return Some(
                block()
                    .child(message)
                    .children(why.map(|why| crate::ui::note(p).mt_0().child(why)))
                    .child(actions)
                    .into_any_element(),
            );
        }
        if state.query().is_empty() {
            let intro = live
                .status
                .as_ref()
                .map(|status| lang.plural(&s.intro, status.indexed));
            let indexing = live
                .status
                .as_ref()
                .filter(|st| {
                    matches!(st.state, IndexState::Indexing | IndexState::Scanning) && st.queued > 0
                })
                .map(|st| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(p.dot(theme::Tone::Busy))
                        .child(lang.plural(&s.still_indexing, st.queued))
                });
            return Some(
                block()
                    .children(intro)
                    .child(
                        Button::new("example")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            // Its label lines up with the intro's text, not
                            // with the button's own padding.
                            .ml(px(-8.))
                            .label(s.try_example.replace("{q}", s.example_query))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.try_example(window, cx)),
                            ),
                    )
                    .children(indexing)
                    .into_any_element(),
            );
        }
        let results = state.results();
        // Kept while typing, so its fade does not replay on every keystroke.
        let hint = (!self.dimmed())
            .then(|| hint(results.len(), &live.features, &live.dismissed))
            .flatten()
            .map(|h| self.hint(h, p, cx));
        if results.is_empty() {
            if state.is_loading() {
                return None;
            }
            return Some(
                block()
                    .child(s.no_match.replace("{q}", state.query()))
                    .children(hint)
                    .into_any_element(),
            );
        }
        let today = chrono::Local::now().date_naive();
        let selected = state.selected_index();
        let dimmed = self.dimmed();
        let reduce_motion = self.copied.is_some() && magi_core::platform::reduce_motion();
        let rows = results.iter().enumerate().map(|(ix, r)| {
            // The row that was copied, not the selection: it may have moved on.
            let flash = self
                .copied
                .as_ref()
                .filter(|(id, _)| *id == r.file_id)
                .map(|_| self.copies);
            // The flash settles to the row's own background: the selection
            // color, or none once the selection has moved off it.
            let accent = p.accent;
            let base = match selected == Some(ix) {
                true => p.selection,
                false => p.selection.opacity(0.),
            };
            row(ix, r, selected == Some(ix), flash, p, lang, today)
                // Mouse move, not hover: keyboard scrolling slides rows under a
                // still cursor and must not steal the selection.
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.state.selected_index() != Some(ix) {
                        this.state.select_at(ix);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state.select_at(ix);
                    this.act(false, window, cx);
                }))
                // The copied row flashes the accent, then settles back. A new
                // copy changes the id and replays it. Under reduced motion it
                // is a steady tint while "Path copied" shows.
                .map(move |row| match flash {
                    Some(_) if reduce_motion => {
                        row.bg(copy_tint(base, accent, 0.5)).into_any_element()
                    }
                    Some(n) => row
                        .with_animation(
                            ("copied", n),
                            Animation::new(COPY_FLASH).with_easing(ease_out_quint()),
                            move |row, t| row.bg(copy_tint(base, accent, 0.8 * (1. - t))),
                        )
                        .into_any_element(),
                    None => row.into_any_element(),
                })
        });
        Some(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("results")
                        .flex()
                        .flex_col()
                        .max_h(LIST_MAX_HEIGHT)
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .px(px(6.))
                        .pt(px(4.))
                        .pb(px(6.))
                        .border_t_1()
                        .border_color(p.line)
                        .children(rows)
                        .map(|list| match dimmed {
                            true => list
                                .with_animation(
                                    "dim",
                                    Animation::new(theme::FADE).with_easing(ease_out_quint()),
                                    |list, t| list.opacity(1. - 0.55 * t),
                                )
                                .into_any_element(),
                            false => list.into_any_element(),
                        }),
                )
                .children(hint.map(|h| div().px(px(12.)).pb(px(10.)).child(h)))
                .into_any_element(),
        )
    }
}

/// A row's background (the selection color, maybe fully transparent) pulled
/// toward the accent by `k` (0..=1), and more opaque: `blend` alone keeps
/// the base's alpha, which would barely show.
fn copy_tint(base: Hsla, accent: Hsla, k: f32) -> Hsla {
    Hsla {
        a: base.a + (0.5 - base.a) * k,
        ..base.blend(accent.opacity(k))
    }
}

fn row(
    ix: usize,
    r: &SearchResult,
    selected: bool,
    // `Some(copies)` while "Copied" shows in place of the match sources.
    copied: Option<u64>,
    p: &Palette,
    lang: Lang,
    today: NaiveDate,
) -> Stateful<Div> {
    let s = lang.strings();
    let page = r.page.map(|n| {
        div().flex_none().text_color(p.mute).child(format!(
            " · {}",
            s.page.replace("{n}", &lang.number(n.max(0) as u64))
        ))
    });
    // The same detail whether selected or not, so the list does not change
    // height as the selection moves. A snippet leaves no room for the path,
    // so the folder's name follows the file's instead, behind a folder icon
    // so it reads as where the file is, not as more of its name.
    let folder = r.snippet.is_some().then(|| {
        let full: SharedString = folder_of(&r.path).into();
        div()
            .id(("folder", ix))
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .ml_3()
            .text_color(p.mute)
            .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
            // Only the selected row shows it; the others keep its space so
            // the folder's name does not move as the selection does.
            .child(
                div()
                    .flex_none()
                    .when(!selected, |d| d.invisible())
                    .child(Icon::new(IconName::Folder).size(px(12.))),
            )
            .child(div().truncate().child(folder_name(&r.path)))
    });
    let detail = match &r.snippet {
        Some(snippet) => div()
            .mt(px(2.))
            .text_size(px(12.5))
            .line_height(relative(1.45))
            .text_color(p.mute)
            .line_clamp(2)
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
            .into_any_element(),
        // The full path is in the tooltip.
        None => div()
            .id(("path", ix))
            .text_xs()
            .text_color(p.mute)
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis_middle()
            .tooltip({
                let full: SharedString = folder_of(&r.path).into();
                move |window, cx| Tooltip::new(full.clone()).build(window, cx)
            })
            .child(short_folder(&r.path))
            .into_any_element(),
    };
    let date = local_date(r.modified_at)
        .map(|d| lang.date(d, today))
        .unwrap_or_default();
    let sources = r
        .match_sources
        .iter()
        .map(|&m| s.source(m))
        .collect::<Vec<_>>()
        .join(", ");
    // What each source means, one per line.
    let sources_tip: SharedString = explain_sources(&r.match_sources, s, "\n").into();
    // Feedback on mouse down; the click opens on release.
    let pressed = Hsla {
        a: p.selection.a * 2.,
        ..p.selection
    };
    div()
        .id(ix)
        // A result opens like a link: the hand, as on every clickable thing.
        .cursor_pointer()
        .relative()
        .flex()
        .items_center()
        .gap_3()
        .pl(px(13.))
        .pr(px(10.))
        .py(px(7.))
        .rounded(px(4.))
        .when(selected, |d| d.bg(p.selection).child(p.accent_bar()))
        .active(move |s| s.bg(pressed))
        .child(icon(r))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .child(div().truncate().child(r.file_name.clone()))
                        .children(page)
                        .children(folder),
                )
                .child(detail),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_end()
                .flex_none()
                .text_xs()
                .text_color(p.mute)
                .child(date)
                .map(|d| match copied {
                    // Where the eyes are when copying; replays per copy.
                    Some(n) => d.child(theme::fade_in(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_color(p.accent)
                            .child(Icon::new(IconName::Check).size(px(12.)))
                            .child(s.row_copied),
                        ("row-copied", n),
                    )),
                    // On every row: why it matched is the product, not a
                    // detail of the selection.
                    None if !sources.is_empty() => d.child(
                        div()
                            .id(("match-sources", ix))
                            .text_color(if selected { p.accent } else { p.mute })
                            .tooltip(move |window, cx| {
                                Tooltip::new(sources_tip.clone()).build(window, cx)
                            })
                            .child(sources),
                    ),
                    None => d,
                }),
        )
}

fn icon(r: &SearchResult) -> AnyElement {
    let (kind, name) = (r.kind, r.file_name.clone());
    match &r.thumb_path {
        Some(path) => img(PathBuf::from(path))
            .size(px(32.))
            .rounded(px(3.))
            .object_fit(ObjectFit::Cover)
            .with_fallback(move || theme::file_glyph(kind, &name, px(32.)).into_any_element())
            .into_any_element(),
        None => theme::file_glyph(kind, &name, px(32.)).into_any_element(),
    }
}

fn folder_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// The last two folders of the path, `…\Documents\Contratos`: enough to
/// tell two files of one name apart; the tooltip has the rest.
fn short_folder(path: &str) -> String {
    let Some(parent) = Path::new(path).parent() else {
        return String::new();
    };
    let names: Vec<_> = parent
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if names.len() <= 2 {
        return parent.display().to_string();
    }
    let sep = std::path::MAIN_SEPARATOR.to_string();
    format!("\u{2026}{sep}{}", names[names.len() - 2..].join(&sep))
}

/// The name of the folder a file is in.
fn folder_name(path: &str) -> String {
    Path::new(path)
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn local_date(unix_ms: i64) -> Option<NaiveDate> {
    chrono::DateTime::from_timestamp_millis(unix_ms)
        .map(|t| t.with_timezone(&chrono::Local).date_naive())
}

#[cfg(test)]
mod tests {
    use super::short_folder;

    #[test]
    fn a_deep_path_keeps_its_last_two_folders() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = ["", "home", "ana", "Documents", "Contratos", "a.pdf"].join(&sep.to_string());
        assert_eq!(
            short_folder(&path),
            format!("\u{2026}{sep}Documents{sep}Contratos")
        );
        assert_eq!(short_folder("a.pdf"), "");
    }
}
