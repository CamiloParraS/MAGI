//! Model manifest, download (with progress, `.partial` files, SHA-256
//! verification, atomic rename), offline import, the shared tokenizer used
//! for token-aware chunking, and [`ModelSlot`] (lazy load / idle unload)
//! (SPEC.md §7 M3).

use std::io::{Read, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// `models/manifest.toml`, embedded at compile time so a production build
/// doesn't depend on the repo layout at runtime (mirrors `db::migrations`'
/// `include_str!` of the SQL files).
const MANIFEST_TOML: &str = include_str!("../../../../models/manifest.toml");

const DOWNLOAD_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    pub slot: String,
    pub id: String,
    pub revision: String,
    pub license: String,
    /// Embedding width; 0 for slots that aren't embedders (OCR).
    #[serde(default)]
    pub dim: usize,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub query_prefix: Option<String>,
    #[serde(default)]
    pub passage_prefix: Option<String>,
    #[serde(default)]
    pub pooling: Option<String>,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelManifest {
    #[serde(rename = "model")]
    pub models: Vec<ModelEntry>,
}

impl ModelManifest {
    /// Parses the manifest embedded at compile time.
    pub fn load() -> Result<Self> {
        Self::parse(MANIFEST_TOML)
    }

    fn parse(toml_str: &str) -> Result<Self> {
        toml::from_str(toml_str).map_err(|e| Error::ManifestParse(e.to_string()))
    }

    pub fn slot(&self, slot: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.slot == slot)
    }
}

/// Parent of every slot's folder: `<data_dir>/models`.
pub fn models_root() -> PathBuf {
    crate::paths::data_dir().join("models")
}

/// Where a slot's model files live: `<data_dir>/models/<slot>/` (see
/// `paths::data_dir`'s doc comment — "database, models, thumbnails" — and
/// SPEC.md §4.5's `MAGI_DATA_DIR`).
pub fn model_dir(slot: &str) -> PathBuf {
    crate::paths::data_dir().join("models").join(slot)
}

fn sha256_of_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; DOWNLOAD_CHUNK_BYTES];
    loop {
        let n = file.read(&mut buf).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Fetches `url`'s bytes starting at byte `offset`, returning whether the
/// server actually honored a partial range (`false` means it sent the
/// whole body from byte 0 despite the `Range` request, which must not be
/// blindly appended to an existing `.partial` file).
pub trait ByteFetcher {
    fn fetch(&self, url: &str, offset: u64) -> Result<(bool, Box<dyn Read>)>;
}

pub(crate) struct UreqFetcher;

impl ByteFetcher for UreqFetcher {
    fn fetch(&self, url: &str, offset: u64) -> Result<(bool, Box<dyn Read>)> {
        let mut request = ureq::get(url);
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let response = request.call().map_err(|e| Error::Network {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
        let resumed = offset > 0 && response.status().as_u16() == 206;
        let reader = response.into_body().into_reader();
        Ok((resumed, Box::new(reader)))
    }
}

/// Downloads (or reuses an already-verified copy of) `file` into
/// `dest_dir`, honoring `cancel` between chunks. Supports resuming a
/// previous partial download via `Range`, and falls back to a full
/// restart if the server doesn't honor it. Returns the final, verified
/// path.
pub fn ensure_model_file(
    file: &ModelFile,
    dest_dir: &Path,
    cancel: &AtomicBool,
    on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    download_with_fetcher(&UreqFetcher, file, dest_dir, cancel, on_progress)
}

pub(crate) fn download_with_fetcher(
    fetcher: &dyn ByteFetcher,
    file: &ModelFile,
    dest_dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_dir).map_err(|source| Error::Io {
        path: dest_dir.to_path_buf(),
        source,
    })?;
    let final_path = dest_dir.join(&file.name);
    let partial_path = dest_dir.join(format!("{}.partial", file.name));

    if final_path.exists() && sha256_of_file(&final_path)? == file.sha256 {
        on_progress(file.size, file.size);
        return Ok(final_path);
    }

    let mut partial = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&partial_path)
        .map_err(|source| Error::Io {
            path: partial_path.clone(),
            source,
        })?;
    let mut offset = partial
        .metadata()
        .map_err(|source| Error::Io {
            path: partial_path.clone(),
            source,
        })?
        .len();

    let (resumed, mut reader) = fetcher.fetch(&file.url, offset)?;
    if offset > 0 && !resumed {
        // The server ignored our Range request and is about to send the
        // whole body again from byte 0 — appending it would duplicate the
        // prefix, so restart the partial file from scratch.
        drop(partial);
        partial = std::fs::File::create(&partial_path).map_err(|source| Error::Io {
            path: partial_path.clone(),
            source,
        })?;
        offset = 0;
    }

    let mut buf = [0u8; DOWNLOAD_CHUNK_BYTES];
    loop {
        if cancel.load(Ordering::Relaxed) {
            // Partial data is kept for a later resume.
            return Err(Error::DownloadCancelled);
        }
        let n = reader.read(&mut buf).map_err(|e| Error::Network {
            url: file.url.clone(),
            reason: e.to_string(),
        })?;
        if n == 0 {
            break;
        }
        partial.write_all(&buf[..n]).map_err(|source| Error::Io {
            path: partial_path.clone(),
            source,
        })?;
        offset += n as u64;
        on_progress(offset, file.size);
    }
    drop(partial);

    let actual = sha256_of_file(&partial_path)?;
    if actual != file.sha256 {
        std::fs::remove_file(&partial_path).ok();
        tracing::warn!(file = %file.name, expected = %file.sha256, %actual, "checksum mismatch");
        return Err(Error::ChecksumMismatch {
            file: file.name.clone(),
        });
    }
    std::fs::rename(&partial_path, &final_path).map_err(|source| Error::Io {
        path: final_path.clone(),
        source,
    })?;
    Ok(final_path)
}

/// Imports `file` from a local folder (no network) instead of downloading
/// it, verifying the same SHA-256 as the download path.
pub fn import_offline_model_file(
    file: &ModelFile,
    source_dir: &Path,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let source_path = source_dir.join(&file.name);
    let actual = sha256_of_file(&source_path)?;
    if actual != file.sha256 {
        return Err(Error::Model(format!(
            "offline import of {}: checksum mismatch (expected {}, got {actual})",
            file.name, file.sha256
        )));
    }
    std::fs::create_dir_all(dest_dir).map_err(|source| Error::Io {
        path: dest_dir.to_path_buf(),
        source,
    })?;
    let dest_path = dest_dir.join(&file.name);
    std::fs::copy(&source_path, &dest_path).map_err(|source| Error::Io {
        path: dest_path.clone(),
        source,
    })?;
    Ok(dest_path)
}

/// e5's pad token — its ONNX `config.json` reports a different, wrong
/// `pad_token_id` for this model (verified by inspecting both files
/// directly), so the id is always looked up from the tokenizer itself.
const PAD_TOKEN: &str = "<pad>";

/// Loads and fully configures the e5 tokenizer once (truncation at
/// [`crate::chunk::MAX_TOKENS`], `BatchLongest` padding), so both
/// `chunk::count_tokens` and `E5Embedder` can share the same in-memory
/// instance instead of each parsing their own copy of the ~17 MB
/// `tokenizer.json` — measured via ADR-0005's RSS investigation to cost
/// ~275 MB per copy, entirely avoidable duplication. Safe for
/// `count_tokens`' single-word, no-special-tokens calls: padding to a
/// batch of one is a no-op, and a lone word never approaches the 512-token
/// truncation ceiling.
fn load_tokenizer_from(path: &Path) -> Option<tokenizers::Tokenizer> {
    use tokenizers::{
        PaddingDirection, PaddingParams, PaddingStrategy, TruncationDirection, TruncationParams,
        TruncationStrategy,
    };

    let mut tokenizer = tokenizers::Tokenizer::from_file(path).ok()?;
    let pad_id = tokenizer.token_to_id(PAD_TOKEN)?;
    tokenizer
        .with_truncation(Some(TruncationParams {
            direction: TruncationDirection::Right,
            max_length: crate::chunk::MAX_TOKENS,
            strategy: TruncationStrategy::LongestFirst,
            stride: 0,
        }))
        .ok()?;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        direction: PaddingDirection::Right,
        pad_to_multiple_of: None,
        pad_id,
        pad_type_id: 0,
        pad_token: PAD_TOKEN.to_string(),
    }));
    Some(tokenizer)
}

static TEXT_TOKENIZER: ModelSlot<Arc<tokenizers::Tokenizer>> = ModelSlot::new();

/// The tokenizer in `slot`, loading it from `path` first if needed. The `Arc`
/// is cloned out so callers don't hold the slot's lock while encoding.
fn tokenizer_in(
    slot: &ModelSlot<Arc<tokenizers::Tokenizer>>,
    path: &Path,
) -> Option<Arc<tokenizers::Tokenizer>> {
    let tokenizer = slot.get_or_load(|| {
        load_tokenizer_from(path)
            .map(Arc::new)
            .ok_or_else(|| Error::Model(format!("no usable tokenizer at {}", path.display())))
    });
    tokenizer.ok().map(|t| Arc::clone(&t))
}

/// The downloaded, fully-configured e5 tokenizer, once `tokenizer.json` has
/// been placed in the text model's directory (by `ensure_model_file` or
/// offline import). `None` before then — callers (`chunk::count_tokens`)
/// fall back to an approximation. Shared process-wide: `E5Embedder` uses
/// this same instance rather than loading its own (see `load_tokenizer_from`'s
/// doc comment). Freed like a model by [`unload_text_tokenizer_if_idle`].
pub fn shared_text_tokenizer() -> Option<Arc<tokenizers::Tokenizer>> {
    tokenizer_in(&TEXT_TOKENIZER, &model_dir("text").join("tokenizer.json"))
}

/// Drops the shared tokenizer if unused for `idle` (see
/// [`ModelSlot::unload_if_idle`]); callers still holding it keep it alive.
pub fn unload_text_tokenizer_if_idle(idle: Duration) {
    TEXT_TOKENIZER.unload_if_idle(idle);
}

/// Holds a model behind lazy load / idle unload (SPEC.md §7 M3: "loads
/// models lazily and unloads them after the idle timeout"). This type is
/// passive — nothing here polls on its own. The owning loop calls
/// `unload_if_idle` on its own timer tick (the embed worker's
/// `recv_timeout` loop, once the persistent engine that owns one exists —
/// SPEC.md §5.3/§7 M5/M7); it only holds the state and the two operations
/// that loop needs.
pub struct ModelSlot<T> {
    inner: Mutex<Option<(T, Instant)>>,
}

impl<T> Default for ModelSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ModelSlot<T> {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.inner.lock().is_ok_and(|g| g.is_some())
    }

    /// Returns the loaded model, loading it via `load` first if it isn't
    /// already, and marks this as the most recent use. A failed `load`
    /// leaves the slot empty (not a stale/poisoned entry).
    pub fn get_or_load(&self, load: impl FnOnce() -> Result<T>) -> Result<ModelGuard<'_, T>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| Error::Model("model slot lock poisoned".to_string()))?;
        Self::use_or_load(guard, load)
    }

    /// Like [`get_or_load`](Self::get_or_load), but waits at most `wait` for
    /// another caller to finish with the model, then fails.
    pub fn get_or_load_within(
        &self,
        wait: Duration,
        load: impl FnOnce() -> Result<T>,
    ) -> Result<ModelGuard<'_, T>> {
        let deadline = Instant::now() + wait;
        loop {
            match self.inner.try_lock() {
                Ok(guard) => return Self::use_or_load(guard, load),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    // ponytail: polling, a Condvar if waits ever get long or many.
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return Err(Error::Model("model busy or its lock poisoned".into())),
            }
        }
    }

    fn use_or_load(
        mut guard: MutexGuard<'_, Option<(T, Instant)>>,
        load: impl FnOnce() -> Result<T>,
    ) -> Result<ModelGuard<'_, T>> {
        match guard.as_mut() {
            Some((_, last_used)) => *last_used = Instant::now(),
            None => *guard = Some((load()?, Instant::now())),
        }
        Ok(ModelGuard { guard })
    }

    /// Drops the model if it's been idle at least `idle_timeout`. Returns
    /// whether it actually unloaded something. A model in use is not idle, so
    /// this never waits for it.
    pub fn unload_if_idle(&self, idle_timeout: Duration) -> bool {
        let Ok(mut guard) = self.inner.try_lock() else {
            return false;
        };
        let is_idle = guard
            .as_ref()
            .is_some_and(|(_, last_used)| last_used.elapsed() >= idle_timeout);
        if is_idle {
            *guard = None;
        }
        is_idle
    }
}

/// Borrowed access to a [`ModelSlot`]'s currently-loaded value.
pub struct ModelGuard<'a, T> {
    guard: MutexGuard<'a, Option<(T, Instant)>>,
}

impl<T> Deref for ModelGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self
            .guard
            .as_ref()
            .expect("ModelGuard always wraps a loaded value")
            .0
    }
}

#[cfg(test)]
mod tests;
