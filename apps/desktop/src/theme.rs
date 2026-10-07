//! Light/dark, the variant A palette and window background resolution
//! (ADR-0010, ADR-0011).

use gpui_kit::component::{Theme, ThemeMode};
use std::time::Duration;

use gpui_kit::{
    Animation, AnimationElement, AnimationExt as _, App, Div, ElementId, FontWeight, Hsla,
    IntoElement, ParentElement as _, Pixels, Rgba, Styled, Window, WindowAppearance,
    WindowBackgroundAppearance, div, ease_out_quint, px, relative, rgb, rgba,
};
use gpui_kit::{black, white};
use magi_core::config::{TRANSPARENCY_INTENSITY, TransparencyMode};
use magi_core::discovery::Kind;
use magi_core::platform::BackdropSupport;

/// How long [`fade_in`] takes.
pub const FADE: Duration = Duration::from_millis(150);

/// Something new arrives with a short ease-out fade instead of popping in.
/// It replays only when `id` changes or `el` was not shown the frame before;
/// leaving is instant. Under reduced motion GPUI shows the end state.
pub fn fade_in<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(FADE).with_easing(ease_out_quint()),
        |el, t| el.opacity(t),
    )
}

/// A file type's colored square, the same in the results and in settings.
pub fn kind_glyph(kind: Kind, size: Pixels) -> Div {
    let (label, color) = match kind {
        Kind::Pdf => ("PDF", 0xc4314b),
        Kind::Office => ("DOC", 0x185abd),
        Kind::Code => ("</>", 0x5c2d91),
        Kind::Text => ("TXT", 0x6b7280),
        Kind::Image => ("IMG", 0x6b7280),
        Kind::Other => ("•", 0x6b7280),
    };
    div()
        .flex_none()
        .size(size)
        .rounded(size * (3. / 32.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(color))
        .text_color(white())
        .text_size(size * (9. / 32.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

/// What a status dot or badge says, by color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Busy,
    Neutral,
    Warn,
    Err,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Backdrop {
    /// `tint_alpha` is `ui.transparency_intensity`: the alpha of the colour
    /// the window paints over the blurred backdrop.
    Blurred {
        tint_alpha: f32,
    },
    Solid,
}

pub fn backdrop(mode: TransparencyMode, intensity: f32, support: BackdropSupport) -> Backdrop {
    let wanted = match mode {
        TransparencyMode::Never => false,
        TransparencyMode::Always => true,
        TransparencyMode::MatchSystem => !support.reduce_transparency,
    };
    if wanted && support.mica {
        Backdrop::Blurred {
            tint_alpha: intensity,
        }
    } else {
        Backdrop::Solid
    }
}

pub fn window_background(backdrop: Backdrop) -> WindowBackgroundAppearance {
    match backdrop {
        Backdrop::Blurred { .. } => WindowBackgroundAppearance::Blurred,
        Backdrop::Solid => WindowBackgroundAppearance::Opaque,
    }
}

/// `ui.theme`: `"light"` or `"dark"` force it; anything else (`"system"`)
/// follows the OS.
pub fn is_dark(theme: &str, appearance: WindowAppearance) -> bool {
    match theme {
        "dark" => true,
        "light" => false,
        _ => matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    }
}

/// Applies `ui.theme` (or the OS appearance) to gpui-component's theme,
/// which colors its widgets; returns whether it is dark.
pub fn sync(theme: &str, window: &mut Window, cx: &mut App) -> bool {
    let dark = is_dark(theme, window.appearance());
    let mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, Some(window), cx);
    // gpui-component's primary is black/white; the mockup's is the accent.
    let palette = Palette::new(dark);
    let accent = palette.accent;
    let on_accent = if dark { black() } else { white() };
    Theme::update(cx, |t| {
        // Unchecked checkboxes and field borders; the dark default is
        // nearly invisible on a box.
        // The off switch is a dark thumb on a dark track by default.
        if dark {
            t.input = palette.mute.opacity(0.6);
            t.switch = palette.mute.opacity(0.6);
            t.switch_thumb = palette.ink;
        }
        t.primary = accent;
        t.primary_hover = accent.opacity(0.9);
        t.primary_active = accent.opacity(0.8);
        t.primary_foreground = on_accent;
        t.button_primary = accent;
        t.button_primary_hover = accent.opacity(0.9);
        t.button_primary_active = accent.opacity(0.8);
        t.button_primary_foreground = on_accent;
    });
    dark
}

/// Light text colors wash out over a blurred backdrop with a dark window
/// behind it, so light mode keeps at least this much tint: the slider's
/// range is squeezed into `LIGHT_MIN_TINT..=1`.
pub const LIGHT_MIN_TINT: f32 = 0.85;

/// Variant A "Pane" colors (M6 visual direction, `docs/screenshots/m6-variant-a/`).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub ink: Hsla,
    pub mute: Hsla,
    pub line: Hsla,
    /// The panel color; its alpha comes from the backdrop.
    pub panel: Rgba,
    pub accent: Hsla,
    pub selection: Hsla,
    pub mark: Hsla,
    /// The settings window.
    pub solid: Hsla,
    /// A settings box.
    pub card: Hsla,
    /// The settings sidebar.
    pub side: Hsla,
    /// The selected sidebar item.
    pub chip: Hsla,
    pub warn: Hsla,
    pub ok: Hsla,
    pub err: Hsla,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                ink: rgb(0xf3f3f5).into(),
                mute: rgb(0xa4a6ad).into(),
                line: rgba(0xffffff14).into(),
                panel: rgb(0x202024),
                accent: rgb(0x4cc2ff).into(),
                selection: rgba(0x4cc2ff24).into(),
                mark: rgba(0xffd6004d).into(),
                solid: rgb(0x202020).into(),
                card: rgb(0x2b2b2b).into(),
                side: rgb(0x191919).into(),
                chip: rgba(0xffffff14).into(),
                warn: rgb(0xfcb452).into(),
                ok: rgb(0x6ccb5f).into(),
                err: rgb(0xff99a4).into(),
            }
        } else {
            Self {
                dark,
                ink: rgb(0x1b1b1f).into(),
                mute: rgb(0x636366).into(),
                line: rgba(0x00000014).into(),
                panel: rgb(0xf6f6f8),
                accent: rgb(0x0a64d8).into(),
                selection: rgba(0x0a64d81f).into(),
                mark: rgba(0xffd60073).into(),
                solid: rgb(0xf5f5f7).into(),
                card: rgb(0xffffff).into(),
                side: rgb(0xececec).into(),
                chip: rgba(0x0000000f).into(),
                warn: rgb(0x9d5d00).into(),
                ok: rgb(0x0f7b0f).into(),
                err: rgb(0xc42b1c).into(),
            }
        }
    }

    pub fn tone(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Ok => self.ok,
            Tone::Busy => self.accent,
            Tone::Neutral => self.mute,
            Tone::Warn => self.warn,
            Tone::Err => self.err,
        }
    }

    /// A small colored dot, as in the tray and the search footer.
    pub fn dot(&self, tone: Tone) -> Div {
        div()
            .flex_none()
            .size(px(7.))
            .rounded_full()
            .bg(self.tone(tone))
    }

    /// A status: a colored dot and a short word, the word colored only
    /// for a problem.
    pub fn status(&self, text: &'static str, tone: Tone) -> Div {
        let color = match tone {
            Tone::Warn | Tone::Err => self.tone(tone),
            _ => self.mute,
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .text_xs()
            .text_color(color)
            .child(self.dot(tone))
            .child(text)
    }

    /// The alpha of the window's own colors over `backdrop`.
    pub fn tint_alpha(&self, backdrop: Backdrop) -> f32 {
        match backdrop {
            Backdrop::Blurred { tint_alpha } if self.dark => tint_alpha,
            Backdrop::Blurred { tint_alpha } => {
                let (lo, hi) = (
                    *TRANSPARENCY_INTENSITY.start(),
                    *TRANSPARENCY_INTENSITY.end(),
                );
                let t = ((tint_alpha - lo) / (hi - lo)).clamp(0., 1.);
                LIGHT_MIN_TINT + t * (1. - LIGHT_MIN_TINT)
            }
            Backdrop::Solid => 1.0,
        }
    }

    /// A status pill.
    pub fn badge(&self, text: &'static str, tone: Tone) -> Div {
        let color = self.tone(tone);
        div()
            .flex_none()
            .px_2()
            .py(px(1.))
            .rounded_full()
            .text_xs()
            .text_color(color)
            .bg(color.opacity(0.12))
            .child(text)
    }

    pub fn background(&self, backdrop: Backdrop) -> Rgba {
        Rgba {
            a: self.tint_alpha(backdrop),
            ..self.panel
        }
    }

    /// Marks the selected row: a quarter of its height, centered on its left
    /// edge. The row must be `relative()`.
    pub fn accent_bar(&self) -> Div {
        div()
            .absolute()
            .left_0()
            .top(relative(0.375))
            .h(relative(0.35))
            .w(px(3.))
            .rounded_full()
            .bg(self.accent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_setting_overrides_the_os_appearance() {
        use WindowAppearance::*;
        assert!(is_dark("system", Dark));
        assert!(is_dark("system", VibrantDark));
        assert!(!is_dark("system", Light));
        assert!(is_dark("dark", Light));
        assert!(!is_dark("light", VibrantDark));
        assert!(
            !is_dark("something-else", Light),
            "unknown values follow the OS"
        );
    }

    const MICA: BackdropSupport = BackdropSupport {
        mica: true,
        reduce_transparency: false,
    };

    #[test]
    fn mica_only_when_wanted_and_supported() {
        use TransparencyMode::*;
        assert_eq!(backdrop(Never, 0.75, MICA), Backdrop::Solid);
        assert_eq!(
            backdrop(Always, 0.75, MICA),
            Backdrop::Blurred { tint_alpha: 0.75 }
        );
        assert_eq!(
            backdrop(MatchSystem, 0.6, MICA),
            Backdrop::Blurred { tint_alpha: 0.6 }
        );
        let reduced = BackdropSupport {
            reduce_transparency: true,
            ..MICA
        };
        assert_eq!(backdrop(MatchSystem, 0.75, reduced), Backdrop::Solid);
        assert_eq!(
            backdrop(Always, 0.75, reduced),
            Backdrop::Blurred { tint_alpha: 0.75 }
        );
    }

    #[test]
    fn light_mode_keeps_a_readable_tint() {
        let blur = |a| Backdrop::Blurred { tint_alpha: a };
        let (light, dark) = (Palette::new(false), Palette::new(true));
        let (lo, hi) = (
            *TRANSPARENCY_INTENSITY.start(),
            *TRANSPARENCY_INTENSITY.end(),
        );
        assert_eq!(light.tint_alpha(blur(lo)), LIGHT_MIN_TINT);
        assert_eq!(light.tint_alpha(blur(hi)), 1.0);
        let (a, b) = (light.tint_alpha(blur(0.5)), light.tint_alpha(blur(0.6)));
        assert!(
            LIGHT_MIN_TINT < a && a < b,
            "every slider step shows in light mode"
        );
        assert_eq!(
            dark.tint_alpha(blur(0.5)),
            0.5,
            "dark text on glass holds up"
        );
        assert_eq!(light.tint_alpha(Backdrop::Solid), 1.0);
    }

    #[test]
    fn no_mica_below_windows_11_22h2_means_solid() {
        let none = BackdropSupport::default();
        for mode in [TransparencyMode::Always, TransparencyMode::MatchSystem] {
            assert_eq!(backdrop(mode, 0.75, none), Backdrop::Solid);
        }
    }
}
