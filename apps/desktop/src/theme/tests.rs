use super::*;

#[test]
fn office_files_are_told_apart_by_extension() {
    assert_eq!(office_glyph("budget.XLSX").0, "XLS");
    assert_eq!(office_glyph("deck.pptx").0, "PPT");
    assert_eq!(office_glyph("lease.docx").0, "DOC");
    assert_eq!(office_glyph("no-extension").0, "DOC");
}

#[test]
fn a_forced_theme_forces_the_window_chrome_too() {
    assert_eq!(chrome("dark"), Some(WindowAppearance::Dark));
    assert_eq!(chrome("light"), Some(WindowAppearance::Light));
    // Following the OS: no override, the chrome follows it too.
    assert_eq!(chrome("system"), None);
}

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
    blur: true,
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

/// WCAG relative luminance and contrast ratio.
fn luminance(c: Rgba) -> f32 {
    let lin = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The panel's colour as it looks over a flat backdrop of grey `level`.
fn over(panel: Rgba, alpha: f32, level: f32) -> Rgba {
    let mix = |c: f32| c * alpha + level * (1. - alpha);
    Rgba {
        r: mix(panel.r),
        g: mix(panel.g),
        b: mix(panel.b),
        a: 1.,
    }
}

/// Muted text (dates, tags, snippets, the footer) at the least tint the
/// slider allows, over the backdrops that hurt most. 4.5:1 is WCAG AA for
/// small text. Over a mid-grey blur both themes pass it; over pure white
/// (dark) or pure black (light) the floors are what is left.
#[test]
fn muted_text_stays_readable_at_the_tint_floor() {
    let blur = |a| Backdrop::Blurred { tint_alpha: a };
    let lo = *TRANSPARENCY_INTENSITY.start();
    for (dark, worst) in [(true, 1.0), (false, 0.0)] {
        let p = Palette::new(dark);
        let alpha = p.tint_alpha(blur(lo));
        let mute = p.mute.to_rgb();
        let mid = contrast(mute, over(p.panel, alpha, 0.5));
        let extreme = contrast(mute, over(p.panel, alpha, worst));
        println!("dark={dark} mid={mid:.2} worst={extreme:.2}");
        assert!(mid >= 4.5, "dark={dark}: {mid:.2} over mid-grey");
        assert!(extreme >= 3.0, "dark={dark}: {extreme:.2} over the worst");
    }
}

#[test]
fn both_themes_keep_a_readable_tint() {
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
    assert_eq!(dark.tint_alpha(blur(lo)), DARK_MIN_TINT);
    assert_eq!(dark.tint_alpha(blur(hi)), 1.0);
    let (a, b) = (dark.tint_alpha(blur(0.5)), dark.tint_alpha(blur(0.6)));
    assert!(
        DARK_MIN_TINT < a && a < b,
        "every slider step shows in dark mode"
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
