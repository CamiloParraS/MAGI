//! Serializable DTOs shared with the UI.
//!
//! Types here derive `Serialize`, `Deserialize`, and `ts_rs::TS`, and are
//! exported to `apps/desktop/src/bindings/`. Populated as IPC commands land
//! (see SPEC.md §5.7). The `ts_rs::TS` derive arrives with M6's bindings.

use serde::{Deserialize, Serialize};

use crate::db::roots::Root;

/// Why a download failed: a stable code, localized by the UI (ADR-0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexState {
    Idle,
    /// Walking the roots (a startup, periodic or requested reconciliation).
    Scanning,
    Indexing,
    /// By the user, or for low memory or battery.
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootStatus {
    pub id: i64,
    pub path: String,
    pub enabled: bool,
    /// `ok`, `missing`, `permission_denied` or `watch_failed`.
    pub status: String,
}

impl From<Root> for RootStatus {
    fn from(root: Root) -> Self {
        Self {
            id: root.id,
            path: root.path.to_string_lossy().into_owned(),
            enabled: root.enabled,
            status: root.status.as_str().to_string(),
        }
    }
}
