//! The search window (SPEC.md M6), styled as variant A "Pane": input,
//! results, states, keyboard. The logic is in [`SearchState`] and
//! [`hint`](super::hint); this file wires it to GPUI and `Host`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Copy as CopyText, Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::dto::{
    ErrorCode, FeatureStatus, IndexState, IndexStatus, Install, SearchRequest, SearchResult,
};
use magi_core::features::Feature;
use magi_core::host::Host;

use super::highlight;
use super::hint::{Hint, hint, teach_shortcut};
use super::state::{Query, SearchState};
use crate::app::{AppEvent, Events};
use crate::i18n::Lang;
use crate::settings::hotkey_keystroke;
use crate::theme::{self, Backdrop, Palette};
use crate::tray::{status_line, status_tone};

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

/// App state the search window shows besides its results. The shell keeps
/// it current; an open window re-renders on every change.
pub struct Live {
    pub lang: Lang,
    /// `ui.theme`.
    pub theme: String,
    pub status: Option<IndexStatus>,
    pub features: Vec<FeatureStatus>,
    /// ponytail: hints closed this session only; persist in config if a
    /// hint that returns after a restart annoys anyone.
    pub dismissed: Vec<Feature>,
    /// The shortcut line was closed (this session, as the hints).
    pub shortcut_dismissed: bool,
    /// Hotkey presses this run; onboarding confirms one it sees.
    pub hotkey_presses: u32,
    /// A shortcut that could not be registered (another app or the OS has
    /// it); settings asks for a different one.
    pub hotkey_conflict: Option<String>,
    /// The search window is open; onboarding says the hotkey closes it.
    pub search_open: bool,
    /// The last query typed; the next window opens with it selected.
    pub last_query: String,
}

impl Live {
    pub fn new(lang: Lang, theme: String) -> Self {
        Self {
            lang,
            theme,
            status: None,
            features: Vec::new(),
            dismissed: Vec::new(),
            shortcut_dismissed: false,
            hotkey_presses: 0,
            hotkey_conflict: None,
            search_open: false,
            last_query: String::new(),
        }
    }
}

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

    /// The hint reports the progress through [`Live::features`].
    fn turn_on(&mut self, feature: Feature, cx: &mut Context<Self>) {
        let host = self.host.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = turn_on(&host, feature) {
                    tracing::warn!(%error, ?feature, "could not turn the feature on");
                }
            })
            .detach();
    }

    /// The gear and `Ctrl/Cmd+,`; this window closes itself when settings
    /// takes the focus.
    fn open_settings(&mut self, _: &OpenSettings, _: &mut Window, _: &mut Context<Self>) {
        let _ = self.events.try_send(AppEvent::Settings);
    }

    fn dismiss(&mut self, feature: Feature, cx: &mut Context<Self>) {
        self.live.update(cx, |live, cx| {
            live.dismissed.push(feature);
            cx.notify();
        });
    }

    /// The shortcut, under the input, so next time search is one keypress
    /// away (see [`teach_shortcut`]).
    fn shortcut_hint(&self, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let show = teach_shortcut(
            self.teach.is_none(),
            live.shortcut_dismissed,
            live.hotkey_conflict.is_some(),
        );
        let keystroke = self
            .teach
            .as_ref()
            .filter(|_| show)
            .and_then(|hotkey| Keystroke::parse(&hotkey_keystroke(hotkey)).ok())?;
        let hint = div()
            .flex()
            .items_center()
            .gap_2()
            .mx(px(12.))
            .mb(px(10.))
            .pl(px(12.))
            .pr(px(6.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(p.selection)
            .text_sm()
            .child(s.teach_shortcut)
            .child(Kbd::new(keystroke))
            .child(div().flex_1())
            .child(
                Button::new("dismiss-shortcut")
                    .cursor_pointer()
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(s.dismiss)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.live.update(cx, |live, cx| {
                            live.shortcut_dismissed = true;
                            cx.notify();
                        });
                    })),
            );
        Some(theme::fade_in(hint, "shortcut-hint"))
    }

    fn body(&self, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
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
            return Some(
                block()
                    .child(s.search_failed)
                    .child(
                        Button::new("retry")
                            .cursor_pointer()
                            .label(s.try_again)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
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

    fn hint(&self, hint: Hint, p: &Palette, cx: &Context<Self>) -> impl IntoElement + use<> {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let feature = hint.feature();
        let text = s.feature(feature);
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let (message, name_at, action) = match hint {
            Hint::Offer { size, .. } => {
                let off = s.feature_off.replace("{name}", text.name);
                let name_at = s.feature_off.find("{name}");
                let button = Button::new("turn-on")
                    .cursor_pointer()
                    .primary()
                    .small()
                    .label(s.turn_on.replace("{size}", &lang.size(size)))
                    .on_click(cx.listener(move |this, _, _, cx| this.turn_on(feature, cx)));
                (format!("{off} {}", text.pitch), name_at, Some(button))
            }
            Hint::Downloading { done, total, .. } => (
                s.downloading
                    .replace("{name}", text.name)
                    .replace("{done}", &lang.size(done))
                    .replace("{total}", &lang.size(total)),
                None,
                None,
            ),
            Hint::Updating { left, .. } => (
                lang.plural(&s.updating, left).replace("{name}", text.name),
                None,
                None,
            ),
        };
        let highlights = name_at
            .map(|at| vec![(at..at + text.name.len(), bold)])
            .unwrap_or_default();
        // A new stage (offer, downloading, updating) fades in again.
        let stage = match hint {
            Hint::Offer { .. } => 0u64,
            Hint::Downloading { .. } => 1,
            Hint::Updating { .. } => 2,
        };
        let hint = div()
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .px(px(12.))
            .py(px(10.))
            .rounded(px(6.))
            .bg(p.selection)
            .text_color(p.ink)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(StyledText::new(message).with_highlights(highlights)),
            )
            .children(action)
            .child(
                Button::new("dismiss")
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .tooltip(s.dismiss)
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss(feature, cx))),
            );
        theme::fade_in(hint, ("hint", stage))
    }

    /// Always there: the settings gear, the index status (as in the tray)
    /// and the keys that apply now.
    fn footer(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let status = live.status.as_ref().map(|st| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(p.dot(status_tone(st)))
                .child(div().truncate().child(status_line(st, lang)))
        });
        let pairs: &[(&str, &'static str)] = if self.state.results().is_empty() {
            &[("escape", s.close)]
        } else {
            &[
                ("enter", s.open),
                ("secondary-enter", s.reveal),
                (
                    COPY_KEY,
                    if self.copied.is_some() {
                        s.copied
                    } else {
                        s.copy_path
                    },
                ),
            ]
        };
        div()
            .flex()
            .items_center()
            .gap_3()
            .pl(px(8.))
            .pr(px(16.))
            .py(px(6.))
            .text_xs()
            .text_color(p.mute)
            .border_t_1()
            .border_color(p.line)
            .child(
                Button::new("settings")
                    .cursor_pointer()
                    .ghost()
                    .xsmall()
                    .icon(IconName::Settings)
                    .tooltip_with_action(s.settings, &OpenSettings, Some(CONTEXT))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(&OpenSettings, window, cx)
                    })),
            )
            .child(div().flex_1().min_w_0().children(status))
            .child(keys(
                pairs,
                self.copied.is_some().then_some((self.copies, p)),
            ))
    }
}

/// Records the wish, then downloads if needed (ADR-0010). Blocking: run it
/// on the background executor.
pub fn turn_on(host: &Host, feature: Feature) -> magi_core::Result<()> {
    let installed = host.features().status().is_ok_and(|all| {
        all.iter()
            .any(|s| s.feature == feature && matches!(s.install, Install::Installed { .. }))
    });
    host.set_feature_enabled(feature, true)?;
    if installed {
        Ok(())
    } else {
        host.features().download(feature)
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
    let detail = match (&r.snippet, selected) {
        (Some(snippet), true) => div()
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
            ),
        _ => div()
            .text_xs()
            .text_color(p.mute)
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis_middle()
            .child(folder_of(&r.path)),
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
                        .children(page),
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
                    None if selected && !sources.is_empty() => {
                        d.child(div().text_color(p.accent).child(sources))
                    }
                    None => d,
                }),
        )
}

fn icon(r: &SearchResult) -> AnyElement {
    let kind = r.kind;
    match &r.thumb_path {
        Some(path) => img(PathBuf::from(path))
            .size(px(32.))
            .rounded(px(3.))
            .object_fit(ObjectFit::Cover)
            .with_fallback(move || theme::kind_glyph(kind, px(32.)).into_any_element())
            .into_any_element(),
        None => theme::kind_glyph(kind, px(32.)).into_any_element(),
    }
}

/// Key hints; `keys` are GPUI keystrokes (`secondary` is Ctrl, or Cmd on
/// macOS), shown the platform's way.
///
/// `copied` is `Some((copies, palette))` while "Path copied" shows: that
/// label starts in the accent color and settles to the others' with the
/// row flash; a new copy replays it.
fn keys(pairs: &[(&str, &'static str)], copied: Option<(u64, &Palette)>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .children(pairs.iter().flat_map(|&(key, label)| {
            let kbd = Keystroke::parse(key)
                .ok()
                .map(|k| Kbd::new(k).into_any_element());
            let label = (!label.is_empty()).then(|| {
                let label = div().mr(px(6.)).child(label);
                match copied {
                    Some((n, p)) if key == COPY_KEY => {
                        let (accent, mute) = (p.accent, p.mute);
                        label
                            .with_animation(
                                ("copied-label", n),
                                Animation::new(COPY_FLASH).with_easing(ease_out_quint()),
                                move |label, t| {
                                    label.text_color(mute.blend(accent.opacity(1. - t)))
                                },
                            )
                            .into_any_element()
                    }
                    _ => label.into_any_element(),
                }
            });
            kbd.into_iter().chain(label)
        }))
}

fn folder_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

fn local_date(unix_ms: i64) -> Option<NaiveDate> {
    chrono::DateTime::from_timestamp_millis(unix_ms)
        .map(|t| t.with_timezone(&chrono::Local).date_naive())
}
