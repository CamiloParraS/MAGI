//! Window background resolution (ADR-0010, ADR-0011).

use gpui_kit::WindowBackgroundAppearance;
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

#[cfg(test)]
mod tests {
    use super::*;

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
