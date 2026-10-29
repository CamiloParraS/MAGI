//! First-run onboarding (FR-10) in two screens. Setup: folders, with each
//! one's status as the permission check; search features, with consent to
//! download; start with the computer. Confirming applies all three and saves
//! `done` to `ui.onboarding`; then the first index's progress. A plain launch
//! opens onboarding until then. Closing it leaves the app running.

mod finish;
mod setup;

use gpui_kit::assets::IconName;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::config::{FeaturesConfig, Onboarding, UiConfig};
use magi_core::dto::{ErrorCode, FeatureStatus, IndexState, IndexStatus, Install, RootStatus};
use magi_core::features::Feature;
use magi_core::host::Host;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

use crate::app::{AppEvent, Events};
use crate::live::Live;
use crate::settings::{error_text, turn_on};
use crate::theme::{self, Palette};
use crate::ui::folders::pick_folders;
use crate::ui::{note, warn_note};

pub const SIZE: Size<Pixels> = size(px(880.), px(600.));
/// The smallest the window resizes to: below this width the two columns and
/// the four suggestion chips start to wrap; the screen scrolls in height.
pub const MIN_SIZE: Size<Pixels> = size(px(800.), px(520.));

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

/// The features checked when the step opens: the ones labelled Recommended
/// (ADR-0010's selection: meaning and image text), whatever the config says
/// until the user confirms.
pub fn preselected(features: &[FeatureStatus]) -> Vec<Feature> {
    let recommended = FeaturesConfig::default();
    features
        .iter()
        .filter(|f| recommended.enabled(f.feature))
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
    /// Documents, Desktop and Pictures, offered with one click.
    user_folders: Vec<PathBuf>,
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
    /// Whether the finish screen has shown its progress: once it has, the
    /// block fades out when ready instead of vanishing.
    was_preparing: std::cell::Cell<bool>,
    focus: FocusHandle,
    /// The folder list's scroll, shared with its scrollbar.
    scroll: ScrollHandle,
    /// Roots being removed: dimmed, their buttons off, until the list no
    /// longer has them.
    removing: Vec<i64>,
    /// Focused while the finish step's shortcut recorder waits for keys.
    recorder: FocusHandle,
    recording: bool,
    /// The modifiers held down, lit on the finish screen's keycaps.
    held: Modifiers,
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
            cx.observe_in(&live, window, |this, _, window, cx| {
                this.prune_removing(cx);
                this.sync_appearance(window, cx);
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_appearance(window, cx);
            }),
            // The key-ups go to whichever window is active by then (search,
            // after the hotkey): unlight them all.
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.held = Modifiers::default();
                    cx.notify();
                }
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
            user_folders: magi_core::paths::user_folders(),
            opening_roots,
            hotkey: ui.hotkey,
            busy: false,
            error: None,
            dark: false,
            forward: true,
            hotkey_presses,
            was_preparing: Default::default(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            removing: Vec::new(),
            recorder: cx.focus_handle(),
            recording: false,
            held: Modifiers::default(),
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
                let chosen = self.chosen(cx);
                let changes = feature_changes(&self.live.read(cx).features, &chosen);
                self.chosen = Some(chosen);
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

    /// Forgets the removals the folder list has caught up with.
    fn prune_removing(&mut self, cx: &App) {
        let ids: Vec<i64> = self.roots(cx).map_or(Vec::new(), |roots| {
            roots.iter().map(|root| root.id).collect()
        });
        self.removing.retain(|id| ids.contains(id));
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
                        view.prune_removing(cx);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a folder change failed");
                        view.error = Some(ErrorCode::from(&error));
                        // The rows still there work again.
                        view.removing.clear();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let picked = pick_folders(cx);
        cx.spawn(async move |this, cx| {
            let paths = picked.await;
            if !paths.is_empty() {
                let _ = this.update(cx, |view, cx| view.add_folders(paths, cx));
            }
        })
        .detach();
    }

    /// Folders dropped from the file manager: the user picking them, as
    /// with the picker. Dropped files are ignored.
    fn drop_folders(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        self.add_folders(paths.paths().to_vec(), cx);
    }

    fn add_folders(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
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
}

impl Render for OnboardingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        let body = match self.step {
            Step::Setup => self.setup(s, &p, cx),
            Step::Indexing => self.indexing(s, &p, cx),
        };
        let can_continue = self.can_continue(cx);
        let no_folder = self.roots(cx).is_none_or(|roots| roots.is_empty());
        let folder_count = self.roots(cx).map_or(0, |roots| roots.len());
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
        // What Start sets up, in one line: the folders and the download.
        let summary: SharedString = if no_folder {
            s.ob_need_folder.into()
        } else {
            let lang = self.live.read(cx).lang;
            let folders = lang.plural(&s.ob_folder_count, folder_count as u64);
            if total > 0 {
                format!(
                    "{folders} · {}",
                    s.ob_download_size.replace("{size}", &lang.size(total))
                )
            } else {
                format!("{folders} · {}", s.ob_nothing_to_download)
            }
            .into()
        };
        // A soft wash of the accent behind the top of the window: light mode
        // otherwise has no color but the button.
        let wash = p.wash(px(260.));
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
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| {
                this.held = e.modifiers;
                cx.notify();
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
                    .pt(px(16.))
                    .pb(px(16.))
                    // The finish screen is short: centered in the space,
                    // not stacked at the top.
                    .flex()
                    .flex_col()
                    .when(self.step == Step::Indexing, |d| d.justify_center())
                    .child(slide_in(body, self.step, self.forward))
                    .children(
                        self.error
                            .as_ref()
                            .map(|error| warn_note(&p, error_text(error, s)).mt(px(10.))),
                    ),
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
                        bar.child(
                            note(&p)
                                .mt_0()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(summary),
                        )
                    })
                    // Back across from the way forward, where wizards keep it.
                    .when(self.step == Step::Indexing, |bar| {
                        bar.child(
                            Button::new("back")
                                .cursor_pointer()
                                .ghost()
                                .icon(IconName::ArrowLeft)
                                .label(s.back)
                                .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                        )
                    })
                    .when(self.step == Step::Indexing, |bar| bar.child(div().flex_1()))
                    .child(
                        Button::new("next")
                            .when(can_continue, |b| b.cursor_pointer())
                            .primary()
                            .label(next_label)
                            .loading(self.busy)
                            .disabled(!can_continue && !self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            )
    }
}

/// How long a screen takes to slide in; onboarding is seen once, so a
/// little longer than [`theme::FADE`].
const STEP_IN: Duration = Duration::from_millis(220);

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

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has its own `test` attribute.
    use super::{Readiness, download_total, feature_changes, preselected, progress, readiness};
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
            paused_for: None,
            scanned: 0,
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
}
