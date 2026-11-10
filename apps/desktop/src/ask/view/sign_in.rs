//! The Ask Magi sign-in form, the same in the Ask window and in Settings ›
//! Account: email, password, Create account and Sign in. Signing in tells
//! every window through `Live`, which is how both places move on.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::host::Host;

use crate::ask::{Problem, ask_error, credentials_ok, problem_of, problem_text};
use crate::live::Live;
use crate::theme::Palette;
use crate::ui::{pressable, subheading, warn_note};

pub struct SignInForm {
    host: Host,
    live: Entity<Live>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    /// Which half of the segmented switch is up: Create account or Sign in.
    create: bool,
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
            create: false,
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

    pub fn creating(&self) -> bool {
        self.create
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
            InputEvent::PressEnter { .. } => self.submit(window, cx),
            _ => {}
        }
    }

    fn credentials(&self, cx: &App) -> (String, String) {
        (
            self.email.read(cx).value().trim().to_owned(),
            self.password.read(cx).value().to_string(),
        )
    }

    /// Does what the switch says: sign in, or create the account.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (email, password) = self.credentials(cx);
        if credentials_ok(&email, &password) {
            let create = self.create;
            self.authenticate(email, password, create, window, cx);
        }
    }

    fn set_mode(&mut self, create: bool, cx: &mut Context<Self>) {
        if self.create != create && !self.working {
            self.create = create;
            self.problem = None;
            cx.notify();
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

/// A labelled field with its icon inside the box.
fn field(label: &'static str, icon: IconName, input: Input, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(subheading(label, p))
        .child(
            input
                .large()
                .prefix(Icon::new(icon).size(px(15.)).text_color(p.mute)),
        )
}

/// One half of the Sign in / Create account switch. The track is the window
/// color, so the chosen half reads as the raised one.
fn segment(
    id: &'static str,
    label: &'static str,
    on: bool,
    p: &Palette,
    click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    pressable(
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(30.))
            .rounded(px(7.))
            .border_1()
            .text_size(px(13.))
            .map(|d| match on {
                true => d
                    .bg(p.card)
                    .border_color(p.line)
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(!p.dark, |d| d.shadow_sm()),
                false => d
                    .border_color(gpui_kit::transparent_black())
                    .text_color(p.mute)
                    .hover(|d| d.text_color(p.ink)),
            })
            .child(label),
        id,
    )
    .flex_1()
    .cursor_pointer()
    .role(Role::Tab)
    .aria_label(label)
    .aria_selected(on)
    .on_click(move |_, window, cx| click(window, cx))
}

impl Render for SignInForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        let (email, password) = self.credentials(cx);
        let ready = credentials_ok(&email, &password) && !self.working;
        let this = cx.entity();
        let (to_sign_in, to_create) = (this.clone(), this);
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .w_full()
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .p(px(3.))
                    .rounded(px(10.))
                    .bg(p.solid)
                    .border_1()
                    .border_color(p.line)
                    .child(segment(
                        "mode-sign-in",
                        s.ask_sign_in,
                        !self.create,
                        &p,
                        move |_, cx| {
                            to_sign_in.update(cx, |form, cx| form.set_mode(false, cx));
                        },
                    ))
                    .child(segment(
                        "mode-create",
                        s.ask_create_account,
                        self.create,
                        &p,
                        move |_, cx| {
                            to_create.update(cx, |form, cx| form.set_mode(true, cx));
                        },
                    )),
            )
            .child(field(
                s.ask_email,
                IconName::Mail,
                Input::new(&self.email),
                &p,
            ))
            .child(field(
                s.ask_password,
                IconName::LockKeyhole,
                Input::new(&self.password).mask_toggle(),
                &p,
            ))
            .children(
                self.problem
                    .map(|problem| warn_note(&p, problem_text(problem, s)).mt_0()),
            )
            .child(
                Button::new("sign-in-submit")
                    .cursor_pointer()
                    .primary()
                    .w_full()
                    .label(if self.create {
                        s.ask_create_account
                    } else {
                        s.ask_sign_in
                    })
                    .loading(self.working)
                    .disabled(!ready)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
    }
}
