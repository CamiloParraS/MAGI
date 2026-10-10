//! Settings › Folders: the searched folders.

use gpui_kit::assets::IconName;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::*;
use magi_core::dto::RootStatus;

use super::{Field, SettingsView};
use crate::i18n::Strings;
use crate::settings::{root_line, root_state};
use crate::theme::{self, Palette, Tone};
use crate::ui::folders::{pick_folders, suggested_folders, suggestions};
use crate::ui::{card, note, stack, subheading};

impl SettingsView {
    /// The native folder picker (FR-1), then `add_root` for each.
    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let picked = pick_folders(cx);
        cx.spawn(async move |this, cx| {
            let paths = picked.await;
            if !paths.is_empty() {
                let _ = this.update(cx, |view, cx| view.add_folders(paths, cx));
            }
        })
        .detach();
    }

    /// Picked, suggested or dropped folders. Dropped files are ignored.
    fn add_folders(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.run(
            Field::AddFolder,
            move |host| {
                let engine = host.engine()?;
                for path in paths.iter().filter(|p| p.is_dir()) {
                    engine.add_root(path)?;
                }
                Ok(())
            },
            cx,
        );
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
                        view.removing.push(id);
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
        let accent = p.accent;
        let live = self.live.read(cx);
        let s = live.lang.strings();
        let roots = live
            .status
            .as_ref()
            .map(|status| &status.roots)
            .or(self.opening_roots.as_ref());
        // Nothing when the roots are unknown: an empty list would read "no folders".
        let list = roots.map(|roots| {
            if roots.is_empty() {
                return stack().child(card(p).text_color(p.mute).child(s.no_folders));
            }
            stack().children(roots.iter().map(|root| self.root_row(root, s, p, cx)))
        });
        // An add that failed has no row yet: its own box says why.
        let add_problem = self.problem(Field::AddFolder, s, p).map(|problem| {
            self.card(Field::AddFolder, p)
                .child(div().flex_1().min_w_0().child(problem))
        });
        div()
            .child(
                div()
                    .flex()
                    .items_center()
                    .mb(px(8.))
                    .child(div().flex_1().child(subheading(s.searched_folders, p)))
                    // Read with the button: "Drag folders onto the list, or
                    // [Add folders]".
                    .child(
                        div()
                            .mr(px(8.))
                            .text_sm()
                            .text_color(p.mute)
                            .child(s.drag_folders_or),
                    )
                    .child(
                        Button::new("add-folder")
                            .cursor_pointer()
                            .primary()
                            .small()
                            .icon(IconName::Plus)
                            .label(s.add_folder)
                            .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
                    ),
            )
            // Folders dropped from the file manager, as in onboarding.
            .child(
                stack()
                    .rounded(px(10.))
                    .children(list)
                    .children(add_problem)
                    .drag_over::<ExternalPaths>(move |style, _, _, _| {
                        style.bg(accent.opacity(0.08))
                    })
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.add_folders(paths.paths().to_vec(), cx)
                    })),
            )
            .children(roots.and_then(|roots| {
                let offered = suggestions(&self.user_folders, roots);
                suggested_folders(offered, s, p, cx, Self::add_folders)
                    .map(|chips| chips.mt(px(12.)))
            }))
            .child(
                div()
                    .mt(px(22.))
                    .mb(px(8.))
                    .child(subheading(s.what_to_index, p)),
            )
            .child(self.what_to_index(s, p, cx))
    }

    fn root_row(
        &self,
        root: &RootStatus,
        s: &Strings,
        p: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (badge, tone, _) = root_state(root, s);
        let line = root_line(root, s, self.live.read(cx).lang);
        let id = root.id;
        let removing = self.removing.contains(&id);
        let icon = match tone {
            Tone::Warn | Tone::Err => Icon::new(IconName::TriangleAlert).text_color(p.tone(tone)),
            _ => Icon::new(IconName::Folder).text_color(p.mute),
        };
        let row = self
            .card(Field::Root(id), p)
            .child(icon.size(px(16.)))
            .child(
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
                    .children(line.map(|line| note(p).child(line)))
                    .children(self.problem(Field::Root(id), s, p)),
            )
            // A changed status fades in, so the change is seen.
            .children(badge.map(|badge| {
                theme::fade_in(
                    p.status(badge, tone),
                    SharedString::from(format!("root-status-{id}-{badge}")),
                )
            }))
            .child(
                Switch::new(("root-enabled", id as u64))
                    .cursor_pointer()
                    .checked(root.enabled)
                    .disabled(removing)
                    .color(p.accent)
                    .accessibility_label(root.path.clone())
                    .on_click(cx.listener(move |this, &on: &bool, _, cx| {
                        this.run(
                            Field::Root(id),
                            move |host| host.engine()?.set_root_enabled(id, on),
                            cx,
                        );
                    })),
            )
            .child(
                Button::new(("remove-root", id as u64))
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .tooltip(s.remove)
                    .accessibility_label(format!("{} {}", s.remove, root.path))
                    // A spinner in the trash can's place; it takes no clicks.
                    .loading(removing)
                    .on_click({
                        let path = root.path.clone();
                        cx.listener(move |this, _, window, cx| {
                            this.confirm_remove(id, &path, window, cx)
                        })
                    }),
            );
        if removing {
            theme::dim(row, ("removing-root", id as u64)).into_any_element()
        } else {
            row.into_any_element()
        }
    }
}
