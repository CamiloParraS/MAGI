//! The Ask Magi window, laid out like a chat: the conversation scrolls
//! above, and the dock under it holds what gets sent (the chips) and the
//! question box. Signed out, the same window shows the sign-in form. Every
//! call to `Host` runs on the background executor.

mod chat;
pub mod sign_in;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::*;
use magi_core::host::Host;

use super::{AskModel, Outgoing, Phase, Request};
use crate::ask::ask_error;
use crate::live::Live;
use crate::theme::{self, Backdrop, Palette};
use crate::ui::loading_track;
use sign_in::SignInForm;

actions!(magi_ask, [Close]);

const CONTEXT: &str = "AskWindow";
pub const SIZE: Size<Pixels> = size(px(620.), px(680.));
pub const MIN_SIZE: Size<Pixels> = size(px(480.), px(440.));

/// [`SIZE`], no taller than most of the screen it opens on: a scaled
/// laptop screen can be under 700 px tall, and the question box is at the
/// bottom.
pub fn fit_to(screen: Option<Size<Pixels>>) -> Size<Pixels> {
    let Some(screen) = screen else {
        return SIZE;
    };
    let room = (screen.height * 0.86).max(MIN_SIZE.height);
    size(SIZE.width, SIZE.height.min(room))
}
/// How wide the chat gets in a maximized window, for readable lines.
const COLUMN: Pixels = px(680.);
/// The question box, from its opening height to where it starts scrolling.
const QUESTION_ROWS: (usize, usize) = (1, 6);

pub struct AskView {
    host: Host,
    live: Entity<Live>,
    model: AskModel,
    question: Entity<TextareaState>,
    sign_in: Entity<SignInForm>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    backdrop: Backdrop,
    dark: bool,
    /// `ui.theme`, as last applied.
    theme: String,
    /// A source whose file could not be found when opened.
    gone: Option<i64>,
    /// The question box's placeholder, as last set.
    placeholder: &'static str,
    _subscriptions: Vec<Subscription>,
}

impl AskView {
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([KeyBinding::new("escape", Close, Some(CONTEXT))]);
    }

    pub fn new(
        host: Host,
        live: Entity<Live>,
        backdrop: Backdrop,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // From the config alone: no keychain read and no request.
        let account = host
            .account_state()
            .inspect_err(|error| tracing::warn!(%error, "could not read the account state"))
            .unwrap_or_default();
        live.update(cx, |live, _| live.account = account.clone());
        let question = cx.new(|cx| {
            let (min, max) = QUESTION_ROWS;
            let mut input = TextareaState::new(window, cx)
                .auto_grow(min, max)
                .submit_on_enter(true);
            input.set_value(request.question.clone(), window, cx);
            input
        });
        let sign_in = cx.new(|cx| SignInForm::new(host.clone(), live.clone(), window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &question,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.send(window, cx),
                    _ => {}
                },
            ),
            cx.observe_in(&live, window, |this, live, window, cx| {
                if live.read(cx).theme != this.theme {
                    this.sync_appearance(window, cx);
                }
                let account = live.read(cx).account.clone();
                if account != *this.model.account() {
                    this.change(window, cx, |model| model.sync_account(account));
                }
                cx.notify();
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_appearance(window, cx);
            }),
        ];
        let mut view = Self {
            host,
            live,
            model: AskModel::new(account, request),
            question,
            sign_in,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            backdrop,
            dark: false,
            theme: String::new(),
            gone: None,
            placeholder: "",
            _subscriptions: subscriptions,
        };
        view.sync_appearance(window, cx);
        view.enter_phase(window, cx);
        view
    }

    pub fn model(&self) -> &AskModel {
        &self.model
    }

    pub fn sign_in_form(&self) -> &Entity<SignInForm> {
        &self.sign_in
    }

    fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme = self.live.read(cx).theme.clone();
        self.dark = theme::sync(&self.theme, window, cx);
        let dark = self.dark;
        self.sign_in.update(cx, |form, _| form.set_dark(dark));
        cx.notify();
    }

    /// Applies a change to the model, then follows it: a new phase gets the
    /// focus, and every window learns who is signed in.
    fn change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut AskModel),
    ) {
        let before = self.model.phase();
        apply(&mut self.model);
        if self.model.phase() != before {
            self.enter_phase(window, cx);
        }
        let account = self.model.account().clone();
        if self.live.read(cx).account != account {
            self.live.update(cx, |live, cx| {
                live.account = account;
                cx.notify();
            });
        }
        cx.notify();
    }

    /// The focus goes where the next keystroke belongs.
    fn enter_phase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gone = None;
        match self.model.phase() {
            Phase::SignIn { .. } => {
                self.sign_in.update(cx, |form, cx| form.focus(window, cx));
            }
            Phase::Chat => {
                self.question
                    .update(cx, |question, cx| question.focus(window, cx));
                self.scroll.scroll_to_bottom();
            }
        }
    }

    /// Asking from another search, with this window already open: a new
    /// chat about its results.
    pub fn replace(&mut self, request: Request, window: &mut Window, cx: &mut Context<Self>) {
        let question = request.question.clone();
        self.model.replace(request);
        self.question
            .update(cx, |input, cx| input.set_value(question, window, cx));
        self.enter_phase(window, cx);
        cx.notify();
    }

    /// Puts `text` in the question box, as typing would.
    pub fn set_question(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.question
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// Sends what is in the box. Does nothing unless the model allows it.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.question.read(cx).value();
        let mut outgoing = None;
        self.change(window, cx, |model| outgoing = model.start(&text));
        if let Some(outgoing) = outgoing {
            self.question
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.dispatch(outgoing, window, cx);
        }
    }

    /// Sends the failed question again.
    pub fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut outgoing = None;
        self.change(window, cx, |model| outgoing = model.retry());
        if let Some(outgoing) = outgoing {
            self.dispatch(outgoing, window, cx);
        }
    }

    /// The first question also saves that the disclosure was seen; if that
    /// fails it only shows again next time.
    fn dispatch(&mut self, outgoing: Outgoing, window: &mut Window, cx: &mut Context<Self>) {
        let Outgoing {
            generation,
            question,
            sources,
            accept_disclosure,
        } = outgoing;
        self.scroll.scroll_to_bottom();
        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    if accept_disclosure && let Err(error) = host.accept_ask_disclosure() {
                        tracing::warn!(%error, "could not save the accepted disclosure");
                    }
                    host.ask(&question, &sources)
                })
                .await
                .map_err(ask_error);
            let _ = this.update_in(cx, |view, window, cx| {
                view.change(window, cx, |model| model.finished(generation, done));
                view.scroll.scroll_to_bottom();
            });
        })
        .detach();
    }

    /// "Got it" on the disclosure, before any question.
    pub fn accept_disclosure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { host.accept_ask_disclosure() })
                .await;
            let _ = this.update_in(cx, |view, window, cx| match done {
                Ok(account) => {
                    view.change(window, cx, |model| model.accepted_disclosure(account));
                }
                Err(error) => tracing::warn!(%error, "could not save the accepted disclosure"),
            });
        })
        .detach();
    }

    /// The first question, then follow-ups; only set when it changes, since
    /// setting it redraws.
    fn sync_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.live.read(cx).lang.strings();
        let wanted = match self.model.turns().is_empty() {
            true => s.ask_placeholder,
            false => s.ask_follow_up,
        };
        if wanted != self.placeholder {
            self.placeholder = wanted;
            self.question
                .update(cx, |input, cx| input.set_placeholder(wanted, window, cx));
        }
    }

    /// By `file_id` only, through `Host`, like the search window.
    fn open_file(&mut self, file_id: i64, cx: &mut Context<Self>) {
        match self.host.file_path(file_id) {
            Ok(path) => {
                self.gone = None;
                cx.open_with_system(&path);
            }
            Err(error) => {
                tracing::warn!(file_id, %error, "a cited file could not be resolved");
                self.gone = Some(file_id);
            }
        }
        cx.notify();
    }
}

impl Render for AskView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        self.sync_placeholder(window, cx);
        let asking = self.model.is_asking();
        let body = match self.model.phase() {
            Phase::SignIn { expired } => self.sign_in_scene(expired, &p, cx).into_any_element(),
            Phase::Chat => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(self.conversation(&p, cx))
                .child(self.dock(&p, cx))
                .into_any_element(),
        };
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Close, window, _| window.remove_window()))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.solid.opacity(p.tint_alpha(self.backdrop)))
            .text_color(p.ink)
            .text_size(px(14.))
            .child(p.wash(px(160.)))
            .child(loading_track(&p, asking))
            .child(body)
    }
}
