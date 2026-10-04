//! First-run onboarding (FR-10): folders, with each one's status as the
//! permission check → search features, with consent to download → start with
//! the computer → the first index's progress. Each step saves the next one to
//! `ui.onboarding` when it completes; a plain launch opens onboarding until
//! the background step saves `done`, and it resumes at the saved step.
//! Closing it at any step leaves the app running.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::config::{Onboarding, UiConfig};
use magi_core::dto::{ErrorCode, FeatureStatus, IndexState, IndexStatus, Install, RootStatus};
use magi_core::features::Feature;
use magi_core::host::Host;
use serde_json::json;
use std::time::Duration;

use crate::app::{AppEvent, Events};
use crate::i18n::Strings;
use crate::search::view::{Live, turn_on};
use crate::settings::view::{card, heading, note, pick_folder, stack};
use crate::settings::{error_text, hotkey_keystroke, root_state};
use crate::theme::{self, Palette, Tone};

pub const SIZE: Size<Pixels> = size(px(640.), px(560.));

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
    Folders,
    Features,
    Background,
    Indexing,
}

/// The saved resume point; `Done` only shows if a window was already open.
impl From<Onboarding> for Step {
    fn from(saved: Onboarding) -> Self {
        match saved {
            Onboarding::Folders => Self::Folders,
            Onboarding::Features => Self::Features,
            Onboarding::Background => Self::Background,
            Onboarding::Done => Self::Indexing,
        }
    }
}

const STEPS: [Step; 4] = [
    Step::Folders,
    Step::Features,
    Step::Background,
    Step::Indexing,
];

impl Step {
    fn index(self) -> usize {
        STEPS.iter().position(|&s| s == self).unwrap_or_default()
    }

    fn title(self, s: &Strings) -> &'static str {
        match self {
            Self::Folders => s.ob_folders,
            Self::Features => s.ob_features,
            Self::Background => s.ob_background,
            Self::Indexing => s.ob_done,
        }
    }

    fn intro(self, s: &Strings) -> Option<&'static str> {
        match self {
            Self::Folders => Some(s.ob_folders_note),
            Self::Features => Some(s.ob_features_note),
            Self::Background => Some(s.ob_background_note),
            // Depends on how far the files are: `indexing` says it.
            Self::Indexing => None,
        }
    }

    /// Indexing has started on the last step: nothing to go back to.
    fn back(self) -> Option<Self> {
        match self {
            Self::Features => Some(Self::Folders),
            Self::Background => Some(Self::Features),
            Self::Folders | Self::Indexing => None,
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
    /// Which way the last step change went; the new step slides in from it.
    forward: bool,
    /// `hotkey_presses` when the window opened: a press after it is the
    /// user trying the hotkey.
    hotkey_presses: u32,
    focus: FocusHandle,
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

    /// Folders needs one folder; features needs the features to show.
    fn can_continue(&self, cx: &App) -> bool {
        !self.busy
            && match self.step {
                Step::Folders => self.roots(cx).is_some_and(|roots| !roots.is_empty()),
                Step::Features => !self.live.read(cx).features.is_empty(),
                Step::Background | Step::Indexing => true,
            }
    }

    /// Applies the step's choices, then moves on; the last step opens search.
    pub fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_continue(cx) {
            return;
        }
        match self.step {
            Step::Folders => self.apply(
                Step::Features,
                |host| save_step(host, json!({ "onboarding": Onboarding::Features })),
                cx,
            ),
            Step::Features => {
                let changes = feature_changes(&self.live.read(cx).features, &self.chosen(cx));
                self.apply(
                    Step::Background,
                    move |host| {
                        for (feature, on) in changes {
                            if on {
                                turn_on(host, feature)?;
                            } else {
                                host.set_feature_enabled(feature, false)?;
                            }
                        }
                        save_step(host, json!({ "onboarding": Onboarding::Background }))
                    },
                    cx,
                );
            }
            Step::Background => {
                let ui = json!({
                    "launch_at_login": self.launch_at_login,
                    "onboarding": Onboarding::Done,
                });
                self.apply(Step::Indexing, move |host| save_step(host, ui), cx);
            }
            Step::Indexing => {
                window.remove_window();
                let _ = self.events.try_send(AppEvent::Toggle);
            }
        }
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.forward = step.index() > self.step.index();
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

    /// FR-10's permission check: each folder's status says whether Magi can
    /// read it.
    fn folders(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let rows = self.roots(cx).map(|roots| {
            if roots.is_empty() {
                return stack().child(card(p).text_color(p.mute).child(s.no_folders));
            }
            stack().children(roots.iter().map(|root| {
                let (badge, tone, explain) = root_state(root, s);
                let id = root.id;
                let icon = match tone {
                    Tone::Warn | Tone::Err => {
                        Icon::new(IconName::TriangleAlert).text_color(p.tone(tone))
                    }
                    _ => Icon::new(IconName::Folder).text_color(p.mute),
                };
                card(p)
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
                                    this.run_root(
                                        |host| host.engine().map(|e| drop(e.rescan())),
                                        cx,
                                    );
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
            }))
        });
        let none_yet = self.roots(cx).is_none_or(|roots| roots.is_empty());
        stack().children(rows).child(
            div().mt(px(6.)).flex().child(
                Button::new("add-folder")
                    .when(none_yet, |b| b.primary())
                    .small()
                    .icon(IconName::Plus)
                    .label(s.add_folder)
                    .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
            ),
        )
    }

    /// The download consent (FR-10, ADR-0010): each feature's exact size, and
    /// the total the chosen ones download.
    fn features(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let lang = live.lang;
        let chosen = self.chosen(cx);
        let rows = live.features.iter().map(|f| {
            let feature = f.feature;
            let text = s.feature(feature);
            let size = match f.install {
                Install::Installed { size_bytes } => {
                    s.installed.replace("{size}", &lang.size(size_bytes))
                }
                _ => s
                    .not_downloaded
                    .replace("{size}", &lang.size(f.download_size)),
            };
            card(p).items_start().child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Checkbox::new(("feature", feature as usize))
                            .label(text.name)
                            .checked(chosen.contains(&feature))
                            .on_click(cx.listener(move |this, &on: &bool, _, cx| {
                                this.toggle_feature(feature, on, cx)
                            })),
                    )
                    .child(note(p).pl(px(24.)).child(text.pitch))
                    .child(note(p).pl(px(24.)).child(size)),
            )
        });
        stack().children(rows)
    }

    /// Beside "Download and continue": the size is read as it is agreed to.
    fn download_summary(&self, s: &Strings, cx: &App) -> String {
        let live = self.live.read(cx);
        let total = download_total(&live.features, &self.chosen(cx));
        if total > 0 {
            s.ob_download_total
                .replace("{size}", &live.lang.size(total))
        } else {
            s.ob_nothing_to_download.into()
        }
    }

    fn background(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let hotkey = Keystroke::parse(&hotkey_keystroke(&self.hotkey))
            .ok()
            .map(Kbd::new);
        stack()
            .child(
                card(p).items_start().child(
                    div()
                        .flex_1()
                        .child(
                            Checkbox::new("start-at-login")
                                .label(s.launch_at_login)
                                .checked(self.launch_at_login)
                                .on_click(cx.listener(|this, &on: &bool, _, cx| {
                                    this.launch_at_login = on;
                                    cx.notify();
                                })),
                        )
                        .child(note(p).pl(px(24.)).child(s.launch_at_login_note)),
                ),
            )
            .child(
                div()
                    .mt(px(8.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(p.mute)
                    .child(s.ob_search_with)
                    .children(hotkey),
            )
            .children(self.hotkey_tried(cx).then(|| {
                theme::fade_in(
                    note(p)
                        .mt(px(6.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(p.tone(Tone::Ok))
                        .child(Icon::new(IconName::Check).size(px(14.)))
                        .child(s.ob_hotkey_works),
                    "hotkey-works",
                )
            }))
    }

    fn hotkey_tried(&self, cx: &App) -> bool {
        self.live.read(cx).hotkey_presses > self.hotkey_presses
    }

    /// The finish line: setup is done, so it leads with the shortcut and
    /// says how far along the files are in one plain sentence, no counts
    /// (Settings › Index has those).
    fn indexing(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        // The page's main element: Kbd's own size is a hint's.
        let hotkey = Keystroke::parse(&hotkey_keystroke(&self.hotkey))
            .ok()
            .map(|k| Kbd::new(k).outline().text_size(px(18.)).px_3().py(px(6.)));
        let tried = self.hotkey_tried(cx);
        let shortcut = card(p)
            .flex_col()
            .items_center()
            .gap_2()
            .py(px(22.))
            .child(div().text_color(p.mute).child(s.ob_try_hotkey))
            .children(hotkey)
            .child(if tried {
                theme::fade_in(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(p.tone(Tone::Ok))
                        .child(Icon::new(IconName::Check).size(px(14.)))
                        .child(s.ob_hotkey_works),
                    "done-hotkey-works",
                )
                .into_any_element()
            } else {
                note(p).child(s.ob_try_it).into_any_element()
            });
        let ready = readiness(live.status.as_ref(), &live.features);
        let sentence = match ready {
            Readiness::Ready => s.ob_ready.to_string(),
            Readiness::Preparing(Some(feature)) => s
                .ob_preparing_feature
                .replace("{name}", s.feature(feature).name),
            Readiness::Preparing(None) => s.ob_preparing.to_string(),
        };
        // A thin bar while preparing; it has no numbers next to it.
        let bar = (ready != Readiness::Ready).then(|| {
            let bar = Progress::new("preparing").color(p.accent);
            match live.status.as_ref().and_then(progress) {
                Some(value) => bar.value(value),
                None => bar.loading(true),
            }
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
        // "Results get better" only while they still will.
        let intro = match ready {
            Readiness::Ready => s.ob_done_note_ready,
            Readiness::Preparing(_) => s.ob_done_note,
        };
        stack()
            .child(note(p).mt_0().mb(px(10.)).text_sm().child(intro))
            .child(shortcut)
            .child(
                div()
                    .mt(px(14.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(sentence)
                    .children(bar),
            )
            .children(failed)
            .child(note(p).mt(px(14.)).child(s.ob_tray_hint))
    }
}

impl Render for OnboardingView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        let body = match self.step {
            Step::Folders => self.folders(s, &p, cx),
            Step::Features => self.features(s, &p, cx),
            Step::Background => self.background(s, &p, cx),
            Step::Indexing => self.indexing(s, &p, cx),
        };
        let can_continue = self.can_continue(cx);
        let next_label = match self.step {
            Step::Features
                if download_total(&self.live.read(cx).features, &self.chosen(cx)) > 0 =>
            {
                s.download_and_continue
            }
            Step::Indexing => s.ob_start_searching,
            _ => s.continue_,
        };
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Next, window, cx| this.next(window, cx)))
            .on_action(cx.listener(|this, _: &Back, _, cx| {
                if let Some(step) = this.step.back().filter(|_| !this.busy) {
                    this.go(step, cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.solid)
            .text_color(p.ink)
            .text_size(px(14.))
            .child(
                div()
                    .id("onboarding")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(32.))
                    .pt(px(24.))
                    // The last step is a finish line, not a step to do.
                    .child(div().mb(px(4.)).text_xs().text_color(p.mute).when(
                        self.step != Step::Indexing,
                        |d| {
                            d.child(
                                s.step_of
                                    .replace("{n}", &(self.step.index() + 1).to_string())
                                    .replace("{total}", &(STEPS.len() - 1).to_string()),
                            )
                        },
                    ))
                    .child(heading(self.step.title(s)).mb(px(6.)))
                    .children(
                        self.step
                            .intro(s)
                            .map(|text| note(&p).mb(px(16.)).text_sm().child(text)),
                    )
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
                    .when(self.step == Step::Features, |bar| {
                        bar.child(
                            note(&p)
                                .text_color(p.ink)
                                .child(self.download_summary(s, cx)),
                        )
                    })
                    .child(div().flex_1())
                    .children(self.step.back().map(|step| {
                        Button::new("back")
                            .ghost()
                            .label(s.back)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.go(step, cx)))
                    }))
                    .child(
                        Button::new("next")
                            .primary()
                            .label(next_label)
                            .loading(self.busy)
                            .disabled(!can_continue && !self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            )
    }
}

/// How long a step takes to slide in; onboarding is seen once, so a little
/// longer than [`theme::FADE`].
const STEP_IN: Duration = Duration::from_millis(220);

/// A step's body fades in from 8px toward where it came from: forward from
/// the right, back from the left. Under reduced motion GPUI shows the end
/// state.
fn slide_in(body: Div, step: Step, forward: bool) -> impl IntoElement {
    let from = if forward { 8. } else { -8. };
    body.relative().with_animation(
        ("step", step.index()),
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
