//! Adding files to the chips: the picker that searches the index, the
//! chip that opens it, and the drop target for files from the file manager.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::dto::{SearchRequest, SearchResult};

use super::AskView;
use super::chip::{CHIP_THUMB, chip_shell, file_icon, preview_tip};
use crate::ask::attach::{Notice, file_info, notice_for};
use crate::ask::{MAX_SOURCES, Phase};
use crate::i18n::Strings;
use crate::theme::Palette;
use crate::ui::folders::folder_name;
use crate::ui::{note, pressable};

/// Results the picker asks for, and how long it waits for typing to stop.
const PICK_ROWS: u32 = 8;
const PICK_DELAY: Duration = Duration::from_millis(160);
/// How many dropped files are looked at.
const MAX_DROPPED: usize = MAX_SOURCES * 2;

/// The picker's search box and what it found.
pub(super) struct Picker {
    input: Entity<InputState>,
    results: Vec<SearchResult>,
    /// A search has come back for the text in the box.
    searched: bool,
    /// Only the newest search's reply is shown.
    generation: u64,
    _subscription: Subscription,
}
impl AskView {
    /// Files dropped on the window join the chips when they are in the
    /// user's Magi folders.
    pub(super) fn drop_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.model.phase() != Phase::Chat || self.model.is_asking() || paths.is_empty() {
            return;
        }
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .iter()
                        .take(MAX_DROPPED)
                        .map(|path| lookup(&host, path))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(notice) = view.model.add_dropped(found) {
                    view.say(notice, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the picker, or closes it.
    pub fn toggle_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            self.question
                .update(cx, |question, cx| question.focus(window, cx));
            cx.notify();
            return;
        }
        if self.model.is_asking() {
            return;
        }
        if self.model.sources().len() >= MAX_SOURCES {
            self.say(Notice::Full, cx);
            return;
        }
        let placeholder = self.live.read(cx).lang.strings().ask_pick_placeholder;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_picker(cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.picker = Some(Picker {
            input,
            results: Vec::new(),
            searched: false,
            generation: 0,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Puts `text` in the open picker's box and searches for it, as typing
    /// would.
    pub fn set_picker_query(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.picker.as_ref().map(|picker| picker.input.clone()) else {
            return;
        };
        let text = text.to_owned();
        input.update(cx, |input, cx| input.set_value(text, window, cx));
        self.search_picker(cx);
    }

    /// Searches the index for what the picker's box says, once typing has
    /// paused, off the main thread.
    fn search_picker(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        let query = picker.input.read(cx).value().trim().to_owned();
        picker.generation += 1;
        let generation = picker.generation;
        if query.is_empty() {
            picker.results.clear();
            picker.searched = false;
            cx.notify();
            return;
        }
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PICK_DELAY).await;
            let current = this
                .update(cx, |view, _| {
                    view.picker
                        .as_ref()
                        .is_some_and(|picker| picker.generation == generation)
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let found = cx
                .background_executor()
                .spawn(async move {
                    host.search(&SearchRequest {
                        query,
                        limit: Some(PICK_ROWS),
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let Some(picker) = view.picker.as_mut() else {
                    return;
                };
                if picker.generation != generation {
                    return;
                }
                picker.results = match found {
                    Ok(found) => found.results,
                    Err(error) => {
                        tracing::warn!(%error, "the picker's search failed");
                        Vec::new()
                    }
                };
                picker.searched = true;
                cx.notify();
            });
        })
        .detach();
    }

    /// Adds picker row `ix`.
    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(result) = self
            .picker
            .as_ref()
            .and_then(|picker| picker.results.get(ix))
        else {
            return;
        };
        let added = self.model.add(file_info(result));
        if let Some(notice) = notice_for(added) {
            self.say(notice, cx);
        }
        cx.notify();
    }

    /// Escape closes the picker first, then the window.
    pub(super) fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.picker.take() {
            Some(_) => {
                self.question
                    .update(cx, |question, cx| question.focus(window, cx));
                cx.notify();
            }
            None => window.remove_window(),
        }
    }

    /// The chip that opens the picker.
    pub(super) fn add_chip(
        &self,
        p: &Palette,
        s: &Strings,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let (open, busy) = (self.picker.is_some(), self.model.is_asking());
        let (line, accent, selection) = (p.line, p.accent, p.selection);
        pressable(
            chip_shell()
                .h(px(32.))
                .px(px(12.))
                .border_dashed()
                .text_color(if open { p.accent } else { p.mute })
                .map(|d| match open {
                    true => d.border_color(accent.opacity(0.45)).bg(selection),
                    false => d.border_color(line),
                })
                .when(busy, |d| d.opacity(0.5))
                .when(!busy && !open, |d| {
                    d.hover(move |d| d.border_color(accent.opacity(0.5)).text_color(accent))
                })
                .child(Icon::new(IconName::Plus).size(px(11.)))
                .child(s.ask_add_file),
            "ask-add",
        )
        .focusable()
        .focus_visible(move |d| d.bg(selection))
        .when(!busy, |d| d.cursor_pointer())
        .role(Role::Button)
        .aria_label(s.ask_add_file)
        .aria_toggled(if open { Toggled::True } else { Toggled::False })
        .on_click(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx)))
    }

    /// The search box and its results, inside the dock's card.
    pub(super) fn picker_panel(
        &self,
        picker: &Picker,
        p: &Palette,
        s: &Strings,
        cx: &Context<Self>,
    ) -> Div {
        let attached = |file_id: i64| self.model.sources().iter().any(|s| s.file_id == file_id);
        let (line, selection) = (p.line, p.selection);
        let rows = picker.results.iter().enumerate().map(|(ix, result)| {
            let there = attached(result.file_id);
            let folder = Path::new(&result.path)
                .parent()
                .map(folder_name)
                .unwrap_or_default();
            let thumb = result.thumb_path.as_ref().map(PathBuf::from);
            let preview: String = file_info(result).preview.chars().take(280).collect();
            let has_preview = thumb.is_some() || !preview.is_empty();
            div()
                .id(("ask-pick", ix))
                .focusable()
                .focus_visible(move |d| d.bg(selection))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(32.))
                .px(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(move |d| d.bg(selection))
                .role(Role::Button)
                .aria_label(result.file_name.clone())
                .when(has_preview, |d| {
                    d.tooltip(preview_tip(preview.into(), thumb.clone()))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.pick(ix, cx)))
                // One width for glyphs and thumbnails, so the names line up.
                .child(
                    div()
                        .flex_none()
                        .w(CHIP_THUMB)
                        .flex()
                        .justify_center()
                        .child(file_icon(result.kind, &result.file_name, thumb.as_ref())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .child(result.file_name.clone()),
                )
                .child(
                    div()
                        .max_w(px(160.))
                        .truncate()
                        .text_size(px(11.))
                        .text_color(p.mute)
                        .child(folder),
                )
                .when(there, |d| {
                    d.child(
                        Icon::new(IconName::Check)
                            .size(px(12.))
                            .text_color(p.accent),
                    )
                })
        });
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(6.))
            .rounded(px(14.))
            .border_1()
            .border_color(line)
            .bg(p.solid)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(6.))
                    .child(Icon::new(IconName::Search).size(px(14.)).text_color(p.mute))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&picker.input).appearance(false)),
                    )
                    .child(
                        Button::new("ask-pick-close")
                            .cursor_pointer()
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(self.live.read(cx).lang.strings().close)
                            .on_click(cx.listener(|this, _, window, cx| this.escape(window, cx))),
                    ),
            )
            .when(!picker.results.is_empty(), |d| {
                d.child(
                    div()
                        .id("ask-pick-list")
                        .flex()
                        .flex_col()
                        .max_h(px(168.))
                        .overflow_y_scroll()
                        .children(rows),
                )
            })
            .when(picker.searched && picker.results.is_empty(), |d| {
                d.child(note(p).px(px(8.)).pb(px(4.)).child(s.ask_pick_none))
            })
    }

    /// Shown over the window while files are dragged onto it.
    pub(super) fn drop_overlay(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let s = self.live.read(cx).lang.strings();
        div()
            .absolute()
            .top(px(10.))
            .left(px(10.))
            .right(px(10.))
            .bottom(px(10.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .rounded(px(20.))
            .border_2()
            .border_dashed()
            .border_color(p.accent)
            .bg(p.solid.opacity(0.92))
            .opacity(0.)
            .group_drag_over::<ExternalPaths>(super::DROP, |d| d.opacity(1.))
            .child(Icon::new(IconName::Plus).size(px(22.)).text_color(p.accent))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s.ask_drop_here),
            )
    }
}

/// The dropped path as a file Ask Magi can use, or `None`.
fn lookup(host: &magi_core::host::Host, path: &Path) -> Option<magi_core::dto::AskFileInfo> {
    host.ask_file_at(path)
        .inspect_err(|error| tracing::warn!(%error, "a dropped file could not be looked up"))
        .ok()
        .flatten()
}
