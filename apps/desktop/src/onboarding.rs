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
use magi_core::dto::{ErrorCode, FeatureStatus, Install, RootStatus};
use magi_core::features::Feature;
use magi_core::host::Host;
use serde_json::json;

use crate::app::{AppEvent, Events};
use crate::i18n::Strings;
use crate::search::view::{Live, turn_on};
use crate::settings::view::{card, heading, note, pick_folder, stack};
use crate::settings::{download_error_label, error_text, hotkey_keystroke, root_state};
use crate::theme::{self, Palette, Tone};
use crate::tray::{status_line, status_tone};

pub const SIZE: Size<Pixels> = size(px(640.), px(560.));

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
            Self::Indexing => s.ob_indexing,
        }
    }

    fn intro(self, s: &Strings) -> Option<&'static str> {
        match self {
            Self::Folders => Some(s.ob_folders_note),
            Self::Features => Some(s.ob_features_note),
            Self::Background => Some(s.ob_background_note),
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
            _subscriptions: subscriptions,
        };
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

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
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
        stack().children(rows).child(
            div().mt(px(6.)).flex().child(
                Button::new("add-folder")
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
        let total = download_total(&live.features, &chosen);
        let summary = if total > 0 {
            s.ob_download_total.replace("{size}", &lang.size(total))
        } else {
            s.ob_nothing_to_download.into()
        };
        stack()
            .children(rows)
            .child(note(p).mt(px(8.)).text_color(p.ink).child(summary))
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
    }

    fn indexing(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let lang = live.lang;
        let progress = live.status.as_ref().map(|st| {
            let total = st.indexed + st.queued;
            stack()
                .child(
                    Progress::new("indexing")
                        .color(p.accent)
                        .value(st.indexed as f32 * 100. / total.max(1) as f32),
                )
                .child(
                    note(p)
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(p.dot(status_tone(st)))
                        .child(status_line(st, lang)),
                )
        });
        // The downloads the features step started.
        let downloads = live.features.iter().filter_map(|f| {
            let name = s.feature(f.feature).name;
            match f.install {
                Install::Downloading { bytes, total } => Some(
                    note(p).child(
                        s.downloading
                            .replace("{name}", name)
                            .replace("{done}", &lang.size(bytes))
                            .replace("{total}", &lang.size(total)),
                    ),
                ),
                Install::Failed { code } => Some(
                    note(p)
                        .text_color(p.warn)
                        .child(format!("{name}: {}", download_error_label(code, s))),
                ),
                _ => None,
            }
        });
        stack()
            .children(progress)
            .children(downloads)
            .child(note(p).mt(px(12.)).child(s.ob_indexing_note))
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
            Step::Indexing => s.tray_open,
            _ => s.continue_,
        };
        div()
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
                    .child(
                        div().mb(px(4.)).text_xs().text_color(p.mute).child(
                            s.step_of
                                .replace("{n}", &(self.step.index() + 1).to_string())
                                .replace("{total}", &STEPS.len().to_string()),
                        ),
                    )
                    .child(heading(self.step.title(s)).mb(px(6.)))
                    .children(
                        self.step
                            .intro(s)
                            .map(|text| note(&p).mb(px(16.)).text_sm().child(text)),
                    )
                    .child(body)
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
                    // The last step: the onboarding is saved, any way out is fine.
                    .when(self.step == Step::Indexing, |bar| {
                        bar.child(
                            Button::new("open-settings")
                                .ghost()
                                .label(s.open_settings)
                                .on_click(cx.listener(|this, _, window, _| {
                                    this.leave(Some(AppEvent::Settings), window)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .when(self.step == Step::Indexing, |bar| {
                        bar.child(
                            Button::new("close").ghost().label(s.close).on_click(
                                cx.listener(|this, _, window, _| this.leave(None, window)),
                            ),
                        )
                    })
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
                            .disabled(!can_continue)
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has its own `test` attribute.
    use super::{download_total, feature_changes, preselected};
    use magi_core::dto::{DownloadError, FeatureStatus, Install};
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
}
