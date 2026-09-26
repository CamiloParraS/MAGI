//! Optional search features (ADR-0010): which ones exist, where their
//! downloads live, and the per-file record of what a file was indexed without.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::embed::manager::{ByteFetcher, ModelEntry, download_with_fetcher};
use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::{Error, Result};
use crate::ocr::OcrEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Search by meaning: e5 text embeddings.
    Meaning,
    /// Read text in images: OCR.
    ImageText,
    /// Find images by what they show: SigLIP 2.
    ImageVisual,
}

impl Feature {
    pub const ALL: [Feature; 3] = [Feature::Meaning, Feature::ImageText, Feature::ImageVisual];

    /// The `models/manifest.toml` slot holding this feature's download.
    pub fn slot(self) -> &'static str {
        match self {
            Self::Meaning => "text",
            Self::ImageText => "ocr",
            Self::ImageVisual => "image",
        }
    }

    /// This feature's bit in `files.features_missing`.
    pub fn bit(self) -> i64 {
        match self {
            Self::Meaning => 1,
            Self::ImageText => 2,
            Self::ImageVisual => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Meaning => "meaning",
            Self::ImageText => "image_text",
            Self::ImageVisual => "image_visual",
        }
    }
}

impl std::str::FromStr for Feature {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|f| f.as_str() == s)
            .ok_or_else(|| Error::UnknownFeature(s.to_string()))
    }
}

/// The loaded parts behind each feature; `None` means the feature is not
/// running (off or not installed), and indexing and search skip it.
#[derive(Clone, Default)]
pub struct Components {
    pub text: Option<Arc<dyn TextEmbedder>>,
    pub ocr: Option<Arc<dyn OcrEngine>>,
    pub image: Option<Arc<dyn ImageEmbedder>>,
}

impl Components {
    pub fn running(&self) -> Vec<Feature> {
        let mut on = Vec::new();
        if self.text.is_some() {
            on.push(Feature::Meaning);
        }
        if self.ocr.is_some() {
            on.push(Feature::ImageText);
        }
        if self.image.is_some() {
            on.push(Feature::ImageVisual);
        }
        on
    }
}

/// Bytes on disk when every file of `entry` is at its final name with the
/// manifest's size; `None` otherwise. Sizes, not hashes: hashing ~500 MB on
/// every status call is too slow, and a file only gets its final name after
/// its SHA-256 matched (`download_with_fetcher`).
pub fn installed_size(models_root: &Path, entry: &ModelEntry) -> Option<u64> {
    let dir = models_root.join(&entry.slot);
    entry.files.iter().try_fold(0u64, |sum, file| {
        let len = std::fs::metadata(dir.join(&file.name)).ok()?.len();
        (len == file.size).then_some(sum + len)
    })
}

/// What downloading `entry` costs, shown before consent.
pub fn download_size(entry: &ModelEntry) -> u64 {
    entry.files.iter().map(|f| f.size).sum()
}

/// Deletes `feature`'s downloaded files, never any index data.
pub fn remove_download_dir(models_root: &Path, feature: Feature) -> Result<()> {
    let dir = models_root.join(feature.slot());
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::Io {
            path: dir,
            source: e,
        }),
        _ => Ok(()),
    }
}

/// Downloads every file of `entry` in order; `on_progress(bytes, total)`
/// counts across all of them.
pub fn download_entry(
    fetcher: &dyn ByteFetcher,
    models_root: &Path,
    entry: &ModelEntry,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<()> {
    let total = download_size(entry);
    let dir = models_root.join(&entry.slot);
    let mut done = 0;
    for file in &entry.files {
        download_with_fetcher(fetcher, file, &dir, cancel, |bytes, _| {
            on_progress(done + bytes, total)
        })?;
        done += file.size;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::io::{Cursor, Read};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    use sha2::{Digest, Sha256};

    use crate::dto::DownloadError;
    use crate::embed::manager::{ByteFetcher, ModelEntry, ModelFile};

    pub(super) fn sha_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// A manifest entry whose files are `(name, bytes)`, fetched from `mem://name`.
    pub(super) fn entry(slot: &str, files: &[(&str, &[u8])]) -> ModelEntry {
        ModelEntry {
            slot: slot.into(),
            id: "test".into(),
            revision: "r".into(),
            license: "MIT".into(),
            dim: 0,
            max_tokens: None,
            query_prefix: None,
            passage_prefix: None,
            pooling: None,
            files: files
                .iter()
                .map(|(name, bytes)| ModelFile {
                    name: name.to_string(),
                    url: format!("mem://{name}"),
                    sha256: sha_hex(bytes),
                    size: bytes.len() as u64,
                })
                .collect(),
        }
    }

    /// Serves `mem://name` from memory, honoring `Range`.
    #[derive(Default, Clone)]
    pub(super) struct MemFetcher(pub HashMap<String, Vec<u8>>);

    impl ByteFetcher for MemFetcher {
        fn fetch(&self, url: &str, offset: u64) -> crate::Result<(bool, Box<dyn Read>)> {
            let bytes = self
                .0
                .get(url)
                .cloned()
                .ok_or_else(|| crate::Error::Network {
                    url: url.into(),
                    reason: "404".into(),
                })?;
            Ok((
                offset > 0,
                Box::new(Cursor::new(bytes[offset as usize..].to_vec())),
            ))
        }
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    #[test]
    fn a_slot_with_every_file_at_its_manifest_size_is_installed() {
        let root = tempfile::tempdir().unwrap();
        let e = entry("text", &[("a.onnx", b"aaaa"), ("t.json", b"tt")]);
        assert_eq!(installed_size(root.path(), &e), None);
        write(&root.path().join("text"), "a.onnx", b"aaaa");
        assert_eq!(installed_size(root.path(), &e), None, "one file missing");
        write(&root.path().join("text"), "t.json", b"tt");
        assert_eq!(installed_size(root.path(), &e), Some(6));
        assert_eq!(download_size(&e), 6);
    }

    #[test]
    fn a_wrong_size_file_is_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let e = entry("text", &[("a.onnx", b"aaaa")]);
        write(&root.path().join("text"), "a.onnx", b"aa");
        assert_eq!(installed_size(root.path(), &e), None);
        // A leftover partial never counts either.
        std::fs::remove_file(root.path().join("text/a.onnx")).unwrap();
        write(&root.path().join("text"), "a.onnx.partial", b"aaaa");
        assert_eq!(installed_size(root.path(), &e), None);
    }

    #[test]
    fn download_entry_installs_every_file_and_reports_cumulative_progress() {
        let root = tempfile::tempdir().unwrap();
        let e = entry("ocr", &[("det.onnx", b"dddd"), ("rec.onnx", b"rr")]);
        let fetcher = MemFetcher(HashMap::from([
            ("mem://det.onnx".to_string(), b"dddd".to_vec()),
            ("mem://rec.onnx".to_string(), b"rr".to_vec()),
        ]));
        let mut seen = Vec::new();
        download_entry(
            &fetcher,
            root.path(),
            &e,
            &AtomicBool::new(false),
            |b, t| seen.push((b, t)),
        )
        .unwrap();
        assert_eq!(installed_size(root.path(), &e), Some(6));
        assert_eq!(seen.last(), Some(&(6, 6)));
        assert!(
            seen.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress never goes back: {seen:?}"
        );
    }

    #[test]
    fn download_failures_map_to_stable_codes() {
        let root = tempfile::tempdir().unwrap();
        let e = entry("ocr", &[("det.onnx", b"dddd")]);
        let wrong = MemFetcher(HashMap::from([(
            "mem://det.onnx".to_string(),
            b"XXXX".to_vec(),
        )]));
        let err = download_entry(&wrong, root.path(), &e, &AtomicBool::new(false), |_, _| {})
            .unwrap_err();
        assert_eq!(
            DownloadError::classify(&err),
            DownloadError::ChecksumMismatch
        );
        assert_eq!(installed_size(root.path(), &e), None);

        let missing = MemFetcher::default();
        let err = download_entry(
            &missing,
            root.path(),
            &e,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap_err();
        assert_eq!(
            DownloadError::classify(&err),
            DownloadError::DownloadNetworkError
        );

        let full = crate::Error::Io {
            path: "x".into(),
            source: std::io::Error::from(std::io::ErrorKind::StorageFull),
        };
        assert_eq!(DownloadError::classify(&full), DownloadError::DiskFull);
        let denied = crate::Error::Io {
            path: "x".into(),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        assert_eq!(
            DownloadError::classify(&denied),
            DownloadError::PermissionDenied
        );
    }

    #[test]
    fn a_cancelled_download_is_its_own_error_not_a_failure_code() {
        let root = tempfile::tempdir().unwrap();
        let e = entry("ocr", &[("det.onnx", b"dddd")]);
        let fetcher = MemFetcher(HashMap::from([(
            "mem://det.onnx".to_string(),
            b"dddd".to_vec(),
        )]));
        let err = download_entry(&fetcher, root.path(), &e, &AtomicBool::new(true), |_, _| {})
            .unwrap_err();
        assert!(matches!(err, crate::Error::DownloadCancelled));
        assert_eq!(installed_size(root.path(), &e), None);
    }

    #[test]
    fn removing_a_download_deletes_only_that_slot_and_tolerates_absence() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("ocr"), "det.onnx", b"d");
        write(&root.path().join("text"), "model.onnx", b"m");
        remove_download_dir(root.path(), Feature::ImageText).unwrap();
        assert!(!root.path().join("ocr").exists());
        assert!(root.path().join("text/model.onnx").exists());
        remove_download_dir(root.path(), Feature::ImageText).unwrap();
    }

    #[test]
    fn features_round_trip_through_their_wire_names() {
        for f in Feature::ALL {
            assert_eq!(f.as_str().parse::<Feature>().unwrap(), f);
        }
        assert!(matches!(
            "ocr".parse::<Feature>(),
            Err(crate::Error::UnknownFeature(_))
        ));
    }

    #[test]
    fn each_feature_has_its_own_bit_and_slot() {
        let bits: Vec<i64> = Feature::ALL.iter().map(|f| f.bit()).collect();
        assert_eq!(bits, [1, 2, 4]);
        let slots: Vec<&str> = Feature::ALL.iter().map(|f| f.slot()).collect();
        assert_eq!(slots, ["text", "ocr", "image"]);
    }
}
