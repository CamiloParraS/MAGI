//! First-run onboarding (FR-10) in two screens, three when Ask Magi is on.
//! Setup: folders, with each one's status as the permission check; search
//! features, with consent to download; start with the computer; Ask Magi,
//! off until chosen. Confirming applies them and saves `done` to
//! `ui.onboarding`. With Ask Magi on and nobody signed in, an account screen
//! follows (skippable: search needs no account); then the first index's
//! progress. A plain launch opens onboarding until then. Closing it leaves
//! the app running.

mod account;
mod finish;
mod frame;
mod plan;
mod setup;

pub use plan::*;

use gpui_kit::*;
use magi_core::config::{Onboarding, UiConfig};
use magi_core::dto::{ErrorCode, RootStatus};
use magi_core::features::Feature;
use magi_core::host::Host;
use serde_json::json;
use std::path::PathBuf;

use crate::app::{AppEvent, Events};
use crate::ask::view::sign_in::SignInForm;
use crate::live::Live;
use crate::settings::turn_on;
use crate::theme;
use crate::ui::folder_list::FolderList;
use crate::ui::folders::{add_roots, pick_and_add};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Setup,
    /// Sign in or create an account; only after setup turned Ask Magi on.
    Account,
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
    /// "Ask Magi", off by default: it is the one feature that goes online.
    ask: bool,
    /// The form on the account screen.
    sign_in: Entity<SignInForm>,
    /// The account screen was shown on the way to the finish screen, so Back
    /// returns to it.
    account_shown: bool,
    /// The account screen just opened: its next render focuses the email.
    focus_account: bool,
    /// Documents, Desktop and Pictures, offered with one click.
    user_folders: Vec<PathBuf>,
    folder_list: FolderList,
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
    /// Whether it has said a feature is still downloading: once that ends,
    /// the title and intro fade to their new words instead of jumping.
    was_downloading: std::cell::Cell<bool>,
    focus: FocusHandle,
    /// The folder list's scroll, shared with its scrollbar.
    scroll: ScrollHandle,
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
        let folder_list = FolderList::open(&host);
        let sign_in = cx.new(|cx| SignInForm::new(host.clone(), live.clone(), window, cx));
        let subscriptions = vec![
            cx.observe_in(&live, window, |this, _, window, cx| {
                this.prune_removing(cx);
                this.sync_appearance(window, cx);
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_appearance(window, cx);
            }),
            cx.observe(&sign_in, |_, _, cx| cx.notify()),
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
            ask: false,
            sign_in,
            account_shown: false,
            focus_account: false,
            user_folders: magi_core::paths::user_folders(),
            folder_list,
            hotkey: ui.hotkey,
            busy: false,
            error: None,
            dark: false,
            forward: true,
            hotkey_presses,
            was_preparing: Default::default(),
            was_downloading: Default::default(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
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

    /// Turns the Ask Magi switch on or off.
    pub fn set_ask(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ask = on;
        cx.notify();
    }

    fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.live.read(cx).theme.clone();
        self.dark = theme::sync(&theme, window, cx);
        let dark = self.dark;
        self.sign_in.update(cx, |form, _| form.set_dark(dark));
        cx.notify();
    }

    fn roots<'a>(&'a self, cx: &'a App) -> Option<&'a Vec<RootStatus>> {
        self.folder_list.roots(self.live.read(cx).status.as_ref())
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
                // Signing in is optional: the same button skips it.
                Step::Account | Step::Indexing => true,
            }
    }

    /// Whether the account screen has anything to do: Ask Magi is on and
    /// nobody is signed in.
    fn needs_account(&self, cx: &App) -> bool {
        self.ask && !self.is_signed_in(cx)
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
                    "ask_magi": self.ask,
                    "onboarding": Onboarding::Done,
                });
                let then = if self.needs_account(cx) {
                    Step::Account
                } else {
                    Step::Indexing
                };
                self.account_shown = then == Step::Account;
                self.apply(
                    then,
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
            Step::Account => self.go(Step::Indexing, cx),
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

    /// One screen back, to change a choice. Setup is already applied;
    /// confirming it again applies only what changed.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.recording {
            return;
        }
        match self.step {
            Step::Setup => {}
            Step::Account => self.go(Step::Setup, cx),
            Step::Indexing if self.account_shown => self.go(Step::Account, cx),
            Step::Indexing => self.go(Step::Setup, cx),
        }
    }

    pub(crate) fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.forward = step > self.step;
        self.focus_account = step == Step::Account;
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

    fn prune_removing(&mut self, cx: &App) {
        self.folder_list.prune(self.live.read(cx).status.as_ref());
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
                        view.folder_list.set_opening(roots);
                        view.prune_removing(cx);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a folder change failed");
                        view.error = Some(ErrorCode::from(&error));
                        // The rows still there work again.
                        view.folder_list.all_removals_failed();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_folder(&mut self, cx: &mut Context<Self>) {
        pick_and_add(cx, Self::add_folders);
    }

    fn add_folders(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.run_root(move |host| add_roots(host, &paths), cx);
    }
}

#[cfg(test)]
mod tests;
