//! The settings window (SPEC.md M6, FR-9), styled as variant A "Pane":
//! a sidebar plus grouped rows. The logic is here; [`view`] wires it to GPUI.

pub mod view;

use magi_core::config::{TRANSPARENCY_INTENSITY, TransparencyMode};
use magi_core::db::roots::Health;
use magi_core::discovery::Kind;
use magi_core::dto::{
    DownloadError, ErrorCode, FeatureStatus, FileErrorCode, Guidance, IndexStatus, Install,
    PermissionIssue, RootStatus,
};
use magi_core::features::Feature;
use magi_core::host::Host;
use magi_core::platform::BackdropSupport;

use std::time::{Duration, Instant};

use crate::i18n::{Lang, Strings};
use crate::theme::{self, Backdrop, Tone};

/// How long a stat tile's count takes to reach a new value.
pub const ROLL: Duration = Duration::from_millis(200);

/// A stat tile's count on its way to a new value, so a number that changes
/// while indexing runs counts up or down instead of jumping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roll {
    from: u64,
    pub to: u64,
    since: Instant,
    /// Changes with each new target: keys the animation so it restarts.
    pub generation: usize,
}

impl Roll {
    /// Already at `value`: the first status animates nothing.
    pub fn steady(value: u64) -> Self {
        Self {
            from: value,
            to: value,
            since: Instant::now(),
            generation: 0,
        }
    }

    /// The count `eased` (0 to 1, already through the easing) of the way.
    pub fn at(&self, eased: f32) -> u64 {
        let (from, to) = (self.from as f64, self.to as f64);
        (from + (to - from) * f64::from(eased)).round() as u64
    }

    /// The count on screen at `now`.
    pub fn shown(&self, now: Instant) -> u64 {
        let t = now.saturating_duration_since(self.since).as_secs_f32() / ROLL.as_secs_f32();
        self.at(gpui_kit::ease_out_quint()(t.clamp(0., 1.)))
    }

    /// Heads for `to` from whatever shows at `now`; the same target is a no-op.
    pub fn retarget(self, to: u64, now: Instant) -> Self {
        if to == self.to {
            return self;
        }
        Self {
            from: self.shown(now),
            to,
            since: now,
            generation: self.generation + 1,
        }
    }
}

/// With no unreadable file listed, whether files are still to be read: queued
/// (retried ones among them), or counted as errors by a status from before a
/// retry. Until then the list cannot say every file was read.
pub fn still_reading(status: Option<&IndexStatus>) -> bool {
    status.is_some_and(|st| st.queued > 0 || st.errors > 0)
}

/// A root's badge, and a line explaining anything but plain watching
/// (FR-11).
/// A healthy root has no badge: lists stay quiet until something is wrong.
pub fn root_state(
    root: &RootStatus,
    s: &Strings,
) -> (Option<&'static str>, Tone, Option<&'static str>) {
    if !root.enabled {
        return (Some(s.badge_paused), Tone::Neutral, Some(s.root_disabled));
    }
    match root.status {
        Health::Ok => (None, Tone::Ok, None),
        Health::WatchFailed => (Some(s.badge_polling), Tone::Neutral, Some(s.root_polling)),
        Health::Missing => (Some(s.badge_missing), Tone::Warn, Some(s.root_missing)),
        Health::PermissionDenied => (Some(s.badge_denied), Tone::Err, Some(s.root_denied)),
    }
}

/// The guidance the permission report gives `root`, if any.
pub fn guidance_of(root: &RootStatus, issues: &[PermissionIssue]) -> Option<Guidance> {
    issues
        .iter()
        .find(|issue| issue.root_id == root.id)
        .map(|issue| issue.guidance)
}

/// What [`root_state`] explains, made specific by the permission report
/// (M7): which permission, or which Linux setting.
pub fn root_explain(
    root: &RootStatus,
    issues: &[PermissionIssue],
    s: &Strings,
    lang: Lang,
) -> Option<String> {
    let text = match root.enabled.then(|| guidance_of(root, issues)).flatten() {
        Some(Guidance::MacFilesAndFolders) => s.guide_mac_files,
        Some(Guidance::MacFullDiskAccess) => s.guide_mac_full_disk,
        Some(Guidance::WatchLimit { limit: Some(n) }) => {
            return Some(s.guide_watch_limit.replace("{n}", &lang.number(n)));
        }
        _ => return root_state(root, s).2.map(str::to_owned),
    };
    Some(text.to_owned())
}

/// A root's second line: how many of its files are indexed, then what
/// [`root_explain`] says. No count before the first file is indexed.
pub fn root_line(
    root: &RootStatus,
    issues: &[PermissionIssue],
    s: &Strings,
    lang: Lang,
) -> Option<String> {
    let count = (root.indexed > 0).then(|| lang.plural(&s.root_files, root.indexed));
    let explain = root_explain(root, issues, s, lang);
    match (count, explain) {
        (Some(count), Some(explain)) => Some(format!("{count} · {explain}")),
        (count, explain) => count.or(explain),
    }
}

/// An image feature is on but the Images type is off, so it finds nothing:
/// images are then indexed by name only.
pub fn images_needed(types: &[Kind], features: &[FeatureStatus]) -> bool {
    !types.contains(&Kind::Image)
        && features
            .iter()
            .any(|f| f.enabled && matches!(f.feature, Feature::ImageText | Feature::ImageVisual))
}

/// A few of the extensions a type covers, to show what it means. `None`
/// where the label already says it.
pub fn kind_examples(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Text => Some("txt, md, csv, json, yaml…"),
        Kind::Code => Some("rs, py, js, ts, java, c, go, cs…"),
        Kind::Office => Some("docx, pptx, xlsx"),
        Kind::Image => Some("png, jpg, webp, heic, tiff…"),
        Kind::Pdf | Kind::Other => None,
    }
}

/// The button a search feature's box offers besides its switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureAction {
    /// Enable and download at once (the stated size is the consent,
    /// ADR-0010); replaces the switch, which could not work yet.
    TurnOn,
    /// Frees the disk space of a switched-off feature.
    Remove,
    Retry,
    Nothing,
}

pub fn feature_action(f: &FeatureStatus) -> FeatureAction {
    match f.install {
        Install::NotInstalled => FeatureAction::TurnOn,
        Install::Installed { .. } if !f.enabled => FeatureAction::Remove,
        Install::Failed { .. } => FeatureAction::Retry,
        _ => FeatureAction::Nothing,
    }
}

/// The "Results shown" choices, with the saved one even if it is not a
/// preset.
pub fn result_counts(current: u32) -> Vec<u32> {
    let mut counts = vec![10, 20, 30, 50, 100];
    if !counts.contains(&current) {
        counts.push(current);
        counts.sort_unstable();
    }
    counts
}

pub fn file_error_label(code: FileErrorCode, s: &Strings) -> &'static str {
    match code {
        FileErrorCode::PermissionDenied => s.read_denied,
        FileErrorCode::Locked => s.read_locked,
        FileErrorCode::ReadFailed => s.read_failed,
        FileErrorCode::ExtractFailed => s.read_damaged,
        FileErrorCode::PasswordProtected => s.read_password,
        FileErrorCode::TimedOut => s.read_timed_out,
        FileErrorCode::Crashed => s.read_crashed,
        FileErrorCode::EmbedFailed => s.read_embed_failed,
        FileErrorCode::WriteFailed => s.read_write_failed,
        FileErrorCode::Other => s.read_other,
    }
}

pub fn download_error_label(code: DownloadError, s: &Strings) -> &'static str {
    match code {
        DownloadError::DownloadNetworkError => s.dl_network,
        DownloadError::ChecksumMismatch => s.dl_checksum,
        DownloadError::DiskFull => s.dl_disk_full,
        DownloadError::PermissionDenied => s.dl_denied,
        DownloadError::WriteFailed => s.dl_write_failed,
    }
}

/// Why a settings change failed, for the user.
pub fn error_text(error: &ErrorCode, s: &Strings) -> String {
    match error {
        ErrorCode::RootNotFound { path } => s.root_not_found.replace("{path}", path),
        ErrorCode::RootAlreadyExists { path } => s.root_exists.replace("{path}", path),
        ErrorCode::NestedRoot {
            path,
            conflicts_with,
        } => s
            .root_nested
            .replace("{path}", path)
            .replace("{other}", conflicts_with),
        ErrorCode::EngineStarting => s.engine_starting.into(),
        ErrorCode::InvalidGlob { glob } => s.invalid_glob.replace("{glob}", glob),
        ErrorCode::InvalidSetting { field } if field == MAX_SIZE_FIELD => s.invalid_size.into(),
        _ => s.change_failed.into(),
    }
}

/// The kinds that can have their content read, in the order they are shown.
pub const FILE_TYPES: [Kind; 5] = [Kind::Text, Kind::Code, Kind::Pdf, Kind::Office, Kind::Image];

/// The field a bad max-size entry is reported under.
pub const MAX_SIZE_FIELD: &str = "indexing.max_file_size_mb";

pub fn kind_label(kind: Kind, s: &Strings) -> &'static str {
    match kind {
        Kind::Text => s.kind_text,
        Kind::Code => s.kind_code,
        Kind::Pdf => s.kind_pdf,
        Kind::Office => s.kind_office,
        Kind::Image | Kind::Other => s.kind_image,
    }
}

/// `types` with `kind` switched on or off, in [`FILE_TYPES`] order.
pub fn toggle_kind(types: &[Kind], kind: Kind, on: bool) -> Vec<Kind> {
    FILE_TYPES
        .into_iter()
        .filter(|&k| if k == kind { on } else { types.contains(&k) })
        .collect()
}

/// The largest file to read, from what the user typed: whole megabytes, 1
/// or more.
pub fn max_size_from_text(text: &str) -> Option<u64> {
    text.trim().parse().ok().filter(|&mb| mb >= 1)
}

/// Exclusion patterns, one per line; blank lines and surrounding spaces
/// are dropped.
pub fn globs_from_text(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect()
}

/// "Same as system (English)": the `system` choice names what it resolves to.
pub fn system_language(s: &Strings, resolved: Lang) -> String {
    s.language_system.replace("{lang}", resolved.name())
}

/// A background choice as the user sees it.
pub fn background_label(mode: TransparencyMode, s: &Strings) -> &'static str {
    match mode {
        TransparencyMode::MatchSystem => s.background_match_system,
        TransparencyMode::Always => s.background_always,
        TransparencyMode::Never => s.background_never,
    }
}

/// The amount slider only matters when the search window would be see-through.
pub fn see_through_adjustable(mode: TransparencyMode, support: BackdropSupport) -> bool {
    matches!(
        theme::backdrop(mode, *TRANSPARENCY_INTENSITY.end(), support),
        Backdrop::Blurred { .. }
    )
}

/// The slider runs from "More solid" to "More transparent"; the config keeps
/// the tint's alpha (`ui.transparency_intensity`), always inside its allowed
/// range and to two decimals, so float error never fails validation.
pub fn intensity_from_slider(see_through: f32) -> f32 {
    let (lo, hi) = (
        *TRANSPARENCY_INTENSITY.start(),
        *TRANSPARENCY_INTENSITY.end(),
    );
    let alpha = hi - see_through.clamp(0., 1.) * (hi - lo);
    ((alpha * 100.).round() / 100.).clamp(lo, hi)
}

pub fn slider_from_intensity(intensity: f32) -> f32 {
    let (lo, hi) = (
        *TRANSPARENCY_INTENSITY.start(),
        *TRANSPARENCY_INTENSITY.end(),
    );
    ((hi - intensity) / (hi - lo)).clamp(0., 1.)
}

/// Records the wish, then downloads if needed (ADR-0010). Blocking: run it
/// on the background executor.
pub fn turn_on(host: &Host, feature: Feature) -> magi_core::Result<()> {
    let installed = host.features().status().is_ok_and(|all| {
        all.iter()
            .any(|s| s.feature == feature && matches!(s.install, Install::Installed { .. }))
    });
    host.set_feature_enabled(feature, true)?;
    if installed {
        Ok(())
    } else {
        host.features().download(feature)
    }
}

/// Taps on the version label that bring the onboarding back (a way to try
/// it again without the command line).
pub const VERSION_TAPS: usize = 10;

/// The longest pause between two taps that still counts as one run.
pub const VERSION_TAP_GAP: Duration = Duration::from_millis(1500);

/// Counts a tap at `now` against the run in `taps`; true on the
/// [`VERSION_TAPS`]th tap of a run, which then starts over.
pub fn version_tap(taps: &mut (usize, Option<Instant>), now: Instant) -> bool {
    let (count, last) = taps;
    if last.is_none_or(|t| now.saturating_duration_since(t) > VERSION_TAP_GAP) {
        *count = 0;
    }
    *last = Some(now);
    *count += 1;
    if *count >= VERSION_TAPS {
        *count = 0;
        return true;
    }
    false
}

#[cfg(test)]
mod tests;
