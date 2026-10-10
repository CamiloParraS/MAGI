//! Choosing folders: the native picker, and the user's own folders
//! offered with one click (onboarding and Settings).

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use magi_core::dto::RootStatus;

use super::note;
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
                        Button::new("suggest-all")
                            .cursor_pointer()
                            .ghost()
                            .xsmall()
                            .label(s.add_all_folders)
                            .on_click(cx.listener(move |this, _, _, cx| add(this, all.clone(), cx)))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(offered.into_iter().enumerate().map(|(ix, dir)| {
                        Button::new(("suggest-folder", ix))
                            .cursor_pointer()
                            .outline()
                            // The dark theme's outline (`input`) reads as white.
                            .when(p.dark, |b| b.border_0())
                            // xsmall keeps all four on one row under onboarding's
                            // folder list.
                            .xsmall()
                            .icon(IconName::Plus)
                            .label(folder_name(&dir))
                            .tooltip(dir.display().to_string())
                            .on_click(
                                cx.listener(move |this, _, _, cx| add(this, vec![dir.clone()], cx)),
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
