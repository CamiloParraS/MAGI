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
