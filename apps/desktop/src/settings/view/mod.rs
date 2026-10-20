//! The settings window: a sidebar and its sections (FR-9), each section
//! grouped rows as on the onboarding setup screen.
//!
//! Folders (FR-1, FR-11): roots come from the latest status in [`Live`], or from the database until
//! the first one (the engine sends none during its startup walk); every
//! change goes through the engine, and the next status shows it. Below them,
//! the exclusions.
//!
//! What's searched: the search features (ADR-0010), each one's switch,
//! download and backfill from [`Live::features`]; then what to index, file
//! types and largest file. Side by side, as the image features need image
//! files.
//!
//! Exclusions, file types, largest file and pause on battery are
//! `indexing` settings: each save restarts the engine, which re-walks every
//! root, so a value is saved only when it changed, and text fields save on
//! Enter, blur or Save, never per keystroke.
//!
//! General and Appearance: `ui` settings, plus pause on battery under
//! General. A saved `ui` change goes to the shell as [`AppEvent::Ui`], which
//! applies it live.
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
use magi_core::platform::{BackdropSupport, SYMBOL_MODIFIERS};
use serde_json::json;

use super::{Roll, error_text, slider_from_intensity};
use crate::app::{AppEvent, Events};
use crate::i18n::{Lang, Strings};
use crate::live::Live;
use crate::theme::{self, Backdrop, Palette};
use crate::tray::TrayAction;
use crate::ui::keycaps::shortcut_text;
use crate::ui::{heading, note, row, row_icon, saved_note, warn_note};
use choices::{
    Choice, Dropdown, THEMES, background_choices, count_choices, language_choices, theme_choices,
};

pub const SIZE: Size<Pixels> = size(px(900.), px(620.));
/// The smallest the window resizes to. The Appearance slider row (label,
/// both end labels and the slider) is the widest and already fits the default
/// only just, so the width stays close to it; sections scroll below this
/// height.
pub const MIN_SIZE: Size<Pixels> = size(px(860.), px(480.));

/// A section button's height, and the space between two: the accent bar
/// moves by their sum per section.
const NAV_ITEM_H: f32 = 32.;
const NAV_GAP: f32 = 2.;
/// How long the accent bar takes to reach another section.
const NAV_SLIDE: std::time::Duration = std::time::Duration::from_millis(240);
/// How long another section takes to fade in.
const SECTION_FADE: std::time::Duration = std::time::Duration::from_millis(220);
/// How long "Saved" stands in for a text field's note or Save button.
const SAVED_FOR: std::time::Duration = std::time::Duration::from_millis(2000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Folders,
    Searched,
    General,
    Appearance,
    Index,
}

const SECTIONS: [Section; 5] = [
    Section::Folders,
    Section::Searched,
    Section::General,
    Section::Appearance,
    Section::Index,
];

impl Section {
    fn title(self, s: &Strings) -> &'static str {
        match self {
            Self::Folders => s.folders,
            Self::Searched => s.whats_searched,
            Self::General => s.general,
            Self::Appearance => s.appearance,
            Self::Index => s.index,
        }
    }

    fn blurb(self, s: &Strings) -> &'static str {
        match self {
            Self::Folders => s.blurb_folders,
            Self::Searched => s.blurb_searched,
            Self::General => s.blurb_general,
            Self::Appearance => s.blurb_appearance,
            Self::Index => s.blurb_index,
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Folders => IconName::Folder,
            Self::Searched => IconName::Sparkles,
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
    Rescan,
    Clear,
    Quit,
}

pub struct SettingsView {
    host: Host,
    live: Entity<Live>,
    events: Events,
    section: Section,
    /// The section before the last switch, where the accent bar slides from.
    prev_section: Section,
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
    /// The Index tiles' counts, each on its way to the latest status.
    rolls: Option<[Roll; 4]>,
    /// Whether Folders' Advanced box is open.
    advanced_open: bool,
    /// How many times it was toggled: the chevron turns only after a click,
    /// not when the section first shows.
    advanced_flips: usize,
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
    /// The text field whose change just saved, for a moment: those save on
    /// Enter, blur or Save and show nothing else.
    saved: Option<Field>,
    /// Roots being removed: dimmed, their controls off, until the list no
    /// longer has them.
    removing: Vec<i64>,
    /// Changes still running, one entry per run: their buttons spin and
    /// take no clicks until the change ends.
    busy: Vec<Field>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    /// Switches the pane, the way a click on its nav item does.
    fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.recording = false;
        self.prev_section = self.section;
        self.section = section;
        self.error = None;
        self.pane_scroll.set_offset(Point::default());
        if section == Section::Index {
            self.load_unreadable(cx);
        }
        cx.notify();
    }

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
                if let Some(status) = &live.read(cx).status {
                    this.roll_stats(status);
                }
                // The search footer's "see which files".
                if live.read(cx).show_unreadable {
                    live.update(cx, |live, _| live.show_unreadable = false);
                    this.show(Section::Index, cx);
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
            prev_section: Section::Folders,
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
            rolls: None,
            advanced_open: false,
            advanced_flips: 0,
            pane_scroll: ScrollHandle::new(),
            dark: false,
            opening_roots,
            user_folders: magi_core::paths::user_folders(),
            unreadable: None,
            errors_listed: None,
            error: None,
            saved: None,
            removing: Vec::new(),
            busy: Vec::new(),
            _subscriptions: subscriptions,
        };
        view.sync_appearance(window, cx);
        view.sync_controls(window, cx);
        if let Some(status) = view.live.read(cx).status.clone() {
            view.roll_stats(&status);
        }
        // Opened by the search footer's "see which files".
        if view.live.read(cx).show_unreadable {
            view.live
                .clone()
                .update(cx, |live, _| live.show_unreadable = false);
            view.show(Section::Index, cx);
        }
        view
    }

    /// The rail's way back to the product: a quiet item at its foot, like the
    /// sections above it, opening the search window (or bringing it forward).
    /// The shortcut is in its tooltip: beside the label it would overflow.
    fn search_button(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let hotkey = Keystroke::parse(&crate::settings::hotkey_keystroke(&self.ui.hotkey))
            .ok()
            .map(|k| shortcut_text(&k, s, SYMBOL_MODIFIERS));
        Button::new("open-search")
            .cursor_pointer()
            .ghost()
            .w_full()
            .h(px(NAV_ITEM_H))
            .px_0()
            .rounded(px(6.))
            .text_size(px(13.))
            .accessibility_label(s.search_files)
            .when_some(hotkey, |b, text| b.tooltip(text))
            .on_click(cx.listener(|this, _, _, _| {
                let _ = this.events.try_send(AppEvent::Tray(TrayAction::OpenSearch));
            }))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px_3()
                    .child(Icon::new(IconName::Search).size(px(15.)).text_color(p.mute))
                    .child(s.search_files),
            )
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

    /// Runs a change on the background executor, `field` busy meanwhile,
    /// and keeps its error. Public for the headless tests.
    pub fn run(
        &mut self,
        field: Field,
        job: impl FnOnce(&Host) -> magi_core::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.busy.push(field);
        cx.notify();
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { job(&host) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(ix) = view.busy.iter().position(|&f| f == field) {
                    view.busy.remove(ix);
                }
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
                        if matches!(field, Field::MaxSize | Field::Excludes) {
                            view.flash_saved(field, cx);
                        }
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

    /// Shows `field` as saved for a moment.
    fn flash_saved(&mut self, field: Field, cx: &mut Context<Self>) {
        self.saved = Some(field);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVED_FOR).await;
            let _ = this.update(cx, |view, cx| {
                if view.saved == Some(field) {
                    view.saved = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The row showing a failed change, if any.
    pub fn failed(&self) -> Option<Field> {
        self.error.as_ref().map(|(field, _)| *field)
    }

    /// Whether a change in `field`'s row is still running.
    pub fn is_busy(&self, field: Field) -> bool {
        self.busy.contains(&field)
    }

    /// Advanced shows when opened, or when a change in it failed: its
    /// message would otherwise be hidden.
    pub fn advanced_shown(&self) -> bool {
        self.advanced_open || self.has_problem(Field::Excludes)
    }

    fn has_problem(&self, field: Field) -> bool {
        self.error.as_ref().is_some_and(|(f, _)| *f == field)
    }

    /// The failed change's message, for the last line of `field`'s row. It
    /// fades in, again for a different message in the same row.
    fn problem(&self, field: Field, s: &Strings, p: &Palette) -> Option<impl IntoElement> {
        self.error
            .as_ref()
            .filter(|(f, _)| *f == field)
            .map(|(_, error)| {
                let text = error_text(error, s);
                let id = SharedString::from(format!("problem-{field:?}-{text}"));
                theme::fade_in(warn_note(p, text), id)
            })
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
            .children(note_text.map(|text| {
                if self.saved == Some(field) {
                    saved_note(s, p)
                } else {
                    note(p).child(text)
                }
            }))
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
        // The accent bar slides from the section before; under reduced motion
        // GPUI shows it in place.
        let (from, to) = (self.prev_section as usize, self.section as usize);
        let pitch = NAV_ITEM_H + NAV_GAP;
        let bar = div()
            .absolute()
            .left_0()
            .w(px(3.))
            .h(px(NAV_ITEM_H * 0.35))
            .rounded_full()
            .bg(p.accent)
            .with_animation(
                ("nav-bar", from * SECTIONS.len() + to),
                Animation::new(NAV_SLIDE).with_easing(ease_in_out),
                move |bar, t| {
                    let row = from as f32 + (to as f32 - from as f32) * t;
                    bar.top(px(row * pitch + NAV_ITEM_H * 0.375))
                },
            );
        let sections = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(NAV_GAP))
            .children(SECTIONS.iter().enumerate().map(|(ix, &section)| {
                let selected = section == self.section;
                // A button, so Tab and Enter reach every section.
                Button::new(("section", ix))
                    .cursor_pointer()
                    .ghost()
                    .selected(selected)
                    .w_full()
                    .h(px(NAV_ITEM_H))
                    .px_0()
                    .rounded(px(6.))
                    .when(selected, |b| b.bg(p.chip))
                    // The content row centers a label; a filling child sits
                    // left. The padding is the label's, so the content row
                    // starts at the button's edge.
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
                            .child(section.title(s)),
                    )
                    .accessibility_label(section.title(s))
                    .on_click(cx.listener(move |this, _, _, cx| this.show(section, cx)))
            }))
            .child(bar);
        let nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(210.))
            .gap(px(NAV_GAP))
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
            .child(sections)
            // A hairline marks it as an action, not a sixth section.
            .child(
                div()
                    .mt_auto()
                    .child(div().h(px(1.)).mx_3().mb(px(6.)).bg(p.line))
                    .child(self.search_button(s, &p, cx)),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(p.mute)
                    .child(concat!("Magi ", env!("CARGO_PKG_VERSION"))),
            );
        let body = match self.section {
            Section::Folders => self.folders(&p, cx),
            Section::Searched => self.features(&p, cx),
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
                    // Another section fades in, its heading with it.
                    .child(theme::fade_in_over(
                        div()
                            .child(heading(self.section.title(s)).mb(px(4.)))
                            .child(
                                div()
                                    .mb(px(14.))
                                    .text_color(p.mute)
                                    .child(self.section.blurb(s)),
                            )
                            .child(body),
                        ("pane-in", self.section as usize),
                        SECTION_FADE,
                    )),
            )
    }
}
