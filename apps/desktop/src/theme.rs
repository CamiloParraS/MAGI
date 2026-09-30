//! Light/dark, the variant A palette and window background resolution
//! (ADR-0010, ADR-0011).

use gpui_kit::{Hsla, Rgba, WindowAppearance, WindowBackgroundAppearance, rgb, rgba};
use magi_core::config::TransparencyMode;
use magi_core::platform::BackdropSupport;

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

/// Variant A "Pane" colors (M6 visual direction, `docs/screenshots/m6-variant-a/`).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub ink: Hsla,
    pub mute: Hsla,
    pub line: Hsla,
    /// The panel color; its alpha comes from the backdrop.
    pub panel: Rgba,
    pub accent: Hsla,
    pub selection: Hsla,
    pub mark: Hsla,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                ink: rgb(0xf3f3f5).into(),
                mute: rgb(0xa4a6ad).into(),
                line: rgba(0xffffff14).into(),
                panel: rgb(0x202024),
                accent: rgb(0x4cc2ff).into(),
                selection: rgba(0x4cc2ff24).into(),
                mark: rgba(0xffd6004d).into(),
            }
        } else {
            Self {
                ink: rgb(0x1b1b1f).into(),
                mute: rgb(0x5f6168).into(),
                line: rgba(0x00000014).into(),
                panel: rgb(0xf6f6f8),
                accent: rgb(0x0a64d8).into(),
                selection: rgba(0x0a64d81f).into(),
                mark: rgba(0xffd60073).into(),
            }
        }
    }

    pub fn background(&self, backdrop: Backdrop) -> Rgba {
        let a = match backdrop {
            Backdrop::Blurred { tint_alpha } => tint_alpha,
            Backdrop::Solid => 1.0,
        };
        Rgba { a, ..self.panel }
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
    fn no_mica_below_windows_11_22h2_means_solid() {
        let none = BackdropSupport::default();
        for mode in [TransparencyMode::Always, TransparencyMode::MatchSystem] {
            assert_eq!(backdrop(mode, 0.75, none), Backdrop::Solid);
        }
    }
}
