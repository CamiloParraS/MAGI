//! First-run onboarding (FR-10) in two screens. Setup: folders, with each
//! one's status as the permission check; search features, with consent to
//! download; start with the computer. Confirming applies all three and saves
//! `done` to `ui.onboarding`; then the first index's progress. A plain launch
//! opens onboarding until then. Closing it leaves the app running.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::config::{FeaturesConfig, Onboarding, UiConfig};
use magi_core::dto::{ErrorCode, FeatureStatus, IndexState, IndexStatus, Install, RootStatus};
use magi_core::features::Feature;
use magi_core::host::Host;
use serde_json::json;
use std::time::Duration;

use crate::app::{AppEvent, Events};
use crate::i18n::Strings;
use crate::search::view::{Live, turn_on};
use crate::settings::view::{card, heading, note, pick_folder};
use crate::settings::{error_text, hotkey_keystroke, root_state};
use crate::theme::{self, Palette, Tone};

pub const SIZE: Size<Pixels> = size(px(880.), px(600.));

actions!(magi_onboarding, [Next, Back]);

const CONTEXT: &str = "OnboardingWindow";

/// Enter and Escape on the window itself; a focused button or checkbox
/// takes them first (its context is deeper).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Next, Some(CONTEXT)),
        KeyBinding::new("escape", Back, Some(CONTEXT)),
    ]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Setup,
    Indexing,
}

/// The saved resume point. `folders`, `features` and `background` were the
/// earlier flow's screens: all of them are on the setup screen now. `Done`
/// only shows if a window was already open.
impl From<Onboarding> for Step {
    fn from(saved: Onboarding) -> Self {
        match saved {
            Onboarding::Done => Self::Indexing,
            Onboarding::Folders | Onboarding::Features | Onboarding::Background => Self::Setup,
        }
    }
}

/// The features checked when the step opens: what the config wants, which
/// by default is ADR-0010's selection (meaning and image text).
pub fn preselected(features: &[FeatureStatus]) -> Vec<Feature> {
    features
        .iter()
        .filter(|f| f.enabled)
        .map(|f| f.feature)
        .collect()
}

/// Not on disk and not on its way.
fn missing(f: &FeatureStatus) -> bool {
    matches!(f.install, Install::NotInstalled | Install::Failed { .. })
}

/// What confirming the features step changes: each feature to turn on
/// (enable, then download if missing) or off. One already as chosen is
/// left alone, so nothing restarts the engine for no change.
pub fn feature_changes(features: &[FeatureStatus], chosen: &[Feature]) -> Vec<(Feature, bool)> {
    features
        .iter()
        .map(|f| (f, chosen.contains(&f.feature)))
        .filter(|&(f, on)| f.enabled != on || (on && missing(f)))
        .map(|(f, on)| (f.feature, on))
        .collect()
}

/// The bytes the chosen features will download: the consent states it.
pub fn download_total(features: &[FeatureStatus], chosen: &[Feature]) -> u64 {
    features
        .iter()
        .filter(|f| chosen.contains(&f.feature) && missing(f))
        .map(|f| f.download_size)
        .sum()
}

/// The first index's percentage; `None` (an indeterminate bar) while the
/// scan is still finding files, since each one found would push it back.
#[derive(Debug, PartialEq)]
pub enum Readiness {
    /// Files still being read, or this feature downloading.
    Preparing(Option<Feature>),
    Ready,
}

/// What the last step says, in one plain sentence and no counts. A failed
/// download is not "preparing": it is reported on its own.
pub fn readiness(status: Option<&IndexStatus>, features: &[FeatureStatus]) -> Readiness {
    if let Some(f) = features
        .iter()
        .find(|f| matches!(f.install, Install::Downloading { .. }))
    {
        return Readiness::Preparing(Some(f.feature));
    }
    match status {
        Some(st) if st.state == IndexState::Idle => Readiness::Ready,
        _ => Readiness::Preparing(None),
    }
}

/// One key of a shortcut, drawn as a keycap.
pub struct Cap {
    pub icon: Option<IconName>,
    /// Empty when the icon says it all (macOS symbols).
    pub label: String,
    /// The space bar.
    pub wide: bool,
}

/// `keystroke` as keycaps, modifiers first in the platform's order;
/// `symbols` is [`magi_core::platform::SYMBOL_MODIFIERS`].
pub fn key_caps(keystroke: &Keystroke, s: &Strings, symbols: bool) -> Vec<Cap> {
    let cap = |icon, label: &str| Cap {
        icon,
        label: label.into(),
        wide: false,
    };
    // Each modifier: its symbol on macOS, its name elsewhere.
    let pick = |symbol, name| {
        if symbols {
            cap(Some(symbol), "")
        } else {
            cap(None, name)
        }
    };
    let m = &keystroke.modifiers;
    let mut caps = Vec::new();
    if m.control {
        caps.push(pick(IconName::ChevronUp, "Ctrl"));
    }
    if m.alt {
        caps.push(pick(IconName::Option, "Alt"));
    }
    if m.shift {
        // The arrow is printed on Shift keys everywhere.
        caps.push(cap(
            Some(IconName::ArrowBigUp),
            if symbols { "" } else { s.key_shift },
        ));
    }
    if m.platform {
        caps.push(pick(IconName::Command, "Win"));
    }
    let key = keystroke.key.as_str();
    caps.push(match key {
        "space" => Cap {
            icon: None,
            label: s.key_space.into(),
            wide: true,
        },
        // `k`, `f1`: as printed. `enter`, `tab`: capitalized.
        _ if key.chars().count() <= 3 => cap(None, &key.to_uppercase()),
        _ => {
            let mut chars = key.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
            cap(
                None,
                &format!("{}{}", first.unwrap_or_default(), chars.as_str()),
            )
        }
    });
    caps
}

pub fn progress(st: &IndexStatus) -> Option<f32> {
    if st.state == IndexState::Scanning {
        return None;
    }
    let total = st.indexed + st.queued;
    Some(if total == 0 {
        100.
    } else {
        st.indexed as f32 * 100. / total as f32
    })
}

/// Saves `ui` settings, a step's completion among them; the shell gets the
/// result (it routes the next plain launch and applies launch at login).
fn save_step(host: &Host, ui: serde_json::Value) -> magi_core::Result<UiConfig> {
    host.update_settings(&json!({ "ui": ui })).map(|c| c.ui)
}

pub struct OnboardingView {
    host: Host,
    live: Entity<Live>,
    events: Events,
    step: Step,
    /// The features checked on the features step; the preselection until
    /// the first change.
    chosen: Option<Vec<Feature>>,
    /// "Start with your computer", checked by default (ADR-0010).
    launch_at_login: bool,
    /// The roots when the window opened, shown until the first status.
    opening_roots: Option<Vec<RootStatus>>,
    /// `ui.hotkey`, shown on the background step.
    hotkey: String,
    /// A step's change running; Continue waits for it.
    busy: bool,
    /// The last change that failed, until the next one.
    error: Option<ErrorCode>,
    dark: bool,
    /// Which way the last screen change went; the new screen slides in
    /// from that side.
    forward: bool,
    /// `hotkey_presses` when the window opened: a press after it is the
    /// user trying the hotkey.
    hotkey_presses: u32,
    focus: FocusHandle,
    /// Focused while the finish step's shortcut recorder waits for keys.
    recorder: FocusHandle,
    recording: bool,
    _subscriptions: Vec<Subscription>,
}

impl OnboardingView {
    pub fn new(
        host: Host,
        live: Entity<Live>,
        events: Events,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // ponytail: small reads on the main thread, as the settings window's.
        let ui = host
            .settings()
            .inspect_err(|error| tracing::warn!(%error, "could not read the settings"))
            .map(|c| c.ui)
            .unwrap_or_default();
        let opening_roots = host
            .list_roots()
            .inspect_err(|error| tracing::warn!(%error, "could not list the roots"))
            .ok();
        let subscriptions = vec![
            cx.observe_in(&live, window, |this, live, window, cx| {
                window.set_window_title(live.read(cx).lang.strings().welcome);
                this.sync_appearance(window, cx);
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_appearance(window, cx);
            }),
        ];
        let hotkey_presses = live.read(cx).hotkey_presses;
        let mut view = Self {
            host,
            live,
            events,
            step: ui.onboarding.into(),
            chosen: None,
            launch_at_login: true,
            opening_roots,
            hotkey: ui.hotkey,
            busy: false,
            error: None,
            dark: false,
            forward: true,
            hotkey_presses,
            focus: cx.focus_handle(),
            recorder: cx.focus_handle(),
            recording: false,
            _subscriptions: subscriptions,
        };
        window.focus(&view.focus, cx);
        view.sync_appearance(window, cx);
        view
    }

    pub fn step(&self) -> Step {
        self.step
    }

    fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.live.read(cx).theme.clone();
        self.dark = theme::sync(&theme, window, cx);
        cx.notify();
    }

    fn roots<'a>(&'a self, cx: &'a App) -> Option<&'a Vec<RootStatus>> {
        self.live
            .read(cx)
            .status
            .as_ref()
            .map(|status| &status.roots)
            .or(self.opening_roots.as_ref())
    }

    /// From the same features the step shows, so the consent covers exactly
    /// what is applied.
    fn chosen(&self, cx: &App) -> Vec<Feature> {
        self.chosen
            .clone()
            .unwrap_or_else(|| preselected(&self.live.read(cx).features))
    }

    pub fn toggle_feature(&mut self, feature: Feature, on: bool, cx: &mut Context<Self>) {
        let mut chosen = self.chosen(cx);
        chosen.retain(|&f| f != feature);
        if on {
            chosen.push(feature);
        }
        self.chosen = Some(chosen);
        cx.notify();
    }

    /// Setup needs one folder and the features to show.
    fn can_continue(&self, cx: &App) -> bool {
        !self.busy
            && match self.step {
                Step::Setup => {
                    self.roots(cx).is_some_and(|roots| !roots.is_empty())
                        && !self.live.read(cx).features.is_empty()
                }
                Step::Indexing => true,
            }
    }

    /// Setup applies the features and the login choice and finishes
    /// onboarding in one go; the last screen opens search.
    pub fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_continue(cx) {
            return;
        }
        match self.step {
            Step::Setup => {
                let changes = feature_changes(&self.live.read(cx).features, &self.chosen(cx));
                let ui = json!({
                    "launch_at_login": self.launch_at_login,
                    "onboarding": Onboarding::Done,
                });
                self.apply(
                    Step::Indexing,
                    move |host| {
                        for (feature, on) in changes {
                            if on {
                                turn_on(host, feature)?;
                            } else {
                                host.set_feature_enabled(feature, false)?;
                            }
                        }
                        save_step(host, ui)
                    },
                    cx,
                );
            }
            Step::Indexing => self.leave(Some(AppEvent::Toggle), window),
        }
    }

    /// Closes onboarding, then opens what the user picked (search or
    /// settings).
    fn leave(&self, then: Option<AppEvent>, window: &mut Window) {
        window.remove_window();
        if let Some(event) = then {
            let _ = self.events.try_send(event);
        }
    }

    /// From the finish screen back to setup, to change a choice. Setup is
    /// already applied; confirming it again applies only what changed.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.step == Step::Indexing && !self.busy && !self.recording {
            self.go(Step::Setup, cx);
        }
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.forward = step == Step::Indexing;
        self.step = step;
        self.error = None;
        cx.notify();
    }

    /// Runs a step's change on the background executor; moves to `then` when
    /// it worked, else shows why. A saved `ui` goes to the shell, which
    /// applies it (launch at login).
    fn apply(
        &mut self,
        then: Step,
        job: impl FnOnce(&Host) -> magi_core::Result<UiConfig> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let host = self.host.clone();
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { job(&host) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match done {
                    Ok(ui) => {
                        let _ = view.events.try_send(AppEvent::Ui(ui));
                        view.go(then, cx);
                    }
                    Err(error) => {
                        tracing::warn!(%error, step = ?view.step, "an onboarding change failed");
                        view.error = Some(ErrorCode::from(&error));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// A root change; the next status shows it, or the error says why not.
    fn run_root(
        &mut self,
        job: impl FnOnce(&Host) -> magi_core::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { job(&host).and_then(|()| host.list_roots()) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match done {
                    Ok(roots) => {
                        view.error = None;
                        view.opening_roots = Some(roots);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a folder change failed");
                        view.error = Some(ErrorCode::from(&error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let picked = pick_folder(cx);
        cx.spawn(async move |this, cx| {
            if let Some(path) = picked.await {
                let _ = this.update(cx, |view, cx| {
                    view.run_root(move |host| host.engine()?.add_root(&path).map(drop), cx);
                });
            }
        })
        .detach();
    }

    /// Folders dropped from the file manager: the user picking them, as
    /// with the picker. Dropped files are ignored.
    fn drop_folders(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let paths = paths.paths().to_vec();
        self.run_root(
            move |host| {
                let engine = host.engine()?;
                for path in paths.iter().filter(|p| p.is_dir()) {
                    engine.add_root(path)?;
                }
                Ok(())
            },
            cx,
        );
    }

    /// FR-10's permission check: each folder's status says whether Magi can
    /// read it. With none yet, a drop zone; folders can be dropped either way.
    fn folders(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let accent = p.accent;
        let roots = self.roots(cx).filter(|roots| !roots.is_empty());
        let body = match roots {
            None => div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .py(px(22.))
                .rounded(px(10.))
                .border_1()
                .border_dashed()
                .border_color(p.mute.opacity(0.4))
                .child(
                    Icon::new(IconName::FolderPlus)
                        .size(px(26.))
                        .text_color(p.accent),
                )
                .child(div().text_color(p.mute).child(s.ob_drop_folders))
                .child(
                    Button::new("add-folder")
                        .primary()
                        .small()
                        .label(s.add_folder)
                        .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
                ),
            // The same grouped list as the features, Add folder its last row.
            Some(roots) => group(p)
                .children(roots.iter().enumerate().map(|(ix, root)| {
                    div()
                        .children((ix > 0).then(|| divider(p)))
                        .child(self.root_row(root, s, p, cx))
                }))
                .child(divider(p))
                .child(
                    div().flex().px(px(6.)).py(px(4.)).child(
                        Button::new("add-folder")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .label(s.add_folder)
                            .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
                    ),
                ),
        };
        body.drag_over::<ExternalPaths>(move |style, _, _, _| {
            style.border_color(accent).bg(accent.opacity(0.08))
        })
        .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.drop_folders(paths, cx)))
    }

    fn root_row(&self, root: &RootStatus, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let (badge, tone, explain) = root_state(root, s);
        let id = root.id;
        let icon = match tone {
            Tone::Warn | Tone::Err => Icon::new(IconName::TriangleAlert).text_color(p.tone(tone)),
            _ => Icon::new(IconName::Folder).text_color(p.accent),
        };
        // Spaced as `row`; its own because a path is cut in the middle,
        // where the folder's name survives.
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(6.))
            .min_h(px(44.))
            .child(icon.size(px(18.)).flex_none())
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
                    .children(explain.map(|e| note(p).child(e))),
            )
            .child(p.status(badge, tone))
            // A rescan probes each root again: a fixed permission or
            // a reconnected drive clears the badge.
            .when(matches!(tone, Tone::Warn | Tone::Err), |row| {
                row.child(
                    Button::new(("retry-root", id as u64))
                        .ghost()
                        .small()
                        .label(s.try_again)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_root(|host| host.engine().map(|e| drop(e.rescan())), cx);
                        })),
                )
            })
            .child(
                Button::new(("remove-root", id as u64))
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .tooltip(s.remove)
                    .accessibility_label(format!("{} {}", s.remove, root.path))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run_root(move |host| host.engine()?.remove_root(id), cx);
                    })),
            )
    }

    /// The download consent (FR-10, ADR-0010): each feature's exact size;
    /// the button states the total. One grouped list, as in system
    /// settings: a switch shows the choice, ⓘ says what a feature does.
    fn features(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let lang = live.lang;
        let chosen = self.chosen(cx);
        let defaults = FeaturesConfig::default();
        let rows = live.features.iter().enumerate().map(|(ix, f)| {
            let feature = f.feature;
            let text = s.feature(feature);
            let pitch = text.pitch;
            // Downloaded: a green check in place of the size.
            let size = match f.install {
                Install::Installed { .. } => div()
                    .id(("installed", ix))
                    .flex_none()
                    .aria_label(s.ob_installed)
                    .child(Icon::new(IconName::Check).size(px(16.)).text_color(p.ok))
                    .into_any_element(),
                _ => note(p)
                    .mt_0()
                    .flex_none()
                    .child(lang.size(f.download_size))
                    .into_any_element(),
            };
            let sub = defaults.enabled(feature).then_some(s.ob_recommended);
            let row = row(feature_icon(feature), text.name, sub, p)
                .child(size)
                .child(
                    Popover::new(("feature-info", ix))
                        .trigger(
                            Button::new(("feature-info-button", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Info)
                                .accessibility_label(text.name),
                        )
                        .content(move |_, _, _| div().max_w(px(260.)).text_sm().child(pitch)),
                )
                .child(
                    Switch::new(("feature", feature as usize))
                        .checked(chosen.contains(&feature))
                        .color(p.accent)
                        .accessibility_label(text.name)
                        .on_click(cx.listener(move |this, &on: &bool, _, cx| {
                            this.toggle_feature(feature, on, cx)
                        })),
                );
            stagger_in(row, ix)
        });
        group(p).children(
            rows.enumerate()
                .map(|(ix, row)| div().children((ix > 0).then(|| divider(p))).child(row)),
        )
    }

    /// "Start with your computer" (FR-10), on by default.
    fn launch_row(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        group(p).child(
            row(IconName::Power, s.launch_at_login, None, p).child(
                Switch::new("start-at-login")
                    .checked(self.launch_at_login)
                    .color(p.accent)
                    .tooltip(s.launch_at_login_note)
                    .accessibility_label(s.launch_at_login)
                    .on_click(cx.listener(|this, &on: &bool, _, cx| {
                        this.launch_at_login = on;
                        cx.notify();
                    })),
            ),
        )
    }

    /// The one setup screen: the welcome over two columns, folders on the
    /// left, features and startup on the right.
    fn setup(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let column = || div().flex_1().min_w_0().flex().flex_col().gap(px(16.));
        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(heading(s.welcome).mb_0().text_center().text_size(px(28.)))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(24.))
                    .child(column().child(section(
                        s.ob_folders,
                        s.ob_folders_note,
                        self.folders(s, p, cx),
                        p,
                    )))
                    .child(
                        column()
                            .child(section(
                                s.ob_features,
                                s.ob_features_note,
                                self.features(s, p, cx),
                                p,
                            ))
                            .child(self.launch_row(s, p, cx)),
                    ),
            )
    }

    fn hotkey_tried(&self, cx: &App) -> bool {
        self.live.read(cx).hotkey_presses > self.hotkey_presses
    }

    /// "Press it again to close" only while search is open.
    fn works(&self, s: &Strings, cx: &App) -> &'static str {
        if self.live.read(cx).search_open {
            s.ob_hotkey_works
        } else {
            s.ob_hotkey_worked
        }
    }

    /// The finish step's Change: the recorder takes the next shortcut, as
    /// in settings; the shell registers it before it is saved (FR-7).
    pub fn record(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = crate::hotkey::from_keystroke(k) else {
            return;
        };
        self.recording = false;
        cx.notify();
        let (host, events) = (self.host.clone(), self.events.clone());
        cx.spawn_in(window, async move |this, cx| {
            if !crate::app::register_hotkey(&events, spec.clone()).await {
                return;
            }
            let saved = cx
                .background_executor()
                .spawn(async move { save_step(&host, json!({ "hotkey": spec })) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match saved {
                    Ok(ui) => {
                        view.hotkey = ui.hotkey.clone();
                        let _ = view.events.try_send(AppEvent::Ui(ui));
                    }
                    Err(error) => view.error = Some(ErrorCode::from(&error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = !self.recording;
        if self.recording {
            self.error = None;
            window.focus(&self.recorder, cx);
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// The finish line, centered: setup is done, so a check, the title, the
    /// shortcut as keycaps, and one plain sentence on how far the files are,
    /// no counts (Settings › Index has those).
    fn indexing(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let ready = readiness(live.status.as_ref(), &live.features);
        // "Results get better" only while they still will.
        let intro = match ready {
            Readiness::Ready => s.ob_done_note_ready,
            Readiness::Preparing(_) => s.ob_done_note,
        };
        // Seen once, so it may arrive with a little life: it grows into its
        // fixed 52px slot (nothing around it moves) as it fades in.
        let accent = p.accent;
        let check = div()
            .size(px(52.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(accent.opacity(0.14))
                    .child(Icon::new(IconName::Check).size(px(26.)).text_color(accent))
                    .with_animation(
                        "done-check",
                        Animation::new(CHECK_IN).with_easing(ease_out_quint()),
                        |el, t| el.size(px(44. + 8. * t)).opacity(t),
                    ),
            );
        let caps = Keystroke::parse(&hotkey_keystroke(&self.hotkey))
            .map(|k| key_caps(&k, s, magi_core::platform::SYMBOL_MODIFIERS))
            .unwrap_or_default();
        let plus = !magi_core::platform::SYMBOL_MODIFIERS;
        let keys =
            div()
                .flex()
                .items_center()
                .gap_2()
                .children(caps.into_iter().enumerate().flat_map(|(ix, c)| {
                    let sep = (plus && ix > 0)
                        .then(|| div().text_color(p.mute).child("+").into_any_element());
                    sep.into_iter().chain([keycap(c, p).into_any_element()])
                }));
        let tried = self.hotkey_tried(cx);
        let try_it = if tried {
            theme::fade_in(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(p.tone(Tone::Ok))
                    .child(Icon::new(IconName::Check).size(px(14.)))
                    .child(self.works(s, cx)),
                "done-hotkey-works",
            )
            .into_any_element()
        } else {
            note(p).mt_0().child(s.ob_try_it).into_any_element()
        };
        let conflict = live.hotkey_conflict.is_some().then(|| {
            note(p)
                .mt_0()
                .text_color(p.warn)
                .child(format!("{}. {}", s.shortcut_taken, s.shortcut_taken_note))
        });
        let recording = self.recording;
        let shortcut = card(p)
            .w_full()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .py(px(24.))
            .track_focus(&self.recorder)
            .when(recording, |d| {
                d.border_color(p.accent).on_key_down(cx.listener(
                    |this, e: &KeyDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.record(&e.keystroke, window, cx);
                    },
                ))
            })
            .child(div().text_color(p.mute).child(s.ob_try_hotkey))
            .child(if recording {
                note(p)
                    .mt_0()
                    .child(s.shortcut_recording)
                    .into_any_element()
            } else {
                keys.into_any_element()
            })
            .children((!recording).then_some(try_it))
            .children(conflict)
            .child(
                Button::new("change-hotkey")
                    .small()
                    .ghost()
                    .label(if recording {
                        s.cancel
                    } else {
                        s.shortcut_change
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_recording(window, cx))),
            );
        let sentence = match ready {
            Readiness::Ready => s.ob_ready.to_string(),
            Readiness::Preparing(Some(feature)) => s
                .ob_preparing_feature
                .replace("{name}", s.feature(feature).name),
            Readiness::Preparing(None) => s.ob_preparing.to_string(),
        };
        let state_icon = match ready {
            Readiness::Ready => Icon::new(IconName::Check)
                .size(px(14.))
                .text_color(p.ok)
                .into_any_element(),
            Readiness::Preparing(_) => p.dot(Tone::Busy).into_any_element(),
        };
        // A thin bar while preparing; it has no numbers next to it.
        let bar = (ready != Readiness::Ready).then(|| {
            let bar = Progress::new("preparing").color(p.accent);
            div()
                .w(px(260.))
                .child(match live.status.as_ref().and_then(progress) {
                    Some(value) => bar.value(value),
                    None => bar.loading(true),
                })
        });
        let failed = live
            .features
            .iter()
            .filter(|f| matches!(f.install, Install::Failed { .. }))
            .map(|f| {
                note(p).text_color(p.warn).child(
                    s.ob_download_failed
                        .replace("{name}", s.feature(f.feature).name),
                )
            });
        div()
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .pt(px(8.))
            .child(check)
            .child(heading(s.ob_done).mt(px(14.)).mb(px(4.)))
            .child(div().text_color(p.mute).max_w(px(460.)).child(intro))
            .child(div().mt(px(22.)).w_full().child(shortcut))
            .child(
                div()
                    .mt(px(18.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .child(state_icon)
                            .child(sentence),
                    )
                    .children(bar)
                    .children(failed)
                    .child(
                        Button::new("open-settings")
                            .link()
                            .small()
                            .label(s.open_settings)
                            .on_click(cx.listener(|this, _, window, _| {
                                this.leave(Some(AppEvent::Settings), window)
                            })),
                    ),
            )
    }
}

/// A key drawn as a keycap: a deeper bottom edge, like a real key.
fn keycap(cap: Cap, p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(42.))
        .min_w(px(42.))
        .px_3()
        .when(cap.wide, |d| d.w(px(150.)))
        .rounded(px(8.))
        .border_1()
        .border_b(px(3.))
        .border_color(p.mute.opacity(0.45))
        .bg(p.solid)
        .text_size(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .children(cap.icon.map(|i| Icon::new(i).size(px(17.))))
        .when(!cap.label.is_empty(), |d| d.child(cap.label))
}

impl Render for OnboardingView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        let body = match self.step {
            Step::Setup => self.setup(s, &p, cx),
            Step::Indexing => self.indexing(s, &p, cx),
        };
        let can_continue = self.can_continue(cx);
        let no_folder = self.roots(cx).is_none_or(|roots| roots.is_empty());
        let total = download_total(&self.live.read(cx).features, &self.chosen(cx));
        // The consent is the button: it says what agreeing downloads.
        let next_label: SharedString = match self.step {
            Step::Setup if total > 0 => s
                .ob_download_and_start
                .replace("{size}", &self.live.read(cx).lang.size(total))
                .into(),
            Step::Setup => s.ob_start.into(),
            Step::Indexing => s.ob_start_searching.into(),
        };
        // A soft wash of the accent behind the top of the window: light mode
        // otherwise has no color but the button.
        let wash = div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(px(260.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(p.accent.opacity(if self.dark { 0.20 } else { 0.16 }), 0.),
                linear_color_stop(p.accent.opacity(0.), 1.),
            ));
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            // Bindings run before the recorder's keys: while it waits, Enter
            // does nothing and Esc cancels it. On setup Enter does nothing
            // either: a stray key must not start a download; the focused
            // button still takes it.
            .on_action(cx.listener(|this, _: &Next, window, cx| {
                if !this.recording && this.step == Step::Indexing {
                    this.next(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Back, window, cx| {
                if this.recording {
                    this.toggle_recording(window, cx);
                } else {
                    this.back(cx);
                }
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.solid)
            .text_color(p.ink)
            .text_size(px(14.))
            .child(wash)
            .child(
                div()
                    .id("onboarding")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(32.))
                    .pt(px(32.))
                    .pb(px(16.))
                    .child(slide_in(body, self.step, self.forward))
                    .children(self.error.as_ref().map(|error| {
                        note(&p)
                            .mt(px(10.))
                            .text_color(p.warn)
                            .child(error_text(error, s))
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(32.))
                    .py(px(16.))
                    .border_t_1()
                    .border_color(p.line)
                    // Why Start is off, or that nothing downloads.
                    .when(self.step == Step::Setup, |bar| {
                        let hint = if no_folder {
                            Some(s.ob_need_folder)
                        } else {
                            (total == 0).then_some(s.ob_nothing_to_download)
                        };
                        bar.children(hint.map(|h| note(&p).mt_0().child(h)))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("next")
                            .primary()
                            .label(next_label)
                            .loading(self.busy)
                            .disabled(!can_continue && !self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            )
            // Last, so it sits above the scroll area.
            .when(self.step == Step::Indexing, |root| {
                root.child(
                    div().absolute().top(px(12.)).left(px(12.)).child(
                        Button::new("back")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label(s.back)
                            .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                    ),
                )
            })
    }
}

/// How long a screen takes to slide in; onboarding is seen once, so a
/// little longer than [`theme::FADE`].
const STEP_IN: Duration = Duration::from_millis(220);
/// How far apart the feature tiles arrive.
const STAGGER: Duration = Duration::from_millis(40);
/// The finish screen's check growing in.
const CHECK_IN: Duration = Duration::from_millis(320);

/// A screen's body fades in from 8px toward where it came from: forward
/// from the right, back from the left. Under reduced motion GPUI shows the
/// end state.
fn slide_in(body: Div, step: Step, forward: bool) -> impl IntoElement {
    let from = if forward { 8. } else { -8. };
    body.relative().with_animation(
        ("step", step as usize),
        Animation::new(STEP_IN).with_easing(ease_out_quint()),
        move |el, t| el.opacity(t).left(px(from * (1. - t))),
    )
}

/// Tile `ix` of a list fades up 6px, [`STAGGER`] after the one before it.
/// It plays when the tile first shows, never on later renders.
fn stagger_in(el: Div, ix: usize) -> impl IntoElement {
    let delay = STAGGER * ix as u32;
    let total = STEP_IN + delay;
    let start = delay.as_secs_f32() / total.as_secs_f32();
    let ease = ease_out_quint();
    el.relative()
        .with_animation(("stagger-in", ix), Animation::new(total), move |el, t| {
            let t = ease(((t - start) / (1. - start)).clamp(0., 1.));
            el.opacity(t).top(px(6. * (1. - t)))
        })
}

/// A titled part of the setup screen.
fn section(title: &'static str, intro: &'static str, body: Div, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(note(p).mt(px(2.)).mb(px(10.)).text_sm().child(intro))
        .child(body)
}

/// What each feature does, as a picture.
fn feature_icon(feature: Feature) -> IconName {
    match feature {
        Feature::Meaning => IconName::TextSearch,
        Feature::ImageText => IconName::ScanText,
        Feature::ImageVisual => IconName::ScanEye,
    }
}

/// Rows grouped in one rounded box, as in system settings.
fn group(p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .border_1()
        .border_color(p.line)
        .bg(p.card)
        .when(!p.dark, |d| d.shadow_xs())
        .overflow_hidden()
}

/// A line between grouped rows, starting where the text does.
fn divider(p: &Palette) -> Div {
    div().h(px(1.)).ml(px(42.)).bg(p.line)
}

/// A grouped row: an icon in the accent, the name with an optional small
/// line under it, then what follows.
fn row(icon: IconName, name: &'static str, sub: Option<&'static str>, p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(6.))
        .min_h(px(44.))
        .child(
            Icon::new(icon)
                .size(px(18.))
                .flex_none()
                .text_color(p.accent),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(name),
                )
                .children(sub.map(|sub| {
                    div()
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .text_color(p.mute)
                        .child(sub)
                })),
        )
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has its own `test` attribute.
    use super::{
        Readiness, download_total, feature_changes, key_caps, preselected, progress, readiness,
    };
    use crate::i18n::Lang;
    use gpui_kit::Keystroke;
    use magi_core::dto::{DownloadError, FeatureStatus, IndexState, IndexStatus, Install};
    use magi_core::features::Feature;

    fn feature(feature: Feature, enabled: bool, install: Install) -> FeatureStatus {
        FeatureStatus {
            feature,
            enabled,
            download_size: 100,
            install,
            backfill: None,
        }
    }

    /// The first run: the config's defaults, nothing downloaded.
    fn first_run() -> Vec<FeatureStatus> {
        vec![
            feature(Feature::Meaning, true, Install::NotInstalled),
            feature(Feature::ImageText, true, Install::NotInstalled),
            feature(Feature::ImageVisual, false, Install::NotInstalled),
        ]
    }

    #[test]
    fn the_config_defaults_are_preselected() {
        assert_eq!(
            preselected(&first_run()),
            vec![Feature::Meaning, Feature::ImageText]
        );
    }

    #[test]
    fn confirming_changes_only_what_differs() {
        use Feature::*;
        // Wanted but not downloaded: turned on again, which downloads it.
        assert_eq!(
            feature_changes(&first_run(), &[Meaning, ImageText]),
            vec![(Meaning, true), (ImageText, true)]
        );
        // Unchecking a default turns it off; checking the third turns it on.
        assert_eq!(
            feature_changes(&first_run(), &[Meaning, ImageVisual]),
            vec![(Meaning, true), (ImageText, false), (ImageVisual, true)]
        );
        // None: keyword and file name search only.
        assert_eq!(
            feature_changes(&first_run(), &[]),
            vec![(Meaning, false), (ImageText, false)]
        );
        let settled = [
            feature(Meaning, true, Install::Installed { size_bytes: 1 }),
            feature(ImageText, true, Install::Downloading { bytes: 0, total: 1 }),
            feature(ImageVisual, false, Install::Installed { size_bytes: 1 }),
        ];
        assert!(feature_changes(&settled, &[Meaning, ImageText]).is_empty());
        // A failed download is retried.
        let failed = [feature(
            Meaning,
            true,
            Install::Failed {
                code: DownloadError::DownloadNetworkError,
            },
        )];
        assert_eq!(feature_changes(&failed, &[Meaning]), vec![(Meaning, true)]);
    }

    #[test]
    fn the_download_total_counts_what_is_still_missing() {
        use Feature::*;
        assert_eq!(download_total(&first_run(), &[Meaning, ImageText]), 200);
        assert_eq!(download_total(&first_run(), &[]), 0);
        let some = [
            feature(Meaning, true, Install::Installed { size_bytes: 100 }),
            feature(
                ImageText,
                true,
                Install::Downloading {
                    bytes: 0,
                    total: 100,
                },
            ),
            feature(
                ImageVisual,
                false,
                Install::Failed {
                    code: DownloadError::DiskFull,
                },
            ),
        ];
        assert_eq!(
            download_total(&some, &[Meaning, ImageText, ImageVisual]),
            100
        );
    }

    fn status(state: IndexState, indexed: u64, queued: u64) -> IndexStatus {
        IndexStatus {
            state,
            queued,
            indexed,
            skipped: 0,
            errors: 0,
            current_file: None,
            roots: vec![],
        }
    }

    #[test]
    fn the_bar_has_no_value_while_the_total_is_still_growing() {
        // Scanning keeps adding to `queued`: a percentage would go backwards.
        assert_eq!(progress(&status(IndexState::Scanning, 10, 5)), None);
        assert_eq!(progress(&status(IndexState::Indexing, 25, 75)), Some(25.));
        assert_eq!(progress(&status(IndexState::Idle, 0, 0)), Some(100.));
        assert_eq!(progress(&status(IndexState::Paused, 50, 50)), Some(50.));
    }

    #[test]
    fn the_last_step_says_ready_only_when_nothing_is_left() {
        let idle = status(IndexState::Idle, 10, 0);
        assert_eq!(readiness(Some(&idle), &first_run()), Readiness::Ready);
        // Before the first status, and while scanning or reading.
        assert_eq!(readiness(None, &[]), Readiness::Preparing(None));
        let busy = status(IndexState::Indexing, 5, 5);
        assert_eq!(readiness(Some(&busy), &[]), Readiness::Preparing(None));
        // A download names its feature, even with the files done.
        let downloading = [feature(
            Feature::Meaning,
            true,
            Install::Downloading { bytes: 1, total: 2 },
        )];
        assert_eq!(
            readiness(Some(&idle), &downloading),
            Readiness::Preparing(Some(Feature::Meaning))
        );
        // A failed download is reported on its own, not as preparing.
        let failed = [feature(
            Feature::Meaning,
            true,
            Install::Failed {
                code: DownloadError::DownloadNetworkError,
            },
        )];
        assert_eq!(readiness(Some(&idle), &failed), Readiness::Ready);
    }

    #[test]
    fn a_shortcut_becomes_keycaps_the_platform_way() {
        let caps = |key: &str, symbols: bool| {
            let k = Keystroke::parse(key).unwrap();
            key_caps(&k, Lang::En.strings(), symbols)
                .into_iter()
                .map(|c| (c.label, c.icon.is_some(), c.wide))
                .collect::<Vec<_>>()
        };
        let cap = |label: &str, icon, wide| (label.to_string(), icon, wide);
        // Windows and Linux: names, Shift with its arrow, a wide Space.
        assert_eq!(
            caps("ctrl-shift-space", false),
            vec![
                cap("Ctrl", false, false),
                cap("Shift", true, false),
                cap("Space", false, true)
            ]
        );
        // macOS: the symbols alone, in its ⌃⌥⇧⌘ order.
        assert_eq!(
            caps("cmd-alt-k", true),
            vec![
                cap("", true, false),
                cap("", true, false),
                cap("K", false, false)
            ]
        );
        assert_eq!(
            caps("alt-f1", false),
            vec![cap("Alt", false, false), cap("F1", false, false)]
        );
        // Spanish says Espacio; Shift stays Shift (eadfe73).
        let k = Keystroke::parse("ctrl-shift-space").unwrap();
        let es: Vec<_> = key_caps(&k, Lang::Es.strings(), false)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(es, ["Ctrl", "Shift", "Espacio"]);
    }
}
