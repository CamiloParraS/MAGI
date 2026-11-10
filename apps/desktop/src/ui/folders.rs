//! Choosing folders: the native picker, and the user's own folders
//! offered with one click (onboarding and Settings).

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::dto::RootStatus;
use magi_core::host::Host;

use super::{PRESS, note, pressable};
use crate::i18n::Strings;
use crate::theme::Palette;

/// The native folder picker (FR-1): the folders chosen, several at once.
pub(crate) fn pick_folders(cx: &App) -> impl Future<Output = Vec<std::path::PathBuf>> + use<> {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: true,
        prompt: None,
    });
    async move {
        match picked.await {
            Ok(Ok(Some(paths))) => paths,
            Ok(Err(error)) => {
                tracing::warn!(%error, "the folder picker failed");
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// The native picker, then `add` with the folders chosen, if any.
pub(crate) fn pick_and_add<V: 'static>(
    cx: &mut Context<V>,
    add: fn(&mut V, Vec<PathBuf>, &mut Context<V>),
) {
    let picked = pick_folders(cx);
    cx.spawn(async move |this, cx| {
        let paths = picked.await;
        if !paths.is_empty() {
            let _ = this.update(cx, |view, cx| add(view, paths, cx));
        }
    })
    .detach();
}

/// Makes `body` take folders dropped from the file manager, the user
/// picking them as with the picker.
pub(crate) fn accept_dropped_folders<V: 'static>(
    body: Div,
    p: &Palette,
    cx: &Context<V>,
    add: fn(&mut V, Vec<PathBuf>, &mut Context<V>),
) -> Div {
    let accent = p.accent;
    body.drag_over::<ExternalPaths>(move |style, _, _, _| {
        style.border_color(accent).bg(accent.opacity(0.08))
    })
    .on_drop(
        cx.listener(move |this, paths: &ExternalPaths, _, cx| {
            add(this, paths.paths().to_vec(), cx)
        }),
    )
}

/// Adds the folders among `paths`; dropped files are ignored. Blocking: run
/// it on the background executor.
pub(crate) fn add_roots(host: &Host, paths: &[PathBuf]) -> magi_core::Result<()> {
    let engine = host.engine()?;
    for path in paths.iter().filter(|p| p.is_dir()) {
        engine.add_root(path)?;
    }
    Ok(())
}

/// The user's folders still worth offering: not chosen, and not inside a
/// chosen folder (already searched).
pub fn suggestions(user: &[PathBuf], roots: &[RootStatus]) -> Vec<PathBuf> {
    user.iter()
        .filter(|dir| !roots.iter().any(|root| dir.starts_with(&root.path)))
        .cloned()
        .collect()
}

/// A folder's name, or the whole path for a drive's root.
pub(crate) fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// With no folders yet: where to drop them, and a button to pick them.
pub(crate) fn drop_zone(
    s: &Strings,
    p: &Palette,
    on_add: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    // The whole zone is the button: click, or drop folders on it. The icon
    // sits in the accent halo the sign-in screen uses, so the empty state
    // reads as part of the same app, and a dashed edge still says "drop here".
    div()
        .rounded(px(12.))
        .border_1()
        .border_dashed()
        .border_color(p.accent.opacity(0.45))
        .bg(p.card)
        .child(
            Button::new("add-folder")
                .cursor_pointer()
                .ghost()
                .group("drop-zone")
                .child(
                    div()
                        .id("drop-zone-content")
                        .relative()
                        .group_active("drop-zone", |s| s.top(PRESS))
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .p(px(6.))
                                .rounded(px(14.))
                                .bg(p.accent.opacity(if p.dark { 0.16 } else { 0.12 }))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(36.))
                                        .rounded(px(10.))
                                        .bg(p.accent)
                                        .child(
                                            Icon::new(IconName::FolderPlus)
                                                .size(px(20.))
                                                .text_color(if p.dark { black() } else { white() }),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .mt(px(8.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(s.add_folder),
                        )
                        .child(div().text_sm().text_color(p.mute).child(s.ob_drop_folders)),
                )
                .w_full()
                .h(px(150.))
                .rounded(px(11.))
                .on_click(on_add),
        )
}

/// A folder list's last row: Add folder. The whole row is the button: click
/// and hover anywhere on it.
pub(crate) fn add_folder_row(
    s: &Strings,
    p: &Palette,
    on_add: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    // gpui-component centers a button's content; one child that fills it
    // keeps the icon and label at the left, the icon in the column of the
    // folder icons above.
    Button::new("add-folder")
        .cursor_pointer()
        .ghost()
        .group("add-folder-row")
        .child(
            div()
                .id("add-folder-content")
                .relative()
                .group_active("add-folder-row", |s| s.top(PRESS))
                .flex_1()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(Icon::new(IconName::Plus).size(px(18.)).text_color(p.accent))
                .child(s.add_folder)
                // Dropping still works once the drop zone is gone; say so
                // where the eye goes to add.
                .child(
                    div()
                        .ml_auto()
                        .text_sm()
                        .text_color(p.mute)
                        .child(s.ob_or_drop_folders),
                ),
        )
        .w_full()
        .h(px(40.))
        .px(px(12.))
        .rounded_none()
        .on_click(on_add)
}

/// The one-click folder suggestions, with "Add all" when there are several;
/// `None` with none left. Onboarding and Settings share it; `add` takes the
/// chosen folders.
pub(crate) fn suggested_folders<V: 'static>(
    offered: Vec<PathBuf>,
    s: &Strings,
    p: &Palette,
    cx: &Context<V>,
    add: fn(&mut V, Vec<PathBuf>, &mut Context<V>),
) -> Option<Div> {
    if offered.is_empty() {
        return None;
    }
    let all = offered.clone();
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(note(p).mt_0().child(s.suggested_folders))
                    // Several at once, as the picker allows.
                    .children((all.len() > 1).then(|| {
                        pressable(
                            Button::new("suggest-all")
                                .cursor_pointer()
                                .ghost()
                                .xsmall()
                                .label(s.add_all_folders)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| add(this, all.clone(), cx)),
                                ),
                            "press-suggest-all",
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(offered.into_iter().enumerate().map(|(ix, dir)| {
                        let group = SharedString::from(format!("suggest-folder-{ix}"));
                        pressable(
                            Button::new(("suggest-folder", ix))
                                .cursor_pointer()
                                .outline()
                                // The dark theme's outline (`input`) reads as white.
                                .when(p.dark, |b| b.border_0())
                                // xsmall keeps all four on one row under onboarding's
                                // folder list.
                                .xsmall()
                                .group(group.clone())
                                // A folder at rest; the plus takes its place
                                // under the pointer, saying what a click does.
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(
                                            div()
                                                .relative()
                                                .size(px(12.))
                                                .child(
                                                    div()
                                                        .group_hover(group.clone(), |s| {
                                                            s.opacity(0.)
                                                        })
                                                        .child(
                                                            Icon::new(IconName::Folder)
                                                                .size(px(12.)),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .top_0()
                                                        .left_0()
                                                        .opacity(0.)
                                                        .group_hover(group.clone(), |s| {
                                                            s.opacity(1.)
                                                        })
                                                        .child(
                                                            Icon::new(IconName::Plus)
                                                                .size(px(12.))
                                                                .text_color(p.accent),
                                                        ),
                                                ),
                                        )
                                        .child(folder_name(&dir)),
                                )
                                .tooltip(dir.display().to_string())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    add(this, vec![dir.clone()], cx)
                                })),
                            ("press-suggest-folder", ix),
                        )
                    })),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::suggestions;

    #[test]
    fn a_suggested_folder_hides_once_it_or_a_parent_is_added() {
        use magi_core::db::roots::Health;
        use magi_core::dto::RootStatus;
        use std::path::PathBuf;
        let root = |path: &str| RootStatus {
            id: 1,
            path: path.into(),
            enabled: true,
            status: Health::Ok,
            indexed: 0,
        };
        let user = [
            PathBuf::from("/home/ana/Documents"),
            PathBuf::from("/home/ana/Desktop"),
            PathBuf::from("/home/ana/Pictures"),
        ];
        assert_eq!(suggestions(&user, &[]), user);
        assert_eq!(
            suggestions(&user, &[root("/home/ana/Desktop")]),
            [user[0].clone(), user[2].clone()]
        );
        // Inside a chosen folder: already searched.
        assert!(suggestions(&user, &[root("/home/ana")]).is_empty());
        // A folder inside a suggestion does not cover all of it.
        assert_eq!(suggestions(&user, &[root("/home/ana/Pictures/2024")]), user);
    }
}
