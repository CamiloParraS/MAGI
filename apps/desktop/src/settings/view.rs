//! The settings window: a sidebar and the Folders section (FR-1, FR-11).
//! Roots come from the latest status in [`Live`], or from the database until
//! the first one (the engine sends none during its startup walk); every
//! change goes through the engine, and the next status shows it.

use gpui_kit::component::button::Button;
use gpui_kit::component::switch::Switch;
use gpui_kit::*;
use magi_core::dto::{ErrorCode, RootStatus};
use magi_core::engine::EngineHandle;
use magi_core::host::Host;

use super::{error_text, root_line};
use crate::i18n::Strings;
use crate::search::view::Live;
use crate::theme::{self, Palette};

pub const SIZE: Size<Pixels> = size(px(900.), px(620.));

pub struct SettingsView {
    host: Host,
    live: Entity<Live>,
    dark: bool,
    /// The roots when the window opened, shown until the first status.
    opening_roots: Option<Vec<RootStatus>>,
    /// The last folder change that failed, until the next one.
    error: Option<ErrorCode>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(
        host: Host,
        live: Entity<Live>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&live, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_appearance(window, cx);
            }),
        ];
        // ponytail: one small read on the main thread; move it to the
        // background executor if it ever shows up in a frame trace.
        let opening_roots = host
            .list_roots()
            .inspect_err(|error| tracing::warn!(%error, "could not list the roots"))
            .ok();
        let mut view = Self {
            host,
            live,
            dark: false,
            opening_roots,
            error: None,
            _subscriptions: subscriptions,
        };
        view.sync_appearance(window, cx);
        view
    }

    fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.live.read(cx).theme.clone();
        self.dark = theme::sync(&theme, window, cx);
        cx.notify();
    }

    /// Runs a root change on the background executor and keeps its error.
    fn change_root(
        &mut self,
        change: impl FnOnce(EngineHandle) -> magi_core::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { host.engine().and_then(change) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.error = done.err().map(|error| {
                    tracing::warn!(%error, "a folder change failed");
                    ErrorCode::from(&error)
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// The native folder picker (FR-1), then `add_root`.
    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| match picked.await {
            Ok(Ok(Some(mut paths))) => {
                if let Some(path) = paths.pop() {
                    let _ = this.update(cx, |view, cx| {
                        view.change_root(move |engine| engine.add_root(&path).map(drop), cx);
                    });
                }
            }
            Ok(Err(error)) => tracing::warn!(%error, "the folder picker failed"),
            _ => {}
        })
        .detach();
    }

    fn folders(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let row = || {
            div()
                .flex()
                .items_center()
                .gap_4()
                .px_4()
                .py(px(14.))
                .rounded(px(4.))
                .bg(p.card)
        };
        let roots = live
            .status
            .as_ref()
            .map(|status| &status.roots)
            .or(self.opening_roots.as_ref());
        // Nothing when the roots are unknown: an empty list would read "no folders".
        let list = roots.map(|roots| {
            let group = div().flex().flex_col().gap(px(2.)).mb_3();
            if roots.is_empty() {
                return group.child(row().text_color(p.mute).child(s.no_folders));
            }
            group.children(
                roots
                    .iter()
                    .map(|root| self.root_row(root, row(), s, p, cx)),
            )
        });
        div()
            .flex()
            .flex_col()
            .items_start()
            .child(heading(s.folders))
            .child(div().w_full().children(list))
            .children(
                self.error
                    .as_ref()
                    .map(|error| div().mb_3().text_color(p.warn).child(error_text(error, s))),
            )
            .child(
                Button::new("add-folder")
                    .label(s.add_folder)
                    .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
            )
    }

    fn root_row(
        &self,
        root: &RootStatus,
        row: Div,
        s: &Strings,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Div {
        let (line, problem) = root_line(root, s);
        let id = root.id;
        row.child(
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
                .child(
                    div()
                        .mt(px(2.))
                        .text_xs()
                        .text_color(if problem { p.warn } else { p.mute })
                        .child(line),
                ),
        )
        .child(
            Switch::new(("root-enabled", id as u64))
                .checked(root.enabled)
                .color(p.accent)
                .accessibility_label(root.path.clone())
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.change_root(move |engine| engine.set_root_enabled(id, on), cx);
                })),
        )
        .child(
            Button::new(("remove-root", id as u64))
                .label(s.remove)
                .accessibility_label(format!("{} {}", s.remove, root.path))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change_root(move |engine| engine.remove_root(id), cx);
                })),
        )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(self.dark);
        let s = self.live.read(cx).lang.strings();
        // ponytail: one section; a `Section` enum and click-to-switch come
        // with the second one (search features, what to index, …).
        let nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(220.))
            .gap(px(2.))
            .px_2()
            .py_4()
            .child(
                div()
                    .px_3()
                    .pb_3()
                    .text_xs()
                    .text_color(p.mute)
                    .child(s.settings),
            )
            .child(
                div()
                    .relative()
                    .px_3()
                    .py_2()
                    .rounded(px(4.))
                    .bg(p.card)
                    .child(p.accent_bar())
                    .child(s.folders),
            );
        div()
            .size_full()
            .flex()
            .bg(p.solid)
            .text_color(p.ink)
            .text_size(px(14.))
            .child(nav)
            .child(
                div()
                    .id("pane")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .px(px(28.))
                    .py(px(24.))
                    .child(self.folders(&p, cx)),
            )
    }
}

fn heading(text: &'static str) -> Div {
    div()
        .mb(px(18.))
        .text_size(px(26.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}
