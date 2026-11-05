//! The window shown when the engine could not start.

use gpui_kit::*;

use super::assets::AppAssets;
use crate::i18n::Lang;
use crate::theme;

/// The engine could not start: what happened, the error as it is, and the
/// log. Closing the window quits.
pub(super) fn show_start_failure(error: String) {
    // The config is often still readable when the engine is not.
    let ui = magi_core::config::load().map(|c| c.ui).unwrap_or_default();
    let lang = Lang::current(ui.language);
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_quit_mode(QuitMode::LastWindowClosed);
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(lang.strings().start_failed.into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(480.), px(260.)),
                    cx,
                ))),
                focus: true,
                show: true,
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                let dark = theme::sync(&ui.theme, window, cx);
                cx.new(|_| StartFailure { error, lang, dark })
            });
            match opened {
                Ok((handle, _)) => {
                    let _ = handle.update(cx, |_, window, _| window.activate_window());
                }
                Err(error) => {
                    tracing::error!(%error, "could not open the start failure window");
                    cx.quit();
                }
            }
        });
}

struct StartFailure {
    error: String,
    lang: Lang,
    dark: bool,
}

impl Render for StartFailure {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use crate::ui::{card, note};
        use gpui_kit::component::button::{Button, ButtonVariants as _};

        let p = theme::Palette::new(self.dark);
        let s = self.lang.strings();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p(px(24.))
            .bg(p.solid)
            .text_color(p.ink)
            .text_size(px(14.))
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s.start_failed),
            )
            .child(div().text_color(p.mute).child(s.start_failed_note))
            .child(card(&p).child(note(&p).mt_0().flex_1().min_w_0().child(self.error.clone())))
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("show-log")
                            .cursor_pointer()
                            .label(s.show_log)
                            .on_click(|_, _, cx| cx.reveal_path(&crate::logging::path())),
                    )
                    .child(
                        Button::new("quit")
                            .cursor_pointer()
                            .primary()
                            .label(s.tray_quit)
                            .on_click(|_, _, cx| cx.quit()),
                    ),
            )
    }
}
