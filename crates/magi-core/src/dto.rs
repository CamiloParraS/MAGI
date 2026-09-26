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

#[cfg(test)]
mod tests {
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
}
