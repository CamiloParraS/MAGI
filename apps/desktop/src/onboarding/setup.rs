//! The setup screen: folders on the left; features, startup and the strip
//! on the right.

use std::path::Path;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::config::FeaturesConfig;
use magi_core::dto::{Install, RootStatus};

use super::{OnboardingView, STEP_IN};
use crate::i18n::Strings;
use crate::settings::root_state;
use crate::theme::{self, Palette, Tone};
use crate::ui::folders::{add_folder_row, drop_zone, folder_name, suggested_folders, suggestions};
use crate::ui::{divider, feature_icon, group, grouped, heading, note, row_icon, scan_row};

impl OnboardingView {
    /// FR-10's permission check: each folder's status says whether Magi can
    /// read it. With none yet, a drop zone; folders can be dropped either way.
    pub(super) fn folders(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let accent = p.accent;
        let roots = self.roots(cx).filter(|roots| !roots.is_empty());
        let live = self.live.read(cx);
        let body = match roots {
            None => drop_zone(s, p, cx.listener(|this, _, _, cx| this.add_folder(cx))),
            // The same grouped list as the features, Add folder its last row.
            // The folders scroll on their own past a few rows, so a long list
            // leaves Add folder, the suggestions and the footer in view.
            Some(roots) => group(p)
                .child(
                    // Not overflow_y_scrollbar(): its scroll area is 100% of
                    // a box with only a max height, so it grows, unscrollable.
                    div()
                        .relative()
                        .child(
                            div()
                                .id("folder-list")
                                .max_h(px(240.))
                                // A lane for the scrollbar, clear of the
                                // remove buttons.
                                .pr(px(10.))
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll)
                                .flex()
                                .flex_col()
                                .children(roots.iter().enumerate().map(|(ix, root)| {
                                    div()
                                        .children((ix > 0).then(|| divider(p)))
                                        .child(self.root_row(root, s, p, cx))
                                })),
                        )
                        .vertical_scrollbar(&self.scroll),
                )
                .child(divider(p))
                .child(add_folder_row(
                    s,
                    p,
                    cx.listener(|this, _, _, cx| this.add_folder(cx)),
                )),
        };
        let offered = suggestions(&self.user_folders, roots.map_or(&[][..], |r| &r[..]));
        let chips = suggested_folders(offered, s, p, cx, Self::add_folders);
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .children(scan_row(live.status.as_ref(), live.lang, p))
            .child(
                body.drag_over::<ExternalPaths>(move |style, _, _, _| {
                    style.border_color(accent).bg(accent.opacity(0.08))
                })
                .on_drop(
                    cx.listener(|this, paths: &ExternalPaths, _, cx| this.drop_folders(paths, cx)),
                ),
            )
            .children(chips)
    }

    fn root_row(
        &self,
        root: &RootStatus,
        s: &Strings,
        p: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (badge, tone, explain) = root_state(root, s);
        let id = root.id;
        let removing = self.removing.contains(&id);
        let icon = match tone {
            Tone::Warn | Tone::Err => Icon::new(IconName::TriangleAlert).text_color(p.tone(tone)),
            _ => Icon::new(IconName::Folder).text_color(p.accent),
        };
        // Spaced as `row`: the folder's name, its parent under it, as file
        // managers show folders.
        let path = Path::new(&root.path);
        let parent = path.parent().map(|parent| parent.display().to_string());
        let row = crate::ui::row()
            .child(icon.size(px(18.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(folder_name(path)),
                    )
                    .children(parent.map(|parent| {
                        note(p)
                            .mt_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis_middle()
                            .child(parent)
                    }))
                    .children(explain.map(|e| note(p).child(e))),
            )
            .children(badge.map(|badge| p.status(badge, tone)))
            // A rescan probes each root again: a fixed permission or
            // a reconnected drive clears the badge.
            .when(matches!(tone, Tone::Warn | Tone::Err), |row| {
                row.child(
                    Button::new(("retry-root", id as u64))
                        .cursor_pointer()
                        .ghost()
                        .small()
                        .label(s.try_again)
                        .disabled(removing)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_root(|host| host.engine().map(|e| drop(e.rescan())), cx);
                        })),
                )
            })
            .child(
                // Minus, not a trash can: it only leaves Magi's list.
                Button::new(("remove-root", id as u64))
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .icon(IconName::Minus)
                    .tooltip(s.remove)
                    .accessibility_label(format!("{} {}", s.remove, root.path))
                    // A spinner in the minus's place; it takes no clicks.
                    .loading(removing)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.removing.push(id);
                        cx.notify();
                        this.run_root(move |host| host.engine()?.remove_root(id), cx);
                    })),
            );
        if removing {
            theme::dim(row, ("removing-root", id as u64)).into_any_element()
        } else {
            row.into_any_element()
        }
    }

    /// The download consent (FR-10, ADR-0010): each feature's exact size;
    /// the button states the total. One grouped list, as in system
    /// settings: a switch shows the choice, ⓘ says what a feature does.
    pub(super) fn features(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let lang = live.lang;
        let chosen = self.chosen(cx);
        let defaults = FeaturesConfig::default();
        let rows = live.features.iter().enumerate().map(|(ix, f)| {
            let feature = f.feature;
            let text = s.feature(feature);
            let pitch = text.pitch;
            // Downloaded: a green check in place of the size.
            let size = match f.install {
                Install::Installed { .. } => div()
                    .id(("installed", ix))
                    .flex_none()
                    .aria_label(s.ob_installed)
                    .child(Icon::new(IconName::Check).size(px(16.)).text_color(p.ok))
                    .into_any_element(),
                _ => note(p)
                    .mt_0()
                    .flex_none()
                    .child(lang.size(f.download_size))
                    .into_any_element(),
            };
            let sub = defaults.enabled(feature).then_some(s.ob_recommended);
            let row = row(feature_icon(feature), text.name, sub, p)
                .child(size)
                .child(
                    Popover::new(("feature-info", ix))
                        .trigger(
                            Button::new(("feature-info-button", ix))
                                .cursor_pointer()
                                .ghost()
                                .xsmall()
                                .icon(IconName::Info)
                                .accessibility_label(text.name),
                        )
                        .content(move |_, _, _| div().max_w(px(260.)).text_sm().child(pitch)),
                )
                .child(
                    Switch::new(("feature", feature as usize))
                        .cursor_pointer()
                        .checked(chosen.contains(&feature))
                        .color(p.accent)
                        .accessibility_label(text.name)
                        .on_click(cx.listener(move |this, &on: &bool, _, cx| {
                            this.toggle_feature(feature, on, cx)
                        })),
                );
            stagger_in(row, ix)
        });
        grouped(p, rows)
    }

    /// "Start with your computer" (FR-10), on by default.
    fn launch_row(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        group(p).child(
            row(IconName::Power, s.launch_at_login, None, p).child(
                Switch::new("start-at-login")
                    .cursor_pointer()
                    .checked(self.launch_at_login)
                    .color(p.accent)
                    .tooltip(s.launch_at_login_note)
                    .accessibility_label(s.launch_at_login)
                    .on_click(cx.listener(|this, &on: &bool, _, cx| {
                        this.launch_at_login = on;
                        cx.notify();
                    })),
            ),
        )
    }

    /// The one setup screen: the welcome over two columns, folders on the
    /// left, features and startup on the right.
    pub(super) fn setup(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let column = || div().flex_1().min_w_0().flex().flex_col().gap(px(16.));
        div()
            // Centered in the space, as the finish screen; a margin, not
            // justify_center, so a long folder list still scrolls from the top.
            .my_auto()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(heading(s.welcome).mb_0().text_center().text_size(px(26.)))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(24.))
                    .child(column().child(section(
                        s.ob_folders,
                        s.ob_folders_note,
                        self.folders(s, p, cx),
                        p,
                    )))
                    .child(
                        column()
                            .child(section(
                                s.ob_features,
                                s.ob_features_note,
                                self.features(s, p, cx),
                                p,
                            ))
                            .child(self.launch_row(s, p, cx))
                            .child(self.strip(s, p, cx)),
                    ),
            )
    }
}

/// How far apart the feature tiles arrive.
const STAGGER: Duration = Duration::from_millis(40);

/// Tile `ix` of a list fades up 6px, [`STAGGER`] after the one before it.
/// It plays when the tile first shows, never on later renders.
fn stagger_in(el: Div, ix: usize) -> impl IntoElement {
    let delay = STAGGER * ix as u32;
    let total = STEP_IN + delay;
    let start = delay.as_secs_f32() / total.as_secs_f32();
    let ease = ease_out_quint();
    el.relative()
        .with_animation(("stagger-in", ix), Animation::new(total), move |el, t| {
            let t = ease(((t - start) / (1. - start)).clamp(0., 1.));
            el.opacity(t).top(px(6. * (1. - t)))
        })
}

/// A titled part of the setup screen.
fn section(title: &'static str, intro: &'static str, body: Div, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        // Two lines tall either way, so the columns' boxes start level.
        .child(
            note(p)
                .mt(px(2.))
                .mb(px(10.))
                .min_h(px(44.))
                .text_sm()
                .child(intro),
        )
        .child(body)
}

/// A grouped row: an icon in the accent, the name with an optional small
/// line under it, then what follows.
fn row(icon: IconName, name: &'static str, sub: Option<&'static str>, p: &Palette) -> Div {
    crate::ui::row().child(row_icon(icon, p)).child(
        div()
            .flex_1()
            .min_w_0()
            // Wraps rather than cutting off a feature's name.
            .child(div().child(name))
            .children(sub.map(|sub| {
                div()
                    .text_size(px(11.))
                    .line_height(px(14.))
                    .text_color(p.mute)
                    .child(sub)
            })),
    )
}
