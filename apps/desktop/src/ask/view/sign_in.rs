//! The Ask Magi sign-in form, the same in the Ask window and in Settings ›
//! Account: email, password, Create account and Sign in. Signing in tells
//! every window through `Live`, which is how both places move on.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::*;
use magi_core::host::Host;

use crate::ask::{Problem, ask_error, credentials_ok, problem_of, problem_text};
use crate::live::Live;
use crate::theme::Palette;
use crate::ui::{subheading, warn_note};

pub struct SignInForm {
    host: Host,
    live: Entity<Live>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    working: bool,
    problem: Option<Problem>,
    dark: bool,
    _subscriptions: Vec<Subscription>,
}

impl SignInForm {
    pub fn new(
        host: Host,
        live: Entity<Live>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("name@example.com"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let subscriptions = vec![
            cx.subscribe_in(&email, window, Self::on_input_event),
            cx.subscribe_in(&password, window, Self::on_input_event),
            cx.observe(&live, |_, _, cx| cx.notify()),
        ];
        Self {
            host,
            live,
            email,
            password,
            working: false,
            problem: None,
            dark: false,
            _subscriptions: subscriptions,
        }
    }

    /// The owner's theme, for the colors this form draws itself.
    pub fn set_dark(&mut self, dark: bool) {
        self.dark = dark;
    }

    pub fn working(&self) -> bool {
        self.working
    }

    pub fn problem(&self) -> Option<Problem> {
        self.problem
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.email.update(cx, |email, cx| email.focus(window, cx));
    }

    fn on_input_event<T: 'static>(
        &mut self,
        _: &Entity<T>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => self.submit(false, window, cx),
            _ => {}
        }
    }

    fn credentials(&self, cx: &App) -> (String, String) {
        (
            self.email.read(cx).value().trim().to_owned(),
            self.password.read(cx).value().to_string(),
        )
    }

    fn submit(&mut self, create: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (email, password) = self.credentials(cx);
        if credentials_ok(&email, &password) {
            self.authenticate(email, password, create, window, cx);
        }
    }

    /// Signs in, or with `create` makes the account first.
    pub fn authenticate(
        &mut self,
        email: String,
        password: String,
        create: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.working {
            return;
        }
        self.working = true;
        self.problem = None;
        cx.notify();
        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    if create {
                        host.sign_up(&email, &password)
                    } else {
                        host.sign_in(&email, &password)
                    }
                })
                .await
                .map_err(ask_error);
            let _ = this.update_in(cx, |form, window, cx| {
                form.working = false;
                match done {
                    Ok(account) => {
                        form.password
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        form.live.update(cx, |live, cx| {
                            live.account = account;
                            cx.notify();
                        });
                    }
                    Err(error) => form.problem = Some(problem_of(&error)),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn field(label: &'static str, input: impl IntoElement, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(subheading(label, p))
        .child(input)
}

impl Render for SignInForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        let (email, password) = self.credentials(cx);
        let ready = credentials_ok(&email, &password) && !self.working;
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .w_full()
            .child(field(s.ask_email, Input::new(&self.email), &p))
            .child(field(s.ask_password, Input::new(&self.password), &p))
            .children(
                self.problem
                    .map(|problem| warn_note(&p, problem_text(problem, s)).mt_0()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .pt(px(4.))
                    .child(
                        Button::new("sign-in-create")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            .label(s.ask_create_account)
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(true, window, cx);
                            })),
                    )
                    .child(
                        Button::new("sign-in-submit")
                            .cursor_pointer()
                            .primary()
                            .small()
                            .label(s.ask_sign_in)
                            .loading(self.working)
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(false, window, cx);
                            })),
                    ),
            )
    }
}
