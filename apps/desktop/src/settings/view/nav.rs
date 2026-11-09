//! The sidebar: one button per section with a sliding accent bar, the search
//! button at its foot, and the version label.

use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::Selectable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::platform::SYMBOL_MODIFIERS;

use super::SettingsView;
use crate::app::AppEvent;
use crate::i18n::Strings;
use crate::settings::version_tap;
use crate::theme::Palette;
use crate::tray::TrayAction;
use crate::ui::keycaps::shortcut_text;

/// A section button's height, and the space between two: the accent bar
/// moves by their sum per section.
const NAV_ITEM_H: f32 = 32.;
const NAV_GAP: f32 = 2.;
/// How long the accent bar takes to reach another section.
const NAV_SLIDE: std::time::Duration = std::time::Duration::from_millis(240);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Folders,
    Searched,
    General,
    Appearance,
    Index,
    Account,
}

pub(super) const SECTIONS: [Section; 6] = [
    Section::Folders,
    Section::Searched,
    Section::General,
    Section::Appearance,
    Section::Index,
    Section::Account,
];

impl Section {
    pub(super) fn title(self, s: &Strings) -> &'static str {
        match self {
            Self::Folders => s.folders,
            Self::Searched => s.whats_searched,
            Self::General => s.general,
            Self::Appearance => s.appearance,
            Self::Index => s.index,
            Self::Account => s.account,
        }
    }

    pub(super) fn blurb(self, s: &Strings) -> &'static str {
        match self {
            Self::Folders => s.blurb_folders,
            Self::Searched => s.blurb_searched,
            Self::General => s.blurb_general,
            Self::Appearance => s.blurb_appearance,
            Self::Index => s.blurb_index,
            Self::Account => s.blurb_account,
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Folders => IconName::Folder,
            Self::Searched => IconName::Sparkles,
            Self::General => IconName::Settings,
            Self::Appearance => IconName::Palette,
            Self::Index => IconName::ChartColumn,
            Self::Account => IconName::CircleUser,
        }
    }
}

impl SettingsView {
    /// The sidebar, its background tinted by `alpha` over a blurred backdrop.
    pub(super) fn nav(&self, s: &Strings, p: &Palette, alpha: f32, cx: &Context<Self>) -> Div {
        // The accent bar slides from the section before; under reduced motion
        // GPUI shows it in place.
        let (from, to) = (self.prev_section as usize, self.section as usize);
        let pitch = NAV_ITEM_H + NAV_GAP;
        let bar = div()
            .absolute()
            .left_0()
            .w(px(3.))
            .h(px(NAV_ITEM_H * 0.35))
            .rounded_full()
            .bg(p.accent)
            .with_animation(
                ("nav-bar", from * SECTIONS.len() + to),
                Animation::new(NAV_SLIDE).with_easing(ease_in_out),
                move |bar, t| {
                    let row = from as f32 + (to as f32 - from as f32) * t;
                    bar.top(px(row * pitch + NAV_ITEM_H * 0.375))
                },
            );
        let sections = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(NAV_GAP))
            .children(SECTIONS.iter().enumerate().map(|(ix, &section)| {
                let selected = section == self.section;
                // A button, so Tab and Enter reach every section.
                Button::new(("section", ix))
                    .cursor_pointer()
                    .ghost()
                    .selected(selected)
                    .w_full()
                    .h(px(NAV_ITEM_H))
                    .px_0()
                    .rounded(px(6.))
                    .when(selected, |b| b.bg(p.selected_fill(2.)))
                    // The content row centers a label; a filling child sits
                    // left. The padding is the label's, so the content row
                    // starts at the button's edge.
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px_3()
                            .child(
                                Icon::new(section.icon())
                                    .size(px(15.))
                                    .text_color(if selected { p.accent } else { p.mute }),
                            )
                            .child(section.title(s)),
                    )
                    .accessibility_label(section.title(s))
                    .on_click(cx.listener(move |this, _, _, cx| this.show(section, cx)))
            }))
            .child(bar);
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(210.))
            .gap(px(NAV_GAP))
            .p_2()
            .bg(p.side.opacity(alpha))
            .child(
                div()
                    .px_3()
                    .pb_3()
                    .text_xs()
                    .text_color(p.mute)
                    .child(s.settings),
            )
            .child(sections)
            // A hairline marks it as an action, not one more section.
            .child(
                div()
                    .mt_auto()
                    .child(div().h(px(1.)).mx_3().mb(px(6.)).bg(p.line))
                    .child(self.search_button(s, p, cx)),
            )
            .child(
                div()
                    .id("version")
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(p.mute)
                    // A testing aid, not a control: ten quick taps run the
                    // onboarding again, as `magi --onboarding` does.
                    .on_click(cx.listener(|this, _, _, _| {
                        if version_tap(&mut this.version_taps, Instant::now()) {
                            let _ = this.events.try_send(AppEvent::Onboarding);
                        }
                    }))
                    .child(concat!("Magi ", env!("CARGO_PKG_VERSION"))),
            )
    }

    /// The rail's way back to the product: a quiet item at its foot, like the
    /// sections above it, opening the search window (or bringing it forward).
    /// The shortcut is in its tooltip: beside the label it would overflow.
    fn search_button(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let hotkey = Keystroke::parse(&crate::hotkey::to_keystroke(&self.ui.hotkey))
            .ok()
            .map(|k| shortcut_text(&k, s, SYMBOL_MODIFIERS));
        Button::new("open-search")
            .cursor_pointer()
            .ghost()
            .w_full()
            .h(px(NAV_ITEM_H))
            .px_0()
            .rounded(px(6.))
            .text_size(px(13.))
            .accessibility_label(s.search_files)
            .when_some(hotkey, |b, text| b.tooltip(text))
            .on_click(cx.listener(|this, _, _, _| {
                let _ = this.events.try_send(AppEvent::Tray(TrayAction::OpenSearch));
            }))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px_3()
                    .child(Icon::new(IconName::Search).size(px(15.)).text_color(p.mute))
                    .child(s.search_files),
            )
    }
}
