//! Optional search features (ADR-0010): which ones exist, where their
//! downloads live, and the per-file record of what a file was indexed without.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use serde::{Deserialize, Serialize};

use crate::dto::{Backfill, DownloadError, FeatureStatus, Install};
use crate::embed::manager::{
    ByteFetcher, ModelEntry, ModelManifest, UreqFetcher, download_with_fetcher, models_root,
};
use crate::embed::{
    E5Embedder, FakeEmbedder, FakeImageEmbedder, ImageEmbedder, SigLipEmbedder, TextEmbedder,
};
use crate::error::{Error, Result};
use crate::ocr::OcrEngine;
use crate::ocr::paddle::PaddleOcr;

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

/// Loads what each enabled, installed feature needs. `MAGI_FAKE_EMBEDDER=1`
/// (SPEC.md §4.5) swaps in the deterministic fakes for meaning and image
/// visual whether installed or not; there is no fake OCR. A feature whose
/// files fail to load is left out with a warning rather than failing startup.
pub fn load_components(config: &crate::config::FeaturesConfig) -> Components {
    let fake = std::env::var("MAGI_FAKE_EMBEDDER").as_deref() == Ok("1");
    let manifest = match ModelManifest::load() {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!(error = %e, "model manifest unreadable; no search features");
            None
        }
    };
    let root = models_root();
    let installed = |f: Feature| {
        manifest
            .as_ref()
            .and_then(|m| m.slot(f.slot()))
            .and_then(|e| installed_size(&root, e))
            .is_some()
    };
    let wanted =
        |f: Feature| config.enabled(f) && (installed(f) || (fake && f != Feature::ImageText));

    let text: Option<Arc<dyn TextEmbedder>> = if !wanted(Feature::Meaning) {
        None
    } else if fake {
        Some(Arc::new(FakeEmbedder))
    } else {
        match E5Embedder::load() {
            Ok(e) => Some(Arc::new(e)),
            Err(e) => {
                tracing::warn!(error = %e, "search by meaning not loaded");
                None
            }
        }
    };
    let ocr: Option<Arc<dyn OcrEngine>> = if !wanted(Feature::ImageText) {
        None
    } else {
        match PaddleOcr::load() {
            Ok(o) => Some(Arc::new(o)),
            Err(e) => {
                tracing::warn!(error = %e, "reading text in images not loaded");
                None
            }
        }
    };
    // `SigLipEmbedder::new()` is lazy: a broken install shows up at first use.
    let image: Option<Arc<dyn ImageEmbedder>> = if !wanted(Feature::ImageVisual) {
        None
    } else if fake {
        Some(Arc::new(FakeImageEmbedder))
    } else {
        Some(Arc::new(SigLipEmbedder::new()))
    };
    Components { text, ocr, image }
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

/// How often a running backfill's progress is re-read, and the minimum gap
/// between two download-progress events.
const TICK: Duration = Duration::from_millis(250);

/// Owns search-feature state (ADR-0010): desire (config), availability
/// (disk plus the download in flight) and backfill progress (database).
/// Every change goes to subscribers as the complete `Vec<FeatureStatus>`.
#[derive(Clone)]
pub struct Features {
    inner: Arc<Inner>,
}

struct Inner {
    db_path: PathBuf,
    config_path: PathBuf,
    models_root: PathBuf,
    manifest: ModelManifest,
    fetcher: Arc<dyn ByteFetcher + Send + Sync>,
    /// `Downloading` or `Failed` for features whose disk state is not the answer.
    runtime: Mutex<HashMap<Feature, Install>>,
    /// Aborts the running download; `download_one` clears it.
    cancel: AtomicBool,
    /// Bumped by each cancel; a queued download from an older one is dropped.
    cancels: AtomicU64,
    queue: Sender<(Feature, u64)>,
    subscribers: Mutex<Vec<Sender<Vec<FeatureStatus>>>>,
    last_sent: Mutex<Option<Vec<FeatureStatus>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Features {
    /// The real manager: `config.toml`, `<data_dir>/models`, the embedded
    /// manifest, HTTPS downloads.
    pub fn start(db_path: &Path) -> Result<Self> {
        Self::start_with(
            db_path,
            crate::config::config_path(),
            models_root(),
            ModelManifest::load()?,
            Arc::new(UreqFetcher),
        )
    }

    pub(crate) fn start_with(
        db_path: &Path,
        config_path: PathBuf,
        models_root: PathBuf,
        manifest: ModelManifest,
        fetcher: Arc<dyn ByteFetcher + Send + Sync>,
    ) -> Result<Self> {
        let (queue, rx) = crossbeam_channel::unbounded();
        let inner = Arc::new(Inner {
            db_path: db_path.to_path_buf(),
            config_path,
            models_root,
            manifest,
            fetcher,
            runtime: Mutex::default(),
            cancel: AtomicBool::new(false),
            cancels: AtomicU64::new(0),
            queue,
            subscribers: Mutex::default(),
            last_sent: Mutex::default(),
        });
        // The baseline: subscribers get it on `subscribe`, so the thread's
        // first tick must not re-send it as a change.
        *lock(&inner.last_sent) = inner.status().ok();
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("features".into())
            .spawn(move || run(weak, rx))
            .map_err(|e| Error::Engine(format!("could not start the features thread: {e}")))?;
        Ok(Self { inner })
    }

    pub fn status(&self) -> Result<Vec<FeatureStatus>> {
        self.inner.status()
    }

    /// The current state now, then every change.
    pub fn subscribe(&self) -> Receiver<Vec<FeatureStatus>> {
        let (tx, rx) = crossbeam_channel::unbounded();
        if let Ok(now) = self.inner.status() {
            let _ = tx.send(now);
        }
        lock(&self.inner.subscribers).push(tx);
        rx
    }

    /// Records what the user wants. Does not download, cancel or delete
    /// anything; the host restarts the engine to apply it.
    pub fn set_enabled(&self, feature: Feature, on: bool) -> Result<()> {
        let mut config = crate::config::load_from(&self.inner.config_path)?;
        config.features.set(feature, on);
        crate::config::save_to(&self.inner.config_path, &config)?;
        self.inner.emit();
        Ok(())
    }

    /// Queues `feature`'s download; one runs at a time.
    pub fn download(&self, feature: Feature) -> Result<()> {
        let total = self.inner.entry(feature).map(download_size).unwrap_or(0);
        lock(&self.inner.runtime).insert(feature, Install::Downloading { bytes: 0, total });
        let cancels = self.inner.cancels.load(Ordering::SeqCst);
        self.inner
            .queue
            .send((feature, cancels))
            .map_err(|_| Error::Engine("the features thread stopped".into()))?;
        self.inner.emit();
        Ok(())
    }

    /// Stops the running download and drops every queued one; they all go
    /// back to `NotInstalled`. Partial files stay for a later resume.
    /// Downloads queued after the call still run.
    pub fn cancel_download(&self) {
        self.inner.cancels.fetch_add(1, Ordering::SeqCst);
        self.inner.cancel.store(true, Ordering::SeqCst);
    }

    /// Deletes `feature`'s downloaded files; its indexed data stays.
    pub fn remove_download(&self, feature: Feature) -> Result<()> {
        if matches!(
            lock(&self.inner.runtime).get(&feature),
            Some(Install::Downloading { .. })
        ) {
            return Err(Error::DownloadInProgress(feature));
        }
        remove_download_dir(&self.inner.models_root, feature)?;
        lock(&self.inner.runtime).remove(&feature);
        self.inner.emit();
        Ok(())
    }
}

impl Inner {
    fn entry(&self, feature: Feature) -> Option<&ModelEntry> {
        self.manifest.slot(feature.slot())
    }

    // ponytail: status re-reads config and opens a connection per tick; keep a connection in Inner if profiling shows it.
    fn status(&self) -> Result<Vec<FeatureStatus>> {
        let config = crate::config::load_from(&self.config_path)?;
        let conn = crate::db::open(&self.db_path)?;
        let runtime = lock(&self.runtime).clone();
        Feature::ALL
            .into_iter()
            .map(|feature| {
                let entry = self.entry(feature);
                let install = match runtime.get(&feature) {
                    Some(install) => install.clone(),
                    None => match entry.and_then(|e| installed_size(&self.models_root, e)) {
                        Some(size_bytes) => Install::Installed { size_bytes },
                        None => Install::NotInstalled,
                    },
                };
                Ok(FeatureStatus {
                    feature,
                    enabled: config.features.enabled(feature),
                    download_size: entry.map(download_size).unwrap_or(0),
                    install,
                    backfill: crate::index::backfill_progress(&conn, feature)?
                        .map(|(done, total)| Backfill { done, total }),
                })
            })
            .collect()
    }

    /// Sends the complete state to every subscriber if it changed.
    fn emit(&self) {
        let now = match self.status() {
            Ok(now) => now,
            Err(e) => {
                tracing::warn!(error = %e, "could not read feature status");
                return;
            }
        };
        let mut last = lock(&self.last_sent);
        if last.as_ref() == Some(&now) {
            return;
        }
        lock(&self.subscribers).retain(|tx| tx.send(now.clone()).is_ok());
        *last = Some(now);
    }

    fn download_one(&self, feature: Feature, queued_at: u64) {
        // Clear before comparing: a cancel landing after the compare sets
        // `cancel` again and aborts this download.
        self.cancel.store(false, Ordering::SeqCst);
        if queued_at != self.cancels.load(Ordering::SeqCst) {
            lock(&self.runtime).remove(&feature);
            return;
        }
        let result = match self.entry(feature) {
            Some(entry) => {
                let mut last = Instant::now();
                download_entry(
                    &*self.fetcher,
                    &self.models_root,
                    entry,
                    &self.cancel,
                    |bytes, total| {
                        if last.elapsed() >= TICK {
                            last = Instant::now();
                            lock(&self.runtime)
                                .insert(feature, Install::Downloading { bytes, total });
                            self.emit();
                        }
                    },
                )
            }
            None => Err(Error::ManifestParse(format!(
                "no manifest slot {}",
                feature.slot()
            ))),
        };
        let mut runtime = lock(&self.runtime);
        match result {
            Ok(()) | Err(Error::DownloadCancelled) => {
                runtime.remove(&feature);
            }
            Err(e) => {
                tracing::warn!(feature = feature.as_str(), error = %e, "download failed");
                runtime.insert(
                    feature,
                    Install::Failed {
                        code: DownloadError::classify(&e),
                    },
                );
            }
        }
    }
}

/// The features thread: runs queued downloads one at a time and, between
/// them, re-reads backfill progress so subscribers see it move.
fn run(inner: Weak<Inner>, rx: Receiver<(Feature, u64)>) {
    loop {
        let next = rx.recv_timeout(TICK);
        let Some(inner) = inner.upgrade() else { return };
        match next {
            Ok((feature, queued_at)) => inner.download_one(feature, queued_at),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        inner.emit();
    }
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

    use std::sync::Arc;
    use std::time::Duration;

    use crossbeam_channel::Receiver;

    use crate::dto::{FeatureStatus, Install};
    use crate::embed::manager::ModelManifest;

    struct Harness {
        _dir: tempfile::TempDir,
        features: Features,
        models_root: std::path::PathBuf,
        config_path: std::path::PathBuf,
    }

    const TEXT: &[u8] = b"text-model-bytes";
    const OCR: &[u8] = b"ocr";
    const IMAGE: &[u8] = b"image-model";

    fn harness(fetcher: impl ByteFetcher + Send + Sync + 'static, bad_ocr_hash: bool) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let mut ocr = entry("ocr", &[("det.onnx", OCR)]);
        if bad_ocr_hash {
            ocr.files[0].sha256 = "0".repeat(64);
        }
        let manifest = ModelManifest {
            models: vec![
                entry("text", &[("model.onnx", TEXT)]),
                ocr,
                entry("image", &[("v.onnx", IMAGE)]),
            ],
        };
        let models_root = dir.path().join("models");
        let config_path = dir.path().join("config.toml");
        let features = Features::start_with(
            &dir.path().join("magi.db"),
            config_path.clone(),
            models_root.clone(),
            manifest,
            Arc::new(fetcher),
        )
        .unwrap();
        Harness {
            _dir: dir,
            features,
            models_root,
            config_path,
        }
    }

    fn mem() -> MemFetcher {
        MemFetcher(HashMap::from([
            ("mem://model.onnx".to_string(), TEXT.to_vec()),
            ("mem://det.onnx".to_string(), OCR.to_vec()),
            ("mem://v.onnx".to_string(), IMAGE.to_vec()),
        ]))
    }

    fn install_of(s: &[FeatureStatus], f: Feature) -> Install {
        s.iter().find(|x| x.feature == f).unwrap().install.clone()
    }

    /// Waits for an event where `f` is in a state `done` accepts; every event
    /// on the way must be complete.
    fn wait_for(
        rx: &Receiver<Vec<FeatureStatus>>,
        f: Feature,
        done: impl Fn(&Install) -> bool,
    ) -> Vec<FeatureStatus> {
        loop {
            let s = rx
                .recv_timeout(Duration::from_secs(10))
                .expect("no status event");
            assert_eq!(s.len(), 3, "events carry the complete state");
            if done(&install_of(&s, f)) {
                return s;
            }
        }
    }

    #[test]
    fn status_reports_desire_availability_and_size_separately() {
        let h = harness(mem(), false);
        let s = h.features.status().unwrap();
        assert_eq!(
            s.iter().map(|x| x.feature).collect::<Vec<_>>(),
            Feature::ALL
        );
        assert!(s[0].enabled && s[1].enabled && !s[2].enabled);
        assert!(
            s.iter()
                .all(|x| x.install == Install::NotInstalled && x.backfill.is_none())
        );
        assert_eq!(s[0].download_size, TEXT.len() as u64);
    }

    #[test]
    fn a_download_reports_progress_then_installed() {
        let h = harness(mem(), false);
        let rx = h.features.subscribe();
        h.features.download(Feature::Meaning).unwrap();
        let s = wait_for(&rx, Feature::Meaning, |i| {
            matches!(i, Install::Installed { .. })
        });
        assert_eq!(
            install_of(&s, Feature::Meaning),
            Install::Installed {
                size_bytes: TEXT.len() as u64
            }
        );
        assert!(h.models_root.join("text/model.onnx").exists());
    }

    #[test]
    fn a_checksum_mismatch_is_failed_and_never_installed() {
        let h = harness(mem(), true);
        let rx = h.features.subscribe();
        h.features.download(Feature::ImageText).unwrap();
        let s = wait_for(&rx, Feature::ImageText, |i| {
            matches!(i, Install::Failed { .. })
        });
        assert_eq!(
            install_of(&s, Feature::ImageText),
            Install::Failed {
                code: DownloadError::ChecksumMismatch
            }
        );
        assert!(!h.models_root.join("ocr/det.onnx").exists());
    }

    /// Hands out one byte per read, slowly, so a test can cancel mid-download.
    #[derive(Clone)]
    struct SlowFetcher(MemFetcher);

    impl ByteFetcher for SlowFetcher {
        fn fetch(&self, url: &str, offset: u64) -> crate::Result<(bool, Box<dyn Read>)> {
            struct Slow(Box<dyn Read>);
            impl Read for Slow {
                fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                    std::thread::sleep(Duration::from_millis(200));
                    self.0.read(&mut buf[..1])
                }
            }
            let (resumed, inner) = self.0.fetch(url, offset)?;
            Ok((resumed, Box::new(Slow(inner))))
        }
    }

    #[test]
    fn cancel_clears_the_queue_and_a_later_download_works() {
        let h = harness(SlowFetcher(mem()), false);
        let rx = h.features.subscribe();
        h.features.download(Feature::Meaning).unwrap();
        h.features.download(Feature::ImageVisual).unwrap();
        wait_for(
            &rx,
            Feature::Meaning,
            |i| matches!(i, Install::Downloading { bytes, .. } if *bytes > 0),
        );
        h.features.cancel_download();
        let s = wait_for(&rx, Feature::ImageVisual, |i| *i == Install::NotInstalled);
        assert_eq!(install_of(&s, Feature::Meaning), Install::NotInstalled);

        h.features.download(Feature::ImageText).unwrap(); // 3 bytes, ~0.6 s
        wait_for(&rx, Feature::ImageText, |i| {
            matches!(i, Install::Installed { .. })
        });
    }

    #[test]
    fn a_download_queued_right_after_a_cancel_still_runs() {
        let h = harness(SlowFetcher(mem()), false);
        let rx = h.features.subscribe();
        h.features.download(Feature::Meaning).unwrap();
        wait_for(
            &rx,
            Feature::Meaning,
            |i| matches!(i, Install::Downloading { bytes, .. } if *bytes > 0),
        );
        h.features.cancel_download();
        // Meaning is still mid-read (200 ms) when this is queued.
        h.features.download(Feature::ImageText).unwrap();
        wait_for(&rx, Feature::ImageText, |i| {
            matches!(i, Install::Installed { .. })
        });
    }

    #[test]
    fn remove_is_refused_while_downloading_and_deletes_only_the_download() {
        let h = harness(SlowFetcher(mem()), false);
        let rx = h.features.subscribe();
        h.features.download(Feature::ImageText).unwrap();
        wait_for(&rx, Feature::ImageText, |i| {
            matches!(i, Install::Downloading { .. })
        });
        assert!(matches!(
            h.features.remove_download(Feature::ImageText),
            Err(Error::DownloadInProgress(Feature::ImageText))
        ));
        wait_for(&rx, Feature::ImageText, |i| {
            matches!(i, Install::Installed { .. })
        });
        h.features.remove_download(Feature::ImageText).unwrap();
        assert_eq!(
            install_of(&h.features.status().unwrap(), Feature::ImageText),
            Install::NotInstalled
        );
        assert!(
            h.features.status().unwrap()[1].enabled,
            "removing a download is not disabling"
        );
    }

    #[test]
    fn set_enabled_persists_the_desire_and_leaves_the_install_alone() {
        let h = harness(mem(), false);
        h.features.set_enabled(Feature::ImageVisual, true).unwrap();
        h.features.set_enabled(Feature::Meaning, false).unwrap();
        let c = crate::config::load_from(&h.config_path).unwrap();
        assert!(c.features.image_visual && !c.features.meaning);
        let s = h.features.status().unwrap();
        assert!(!s[0].enabled && s[2].enabled);
        assert!(s.iter().all(|x| x.install == Install::NotInstalled));
    }

    #[test]
    fn a_running_backfill_shows_in_status_and_events() {
        let h = harness(mem(), false);
        let db_path = h._dir.path().join("magi.db");
        let mut conn = crate::db::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO roots (id, path, added_at) VALUES (1, '/r', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files (root_id, path, rel_path, file_name, kind, size, mtime_ns,
                                state, seen_scan_id, features_missing)
             VALUES (1, '/r/a', 'a', 'a', 'text', 1, 0, 'indexed', 0, 1)",
            [],
        )
        .unwrap();
        let rx = h.features.subscribe();
        rx.recv_timeout(Duration::from_secs(5)).unwrap(); // initial snapshot
        crate::index::requeue_missing(&mut conn, &[Feature::Meaning]).unwrap();
        let s = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("backfill change emitted");
        assert_eq!(
            s[0].backfill,
            Some(crate::dto::Backfill { done: 0, total: 1 })
        );
    }

    #[test]
    fn nothing_loads_for_a_feature_that_is_off_or_not_installed() {
        // MAGI_DATA_DIR is not a test-controlled dir here; with fakes off and
        // every feature disabled nothing is even looked up.
        let off = crate::config::FeaturesConfig {
            meaning: false,
            image_text: false,
            image_visual: false,
        };
        assert!(load_components(&off).running().is_empty());
    }
}
