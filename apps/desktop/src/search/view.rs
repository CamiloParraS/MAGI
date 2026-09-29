//! The search window (SPEC.md M6): input, results, keyboard. The logic is in
//! [`SearchState`]; this file wires it to GPUI and `Host`.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::dto::{ErrorCode, SearchRequest, SearchResult};
use magi_core::host::Host;

use super::highlight;
use super::state::SearchState;
use crate::theme::Backdrop;

actions!(magi_search, [SelectUp, SelectDown, Close]);

const CONTEXT: &str = "SearchWindow";
pub const DEBOUNCE: Duration = Duration::from_millis(150);

pub struct SearchView {
    host: Host,
    input: Entity<InputState>,
    state: SearchState,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    backdrop: Backdrop,
    /// Logged once, at the first frame (M6: visible < 150 ms after the hotkey).
    opened_at: Option<Instant>,
    /// Replacing it cancels the debounce or search still pending.
    pending: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SearchView {
    /// Enter / Ctrl+Enter come from the input's own `PressEnter` event, not
    /// bindings: a single-line input also propagates `enter`, and binding it
    /// here would open the file twice.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("up", SelectUp, Some(CONTEXT)),
            KeyBinding::new("down", SelectDown, Some(CONTEXT)),
            KeyBinding::new("escape", Close, Some(CONTEXT)),
        ]);
    }

    pub fn new(
        host: Host,
        backdrop: Backdrop,
        opened_at: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search your files"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, Self::on_input_event),
            // Hide on blur; the window is recreated on the next show.
            cx.observe_window_activation(window, |_, window, _| {
                if !window.is_window_active() {
                    window.remove_window();
                }
            }),
        ];
        input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            host,
            input,
            state: SearchState::default(),
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            backdrop,
            opened_at: Some(opened_at),
            pending: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn state(&self) -> &SearchState {
        &self.state
    }

    fn on_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let text = input.read(cx).value().to_string();
                self.search(&text, cx);
            }
            InputEvent::PressEnter { secondary, .. } => self.act(*secondary, window, cx),
            _ => {}
        }
    }

    fn search(&mut self, text: &str, cx: &mut Context<Self>) {
        self.pending = None;
        let Some(query) = self.state.set_query(text) else {
            cx.notify();
            return;
        };
        let host = self.host.clone();
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let request = SearchRequest {
                query: query.text,
                limit: None,
            };
            let reply = cx
                .background_executor()
                .spawn(async move { host.search(&request).map_err(|e| ErrorCode::from(&e)) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.state.apply(query.generation, reply);
                cx.notify();
            });
        }));
    }

    /// Opens (or, with `reveal`, shows in the file manager) the selected
    /// result, by `file_id` through `Host` only (SPEC.md §5.7 Security).
    fn act(&mut self, reveal: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file_id) = self.state.selected().map(|r| r.file_id) else {
            return;
        };
        // ponytail: one indexed row read on the main thread; move it to the
        // background executor if it ever shows up in a frame trace.
        match self.host.file_path(file_id) {
            Ok(path) => {
                if reveal {
                    cx.reveal_path(&path);
                } else {
                    cx.open_with_system(&path);
                }
                window.remove_window();
            }
            Err(error) => tracing::warn!(file_id, %error, "a result's file could not be resolved"),
        }
    }

    fn select(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.state.select(delta);
        if let Some(ix) = self.state.selected_index() {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
        cx.notify();
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(opened_at) = self.opened_at.take() {
            tracing::info!(elapsed = ?opened_at.elapsed(), "search window first frame");
        }
        let alpha = match self.backdrop {
            Backdrop::Blurred { tint_alpha } => tint_alpha,
            Backdrop::Solid => 1.0,
        };
        let selected = self.state.selected_index();
        let rows = self.state.results().len();
        div()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(hsla(0., 0., 0.97, alpha))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|_, _: &Close, window, _| window.remove_window()))
            .child(div().p_3().child(Input::new(&self.input)))
            .child(
                uniform_list(
                    "results",
                    rows,
                    cx.processor(move |this, range: Range<usize>, _, _| {
                        range
                            .map(|ix| row(ix, &this.state.results()[ix], selected == Some(ix)))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1(),
            )
            .children(
                self.state
                    .error()
                    .map(|e| div().p_3().child(format!("Search failed: {e:?}"))),
            )
    }
}

fn row(ix: usize, result: &SearchResult, selected: bool) -> Stateful<Div> {
    let emphasis = HighlightStyle {
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    let snippet = result.snippet.as_ref().map(|s| {
        StyledText::new(s.text.clone()).with_highlights(highlight::runs(
            &s.text,
            &s.highlights,
            emphasis,
        ))
    });
    div()
        .id(ix)
        .px_3()
        .py_1()
        .when(selected, |d| d.bg(hsla(0.6, 0.5, 0.88, 1.)))
        .child(div().child(result.file_name.clone()))
        .child(div().text_xs().child(result.path.clone()))
        .children(snippet)
}
