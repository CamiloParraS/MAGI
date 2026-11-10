//! The icons the app embeds on top of the component bundle.

use gpui_kit::*;

use crate::ui::MARK;

// The default bundle embeds only the component icons; these are the
// settings sidebar's and rows' extras.
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Sparkles,
        ChartColumn,
        Keyboard,
        Trash,
        Power,
        // Settings rows that had no picture: the bundle lacked them.
        SunMoon,
        AppWindow,
        Blend,
        FileUp,
        LogOut,
        SlidersHorizontal,
        FolderX,
        Files,
        FileCheck,
        Clock,
        Tag,
        // Onboarding: the folder drop zone and the search features.
        FolderPlus,
        Folders,
        TextSearch,
        ScanText,
        ScanEye,
        // The detail pane's tag for a QR match.
        ScanQrCode,
        // Ask Magi: the entry points, then the chat's send, retry and
        // disclosure.
        MessageCircleQuestionMark,
        ArrowUp,
        RotateCcw,
        ShieldCheck,
        // Ask Magi's chips (how each file is sent) and the chat's
        // scroll-down arrow.
        TextQuote,
        FileText,
        Image,
        ChevronsUpDown,
        ArrowDown,
        // Ask Magi's sign-in fields.
        Mail,
        LockKeyhole,
        // Keycaps (onboarding).
        ArrowBigUp,
        ChevronUp,
        Command,
        Option,
    ]
);

const MARK_SVG: &[u8] = include_bytes!("../../assets/brand/magi-mark.svg");

pub(super) struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path == MARK {
            return Ok(Some(std::borrow::Cow::Borrowed(MARK_SVG)));
        }
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    use gpui_kit::assets::IconName;

    use super::AppAssets;
    use gpui_kit::{AssetSource, SharedString};

    /// Every icon name written as `IconName::<name>` under `dir`.
    fn icons_named_in(dir: &Path, found: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                icons_named_in(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (at, _) in text.match_indices("IconName::") {
                    let name: String = text[at + "IconName::".len()..]
                        .chars()
                        .take_while(char::is_ascii_alphanumeric)
                        .collect();
                    if name != "ALL" && !name.is_empty() {
                        found.insert(name);
                    }
                }
            }
        }
    }

    // An icon missing from the bundle and from ExtraIcons renders as blank
    // space, and nothing else fails.
    #[test]
    fn every_icon_the_app_names_is_embedded() {
        let catalog: BTreeMap<String, SharedString> = IconName::ALL
            .iter()
            .map(|name| (format!("{name:?}"), name.path()))
            .collect();
        let mut named = BTreeSet::new();
        icons_named_in(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut named,
        );
        assert!(named.contains("Check"), "the scan found no icons");
        let missing: Vec<_> = named
            .iter()
            .filter(|name| {
                catalog
                    .get(*name)
                    .is_none_or(|path| AppAssets.load(path).ok().flatten().is_none())
            })
            .collect();
        assert!(
            missing.is_empty(),
            "not embedded (add to ExtraIcons): {missing:?}"
        );
    }
}
