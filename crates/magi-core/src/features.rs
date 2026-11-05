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
mod tests;
