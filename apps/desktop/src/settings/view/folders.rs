//! Settings › Folders: the searched folders.

use gpui_kit::assets::IconName;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon};
use gpui_kit::*;
use magi_core::dto::RootStatus;

use super::{Field, SettingsView};
use crate::i18n::Strings;
use crate::settings::root_line;
use crate::theme::{self, Palette};
use crate::ui::folder_list::{RootRow, RowParts, root_row};
use crate::ui::folders::{
    accept_dropped_folders, add_folder_row, add_roots, drop_zone, pick_and_add, suggested_folders,
    suggestions,
};
use crate::ui::{PRESS, grouped, note, row, row_icon, scan_row, subheading};

impl SettingsView {
    /// The native folder picker (FR-1), then `add_root` for each.
    fn add_folder(&mut self, cx: &mut Context<Self>) {
        pick_and_add(cx, Self::add_folders);
    }

    /// Picked, suggested or dropped folders.
    fn add_folders(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.run(Field::AddFolder, move |host| add_roots(host, &paths), cx);
    }

    /// Asks first: removing forgets the folder's index, and the folders
    /// collapsed into it when it was added go with it.
    fn confirm_remove(&mut self, id: i64, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.live.read(cx).lang.strings();
        let body: SharedString = s.remove_confirm_body.replace("{path}", path).into();
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title(s.remove_confirm_title)
                .description(body.clone())
                .ok_text(s.remove)
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(s.cancel)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = view.update(cx, |view, cx| {
                        view.folder_list.start_removing(id);
                        view.run(
                            Field::Root(id),
                            move |host| host.engine()?.remove_root(id),
                            cx,
                        );
                    });
                    true
                })
        });
    }

    pub(super) fn folders(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let roots = self.folder_list.roots(live.status.as_ref());
        let add = || cx.listener(|this: &mut Self, _, _, cx| this.add_folder(cx));
        // As in onboarding: a drop zone with none yet, else one grouped list
        // with Add folder its last row. Unknown roots show only Add folder:
        // an empty list would read "no folders".
        let body = match roots {
            Some(roots) if roots.is_empty() => drop_zone(s, p, add()),
            _ => grouped(
                p,
                roots
                    .into_iter()
                    .flatten()
                    .map(|root| self.root_row(root, s, p, cx))
                    .chain([add_folder_row(s, p, add()).into_any_element()]),
            ),
        };
        div()
            .child(div().mb(px(8.)).child(subheading(s.searched_folders, p)))
            .children(scan_row(live, p).map(|row| div().mb(px(8.)).child(row)))
            // Folders dropped from the file manager, as in onboarding.
            .child(accept_dropped_folders(body, p, cx, Self::add_folders))
            // An add that failed has no row yet: say why under the list.
            .children(self.problem(Field::AddFolder, s, p))
            .children(roots.and_then(|roots| {
                let offered = suggestions(&self.user_folders, roots);
                suggested_folders(offered, s, p, cx, Self::add_folders)
                    .map(|chips| chips.mt(px(12.)))
            }))
            .child(
                div()
                    .mt(px(16.))
                    .mb(px(2.))
                    .child(subheading(s.what_to_index, p)),
            )
            .child(self.what_to_index(s, p, cx))
            .child(self.advanced(s, p, cx))
    }

    /// The exclude patterns behind a toggle: raw globs are for few users. One
    /// card, its header always there and the patterns opening under it.
    fn advanced(&self, s: &Strings, p: &Palette, cx: &Context<Self>) -> Div {
        let shown = self.advanced_shown();
        // Turned a quarter, as it opens; back as it closes. Static until the
        // first click, and when a failed change holds it open.
        let quarter = std::f32::consts::FRAC_PI_2;
        let chevron = Icon::new(IconName::ChevronRight)
            .size(px(16.))
            .flex_none()
            .text_color(p.mute);
        let chevron = if self.advanced_flips == 0 {
            chevron
                .rotate(radians(if shown { quarter } else { 0. }))
                .into_any_element()
        } else {
            chevron
                .rotate(radians(if shown { quarter } else { 0. }))
                .with_animation(
                    ("advanced-chevron", self.advanced_flips),
                    Animation::new(theme::FADE).with_easing(ease_out_quint()),
                    move |icon, t| {
                        let t = if shown { t } else { 1. - t };
                        icon.rotate(radians(quarter * t))
                    },
                )
                .into_any_element()
        };
        let header = Button::new("advanced")
            .cursor_pointer()
            .ghost()
            .group("advanced")
            .w_full()
            .h_auto()
            .p_0()
            .rounded_none()
            .text_size(px(14.))
            .accessibility_label(s.advanced)
            // A failed change keeps it open, so a click can't hide its message.
            .on_click(cx.listener(|this, _, _, cx| {
                this.advanced_open = !this.advanced_open;
                this.advanced_flips += 1;
                cx.notify();
            }))
            .child(
                row()
                    .id("advanced-content")
                    .relative()
                    .group_active("advanced", |s| s.top(PRESS))
                    .w_full()
                    .child(row_icon(IconName::SlidersHorizontal, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_left()
                            .child(s.advanced)
                            .child(note(p).child(s.advanced_note)),
                    )
                    .child(chevron),
            );
        // The patterns fade in and settle 6px down from the header.
        let patterns = self.excludes(s, p, cx).relative().with_animation(
            "advanced-in",
            Animation::new(theme::FADE).with_easing(ease_out_quint()),
            |el, t| el.opacity(t).top(px(-6. * (1. - t))),
        );
        let rows = [header.into_any_element()]
            .into_iter()
            .chain(shown.then(|| patterns.into_any_element()));
        div().mt(px(16.)).child(grouped(p, rows))
    }

    fn root_row(
        &self,
        root: &RootStatus,
        s: &Strings,
        p: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let live = self.live.read(cx);
        let line = root_line(root, &live.permissions, s, live.lang);
        let row = RootRow::new(root, line, &live.permissions, &self.folder_list, s);
        let id = root.id;
        let enabled = Switch::new(("root-enabled", id as u64))
            .cursor_pointer()
            .checked(root.enabled)
            .disabled(row.removing() || self.is_busy(Field::Root(id)))
            .color(p.accent)
            .accessibility_label(root.path.clone())
            .on_click(cx.listener(move |this, &on: &bool, _, cx| {
                this.run(
                    Field::Root(id),
                    move |host| host.engine()?.set_root_enabled(id, on),
                    cx,
                );
            }));
        let path = root.path.clone();
        root_row(
            row,
            RowParts {
                base: self.row(Field::Root(id), p),
                fade_badge: true,
                problem: self
                    .problem(Field::Root(id), s, p)
                    .map(IntoElement::into_any_element),
                controls: vec![enabled.into_any_element()],
                on_remove: cx.listener(move |this, _, window, cx| {
                    this.confirm_remove(id, &path, window, cx)
                }),
            },
            s,
            p,
        )
    }
}
