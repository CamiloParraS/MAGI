//! The search window (SPEC.md M6), styled as variant A "Pane": input,
//! results, states, keyboard. The logic is in [`SearchState`] and
//! [`hint`](super::hint); this file wires it to GPUI and `Host`.

mod footer;
mod hints;
mod results;

use std::time::{Duration, Instant};

use gpui_kit::component::input::{Copy as CopyText, Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::dto::{ErrorCode, SearchRequest};
use magi_core::host::Host;

use super::state::{Query, SearchState};
use crate::app::{AppEvent, Events};
use crate::live::Live;
use crate::theme::{self, Backdrop, Palette};

actions!(magi_search, [SelectUp, SelectDown, Close, OpenSettings]);

const CONTEXT: &str = "SearchWindow";
/// The wait after a keystroke typed while a search is still pending.
pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// A search shows it is loading only once it has run this long, so fast ones
/// never flash the bar or dim the list.
const SLOW: Duration = Duration::from_millis(150);
/// How long "Path copied" replaces "Copy path".
const COPIED: Duration = Duration::from_millis(1200);
/// The footer's copy hint, whose label becomes "Path copied".
const COPY_KEY: &str = "secondary-c";
/// How long the copied row's highlight takes to fade.
const COPY_FLASH: Duration = Duration::from_millis(600);
pub const WIDTH: Pixels = px(820.);
/// The tallest the window gets. It opens with its top where a window this
/// tall would be centered, then fits its content, growing downward.
pub const MAX_HEIGHT: Pixels = px(560.);
/// The opening height: the input row alone.
pub const OPEN_HEIGHT: Pixels = px(58.);
const LIST_MAX_HEIGHT: Pixels = px(420.);

/// How a search window was opened.
pub struct Opening {
    pub at: Instant,
    /// `ui.hotkey`, when it was opened some other way than by it.
    pub teach: Option<String>,
}

pub struct SearchView {
    host: Host,
    live: Entity<Live>,
    events: Events,
    input: Entity<InputState>,
    state: SearchState,
    scroll: ScrollHandle,
    focus: FocusHandle,
    backdrop: Backdrop,
    dark: bool,
    /// Logged once, at the first frame (M6: visible < 150 ms after the hotkey).
    opened_at: Option<Instant>,
    /// Replacing it cancels the debounce or search still pending.
    pending: Option<Task<()>>,
    /// The search has run past [`SLOW`]; see [`dimmed`](Self::dimmed).
    slow: bool,
    slow_timer: Option<Task<()>>,
    /// The copied result's `file_id`, set while the footer says the path was
    /// copied; its task clears it.
    copied: Option<(i64, Task<()>)>,
    /// Copies so far; keys the row flash so each copy replays it.
    copies: u64,
    /// `ui.hotkey`, when this window was opened some other way than by it.
    teach: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SearchView {
    /// Enter / Ctrl+Enter come from the input's own `PressEnter` event, not
    /// bindings: a single-line input also propagates `enter`, and binding it
    /// here would open the file twice. Copy is the input's own action,
    /// captured in [`copy_path`](Self::copy_path).
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("up", SelectUp, Some(CONTEXT)),
            KeyBinding::new("down", SelectDown, Some(CONTEXT)),
            KeyBinding::new("escape", Close, Some(CONTEXT)),
            KeyBinding::new("secondary-,", OpenSettings, Some(CONTEXT)),
        ]);
    }

    pub fn new(
        host: Host,
        live: Entity<Live>,
        events: Events,
        backdrop: Backdrop,
        opening: Opening,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = live.read(cx).lang.strings().placeholder;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, Self::on_input_event),
            cx.observe(&live, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_appearance(window, cx);
            }),
            // Hide on blur; the window is recreated on the next show.
            cx.observe_window_activation(window, |_, window, _| {
                if !window.is_window_active() {
                    window.remove_window();
                }
            }),
        ];
        input.update(cx, |input, cx| input.focus(window, cx));
        let mut view = Self {
            host,
            live,
            events,
            input,
            state: SearchState::default(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            backdrop,
            dark: false,
            opened_at: Some(opening.at),
            pending: None,
            slow: false,
            slow_timer: None,
            copied: None,
            copies: 0,
            teach: opening.teach,
            _subscriptions: subscriptions,
        };
        view.sync_appearance(window, cx);
        view.restore_query(window, cx);
        view
    }

    /// Reopens on the last query, selected so typing replaces it.
    /// `set_value` emits no `Change`, so the search is started here.
    fn restore_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.live.read(cx).last_query.clone();
        if text.is_empty() {
            return;
        }
        self.input.update(cx, |input, cx| {
            input.set_value(text.clone(), window, cx);
            input.select_all(window, cx);
        });
        if let Some(query) = self.state.set_query(&text) {
            self.run(query, Duration::ZERO, cx);
        }
    }

    pub fn state(&self) -> &SearchState {
        &self.state
    }

    /// Follows `ui.theme` and the OS; gpui-component's theme colors the input.
    fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.live.read(cx).theme.clone();
        self.dark = theme::sync(&theme, window, cx);
        cx.notify();
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
                self.live
                    .update(cx, |live, _| live.last_query.clone_from(&text));
                self.pending = None;
                // Leading edge: a keystroke while idle searches at once; only
                // the ones that follow while a search is pending wait.
                let delay = if self.state.is_loading() {
                    DEBOUNCE
                } else {
                    Duration::ZERO
                };
                match self.state.set_query(&text) {
                    Some(query) => self.run(query, delay, cx),
                    None => cx.notify(),
                }
            }
            InputEvent::PressEnter { secondary, .. } => {
                if self.state.error().is_some() {
                    self.retry(cx);
                } else {
                    self.act(*secondary, window, cx);
                }
            }
            _ => {}
        }
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(query) = self.state.retry() {
            self.run(query, Duration::ZERO, cx);
        }
    }

    fn run(&mut self, query: Query, delay: Duration, cx: &mut Context<Self>) {
        let host = self.host.clone();
        // The last keystroke; the debounce counts toward the M6 < 400 ms.
        let typed_at = Instant::now();
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let request = SearchRequest {
                query: query.text,
                limit: None,
            };
            let reply = cx
                .background_executor()
                .spawn(async move { host.search(&request).map_err(|e| ErrorCode::from(&e)) })
                .await;
            tracing::info!(elapsed = ?typed_at.elapsed(), "search results after typing stopped");
            let _ = this.update(cx, |view, cx| {
                view.state.apply(query.generation, reply);
                view.scroll.scroll_to_item(0);
                cx.notify();
            });
        }));
        self.slow = false;
        self.slow_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay + SLOW).await;
            let _ = this.update(cx, |view, cx| {
                view.slow = true;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The search has been loading past [`SLOW`]: the list dims and the bar
    /// pulses.
    fn dimmed(&self) -> bool {
        self.state.is_loading() && self.slow
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

    /// `Ctrl/Cmd+C` copies the selected result's path, unless the input has
    /// text selected: then the input copies it.
    fn copy_path(&mut self, _: &CopyText, _: &mut Window, cx: &mut Context<Self>) {
        let text_selected = !self.input.read(cx).selected_range().is_empty();
        match self.state.selected() {
            Some(result) if !text_selected => {
                cx.write_to_clipboard(ClipboardItem::new_string(result.path.clone()));
                self.copies += 1;
                let file_id = result.file_id;
                self.copied = Some((
                    file_id,
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(COPIED).await;
                        let _ = this.update(cx, |view, cx| {
                            view.copied = None;
                            cx.notify();
                        });
                    }),
                ));
                cx.notify();
            }
            _ => cx.propagate(),
        }
    }

    fn select(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.state.select(delta);
        if let Some(ix) = self.state.selected_index() {
            self.scroll.scroll_to_item(ix);
        }
        cx.notify();
    }

    /// The gear and `Ctrl/Cmd+,`; this window closes itself when settings
    /// takes the focus.
    fn open_settings(&mut self, _: &OpenSettings, _: &mut Window, _: &mut Context<Self>) {
        let _ = self.events.try_send(AppEvent::Settings);
    }
}

/// Resizes the window to its content, once layout knows the height.
fn fit(children: Vec<Bounds<Pixels>>, window: &mut Window, cx: &mut App) {
    let Some(panel) = children.first() else {
        return;
    };
    let height = panel.size.height.min(MAX_HEIGHT);
    if (window.viewport_size().height - height).abs() > px(0.5) {
        window.defer(cx, move |window, _| window.resize(size(WIDTH, height)));
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(opened_at) = self.opened_at.take() {
            tracing::info!(elapsed = ?opened_at.elapsed(), "search window first frame");
        }
        let p = Palette::new(self.dark);
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(p.background(self.backdrop))
            .text_color(p.ink)
            .text_size(px(14.))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|_, _: &Close, window, _| window.remove_window()))
            .capture_action(cx.listener(Self::copy_path))
            .on_action(cx.listener(Self::open_settings))
            .on_children_prepainted(fit)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px(px(18.))
                            .py(px(7.))
                            .child(Icon::new(IconName::Search).size(px(18.)).text_color(p.mute))
                            // The large size's own text size: its line box is
                            // fixed at 20 px, so bigger text clips descenders.
                            .child(Input::new(&self.input).appearance(false).large()),
                    )
                    .children(self.shortcut_hint(&p, cx))
                    .when(self.dimmed(), |d| {
                        d.child(
                            div().h(px(2.)).bg(p.accent).with_animation(
                                "loading",
                                Animation::new(Duration::from_millis(1200))
                                    .repeat()
                                    .with_easing(pulsating_between(0.25, 0.5)),
                                |bar, alpha| bar.opacity(alpha),
                            ),
                        )
                    })
                    .children(self.body(&p, cx))
                    .child(self.footer(&p, cx)),
            )
    }
}
