//! The searched folders as Settings and onboarding both list them: which
//! roots to show, which are being removed, and each one's row.

use std::path::Path;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::*;
use magi_core::dto::{IndexStatus, PermissionIssue, RootStatus};
use magi_core::host::Host;

use super::folders::folder_name;
use super::note;
use crate::i18n::Strings;
use crate::settings::{guidance_of, root_state};
use crate::theme::{self, Palette, Tone};

/// A window's view of the roots.
pub(crate) struct FolderList {
    /// The roots when the window opened, shown until the first status (the
    /// engine sends none during its startup walk).
    opening: Option<Vec<RootStatus>>,
    /// Roots being removed: dimmed, their controls off, until the list no
    /// longer has them.
    removing: Vec<i64>,
}

impl FolderList {
    /// ponytail: a small read on the main thread; move it to the background
    /// executor if it ever shows up in a frame trace.
    pub(crate) fn open(host: &Host) -> Self {
        Self::with_opening(
            host.list_roots()
                .inspect_err(|error| tracing::warn!(%error, "could not list the roots"))
                .ok(),
        )
    }

    fn with_opening(opening: Option<Vec<RootStatus>>) -> Self {
        Self {
            opening,
            removing: Vec::new(),
        }
    }

    /// The latest status's roots, else the opening ones. `None`: not known
    /// yet, which an empty list would misreport as no folders.
    pub(crate) fn roots<'a>(
        &'a self,
        status: Option<&'a IndexStatus>,
    ) -> Option<&'a Vec<RootStatus>> {
        status.map(|status| &status.roots).or(self.opening.as_ref())
    }

    pub(crate) fn set_opening(&mut self, roots: Vec<RootStatus>) {
        self.opening = Some(roots);
    }

    pub(crate) fn start_removing(&mut self, id: i64) {
        self.removing.push(id);
    }

    pub(crate) fn is_removing(&self, id: i64) -> bool {
        self.removing.contains(&id)
    }

    /// Forgets the removals the list has caught up with.
    pub(crate) fn prune(&mut self, status: Option<&IndexStatus>) {
        let ids: Vec<i64> = self.roots(status).map_or(Vec::new(), |roots| {
            roots.iter().map(|root| root.id).collect()
        });
        self.removing.retain(|id| ids.contains(id));
    }

    /// That removal failed: its row works again.
    pub(crate) fn removal_failed(&mut self, id: i64) {
        self.removing.retain(|&r| r != id);
    }

    /// A change that failed without saying which root: every row works again.
    pub(crate) fn all_removals_failed(&mut self) {
        self.removing.clear();
    }
}

/// What a root's row shows, before any drawing.
pub(crate) struct RootRow<'a> {
    root: &'a RootStatus,
    badge: Option<&'static str>,
    tone: Tone,
    /// The line under the name; the windows word it differently.
    line: Option<String>,
    /// Where the OS grants what this root is missing.
    settings_url: Option<&'static str>,
    removing: bool,
}

impl<'a> RootRow<'a> {
    pub(crate) fn new(
        root: &'a RootStatus,
        line: Option<String>,
        issues: &[PermissionIssue],
        list: &FolderList,
        s: &Strings,
    ) -> Self {
        let (badge, tone, _) = root_state(root, s);
        Self {
            root,
            badge,
            tone,
            line,
            settings_url: root
                .enabled
                .then(|| guidance_of(root, issues)?.settings_url())
                .flatten(),
            removing: list.is_removing(root.id),
        }
    }

    pub(crate) fn has_trouble(&self) -> bool {
        matches!(self.tone, Tone::Warn | Tone::Err)
    }

    pub(crate) fn removing(&self) -> bool {
        self.removing
    }
}

/// What differs between the windows' rows.
pub(crate) struct RowParts<F> {
    /// The empty row, which the window may have tinted.
    pub(crate) base: Div,
    /// A changed status fades in, so the change is seen.
    pub(crate) fade_badge: bool,
    /// Under the line: why the last change to this root failed.
    pub(crate) problem: Option<AnyElement>,
    /// Between the badge and the remove button.
    pub(crate) controls: Vec<AnyElement>,
    pub(crate) on_remove: F,
}

/// A root's row: the folder's name with its parent under it, as file
/// managers show folders; then its state, the window's controls, and remove.
pub(crate) fn root_row<F>(row: RootRow, parts: RowParts<F>, s: &Strings, p: &Palette) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let RowParts {
        base,
        fade_badge,
        problem,
        controls,
        on_remove,
    } = parts;
    let root = row.root;
    let id = root.id;
    let tone = row.tone;
    let icon = if row.has_trouble() {
        Icon::new(IconName::TriangleAlert).text_color(p.tone(tone))
    } else {
        Icon::new(IconName::Folder).text_color(p.accent)
    };
    let path = Path::new(&root.path);
    let parent = path.parent().map(|parent| parent.display().to_string());
    let element = base
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
                .children(row.line.map(|line| note(p).child(line)))
                .children(row.settings_url.map(|url| {
                    // Granting there is enough: the 30 s re-probe picks the
                    // folder up again.
                    Button::new(("open-system-settings", id as u64))
                        .cursor_pointer()
                        .small()
                        .mt(px(6.))
                        .label(s.open_system_settings)
                        .on_click(move |_, _, cx| cx.open_url(url))
                }))
                .children(problem),
        )
        .children(row.badge.map(|badge| {
            if fade_badge {
                theme::fade_in(
                    p.status(badge, tone),
                    SharedString::from(format!("root-status-{id}-{badge}")),
                )
                .into_any_element()
            } else {
                p.status(badge, tone).into_any_element()
            }
        }))
        .children(controls)
        .child(
            // Minus, not a trash can: the files stay; only Magi forgets the
            // folder.
            Button::new(("remove-root", id as u64))
                .cursor_pointer()
                .ghost()
                .small()
                .icon(IconName::Minus)
                .tooltip(s.remove)
                .accessibility_label(format!("{} {}", s.remove, root.path))
                // A spinner in the minus's place; it takes no clicks.
                .loading(row.removing)
                .on_click(on_remove),
        );
    if row.removing {
        theme::dim(element, ("removing-root", id as u64)).into_any_element()
    } else {
        element.into_any_element()
    }
}

#[cfg(test)]
mod tests;
