//! The settings window: a sidebar and its sections (FR-9).
//!
//! Folders (FR-1, FR-11): roots come from the latest status in [`Live`], or from the database until
//! the first one (the engine sends none during its startup walk); every
//! change goes through the engine, and the next status shows it.
//!
//! What to index: file types, largest file, exclusions. Each save restarts
//! the engine, which re-walks every root, so a value is saved only when it
//! changed, and text fields save on Enter, blur or Save, never per keystroke.
//!
//! Appearance: language and the search window's background. A saved change
//! goes to the shell as [`AppEvent::Ui`], which applies it live.

use gpui_kit::component::Selectable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::config::{IndexingConfig, Language, TransparencyMode, UiConfig};
use magi_core::dto::{ErrorCode, RootStatus};
use magi_core::engine::EngineHandle;
use magi_core::host::Host;
use magi_core::platform::BackdropSupport;
use serde_json::json;

use super::{
    FILE_TYPES, MAX_SIZE_FIELD, background_label, error_text, globs_from_text,
    intensity_from_slider, kind_label, max_size_from_text, root_line, see_through_adjustable,
    slider_from_intensity, system_language, toggle_kind,
};
use crate::app::{AppEvent, Events};
use crate::i18n::{Lang, Strings};
use crate::search::view::Live;
use crate::theme::{self, Palette};

pub const SIZE: Size<Pixels> = size(px(900.), px(620.));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Folders,
    WhatToIndex,
    Appearance,
}

const SECTIONS: [Section; 3] = [Section::Folders, Section::WhatToIndex, Section::Appearance];

impl Section {
    fn title(self, s: &Strings) -> &'static str {
        match self {
            Self::Folders => s.folders,
            Self::WhatToIndex => s.what_to_index,
            Self::Appearance => s.appearance,
        }
    }
}

const LANGUAGES: [Language; 3] = [Language::System, Language::En, Language::Es];
const BACKGROUNDS: [TransparencyMode; 3] = [
    TransparencyMode::MatchSystem,
    TransparencyMode::Always,
    TransparencyMode::Never,
];

/// A dropdown entry: the label shown and the setting it stands for.
#[derive(Clone)]
struct Choice<T> {
    label: SharedString,
    value: T,
}

impl<T: Clone + PartialEq + 'static> SearchableListItem for Choice<T> {
    type Value = T;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &T {
        &self.value
    }
}

type Dropdown<T> = Entity<SelectState<Vec<Choice<T>>>>;

fn language_choices(s: &Strings) -> Vec<Choice<Language>> {
    LANGUAGES
        .iter()
        .map(|&value| Choice {
            label: match value {
                Language::System => system_language(s, Lang::current(value)).into(),
                _ => Lang::current(value).name().into(),
            },
            value,
        })
        .collect()
}

fn background_choices(s: &Strings) -> Vec<Choice<TransparencyMode>> {
    BACKGROUNDS
        .iter()
        .map(|&value| Choice {
            label: background_label(value, s).into(),
            value,
        })
        .collect()
}

pub struct SettingsView {
    host: Host,
    live: Entity<Live>,
    events: Events,
    section: Section,
    /// The saved `ui` settings; the Appearance controls show these.
    ui: UiConfig,
    /// The saved `indexing` settings.
    indexing: IndexingConfig,
    max_size: Entity<InputState>,
    excludes: Entity<TextareaState>,
    /// The language the dropdown labels are in.
    lang: Lang,
    language: Dropdown<Language>,
    background: Dropdown<TransparencyMode>,
    support: BackdropSupport,
    see_through: Entity<SliderState>,
    /// The slider takes no keys; this wrapper moves it with Left/Right (NFR-10).
    see_through_focus: FocusHandle,
    dark: bool,
    /// The roots when the window opened, shown until the first status.
    opening_roots: Option<Vec<RootStatus>>,
    /// The last change that failed, until the next one or a section switch.
    error: Option<ErrorCode>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(
        host: Host,
        live: Entity<Live>,
        events: Events,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // ponytail: this and `list_roots` below are small reads on the main
        // thread; move them to the background executor if they ever show up
        // in a frame trace.
        let config = host
            .settings()
            .inspect_err(|error| tracing::warn!(%error, "could not read the settings"))
            .unwrap_or_default();
        let (ui, indexing) = (config.ui, config.indexing);
        let max_size = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(indexing.max_file_size_mb.to_string(), window, cx);
            input
        });
        let excludes = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx).rows(6);
            input.set_value(indexing.exclude_globs.join("\n"), window, cx);
            input
        });
        let see_through = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.05)
                .default_value(slider_from_intensity(ui.transparency_intensity))
        });
        let lang = live.read(cx).lang;
        let language =
            cx.new(|cx| SelectState::new(language_choices(lang.strings()), None, window, cx));
        let background =
            cx.new(|cx| SelectState::new(background_choices(lang.strings()), None, window, cx));
        let subscriptions = vec![
            // Status events notify too; only a language switch relabels.
            cx.observe_in(&live, window, |this, live, window, cx| {
                let lang = live.read(cx).lang;
                if lang != this.lang {
                    this.relabel(lang, window, cx);
                }
                cx.notify();
            }),
            cx.subscribe_in(
                &language,
                window,
                |this, _, event: &SelectEvent<Vec<Choice<Language>>>, window, cx| {
                    if let SelectEvent::Confirm(Some(language)) = event {
                        this.save(json!({ "ui": { "language": language } }), window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &background,
                window,
                |this, _, event: &SelectEvent<Vec<Choice<TransparencyMode>>>, window, cx| {
                    if let SelectEvent::Confirm(Some(mode)) = event {
                        this.save(json!({ "ui": { "transparency_mode": mode } }), window, cx);
                    }
                },
            ),
            // Saved on release: dragging would write the config at every step.
            cx.subscribe_in(
                &see_through,
                window,
                |this, _, event: &SliderEvent, window, cx| {
                    if let SliderEvent::Release(value) = event {
                        this.save_intensity(value.end(), window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &max_size,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        let text = input.read(cx).value();
                        this.save_max_size(&text, window, cx);
                    }
                },
            ),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_appearance(window, cx);
            }),
        ];
        let opening_roots = host
            .list_roots()
            .inspect_err(|error| tracing::warn!(%error, "could not list the roots"))
            .ok();
        let mut view = Self {
            host,
            live,
            events,
            section: Section::Folders,
            ui,
            indexing,
            max_size,
            excludes,
            lang,
            language,
            background,
            support: magi_core::platform::backdrop_support(),
            see_through,
            see_through_focus: cx.focus_handle().tab_stop(true),
            dark: false,
            opening_roots,
            error: None,
            _subscriptions: subscriptions,
        };
        view.sync_appearance(window, cx);
        view.sync_controls(window, cx);
        view
    }

    /// Makes the Appearance controls show the saved settings.
    fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (language, mode) = (self.ui.language, self.ui.transparency_mode);
        let see_through = slider_from_intensity(self.ui.transparency_intensity);
        self.language
            .update(cx, |d, cx| d.set_selected_value(&language, window, cx));
        self.background
            .update(cx, |d, cx| d.set_selected_value(&mode, window, cx));
        self.see_through
            .update(cx, |slider, cx| slider.set_value(see_through, window, cx));
    }

    /// A live language switch: the window title and the dropdown labels.
    fn relabel(&mut self, lang: Lang, window: &mut Window, cx: &mut Context<Self>) {
        let s = lang.strings();
        self.lang = lang;
        window.set_window_title(s.settings);
        self.language
            .update(cx, |d, cx| d.set_items(language_choices(s), window, cx));
        self.background
            .update(cx, |d, cx| d.set_items(background_choices(s), window, cx));
        self.sync_controls(window, cx);
    }

    fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.live.read(cx).theme.clone();
        self.dark = theme::sync(&theme, window, cx);
        cx.notify();
    }

    /// Runs a root change on the background executor and keeps its error.
    fn change_root(
        &mut self,
        change: impl FnOnce(EngineHandle) -> magi_core::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { host.engine().and_then(change) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.error = done.err().map(|error| {
                    tracing::warn!(%error, "a folder change failed");
                    ErrorCode::from(&error)
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves a settings patch; on success the shell applies the new `ui`.
    /// Public for the headless tests.
    pub fn save(&mut self, patch: serde_json::Value, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { host.update_settings(&patch) })
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                match saved {
                    Ok(config) => {
                        view.error = None;
                        view.indexing = config.indexing;
                        view.ui = config.ui.clone();
                        let _ = view.events.try_send(AppEvent::Ui(config.ui));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a settings change failed");
                        view.error = Some(ErrorCode::from(&error));
                    }
                }
                // The controls show what is saved: the rounded amount, or the
                // old values when the save failed.
                view.sync_controls(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn save_max_size(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        match max_size_from_text(text) {
            Some(mb) if mb == self.indexing.max_file_size_mb => {}
            Some(mb) => self.save(
                json!({ "indexing": { "max_file_size_mb": mb } }),
                window,
                cx,
            ),
            None => {
                self.error = Some(ErrorCode::InvalidSetting {
                    field: MAX_SIZE_FIELD.into(),
                });
                cx.notify();
            }
        }
    }

    fn save_excludes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let globs = globs_from_text(&self.excludes.read(cx).value());
        if globs != self.indexing.exclude_globs {
            self.save(
                json!({ "indexing": { "exclude_globs": globs } }),
                window,
                cx,
            );
        }
    }

    fn save_intensity(&mut self, see_through: f32, window: &mut Window, cx: &mut Context<Self>) {
        let intensity = intensity_from_slider(see_through);
        self.save(
            json!({ "ui": { "transparency_intensity": intensity } }),
            window,
            cx,
        );
    }

    fn nudge_see_through(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let step = match event.keystroke.key.as_str() {
            "left" => -0.05,
            "right" => 0.05,
            _ => return cx.propagate(),
        };
        if !see_through_adjustable(self.ui.transparency_mode, self.support) {
            return;
        }
        let value = (self.see_through.read(cx).value().end() + step).clamp(0., 1.);
        self.see_through
            .update(cx, |slider, cx| slider.set_value(value, window, cx));
        self.save_intensity(value, window, cx);
    }

    /// The native folder picker (FR-1), then `add_root`.
    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| match picked.await {
            Ok(Ok(Some(mut paths))) => {
                if let Some(path) = paths.pop() {
                    let _ = this.update(cx, |view, cx| {
                        view.change_root(move |engine| engine.add_root(&path).map(drop), cx);
                    });
                }
            }
            Ok(Err(error)) => tracing::warn!(%error, "the folder picker failed"),
            _ => {}
        })
        .detach();
    }

    fn error_line(&self, s: &Strings, p: &Palette) -> Option<Div> {
        self.error
            .as_ref()
            .map(|error| div().mb_3().text_color(p.warn).child(error_text(error, s)))
    }

    fn folders(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let row = || row(p);
        let roots = live
            .status
            .as_ref()
            .map(|status| &status.roots)
            .or(self.opening_roots.as_ref());
        // Nothing when the roots are unknown: an empty list would read "no folders".
        let list = roots.map(|roots| {
            let group = div().flex().flex_col().gap(px(2.)).mb_3();
            if roots.is_empty() {
                return group.child(row().text_color(p.mute).child(s.no_folders));
            }
            group.children(
                roots
                    .iter()
                    .map(|root| self.root_row(root, row(), s, p, cx)),
            )
        });
        div()
            .flex()
            .flex_col()
            .items_start()
            .child(div().w_full().children(list))
            .children(self.error_line(s, p))
            .child(
                Button::new("add-folder")
                    .label(s.add_folder)
                    .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
            )
    }

    fn root_row(
        &self,
        root: &RootStatus,
        row: Div,
        s: &Strings,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Div {
        let (line, problem) = root_line(root, s);
        let id = root.id;
        row.child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_middle()
                        .child(root.path.clone()),
                )
                .child(
                    div()
                        .mt(px(2.))
                        .text_xs()
                        .text_color(if problem { p.warn } else { p.mute })
                        .child(line),
                ),
        )
        .child(
            Switch::new(("root-enabled", id as u64))
                .checked(root.enabled)
                .color(p.accent)
                .accessibility_label(root.path.clone())
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.change_root(move |engine| engine.set_root_enabled(id, on), cx);
                })),
        )
        .child(
            Button::new(("remove-root", id as u64))
                .label(s.remove)
                .accessibility_label(format!("{} {}", s.remove, root.path))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change_root(move |engine| engine.remove_root(id), cx);
                })),
        )
    }

    fn what_to_index(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let s = self.live.read(cx).lang.strings();
        let labelled = |label: &'static str, note: &'static str| {
            div()
                .flex_1()
                .min_w_0()
                .child(label)
                .child(div().mt(px(2.)).text_xs().text_color(p.mute).child(note))
        };
        let kinds = FILE_TYPES.into_iter().map(|kind| {
            let on = self.indexing.file_types.contains(&kind);
            Checkbox::new(("kind", kind as usize))
                .label(kind_label(kind, s))
                .checked(on)
                .on_click(cx.listener(move |this, &on: &bool, window, cx| {
                    let types = toggle_kind(&this.indexing.file_types, kind, on);
                    this.save(json!({ "indexing": { "file_types": types } }), window, cx);
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .mb_3()
            .child(
                row(p)
                    .flex_col()
                    .items_start()
                    .gap_3()
                    .child(labelled(s.file_types, s.file_types_note))
                    .child(div().flex().flex_wrap().gap_4().children(kinds)),
            )
            .child(
                row(p).child(labelled(s.max_size, s.max_size_note)).child(
                    div().flex_none().w(px(140.)).child(
                        Input::new(&self.max_size).suffix(div().text_color(p.mute).child("MB")),
                    ),
                ),
            )
            .child(
                row(p)
                    .flex_col()
                    .items_stretch()
                    .gap_3()
                    .child(labelled(s.excludes, s.excludes_note))
                    .child(Textarea::new(&self.excludes))
                    .child(
                        div().child(Button::new("save-excludes").label(s.save).on_click(
                            cx.listener(|this, _, window, cx| this.save_excludes(window, cx)),
                        )),
                    ),
            )
            .children(self.error_line(s, p))
    }

    fn appearance(&self, p: &Palette, window: &Window, cx: &Context<Self>) -> Div {
        let s = self.live.read(cx).lang.strings();
        let adjustable = see_through_adjustable(self.ui.transparency_mode, self.support);
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .mb_3()
            .child(choice(
                s.language,
                Select::new(&self.language).accessibility_label(s.language),
                p,
            ))
            .child(choice(
                s.window_background,
                Select::new(&self.background).accessibility_label(s.window_background),
                p,
            ))
            .child(
                row(p)
                    .child(div().flex_1().min_w_0().child(s.see_through_amount).when(
                        !adjustable,
                        |d| {
                            d.child(
                                div()
                                    .mt(px(2.))
                                    .text_xs()
                                    .text_color(p.mute)
                                    .child(s.see_through_off),
                            )
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_3()
                            .text_xs()
                            .text_color(p.mute)
                            .child(s.more_solid)
                            .child(
                                div()
                                    .id("see-through")
                                    .track_focus(&self.see_through_focus)
                                    .on_key_down(cx.listener(Self::nudge_see_through))
                                    .w(px(172.))
                                    .px(px(6.))
                                    .py_1()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(if self.see_through_focus.is_focused(window) {
                                        p.accent
                                    } else {
                                        transparent_black()
                                    })
                                    .child(Slider::new(&self.see_through).disabled(!adjustable)),
                            )
                            .child(s.more_transparent),
                    ),
            )
            .children(self.error_line(s, p))
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        let nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(220.))
            .gap(px(2.))
            .px_2()
            .py_4()
            .child(
                div()
                    .px_3()
                    .pb_3()
                    .text_xs()
                    .text_color(p.mute)
                    .child(s.settings),
            )
            .children(SECTIONS.iter().enumerate().map(|(ix, &section)| {
                let selected = section == self.section;
                // A button, so Tab and Enter reach every section.
                Button::new(("section", ix))
                    .ghost()
                    .selected(selected)
                    .w_full()
                    .relative()
                    .px_3()
                    .when(selected, |b| b.bg(p.card).child(p.accent_bar()))
                    // The content row centers a label; a filling child sits left.
                    .child(div().flex_1().child(section.title(s)))
                    .accessibility_label(section.title(s))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.section = section;
                        this.error = None;
                        cx.notify();
                    }))
            }));
        let body = match self.section {
            Section::Folders => self.folders(&p, cx),
            Section::WhatToIndex => self.what_to_index(&p, cx),
            Section::Appearance => self.appearance(&p, window, cx),
        };
        div()
            .size_full()
            .flex()
            .bg(p.solid)
            .text_color(p.ink)
            .text_size(px(14.))
            .child(nav)
            .child(
                div()
                    .id("pane")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .px(px(28.))
                    .py(px(24.))
                    .child(heading(self.section.title(s)))
                    .child(body),
            )
    }
}

/// A labelled setting with a dropdown on the right.
fn choice(label: &'static str, select: impl IntoElement, p: &Palette) -> Div {
    row(p)
        .child(div().flex_1().min_w_0().child(label))
        .child(div().flex_none().w(px(260.)).child(select))
}

fn row(p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_4()
        .px_4()
        .py(px(14.))
        .rounded(px(4.))
        .bg(p.card)
}

fn heading(text: &'static str) -> Div {
    div()
        .mb(px(18.))
        .text_size(px(26.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}
