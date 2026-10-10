//! The settings window: a sidebar and its sections (FR-9), each section
//! grouped rows as on the onboarding setup screen.
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

mod appearance;
mod choices;
mod features;
mod folders;
mod general;
mod index;
mod what_to_index;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::Selectable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::config::{IndexingConfig, Language, TransparencyMode, UiConfig};
use magi_core::dto::{ErrorCode, FileError, RootStatus};
use magi_core::features::Feature;
use magi_core::host::Host;
use magi_core::platform::BackdropSupport;
use serde_json::json;

use super::{error_text, slider_from_intensity};
use crate::app::{AppEvent, Events};
use crate::i18n::{Lang, Strings};
use crate::live::Live;
use crate::theme::{self, Backdrop, Palette};
use crate::ui::{heading, note, row, row_icon};
use choices::{
    Choice, Dropdown, THEMES, background_choices, count_choices, language_choices, theme_choices,
};

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

/// Where a failed change is shown: in the row it came from.
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
    /// Offered for one-click adding, as in onboarding.
    user_folders: Vec<std::path::PathBuf>,
    /// Read when the Index section opens, after its actions, and when the
    /// error count changes.
    unreadable: Option<Vec<FileError>>,
    /// The status's error count when `unreadable` was read.
    errors_listed: Option<u64>,
    /// The last change that failed and where, until the next change or a
    /// section switch.
    error: Option<(Field, ErrorCode)>,
    /// Roots being removed: dimmed, their controls off, until the list no
    /// longer has them.
    removing: Vec<i64>,
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
                // Removals the list has caught up with.
                if let Some(status) = &live.read(cx).status {
                    this.removing
                        .retain(|id| status.roots.iter().any(|root| root.id == *id));
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
            user_folders: magi_core::paths::user_folders(),
            unreadable: None,
            errors_listed: None,
            error: None,
            removing: Vec::new(),
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
                // A failed removal leaves its row working again.
                if let (Field::Root(id), Err(_)) = (field, &done) {
                    view.removing.retain(|&r| r != id);
                }
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

    /// Saves a settings patch; on success the shell applies the new `ui`, on
    /// failure `field`'s row shows why. Public for the headless tests.
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

    /// The row showing a failed change, if any.
    pub fn failed(&self) -> Option<Field> {
        self.error.as_ref().map(|(field, _)| *field)
    }

    fn has_problem(&self, field: Field) -> bool {
        self.error.as_ref().is_some_and(|(f, _)| *f == field)
    }

    /// The failed change's message, for the last line of `field`'s row.
    fn problem(&self, field: Field, s: &Strings, p: &Palette) -> Option<Div> {
        self.error
            .as_ref()
            .filter(|(f, _)| *f == field)
            .map(|(_, error)| note(p).text_color(p.warn).child(error_text(error, s)))
    }

    /// A grouped row; a failed change in it tints it.
    fn row(&self, field: Field, p: &Palette) -> Div {
        row().when(self.has_problem(field), |d| d.bg(p.warn.opacity(0.08)))
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

    /// One setting's row: its icon, its name and note, and its control.
    #[allow(clippy::too_many_arguments)]
    fn setting(
        &self,
        field: Field,
        icon: IconName,
        label: &'static str,
        note_text: Option<&'static str>,
        control: impl IntoElement,
        s: &Strings,
        p: &Palette,
    ) -> Div {
        self.row(field, p)
            .child(row_icon(icon, p))
            .child(self.labelled(label, note_text, field, s, p))
            .child(div().flex_none().child(control))
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
