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
use magi_core::discovery::Kind;
use magi_core::dto::{
    ErrorCode, FeatureStatus, IndexState, IndexStatus, Install, SearchRequest, SearchResult,
};
use magi_core::features::Feature;
use magi_core::host::Host;

use super::highlight;
use super::hint::{Hint, hint};
use super::state::{Query, SearchState};
use crate::i18n::Lang;
use crate::theme::{self, Backdrop, Palette};

actions!(magi_search, [SelectUp, SelectDown, Close]);

const CONTEXT: &str = "SearchWindow";
pub const DEBOUNCE: Duration = Duration::from_millis(150);
pub const WIDTH: Pixels = px(680.);
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
}

impl Live {
    pub fn new(lang: Lang, theme: String) -> Self {
        Self {
            lang,
            theme,
            status: None,
            features: Vec::new(),
            dismissed: Vec::new(),
        }
    }
}

pub struct SearchView {
    host: Host,
    live: Entity<Live>,
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
        ]);
    }

    pub fn new(
        host: Host,
        live: Entity<Live>,
        backdrop: Backdrop,
        opened_at: Instant,
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
            input,
            state: SearchState::default(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            backdrop,
            dark: false,
            opened_at: Some(opened_at),
            pending: None,
            _subscriptions: subscriptions,
        };
        view.sync_appearance(window, cx);
        view
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
                self.pending = None;
                match self.state.set_query(&text) {
                    Some(query) => self.run(query, DEBOUNCE, cx),
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
        cx.notify();
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

    /// Records the wish, then downloads if needed (ADR-0010); the hint
    /// reports the progress through [`Live::features`].
    fn turn_on(&mut self, feature: Feature, cx: &mut Context<Self>) {
        let host = self.host.clone();
        cx.background_executor()
            .spawn(async move {
                let installed = host.features().status().is_ok_and(|all| {
                    all.iter().any(|s| {
                        s.feature == feature && matches!(s.install, Install::Installed { .. })
                    })
                });
                let done = host.set_feature_enabled(feature, true).and_then(|()| {
                    if installed {
                        Ok(())
                    } else {
                        host.features().download(feature)
                    }
                });
                if let Err(error) = done {
                    tracing::warn!(%error, ?feature, "could not turn the feature on");
                }
            })
            .detach();
    }

    fn dismiss(&mut self, feature: Feature, cx: &mut Context<Self>) {
        self.live.update(cx, |live, cx| {
            live.dismissed.push(feature);
            cx.notify();
        });
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
            return Some(
                block()
                    .children(intro)
                    .child(keys(&[
                        ("up", ""),
                        ("down", s.move_selection),
                        ("enter", s.open),
                        ("escape", s.close),
                    ]))
                    .into_any_element(),
            );
        }
        let results = state.results();
        let hint = (!state.is_loading())
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
        let rows = results.iter().enumerate().map(|(ix, r)| {
            row(ix, r, selected == Some(ix), p, lang, today).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.state.select_at(ix);
                    this.act(false, window, cx);
                },
            ))
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
                        .when(state.is_loading(), |d| d.opacity(0.45))
                        .children(rows),
                )
                .children(hint.map(|h| div().px(px(12.)).pb(px(10.)).child(h)))
                .into_any_element(),
        )
    }

    fn hint(&self, hint: Hint, p: &Palette, cx: &Context<Self>) -> Div {
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
        div()
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
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .tooltip(s.dismiss)
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss(feature, cx))),
            )
    }

    fn footer(&self, p: &Palette, cx: &Context<Self>) -> Option<Div> {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let indexing = live
            .status
            .as_ref()
            .filter(|st| {
                matches!(st.state, IndexState::Indexing | IndexState::Scanning) && st.queued > 0
            })
            .map(|st| lang.plural(&s.still_indexing, st.queued));
        let has_results = !self.state.results().is_empty();
        if !(has_results || indexing.is_some()) {
            return None;
        }
        Some(
            div()
                .flex()
                .justify_between()
                .items_center()
                .gap_3()
                .px(px(16.))
                .py(px(8.))
                .text_xs()
                .text_color(p.mute)
                .border_t_1()
                .border_color(p.line)
                .child(div().flex_1().min_w_0().truncate().children(indexing))
                .when(has_results, |d| {
                    d.child(keys(&[
                        ("enter", s.open),
                        ("secondary-enter", s.reveal),
                        ("secondary-c", s.copy_path),
                    ]))
                }),
        )
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
                    // ponytail: a static bar; animate it if searches ever
                    // take long enough for it to look frozen.
                    .when(self.state.is_loading(), |d| {
                        d.child(div().h(px(2.)).bg(p.accent).opacity(0.4))
                    })
                    .children(self.body(&p, cx))
                    .children(self.footer(&p, cx)),
            )
    }
}

fn row(
    ix: usize,
    r: &SearchResult,
    selected: bool,
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
    div()
        .id(ix)
        .relative()
        .flex()
        .items_center()
        .gap_3()
        .pl(px(13.))
        .pr(px(10.))
        .py(px(7.))
        .rounded(px(4.))
        .when(selected, |d| d.bg(p.selection).child(p.accent_bar()))
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
                .when(selected && !sources.is_empty(), |d| {
                    d.child(div().text_color(p.accent).child(sources))
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
            .with_fallback(move || glyph(kind).into_any_element())
            .into_any_element(),
        None => glyph(kind).into_any_element(),
    }
}

fn glyph(kind: Kind) -> Div {
    let (label, color) = match kind {
        Kind::Pdf => ("PDF", 0xc4314b),
        Kind::Office => ("DOC", 0x185abd),
        Kind::Code => ("</>", 0x5c2d91),
        Kind::Text => ("TXT", 0x6b7280),
        Kind::Image => ("IMG", 0x6b7280),
        Kind::Other => ("•", 0x6b7280),
    };
    div()
        .flex_none()
        .size(px(32.))
        .rounded(px(3.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(color))
        .text_color(white())
        .text_size(px(9.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

/// Key hints; `keys` are GPUI keystrokes (`secondary` is Ctrl, or Cmd on
/// macOS), shown the platform's way.
fn keys(pairs: &[(&str, &'static str)]) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .children(pairs.iter().flat_map(|&(key, label)| {
            let kbd = Keystroke::parse(key)
                .ok()
                .map(|k| Kbd::new(k).into_any_element());
            let label =
                (!label.is_empty()).then(|| div().mr(px(6.)).child(label).into_any_element());
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
