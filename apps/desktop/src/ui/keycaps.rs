//! A shortcut as keycaps, or as one line of text where keycaps cannot be
//! drawn.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::i18n::Strings;
use crate::theme::Palette;

/// One key of a shortcut, drawn as a keycap.
pub struct Cap {
    pub icon: Option<IconName>,
    /// Empty when the icon says it all (macOS symbols).
    pub label: String,
    /// The space bar.
    pub wide: bool,
    /// Held down right now.
    pub held: bool,
}

/// `keystroke` as keycaps, modifiers first in the platform's order, those
/// in `held` lit; `symbols` is [`magi_core::platform::SYMBOL_MODIFIERS`].
/// The last key never lights: the registered hotkey swallows it.
pub fn key_caps(keystroke: &Keystroke, s: &Strings, symbols: bool, held: &Modifiers) -> Vec<Cap> {
    let cap = |icon, label: &str| Cap {
        icon,
        label: label.into(),
        wide: false,
        held: false,
    };
    // Each modifier: its symbol on macOS, its name elsewhere.
    let pick = |symbol, name, down| Cap {
        held: down,
        ..if symbols {
            cap(Some(symbol), "")
        } else {
            cap(None, name)
        }
    };
    let m = &keystroke.modifiers;
    let mut caps = Vec::new();
    if m.control {
        caps.push(pick(IconName::ChevronUp, "Ctrl", held.control));
    }
    if m.alt {
        caps.push(pick(IconName::Option, "Alt", held.alt));
    }
    if m.shift {
        // The arrow is printed on Shift keys everywhere.
        caps.push(Cap {
            held: held.shift,
            ..cap(
                Some(IconName::ArrowBigUp),
                if symbols { "" } else { s.key_shift },
            )
        });
    }
    if m.platform {
        caps.push(pick(IconName::Command, "Win", held.platform));
    }
    let key = keystroke.key.as_str();
    caps.push(match key {
        "space" => Cap {
            icon: None,
            label: s.key_space.into(),
            wide: true,
            held: false,
        },
        // `k`, `f1`: as printed. `enter`, `tab`: capitalized.
        _ if key.chars().count() <= 3 => cap(None, &key.to_uppercase()),
        _ => {
            let mut chars = key.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
            cap(
                None,
                &format!("{}{}", first.unwrap_or_default(), chars.as_str()),
            )
        }
    });
    caps
}

/// `keystroke` as one line of text, where keycaps cannot be drawn (the
/// tray's tooltip): names joined by `+`, or macOS's symbols run together.
pub fn shortcut_text(keystroke: &Keystroke, s: &Strings, symbols: bool) -> String {
    let label = |c: Cap| match c.icon {
        _ if !c.label.is_empty() => c.label,
        Some(IconName::ChevronUp) => "⌃".into(),
        Some(IconName::Option) => "⌥".into(),
        Some(IconName::ArrowBigUp) => "⇧".into(),
        Some(IconName::Command) => "⌘".into(),
        _ => String::new(),
    };
    key_caps(keystroke, s, symbols, &Modifiers::default())
        .into_iter()
        .map(label)
        .collect::<Vec<_>>()
        .join(if symbols { "" } else { "+" })
}

/// A key drawn as a keycap: a deeper bottom edge, like a real key, its
/// face lighter than the card in dark mode (raised reads lighter there).
/// Held, it sinks 2px and takes the accent, instantly: it is the user's own
/// keypress. While recording, an empty dashed outline of the same size.
pub(crate) fn keycap(cap: Cap, recording: bool, p: &Palette) -> Div {
    let face: Hsla = if p.dark {
        rgb(0x3a3a3a).into()
    } else {
        p.solid
    };
    let edge: Hsla = if p.dark {
        rgb(0x151515).into()
    } else {
        p.mute.opacity(0.45)
    };
    let key = div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(42.))
        .min_w(px(42.))
        .px_3()
        .when(cap.wide, |d| d.w(px(110.)))
        .rounded(px(8.))
        .border_1()
        .text_size(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .children(cap.icon.map(|i| Icon::new(i).size(px(17.))))
        .when(!cap.label.is_empty(), |d| d.child(cap.label));
    // In a fixed slot, so a sinking key moves nothing around it.
    let slot = div().h(px(47.)).flex().items_end();
    slot.child(if recording {
        key.mb(px(2.))
            .border_dashed()
            .border_color(p.mute.opacity(0.6))
            .text_color(transparent_black())
    } else if cap.held {
        key.border_b(px(3.))
            .border_color(p.accent)
            .bg(p.accent.opacity(0.14))
            .text_color(p.accent)
    } else {
        key.mb(px(2.)).border_b(px(3.)).border_color(edge).bg(face)
    })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has its own `test` attribute.
    use super::{key_caps, shortcut_text};
    use crate::i18n::Lang;
    use gpui_kit::{Keystroke, Modifiers};

    #[test]
    fn a_shortcut_becomes_keycaps_the_platform_way() {
        let caps = |key: &str, symbols: bool| {
            let k = Keystroke::parse(key).unwrap();
            key_caps(&k, Lang::En.strings(), symbols, &Modifiers::default())
                .into_iter()
                .map(|c| (c.label, c.icon.is_some(), c.wide))
                .collect::<Vec<_>>()
        };
        let cap = |label: &str, icon, wide| (label.to_string(), icon, wide);
        // Windows and Linux: names, Shift with its arrow, a wide Space.
        assert_eq!(
            caps("ctrl-shift-space", false),
            vec![
                cap("Ctrl", false, false),
                cap("Shift", true, false),
                cap("Space", false, true)
            ]
        );
        // macOS: the symbols alone, in its ⌃⌥⇧⌘ order.
        assert_eq!(
            caps("cmd-alt-k", true),
            vec![
                cap("", true, false),
                cap("", true, false),
                cap("K", false, false)
            ]
        );
        assert_eq!(
            caps("alt-f1", false),
            vec![cap("Alt", false, false), cap("F1", false, false)]
        );
        // Spanish says Espacio; Shift stays Shift (eadfe73).
        let k = Keystroke::parse("ctrl-shift-space").unwrap();
        let es: Vec<_> = key_caps(&k, Lang::Es.strings(), false, &Modifiers::default())
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(es, ["Ctrl", "Shift", "Espacio"]);
    }

    #[test]
    fn a_shortcut_reads_as_text_the_platform_way() {
        let k = Keystroke::parse("ctrl-shift-space").unwrap();
        assert_eq!(
            shortcut_text(&k, Lang::Es.strings(), false),
            "Ctrl+Shift+Espacio"
        );
        let k = Keystroke::parse("cmd-alt-k").unwrap();
        assert_eq!(shortcut_text(&k, Lang::En.strings(), true), "⌥⌘K");
    }

    #[test]
    fn the_modifiers_held_down_light_their_keycaps() {
        let k = Keystroke::parse("ctrl-shift-space").unwrap();
        let lit = |held: Modifiers| {
            key_caps(&k, Lang::En.strings(), false, &held)
                .into_iter()
                .map(|c| c.held)
                .collect::<Vec<_>>()
        };
        assert_eq!(lit(Modifiers::default()), [false, false, false]);
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        assert_eq!(lit(ctrl), [true, false, false]);
        // A held modifier the shortcut does not use lights nothing.
        let alt_shift = Modifiers {
            alt: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(lit(alt_shift), [false, true, false]);
    }
}
