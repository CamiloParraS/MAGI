//! The settings window: a sidebar and its sections (FR-9), laid out as the
//! variant A mockup (`mockup/index.html`).
//!
//! Folders (FR-1, FR-11): roots come from the latest status in [`Live`], or from the database until
//! the first one (the engine sends none during its startup walk); every
//! change goes through the engine, and the next status shows it. Below them,
//! what to index: file types, largest file, battery, exclusions. Each of
//! those saves restarts the engine, which re-walks every root, so a value is
//! saved only when it changed, and text fields save on Enter, blur or Save,
//! never per keystroke.
//!
//! Search features (ADR-0010): each feature's switch, download and backfill,
//! from [`Live::features`].
//!
//! General and Appearance: `ui` settings. A saved change goes to the shell
//! as [`AppEvent::Ui`], which applies it live.
//!
//! Index: counts from the status, the files that could not be read, and
//! clearing the index.

use gpui_kit::assets::IconName;
use gpui_kit::component::Selectable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::config::{IndexingConfig, Language, TransparencyMode, UiConfig};
use magi_core::discovery::Kind;
use magi_core::dto::{ErrorCode, FeatureStatus, FileError, Install, RootStatus};
use magi_core::features::Feature;
use magi_core::host::Host;
use magi_core::platform::BackdropSupport;
use serde_json::json;

use super::{
    FILE_TYPES, FeatureAction, MAX_SIZE_FIELD, background_label, download_error_label, error_text,
    feature_action, file_error_label, globs_from_text, hotkey_keystroke, images_needed,
    intensity_from_slider, kind_examples, kind_label, max_size_from_text, result_counts, root_line,
    root_state, see_through_adjustable, slider_from_intensity, system_language, toggle_kind,
};
use crate::app::{AppEvent, Events};
use crate::i18n::{Lang, Strings};
use crate::search::view::{Live, turn_on};
use crate::theme::{self, Backdrop, Palette, Tone};

pub const SIZE: Size<Pixels> = size(px(900.), px(620.));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Folders,
    Features,
    General,
    Appearance,
    Index,
}

const SECTIONS: [Section; 5] = [
    Section::Folders,
    Section::Features,
    Section::General,
    Section::Appearance,
    Section::Index,
];

impl Section {
    fn title(self, s: &Strings) -> &'static str {
        match self {
            Self::Folders => s.folders,
            Self::Features => s.search_features,
            Self::General => s.general,
            Self::Appearance => s.appearance,
            Self::Index => s.index,
        }
    }

    /// The sidebar's label: shorter where the title does not fit.
    fn nav_title(self, s: &Strings) -> &'static str {
        match self {
            Self::Features => s.search_features_nav,
            _ => self.title(s),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Folders => IconName::Folder,
            Self::Features => IconName::Sparkles,
            Self::General => IconName::Settings,
            Self::Appearance => IconName::Palette,
            Self::Index => IconName::ChartColumn,
        }
    }
}

const LANGUAGES: [Language; 3] = [Language::System, Language::En, Language::Es];
const BACKGROUNDS: [TransparencyMode; 3] = [
    TransparencyMode::MatchSystem,
    TransparencyMode::Always,
    TransparencyMode::Never,
];
/// `ui.theme` values; anything else follows the OS like `system`.
const THEMES: [&str; 3] = ["system", "light", "dark"];
/// How many unreadable files the Index section lists.
const ERRORS_SHOWN: u32 = 50;

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

fn theme_choices(s: &Strings) -> Vec<Choice<&'static str>> {
    THEMES
        .iter()
        .zip([s.background_match_system, s.theme_light, s.theme_dark])
        .map(|(&value, label)| Choice {
            label: label.into(),
            value,
        })
        .collect()
}

fn count_choices(lang: Lang, current: u32) -> Vec<Choice<u32>> {
    result_counts(current)
        .into_iter()
        .map(|n| Choice {
            label: lang.number(n.into()).into(),
            value: n,
        })
        .collect()
}

/// Where a failed change is shown: inside the box it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Hotkey,
    AddFolder,
    Root(i64),
    FileTypes,
    MaxSize,
    Battery,
    Excludes,
    Feature(Feature),
    LaunchAtLogin,
    Language,
    Results,
    Theme,
    Background,
    SeeThrough,
    Unreadable,
    Clear,
    Quit,
}

pub struct SettingsView {
    host: Host,
    live: Entity<Live>,
    events: Events,
    section: Section,
    /// The saved `ui` settings; the General and Appearance controls show
    /// these.
    ui: UiConfig,
    /// The saved `indexing` settings.
    indexing: IndexingConfig,
    max_size: Entity<InputState>,
    excludes: Entity<TextareaState>,
    /// The language the dropdown labels are in.
    lang: Lang,
    /// The `ui.theme` the window is drawn in.
    theme: String,
    language: Dropdown<Language>,
    results: Dropdown<u32>,
    theme_choice: Dropdown<&'static str>,
    background: Dropdown<TransparencyMode>,
    support: BackdropSupport,
    see_through: Entity<SliderState>,
    /// The slider takes no keys; this wrapper moves it with Left/Right (NFR-10).
    see_through_focus: FocusHandle,
    /// Focused while the shortcut recorder waits for keys.
    recorder: FocusHandle,
    recording: bool,
    /// Back to the top on every section change.
    pane_scroll: ScrollHandle,
    dark: bool,
    /// The roots when the window opened, shown until the first status.
    opening_roots: Option<Vec<RootStatus>>,
    /// Read when the Index section opens, after its actions, and when the
    /// error count changes.
    unreadable: Option<Vec<FileError>>,
    /// The status's error count when `unreadable` was read.
    errors_listed: Option<u64>,
    /// The last change that failed and where, until the next change or a
    /// section switch.
    error: Option<(Field, ErrorCode)>,
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
        let s = lang.strings();
        let language = cx.new(|cx| SelectState::new(language_choices(s), None, window, cx));
        let results =
            cx.new(|cx| SelectState::new(count_choices(lang, ui.max_results), None, window, cx));
        let theme_choice = cx.new(|cx| SelectState::new(theme_choices(s), None, window, cx));
        let background = cx.new(|cx| SelectState::new(background_choices(s), None, window, cx));
        let subscriptions = vec![
            // Status events notify too; only a language or theme switch
            // redraws more than the window.
            cx.observe_in(&live, window, |this, live, window, cx| {
                let (lang, theme) = (live.read(cx).lang, live.read(cx).theme.clone());
                if lang != this.lang {
                    this.relabel(lang, window, cx);
                }
                if theme != this.theme {
                    this.sync_appearance(window, cx);
                }
                // A file failed or a retry worked: the list follows.
                let errors = live.read(cx).status.as_ref().map(|st| st.errors);
                if this.section == Section::Index && errors != this.errors_listed {
                    this.load_unreadable(cx);
                }
                cx.notify();
            }),
            Self::save_on_confirm(
                &language,
                Field::Language,
                |v| json!({ "ui": { "language": v } }),
                window,
                cx,
            ),
            Self::save_on_confirm(
                &results,
                Field::Results,
                |v| json!({ "ui": { "max_results": v } }),
                window,
                cx,
            ),
            Self::save_on_confirm(
                &theme_choice,
                Field::Theme,
                |v| json!({ "ui": { "theme": v } }),
                window,
                cx,
            ),
            Self::save_on_confirm(
                &background,
                Field::Background,
                |v| json!({ "ui": { "transparency_mode": v } }),
                window,
                cx,
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
            // Save lights up as soon as the patterns differ from the saved ones.
            cx.subscribe(&excludes, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_appearance(window, cx);
            }),
            // "Transparency effects" is switched in the OS settings, so it
            // is read again when the user comes back.
            cx.observe_window_activation(window, |this, window, cx| {
                let support = magi_core::platform::backdrop_support();
                if window.is_window_active() && support != this.support {
                    this.support = support;
                    this.sync_controls(window, cx);
                    cx.notify();
                }
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
            theme: String::new(),
            language,
            results,
            theme_choice,
            background,
            support: magi_core::platform::backdrop_support(),
            see_through,
            see_through_focus: cx.focus_handle().tab_stop(true),
            recorder: cx.focus_handle(),
            recording: false,
            pane_scroll: ScrollHandle::new(),
            dark: false,
            opening_roots,
            unreadable: None,
            errors_listed: None,
            error: None,
            _subscriptions: subscriptions,
        };
        view.sync_appearance(window, cx);
        view.sync_controls(window, cx);
        view
    }

    /// Saves `patch(choice)` when a dropdown confirms a choice.
    fn save_on_confirm<T>(
        dropdown: &Dropdown<T>,
        field: Field,
        patch: fn(&T) -> serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription
    where
        T: Clone + PartialEq + 'static,
    {
        cx.subscribe_in(
            dropdown,
            window,
            move |this, _, event: &SelectEvent<Vec<Choice<T>>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.save(field, patch(value), window, cx);
                }
            },
        )
    }

    /// The window's background, from the saved settings (like the search
    /// window's).
    fn backdrop(&self) -> Backdrop {
        theme::backdrop(
            self.ui.transparency_mode,
            self.ui.transparency_intensity,
            self.support,
        )
    }

    /// Makes the General and Appearance controls and the window background
    /// show the saved settings.
    fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_background_appearance(theme::window_background(self.backdrop()));
        let (language, mode, count) = (
            self.ui.language,
            self.ui.transparency_mode,
            self.ui.max_results,
        );
        let theme = THEMES
            .into_iter()
            .find(|&t| t == self.ui.theme)
            .unwrap_or(THEMES[0]);
        let see_through = slider_from_intensity(self.ui.transparency_intensity);
        let lang = self.lang;
        self.language
            .update(cx, |d, cx| d.set_selected_value(&language, window, cx));
        self.results.update(cx, |d, cx| {
            d.set_items(count_choices(lang, count), window, cx);
            d.set_selected_value(&count, window, cx);
        });
        self.theme_choice
            .update(cx, |d, cx| d.set_selected_value(&theme, window, cx));
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
        self.theme_choice
            .update(cx, |d, cx| d.set_items(theme_choices(s), window, cx));
        self.background
            .update(cx, |d, cx| d.set_items(background_choices(s), window, cx));
        self.sync_controls(window, cx);
    }

    fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme = self.live.read(cx).theme.clone();
        self.dark = theme::sync(&self.theme, window, cx);
        cx.notify();
    }

    /// Runs a change on the background executor and keeps its error.
    fn run(
        &mut self,
        field: Field,
        job: impl FnOnce(&Host) -> magi_core::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { job(&host) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.error = done.err().map(|error| {
                    tracing::warn!(%error, ?field, "a change failed");
                    (field, ErrorCode::from(&error))
                });
                if view.section == Section::Index {
                    view.load_unreadable(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A failed read leaves `None`, which shows no list rather than "every
    /// file was read".
    fn load_unreadable(&mut self, cx: &mut Context<Self>) {
        self.errors_listed = self.live.read(cx).status.as_ref().map(|st| st.errors);
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move { host.list_errors(ERRORS_SHOWN) })
                .await
                .inspect_err(|error| tracing::warn!(%error, "could not list the unreadable files"))
                .ok();
            let _ = this.update(cx, |view, cx| {
                view.unreadable = listed;
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves a settings patch; on success the shell applies the new `ui`, on
    /// failure `field`'s box shows why. Public for the headless tests.
    pub fn save(
        &mut self,
        field: Field,
        patch: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                        view.error = Some((field, ErrorCode::from(&error)));
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
                Field::MaxSize,
                json!({ "indexing": { "max_file_size_mb": mb } }),
                window,
                cx,
            ),
            None => {
                self.error = Some((
                    Field::MaxSize,
                    ErrorCode::InvalidSetting {
                        field: MAX_SIZE_FIELD.into(),
                    },
                ));
                cx.notify();
            }
        }
    }

    fn save_excludes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let globs = globs_from_text(&self.excludes.read(cx).value());
        if globs != self.indexing.exclude_globs {
            self.save(
                Field::Excludes,
                json!({ "indexing": { "exclude_globs": globs } }),
                window,
                cx,
            );
        }
    }

    fn save_intensity(&mut self, see_through: f32, window: &mut Window, cx: &mut Context<Self>) {
        let intensity = intensity_from_slider(see_through);
        self.save(
            Field::SeeThrough,
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

    /// The recorder's keys: Esc cancels; a usable shortcut is registered by
    /// the shell first, and saved only if that worked. Anything else (a
    /// modifier still held, plain typing) keeps waiting.
    fn record_hotkey(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.record(&event.keystroke, window, cx);
    }

    pub fn record(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        if k.key == "escape" && !k.modifiers.modified() {
            self.recording = false;
            return cx.notify();
        }
        let Some(spec) = crate::hotkey::from_keystroke(k) else {
            return;
        };
        self.recording = false;
        cx.notify();
        let events = self.events.clone();
        cx.spawn_in(window, async move |this, cx| {
            // ponytail: a failed save leaves the new hotkey registered until
            // restart; re-register the old one if that ever matters.
            if crate::app::register_hotkey(&events, spec.clone()).await {
                let _ = this.update_in(cx, |view, window, cx| {
                    if spec != view.ui.hotkey {
                        view.save(
                            Field::Hotkey,
                            json!({ "ui": { "hotkey": spec } }),
                            window,
                            cx,
                        );
                    }
                });
            }
        })
        .detach();
    }

    /// The native folder picker (FR-1), then `add_root`.
    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let picked = pick_folder(cx);
        cx.spawn(async move |this, cx| {
            if let Some(path) = picked.await {
                let _ = this.update(cx, |view, cx| {
                    view.run(
                        Field::AddFolder,
                        move |host| host.engine()?.add_root(&path).map(drop),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    /// Asks first: clearing forgets every indexed file.
    fn confirm_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let indexed = live.status.as_ref().map_or(0, |st| st.indexed);
        let body: SharedString = lang.plural(&s.clear_confirm_body, indexed).into();
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title(s.clear_confirm_title)
                .description(body.clone())
                .ok_text(s.clear_index)
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(s.cancel)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = view.update(cx, |view, cx| {
                        view.run(Field::Clear, |host| host.clear_index(), cx);
                    });
                    true
                })
        });
    }

    /// Asks first: removing forgets the folder's index, and the folders
    /// collapsed into it when it was added go with it.
    fn confirm_remove(&mut self, id: i64, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.live.read(cx).lang.strings();
        let body: SharedString = s.remove_confirm_body.replace("{path}", path).into();
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title(s.remove_confirm_title)
                .description(body.clone())
                .ok_text(s.remove)
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(s.cancel)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = view.update(cx, |view, cx| {
                        view.run(
                            Field::Root(id),
                            move |host| host.engine()?.remove_root(id),
                            cx,
                        );
                    });
                    true
                })
        });
    }

    /// The box showing a failed change, if any.
    pub fn failed(&self) -> Option<Field> {
        self.error.as_ref().map(|(field, _)| *field)
    }

    fn has_problem(&self, field: Field) -> bool {
        self.error.as_ref().is_some_and(|(f, _)| *f == field)
    }

    /// The failed change's message, for the last line of `field`'s box.
    fn problem(&self, field: Field, s: &Strings, p: &Palette) -> Option<Div> {
        self.error
            .as_ref()
            .filter(|(f, _)| *f == field)
            .map(|(_, error)| note(p).text_color(p.warn).child(error_text(error, s)))
    }

    /// A settings box; a failed change in it gives it a warning border.
    fn card(&self, field: Field, p: &Palette) -> Div {
        card(p).when(self.has_problem(field), |d| d.border_color(p.warn))
    }

    /// A setting's name and note, and its problem if it has one.
    fn labelled(
        &self,
        label: &'static str,
        note_text: Option<&'static str>,
        field: Field,
        s: &Strings,
        p: &Palette,
    ) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .child(label)
            .children(note_text.map(|text| note(p).child(text)))
            .children(self.problem(field, s, p))
    }

    /// One setting in its box: an optional icon, its name and note, and
    /// its control.
    #[allow(clippy::too_many_arguments)]
    fn setting(
        &self,
        field: Field,
        icon: Option<IconName>,
        label: &'static str,
        note_text: Option<&'static str>,
        control: impl IntoElement,
        s: &Strings,
        p: &Palette,
    ) -> Div {
        self.card(field, p)
            .children(icon.map(|i| Icon::new(i).size(px(16.)).text_color(p.mute)))
            .child(self.labelled(label, note_text, field, s, p))
            .child(div().flex_none().child(control))
    }

    fn folders(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let roots = live
            .status
            .as_ref()
            .map(|status| &status.roots)
            .or(self.opening_roots.as_ref());
        // Nothing when the roots are unknown: an empty list would read "no folders".
        let list = roots.map(|roots| {
            if roots.is_empty() {
                return stack().child(card(p).text_color(p.mute).child(s.no_folders));
            }
            stack().children(roots.iter().map(|root| self.root_row(root, s, p, cx)))
        });
        // An add that failed has no row yet: its own box says why.
        let add_problem = self.problem(Field::AddFolder, s, p).map(|problem| {
            self.card(Field::AddFolder, p)
                .child(div().flex_1().min_w_0().child(problem))
        });
        div()
            .child(
                div()
                    .flex()
                    .items_center()
                    .mb(px(8.))
                    .child(div().flex_1().child(subheading(s.searched_folders, p)))
                    .child(
                        Button::new("add-folder")
                            .cursor_pointer()
                            .primary()
                            .small()
                            .icon(IconName::Plus)
                            .label(s.add_folder)
                            .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
                    ),
            )
            .child(stack().children(list).children(add_problem))
            .child(
                div()
                    .mt(px(22.))
                    .mb(px(8.))
                    .child(subheading(s.what_to_index, p)),
            )
            .child(self.what_to_index(s, p, cx))
    }

    fn root_row(&self, root: &RootStatus, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let (badge, tone, _) = root_state(root, s);
        let line = root_line(root, s, self.live.read(cx).lang);
        let id = root.id;
        let icon = match tone {
            Tone::Warn | Tone::Err => Icon::new(IconName::TriangleAlert).text_color(p.tone(tone)),
            _ => Icon::new(IconName::Folder).text_color(p.mute),
        };
        self.card(Field::Root(id), p)
            .child(icon.size(px(16.)))
            .child(
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
                    .children(line.map(|line| note(p).child(line)))
                    .children(self.problem(Field::Root(id), s, p)),
            )
            // A changed status fades in, so the change is seen.
            .children(badge.map(|badge| {
                theme::fade_in(
                    p.status(badge, tone),
                    SharedString::from(format!("root-status-{id}-{badge}")),
                )
            }))
            .child(
                Switch::new(("root-enabled", id as u64))
                    .cursor_pointer()
                    .checked(root.enabled)
                    .color(p.accent)
                    .accessibility_label(root.path.clone())
                    .on_click(cx.listener(move |this, &on: &bool, _, cx| {
                        this.run(
                            Field::Root(id),
                            move |host| host.engine()?.set_root_enabled(id, on),
                            cx,
                        );
                    })),
            )
            .child(
                Button::new(("remove-root", id as u64))
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .tooltip(s.remove)
                    .accessibility_label(format!("{} {}", s.remove, root.path))
                    .on_click({
                        let path = root.path.clone();
                        cx.listener(move |this, _, window, cx| {
                            this.confirm_remove(id, &path, window, cx)
                        })
                    }),
            )
    }

    fn what_to_index(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let types = &self.indexing.file_types;
        let images_needed = images_needed(types, &self.live.read(cx).features);
        let kinds = FILE_TYPES.into_iter().map(|kind| {
            let on = types.contains(&kind);
            let checkbox = Checkbox::new(("kind", kind as usize))
                .cursor_pointer()
                .label(kind_label(kind, s))
                .checked(on)
                .on_click(cx.listener(move |this, &on: &bool, window, cx| {
                    let types = toggle_kind(&this.indexing.file_types, kind, on);
                    this.save(
                        Field::FileTypes,
                        json!({ "indexing": { "file_types": types } }),
                        window,
                        cx,
                    );
                }));
            let warning = (kind == Kind::Image && images_needed)
                .then(|| note(p).text_color(p.warn).child(s.images_needed));
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(theme::kind_glyph(kind, px(22.)))
                .child(
                    div()
                        .min_w_0()
                        .child(checkbox)
                        .children(kind_examples(kind).map(|e| note(p).child(e)))
                        .children(warning),
                )
        });
        let none = types
            .is_empty()
            .then(|| note(p).mt_0().text_color(p.warn).child(s.kind_none));
        let battery = self.indexing.pause_on_battery;
        let excludes_edited =
            globs_from_text(&self.excludes.read(cx).value()) != self.indexing.exclude_globs;
        stack()
            .child(
                self.card(Field::FileTypes, p)
                    .flex_col()
                    .items_start()
                    .gap_3()
                    .child(self.labelled(
                        s.file_types,
                        Some(s.file_types_note),
                        Field::FileTypes,
                        s,
                        p,
                    ))
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .w_full()
                            .gap_x_6()
                            .gap_y_3()
                            .children(kinds),
                    )
                    .children(none),
            )
            .child(
                self.setting(
                    Field::MaxSize,
                    None,
                    s.max_size,
                    Some(s.max_size_note),
                    div().w(px(140.)).child(
                        Input::new(&self.max_size)
                            .bg(p.solid)
                            .suffix(div().text_color(p.mute).child("MB")),
                    ),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Battery,
                    None,
                    s.pause_on_battery,
                    Some(s.pause_on_battery_note),
                    Switch::new("battery")
                        .cursor_pointer()
                        .checked(battery)
                        .color(p.accent)
                        .accessibility_label(s.pause_on_battery)
                        .on_click(cx.listener(|this, &on: &bool, window, cx| {
                            this.save(
                                Field::Battery,
                                json!({ "indexing": { "pause_on_battery": on } }),
                                window,
                                cx,
                            );
                        })),
                    s,
                    p,
                ),
            )
            .child(
                self.card(Field::Excludes, p)
                    .flex_col()
                    .items_stretch()
                    .gap_3()
                    .child(self.labelled(s.excludes, Some(s.excludes_note), Field::Excludes, s, p))
                    // `rows` does not size the box; a height does.
                    .child(Textarea::new(&self.excludes).h(px(150.)).bg(p.solid))
                    .child(
                        div().flex().justify_end().child(
                            Button::new("save-excludes")
                                .when(excludes_edited, |b| b.cursor_pointer())
                                .primary()
                                .label(s.save)
                                .disabled(!excludes_edited)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_excludes(window, cx)
                                })),
                        ),
                    ),
            )
    }

    fn features(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        div()
            .child(note(p).mt_0().mb(px(14.)).child(s.features_note))
            .child(
                stack().children(
                    live.features
                        .iter()
                        .map(|f| self.feature_row(f, live.lang, p, cx)),
                ),
            )
            .child(note(p).mt(px(10.)).child(s.features_footnote))
    }

    fn feature_row(&self, f: &FeatureStatus, lang: Lang, p: &Palette, cx: &Context<Self>) -> Div {
        let s = lang.strings();
        let (feature, field) = (f.feature, Field::Feature(f.feature));
        let text = s.feature(feature);
        let progress = |id: &'static str, done: u64, total: u64, label: String| {
            div()
                .mt(px(6.))
                .child(
                    Progress::new((id, feature as usize))
                        .color(p.accent)
                        .value(done as f32 * 100. / total.max(1) as f32),
                )
                .child(note(p).child(label))
        };
        let state = match &f.install {
            Install::Installed { size_bytes } => note(p)
                .text_color(p.ok)
                .child(s.installed.replace("{size}", &lang.size(*size_bytes)))
                .into_any_element(),
            Install::NotInstalled => note(p)
                .child(
                    s.not_downloaded
                        .replace("{size}", &lang.size(f.download_size)),
                )
                .into_any_element(),
            Install::Downloading { bytes, total } => progress(
                "download",
                *bytes,
                *total,
                s.feature_downloading
                    .replace("{done}", &lang.size(*bytes))
                    .replace("{total}", &lang.size(*total)),
            )
            .into_any_element(),
            Install::Failed { code } => div()
                .mt(px(4.))
                .flex()
                .items_center()
                .gap_2()
                .child(p.badge(s.download_failed, Tone::Err))
                .child(note(p).mt_0().child(download_error_label(*code, s)))
                .into_any_element(),
        };
        let backfill = f.backfill.filter(|b| b.total > b.done).map(|b| {
            progress(
                "backfill",
                b.done,
                b.total,
                lang.plural(&s.feature_updating, b.total - b.done),
            )
        });
        let action = feature_action(f);
        let button = match action {
            FeatureAction::TurnOn => Some(
                Button::new(("turn-on", feature as usize))
                    .cursor_pointer()
                    .primary()
                    .small()
                    .label(s.turn_on.replace("{size}", &lang.size(f.download_size))),
            ),
            FeatureAction::Retry => Some(
                Button::new(("retry", feature as usize))
                    .cursor_pointer()
                    .small()
                    .label(s.try_again),
            ),
            FeatureAction::Remove => Some(
                Button::new(("remove", feature as usize))
                    .cursor_pointer()
                    .danger()
                    .small()
                    .label(s.remove_download),
            ),
            FeatureAction::Nothing => None,
        }
        .map(|b| {
            b.on_click(cx.listener(move |this, _, _, cx| {
                this.run(
                    field,
                    move |host| match action {
                        FeatureAction::Remove => host.features().remove_download(feature),
                        _ => turn_on(host, feature),
                    },
                    cx,
                );
            }))
        });
        // Without a download a switch could only turn it off: shown once it
        // is wanted (a cancelled download), hidden before.
        let switch = (action != FeatureAction::TurnOn || f.enabled).then(|| {
            Switch::new(("feature", feature as usize))
                .cursor_pointer()
                .checked(f.enabled)
                .color(p.accent)
                .accessibility_label(text.name)
                .on_click(cx.listener(move |this, &on: &bool, _, cx| {
                    this.run(field, move |host| host.set_feature_enabled(feature, on), cx);
                }))
        });
        self.card(field, p)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::MEDIUM).child(text.name))
                    .child(note(p).child(text.pitch))
                    .children(
                        (images_needed(&self.indexing.file_types, std::slice::from_ref(f)))
                            .then(|| note(p).text_color(p.warn).child(s.images_off)),
                    )
                    .child(state)
                    .children(backfill)
                    .children(self.problem(field, s, p)),
            )
            .children(button)
            .children(switch)
    }

    fn general(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let kbd = |spec: &str| Keystroke::parse(&hotkey_keystroke(spec)).ok().map(Kbd::new);
        let recording = self.recording;
        let conflict = live.hotkey_conflict.as_deref().map(|taken| {
            card(p)
                .border_color(p.warn)
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(16.))
                        .text_color(p.warn),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_color(p.warn).child(s.shortcut_taken))
                        .child(note(p).child(s.shortcut_taken_note)),
                )
                .children(kbd(taken))
        });
        stack()
            .child(
                self.card(Field::Hotkey, p)
                    .track_focus(&self.recorder)
                    .when(recording, |d| {
                        d.border_color(p.accent)
                            .on_key_down(cx.listener(Self::record_hotkey))
                    })
                    .child(
                        Icon::new(IconName::Keyboard)
                            .size(px(16.))
                            .text_color(p.mute),
                    )
                    .child(self.labelled(
                        s.shortcut,
                        Some(if recording {
                            s.shortcut_recording
                        } else {
                            s.shortcut_note
                        }),
                        Field::Hotkey,
                        s,
                        p,
                    ))
                    .children((!recording).then(|| kbd(&self.ui.hotkey)).flatten())
                    .child(
                        Button::new("change-hotkey")
                            .cursor_pointer()
                            .label(if recording {
                                s.cancel
                            } else {
                                s.shortcut_change
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.recording = !this.recording;
                                if this.recording {
                                    this.error = None;
                                    window.focus(&this.recorder, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .children(conflict)
            .child(
                self.setting(
                    Field::LaunchAtLogin,
                    Some(IconName::Power),
                    s.launch_at_login,
                    Some(s.launch_at_login_note),
                    Switch::new("launch-at-login")
                        .cursor_pointer()
                        .checked(self.ui.launch_at_login)
                        .color(p.accent)
                        .accessibility_label(s.launch_at_login)
                        .on_click(cx.listener(|this, &on: &bool, window, cx| {
                            this.save(
                                Field::LaunchAtLogin,
                                json!({ "ui": { "launch_at_login": on } }),
                                window,
                                cx,
                            );
                        })),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Language,
                    Some(IconName::Globe),
                    s.language,
                    None,
                    div().w(px(260.)).child(
                        Select::new(&self.language)
                            .cursor_pointer()
                            .bg(p.solid)
                            .accessibility_label(s.language),
                    ),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Results,
                    Some(IconName::Search),
                    s.results_shown,
                    Some(s.results_shown_note),
                    div().w(px(120.)).child(
                        Select::new(&self.results)
                            .cursor_pointer()
                            .bg(p.solid)
                            .accessibility_label(s.results_shown),
                    ),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Quit,
                    None,
                    s.quit_app,
                    Some(s.quit_app_note),
                    Button::new("quit")
                        .cursor_pointer()
                        .label(s.tray_quit)
                        .on_click(cx.listener(|this, _, _, _| {
                            let _ = this.events.try_send(AppEvent::Quit);
                        })),
                    s,
                    p,
                ),
            )
    }

    fn appearance(&self, p: &Palette, window: &Window, cx: &Context<Self>) -> Div {
        let s = self.live.read(cx).lang.strings();
        let adjustable = see_through_adjustable(self.ui.transparency_mode, self.support);
        stack()
            .child(
                self.setting(
                    Field::Theme,
                    None,
                    s.theme,
                    None,
                    div().w(px(260.)).child(
                        Select::new(&self.theme_choice)
                            .cursor_pointer()
                            .bg(p.solid)
                            .accessibility_label(s.theme),
                    ),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::Background,
                    None,
                    s.window_background,
                    None,
                    div().w(px(260.)).child(
                        Select::new(&self.background)
                            .cursor_pointer()
                            .bg(p.solid)
                            .accessibility_label(s.window_background),
                    ),
                    s,
                    p,
                ),
            )
            .child(
                self.setting(
                    Field::SeeThrough,
                    None,
                    s.see_through_amount,
                    (!adjustable).then_some(s.see_through_off),
                    div()
                        .flex()
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
                                .child(
                                    Slider::new(&self.see_through)
                                        .when(adjustable, |s| s.cursor_pointer())
                                        .disabled(!adjustable),
                                ),
                        )
                        .child(s.more_transparent),
                    s,
                    p,
                ),
            )
    }

    fn index(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let (lang, s) = (live.lang, live.lang.strings());
        let stats = live.status.as_ref().map(|st| {
            let stat = |value: u64, label: &'static str, tone: Option<Tone>| {
                card(p)
                    .flex_1()
                    .flex_col()
                    .items_start()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .when_some(tone, |d, t| d.text_color(p.tone(t)))
                            .child(lang.number(value)),
                    )
                    .child(div().text_xs().text_color(p.mute).child(label))
            };
            div()
                .flex()
                .gap(px(8.))
                .child(stat(st.indexed, s.stat_indexed, None))
                .child(stat(st.queued, s.stat_waiting, None))
                .child(stat(st.skipped, s.stat_skipped, None))
                .child(stat(
                    st.errors,
                    s.stat_errors,
                    (st.errors > 0).then_some(Tone::Err),
                ))
        });
        let unreadable = self.unreadable.as_deref().unwrap_or_default();
        let files = match self.unreadable.as_deref() {
            None => stack(),
            Some([]) => stack().child(card(p).text_color(p.mute).child(s.all_read)),
            Some(unreadable) => stack().children(unreadable.iter().map(|e| {
                let path = std::path::Path::new(&e.path);
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let folder = path
                    .parent()
                    .map(std::path::Path::display)
                    .map(|d| d.to_string());
                let folder = folder.unwrap_or_default();
                card(p)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(16.))
                            .text_color(p.err),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(name.to_string()))
                            .child(
                                note(p)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis_middle()
                                    .child(format!("{} · {folder}", file_error_label(e.code, s))),
                            ),
                    )
            })),
        };
        div()
            .children(stats)
            .child(
                div()
                    .mt(px(22.))
                    .mb(px(8.))
                    .child(subheading(s.unreadable, p)),
            )
            .child(files)
            .children(self.problem(Field::Unreadable, s, p))
            .when(!unreadable.is_empty(), |d| {
                d.child(
                    div().mt(px(8.)).child(
                        Button::new("retry-all")
                            .cursor_pointer()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(s.retry_all)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run(
                                    Field::Unreadable,
                                    |host| host.engine()?.retry_errors().map(drop),
                                    cx,
                                );
                            })),
                    ),
                )
            })
            .child(
                div()
                    .mt(px(22.))
                    .mb(px(8.))
                    .child(subheading(s.danger_zone, p)),
            )
            .child(
                self.card(Field::Clear, p)
                    .border_color(p.err)
                    .child(self.labelled(
                        s.clear_index,
                        Some(s.clear_index_note),
                        Field::Clear,
                        s,
                        p,
                    ))
                    .child(
                        Button::new("clear-index")
                            .cursor_pointer()
                            .danger()
                            .small()
                            .label(s.clear_index_button)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_clear(window, cx)),
                            ),
                    ),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        // Over a blurred backdrop the window's own colors take the tint's
        // alpha, as the search window's panel does; the boxes stay opaque.
        let alpha = p.tint_alpha(self.backdrop());
        let tint = |color: Hsla| color.opacity(alpha);
        let nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(210.))
            .gap(px(2.))
            .p_2()
            .bg(tint(p.side))
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
                    .cursor_pointer()
                    .ghost()
                    .selected(selected)
                    .w_full()
                    .px_0()
                    .rounded(px(6.))
                    .when(selected, |b| b.bg(p.chip).child(p.accent_bar()))
                    // The content row centers a label; a filling child sits
                    // left. The padding is the label's, so the content row
                    // starts at the button's edge: an absolute child (the
                    // accent bar) is placed against its direct parent.
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px_3()
                            .child(
                                Icon::new(section.icon())
                                    .size(px(15.))
                                    .text_color(if selected { p.accent } else { p.mute }),
                            )
                            .child(section.nav_title(s)),
                    )
                    .accessibility_label(section.title(s))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.recording = false;
                        this.section = section;
                        this.error = None;
                        this.pane_scroll.set_offset(Point::default());
                        if section == Section::Index {
                            this.load_unreadable(cx);
                        }
                        cx.notify();
                    }))
            }))
            .child(
                div()
                    .mt_auto()
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(p.mute)
                    .child(concat!("Magi ", env!("CARGO_PKG_VERSION"))),
            );
        let body = match self.section {
            Section::Folders => self.folders(&p, cx),
            Section::Features => self.features(&p, cx),
            Section::General => self.general(&p, cx),
            Section::Appearance => self.appearance(&p, window, cx),
            Section::Index => self.index(&p, cx),
        };
        div()
            .size_full()
            .flex()
            .bg(tint(p.solid))
            .text_color(p.ink)
            .text_size(px(14.))
            .child(nav)
            .child(
                div()
                    .id("pane")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.pane_scroll)
                    .px(px(32.))
                    .pt(px(22.))
                    .pb(px(28.))
                    .child(heading(self.section.title(s)))
                    .child(body),
            )
    }
}

/// The native folder picker (FR-1): the folder chosen, if any.
pub(crate) fn pick_folder(cx: &App) -> impl Future<Output = Option<std::path::PathBuf>> + use<> {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    async move {
        match picked.await {
            Ok(Ok(Some(mut paths))) => paths.pop(),
            Ok(Err(error)) => {
                tracing::warn!(%error, "the folder picker failed");
                None
            }
            _ => None,
        }
    }
}

/// The space between boxes (the variant A mockup's 6 px).
const STACK_GAP: Pixels = px(6.);

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
        .rounded(px(8.))
        .border_1()
        .border_color(p.line)
        .bg(p.card)
        .when(!p.dark, |d| d.shadow_xs())
}

/// A setting's second line.
pub(crate) fn note(p: &Palette) -> Div {
    div().mt(px(2.)).text_xs().text_color(p.mute)
}

pub(crate) fn heading(text: &'static str) -> Div {
    div()
        .mb(px(18.))
        .text_size(px(26.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// A group's name inside a section, small caps as in the mockup.
fn subheading(text: &'static str, p: &Palette) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.mute)
        .child(text)
}
