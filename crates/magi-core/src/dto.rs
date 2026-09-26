//! Serializable DTOs shared with the UI.
//!
//! Types here derive `Serialize`, `Deserialize`, and `ts_rs::TS`, and are
//! exported to `apps/desktop/src/bindings/`. Populated as IPC commands land
//! (see SPEC.md §5.7).

use serde::{Deserialize, Serialize};

use crate::db::roots::{Health, Root};
use crate::features::Feature;

/// Whether a feature's download is on disk (ADR-0010). Independent of
/// `FeatureStatus::enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum Install {
    NotInstalled,
    Downloading { bytes: u64, total: u64 },
    Installed { size_bytes: u64 },
    Failed { code: DownloadError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Backfill {
    pub done: u64,
    pub total: u64,
}

/// One search feature at a glance (`features_status`, `engine://features`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FeatureStatus {
    pub feature: Feature,
    /// What the user wants (config).
    pub enabled: bool,
    /// Bytes to download, shown before consent.
    pub download_size: u64,
    pub install: Install,
    /// Files still being brought up to date after the feature came online.
    pub backfill: Option<Backfill>,
}

/// Why a download failed: a stable code, localized by the UI (ADR-0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub enum DownloadError {
    DownloadNetworkError,
    ChecksumMismatch,
    DiskFull,
    PermissionDenied,
    /// Any other local I/O failure while saving the download.
    WriteFailed,
}

impl DownloadError {
    pub fn classify(error: &crate::Error) -> Self {
        use std::io::ErrorKind;
        match error {
            crate::Error::Network { .. } => Self::DownloadNetworkError,
            crate::Error::ChecksumMismatch { .. } => Self::ChecksumMismatch,
            crate::Error::Io { source, .. } => match source.kind() {
                ErrorKind::StorageFull => Self::DiskFull,
                ErrorKind::PermissionDenied => Self::PermissionDenied,
                _ => Self::WriteFailed,
            },
            _ => Self::WriteFailed,
        }
    }
}

/// What the engine is doing (`get_status`, `engine://status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum IndexState {
    Idle,
    /// Walking the roots (a startup, periodic or requested reconciliation).
    Scanning,
    Indexing,
    /// By the user, or for low memory or battery.
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct IndexStatus {
    pub state: IndexState,
    /// Files `pending` or `indexing`.
    pub queued: u64,
    pub indexed: u64,
    pub skipped: u64,
    pub errors: u64,
    /// The file most recently started, while indexing.
    pub current_file: Option<String>,
    pub roots: Vec<RootStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RootStatus {
    pub id: i64,
    pub path: String,
    pub enabled: bool,
    pub status: Health,
}

impl From<Root> for RootStatus {
    fn from(root: Root) -> Self {
        Self {
            id: root.id,
            path: root.path.to_string_lossy().into_owned(),
            enabled: root.enabled,
            status: root.status,
        }
    }
}

/// A snippet plus the ranges of its matched terms, in UTF-16 code units as
/// JavaScript indexes strings (`text.slice(start, end)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Snippet {
    pub text: String,
    pub highlights: Vec<[u32; 2]>,
}

impl Snippet {
    /// Strips the `search::fts` highlight markers, recording where they were.
    /// An unclosed highlight ends with the text.
    pub fn from_marked(marked: &str) -> Self {
        use crate::search::fts::{HIGHLIGHT_END, HIGHLIGHT_START};
        let mut text = String::with_capacity(marked.len());
        let mut highlights = Vec::new();
        let (mut units, mut open) = (0u32, None);
        for c in marked.chars() {
            match c {
                HIGHLIGHT_START => open = Some(units),
                HIGHLIGHT_END => {
                    if let Some(start) = open.take() {
                        highlights.push([start, units]);
                    }
                }
                c => {
                    text.push(c);
                    units += c.len_utf16() as u32;
                }
            }
        }
        if let Some(start) = open {
            highlights.push([start, units]);
        }
        Self { text, highlights }
    }
}

/// A failed command as a stable code plus parameters (SPEC.md §5.7 locale
/// neutrality). The UI localizes it; `Internal.detail` is for logs only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "code")]
#[ts(export)]
pub enum ErrorCode {
    RootNotFound {
        path: String,
    },
    NestedRoot {
        path: String,
        conflicts_with: String,
    },
    RootAlreadyExists {
        path: String,
    },
    RootIdNotFound {
        id: i64,
    },
    FileIdNotFound {
        file_id: i64,
    },
    UnknownFeature {
        name: String,
    },
    DownloadInProgress {
        feature: Feature,
    },
    InvalidSetting {
        field: String,
    },
    InvalidGlob {
        glob: String,
    },
    /// The engine is starting or restarting; retry shortly.
    EngineStarting,
    Internal {
        detail: String,
    },
}

impl From<&crate::Error> for ErrorCode {
    fn from(error: &crate::Error) -> Self {
        use crate::Error as E;
        let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
        match error {
            E::RootNotFound(p) => Self::RootNotFound { path: path(p) },
            E::NestedRoot {
                path: p,
                conflicts_with,
            } => Self::NestedRoot {
                path: path(p),
                conflicts_with: path(conflicts_with),
            },
            E::RootAlreadyExists(p) => Self::RootAlreadyExists { path: path(p) },
            E::RootIdNotFound(id) => Self::RootIdNotFound { id: *id },
            E::FileIdNotFound(file_id) => Self::FileIdNotFound { file_id: *file_id },
            E::UnknownFeature(name) => Self::UnknownFeature { name: name.clone() },
            E::DownloadInProgress(feature) => Self::DownloadInProgress { feature: *feature },
            E::InvalidSetting { field, .. } => Self::InvalidSetting {
                field: field.to_string(),
            },
            E::InvalidGlob { glob, .. } => Self::InvalidGlob { glob: glob.clone() },
            E::EngineStarting => Self::EngineStarting,
            other => Self::Internal {
                detail: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::roots::Health;

    #[test]
    fn root_health_crosses_ipc_as_its_database_name() {
        for health in [
            Health::Ok,
            Health::PermissionDenied,
            Health::Missing,
            Health::WatchFailed,
        ] {
            assert_eq!(
                serde_json::to_value(health).unwrap(),
                serde_json::json!(health.as_str())
            );
        }
    }

    #[test]
    fn snippet_highlights_are_utf16_ranges_and_brackets_stay_text() {
        let s = Snippet::from_marked("😀 [draft] \u{E000}invoice\u{E001} total");
        assert_eq!(s.text, "😀 [draft] invoice total");
        // "😀 [draft] " is 11 UTF-16 units: the emoji takes two.
        assert_eq!(s.highlights, vec![[11, 18]]);
        // An unclosed highlight ends with the text.
        assert_eq!(
            Snippet::from_marked("\u{E000}open").highlights,
            vec![[0, 4]]
        );
    }

    #[test]
    fn errors_cross_ipc_as_codes_with_parameters() {
        let json = |e: crate::Error| serde_json::to_value(ErrorCode::from(&e)).unwrap();
        assert_eq!(
            json(crate::Error::RootIdNotFound(7)),
            serde_json::json!({"code": "RootIdNotFound", "id": 7})
        );
        assert_eq!(
            json(crate::Error::InvalidSetting {
                field: "ui.transparency_intensity",
                reason: "english prose".into()
            }),
            serde_json::json!({"code": "InvalidSetting", "field": "ui.transparency_intensity"})
        );
        assert_eq!(json(crate::Error::EngineStarting)["code"], "EngineStarting");
        assert_eq!(
            json(crate::Error::Engine("boom".into()))["code"],
            "Internal"
        );
    }
}
