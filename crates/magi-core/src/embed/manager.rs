//! Model manifest, download (with progress, `.partial` files, SHA-256
//! verification, atomic rename), offline import, the shared tokenizer used
//! for token-aware chunking, and [`ModelSlot`] (lazy load / idle unload)
//! (SPEC.md §7 M3).

use std::io::{Read, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
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

struct UreqFetcher;

impl ByteFetcher for UreqFetcher {
    fn fetch(&self, url: &str, offset: u64) -> Result<(bool, Box<dyn Read>)> {
        let mut request = ureq::get(url);
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let response = request
            .call()
            .map_err(|e| Error::Model(format!("GET {url}: {e}")))?;
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

fn download_with_fetcher(
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
            return Err(Error::Model(format!(
                "download of {} cancelled (partial data kept for resume)",
                file.name
            )));
        }
        let n = reader
            .read(&mut buf)
            .map_err(|e| Error::Model(format!("reading {} response body: {e}", file.url)))?;
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
        return Err(Error::Model(format!(
            "checksum mismatch for {}: expected {}, got {actual} (deleted; retry to re-download)",
            file.name, file.sha256
        )));
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

static TEXT_TOKENIZER: OnceLock<Option<tokenizers::Tokenizer>> = OnceLock::new();

/// The downloaded, fully-configured e5 tokenizer, once `tokenizer.json` has
/// been placed in the text model's directory (by `ensure_model_file` or
/// offline import). `None` before then — callers (`chunk::count_tokens`)
/// fall back to an approximation. Cached process-wide on first use, like
/// `extract::pdf`'s `shared_pdfium`; `E5Embedder` uses this same instance
/// rather than loading its own (see `load_tokenizer_from`'s doc comment).
pub fn shared_text_tokenizer() -> Option<&'static tokenizers::Tokenizer> {
    TEXT_TOKENIZER
        .get_or_init(|| load_tokenizer_from(&model_dir("text").join("tokenizer.json")))
        .as_ref()
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
        Self {
            inner: Mutex::new(None),
        }
    }
}

impl<T> ModelSlot<T> {
    pub fn new() -> Self {
        Self::default()
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
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn manifest_parses_and_has_a_text_entry_with_real_looking_hashes() {
        let manifest = ModelManifest::load().unwrap();
        let text = manifest.slot("text").expect("text slot present");
        assert_eq!(text.id, "intfloat/multilingual-e5-small");
        assert_eq!(text.dim, 384);
        assert_eq!(text.max_tokens, Some(512));
        assert_eq!(text.query_prefix.as_deref(), Some("query: "));
        assert_eq!(text.passage_prefix.as_deref(), Some("passage: "));
        assert!(!text.revision.is_empty());
        assert!(!text.files.is_empty());
        for file in &text.files {
            assert_eq!(file.sha256.len(), 64, "not a placeholder: {}", file.name);
            assert!(file.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(file.url.starts_with("https://"));
            assert!(file.size > 0);
        }
    }

    #[test]
    fn missing_slot_is_none() {
        let manifest = ModelManifest::load().unwrap();
        assert!(manifest.slot("nonexistent").is_none());
    }

    #[test]
    fn malformed_manifest_is_a_typed_error_not_a_panic() {
        let err = ModelManifest::parse("not valid toml [[[").unwrap_err();
        assert!(matches!(err, Error::ManifestParse(_)));
    }

    fn test_file(content: &[u8]) -> ModelFile {
        let mut hasher = Sha256::new();
        hasher.update(content);
        let sha256 = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        ModelFile {
            name: "weights.bin".to_string(),
            url: "https://example.invalid/weights.bin".to_string(),
            sha256,
            size: content.len() as u64,
        }
    }

    /// An in-memory `ByteFetcher` for deterministic, offline download
    /// tests. `resumed` controls whether it honors the requested offset
    /// (simulating a server that supports/ignores `Range`).
    struct FakeFetcher {
        full_content: Vec<u8>,
        resumed: bool,
        calls: Mutex<u32>,
    }

    impl ByteFetcher for FakeFetcher {
        fn fetch(&self, _url: &str, offset: u64) -> Result<(bool, Box<dyn Read>)> {
            *self.calls.lock().unwrap() += 1;
            let bytes = if self.resumed {
                self.full_content[offset as usize..].to_vec()
            } else {
                self.full_content.clone()
            };
            Ok((
                self.resumed && offset > 0,
                Box::new(std::io::Cursor::new(bytes)),
            ))
        }
    }

    #[test]
    fn fresh_download_writes_verified_file_and_removes_partial() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"hello model weights".to_vec();
        let file = test_file(&content);
        let fetcher = FakeFetcher {
            full_content: content.clone(),
            resumed: true,
            calls: Mutex::new(0),
        };
        let cancel = AtomicBool::new(false);

        let mut progress_calls = Vec::new();
        let path = download_with_fetcher(&fetcher, &file, dir.path(), &cancel, |done, total| {
            progress_calls.push((done, total));
        })
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), content);
        assert!(!dir.path().join("weights.bin.partial").exists());
        assert_eq!(
            progress_calls.last(),
            Some(&(content.len() as u64, file.size))
        );
    }

    #[test]
    fn already_verified_file_is_not_redownloaded() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"already here".to_vec();
        let file = test_file(&content);
        std::fs::write(dir.path().join(&file.name), &content).unwrap();

        let fetcher = FakeFetcher {
            full_content: b"WOULD BE WRONG IF FETCHED".to_vec(),
            resumed: true,
            calls: Mutex::new(0),
        };
        let cancel = AtomicBool::new(false);

        let path = download_with_fetcher(&fetcher, &file, dir.path(), &cancel, |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), content);
        assert_eq!(*fetcher.calls.lock().unwrap(), 0, "fetch must be skipped");
    }

    #[test]
    fn resumes_from_existing_partial_file_via_range() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"0123456789abcdefghij".to_vec();
        let file = test_file(&content);
        std::fs::write(dir.path().join("weights.bin.partial"), &content[..10]).unwrap();

        let fetcher = FakeFetcher {
            full_content: content.clone(),
            resumed: true,
            calls: Mutex::new(0),
        };
        let cancel = AtomicBool::new(false);

        let path = download_with_fetcher(&fetcher, &file, dir.path(), &cancel, |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), content);
    }

    #[test]
    fn restarts_when_server_ignores_range() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"0123456789abcdefghij".to_vec();
        let file = test_file(&content);
        // Garbage partial content that would corrupt the result if blindly
        // appended to.
        std::fs::write(dir.path().join("weights.bin.partial"), b"XXXXXXXXXX").unwrap();

        let fetcher = FakeFetcher {
            full_content: content.clone(),
            resumed: false, // server sends the whole body regardless of Range
            calls: Mutex::new(0),
        };
        let cancel = AtomicBool::new(false);

        let path = download_with_fetcher(&fetcher, &file, dir.path(), &cancel, |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), content);
    }

    #[test]
    fn corrupted_download_is_rejected_and_can_be_retried() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"the real content".to_vec();
        let file = test_file(&content);

        let bad_fetcher = FakeFetcher {
            full_content: b"the WRONG content".to_vec(),
            resumed: true,
            calls: Mutex::new(0),
        };
        let cancel = AtomicBool::new(false);
        let err =
            download_with_fetcher(&bad_fetcher, &file, dir.path(), &cancel, |_, _| {}).unwrap_err();
        assert!(matches!(err, Error::Model(_)));
        assert!(!dir.path().join("weights.bin.partial").exists());
        assert!(!dir.path().join("weights.bin").exists());

        let good_fetcher = FakeFetcher {
            full_content: content.clone(),
            resumed: true,
            calls: Mutex::new(0),
        };
        let path =
            download_with_fetcher(&good_fetcher, &file, dir.path(), &cancel, |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), content);
    }

    #[test]
    fn cancel_flag_stops_download_leaving_a_resumable_partial() {
        let dir = tempfile::tempdir().unwrap();
        let content = vec![7u8; DOWNLOAD_CHUNK_BYTES * 3];
        let file = test_file(&content);
        let fetcher = FakeFetcher {
            full_content: content.clone(),
            resumed: true,
            calls: Mutex::new(0),
        };
        let cancel = AtomicBool::new(true);

        let err =
            download_with_fetcher(&fetcher, &file, dir.path(), &cancel, |_, _| {}).unwrap_err();
        assert!(matches!(err, Error::Model(_)));
        let partial_len = std::fs::metadata(dir.path().join("weights.bin.partial"))
            .unwrap()
            .len();
        assert!(partial_len < content.len() as u64);
    }

    #[test]
    fn offline_import_copies_and_verifies_checksum() {
        let source_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        let content = b"offline weights".to_vec();
        let file = test_file(&content);
        std::fs::write(source_dir.path().join(&file.name), &content).unwrap();

        let path = import_offline_model_file(&file, source_dir.path(), dest_dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), content);
    }

    #[test]
    fn offline_import_rejects_checksum_mismatch() {
        let source_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        let file = test_file(b"expected content");
        std::fs::write(source_dir.path().join(&file.name), b"tampered content").unwrap();

        let err = import_offline_model_file(&file, source_dir.path(), dest_dir.path()).unwrap_err();
        assert!(matches!(err, Error::Model(_)));
        assert!(!dest_dir.path().join(&file.name).exists());
    }

    #[test]
    fn nonexistent_tokenizer_path_loads_as_none() {
        assert!(load_tokenizer_from(Path::new("/does/not/exist/tokenizer.json")).is_none());
    }

    #[test]
    fn invalid_tokenizer_file_loads_as_none_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokenizer.json");
        std::fs::write(&path, b"not a real tokenizer").unwrap();
        assert!(load_tokenizer_from(&path).is_none());
    }

    #[test]
    #[ignore = "requires the real tokenizer.json from `just models` in MAGI_DATA_DIR"]
    fn real_tokenizer_file_loads_and_encodes_plausible_counts() {
        let path = model_dir("text").join("tokenizer.json");
        let tokenizer =
            load_tokenizer_from(&path).expect("run `just models` first, or set MAGI_DATA_DIR");
        let count = tokenizer.encode("hello world", false).unwrap().len();
        assert!((1..=6).contains(&count), "got {count} tokens");
    }

    #[test]
    fn model_slot_loads_once_and_reuses_without_reloading() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        let load_calls = Mutex::new(0u32);
        let load = || {
            *load_calls.lock().unwrap() += 1;
            Ok(42)
        };

        assert!(!slot.is_loaded());
        assert_eq!(*slot.get_or_load(load).unwrap(), 42);
        assert!(slot.is_loaded());
        assert_eq!(*slot.get_or_load(load).unwrap(), 42);
        assert_eq!(
            *load_calls.lock().unwrap(),
            1,
            "second get_or_load must reuse the loaded value, not reload"
        );
    }

    #[test]
    fn model_slot_get_or_load_propagates_loader_error_without_leaving_a_stale_entry() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        let err = slot
            .get_or_load(|| Err(Error::Model("boom".to_string())))
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, Error::Model(_)));
        assert!(!slot.is_loaded());
    }

    #[test]
    fn model_slot_unload_if_idle_only_unloads_past_the_timeout() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        slot.get_or_load(|| Ok(7)).unwrap();

        assert!(
            !slot.unload_if_idle(Duration::from_secs(3600)),
            "just-used model isn't idle yet"
        );
        assert!(slot.is_loaded());

        assert!(
            slot.unload_if_idle(Duration::from_secs(0)),
            "any elapsed time clears a 0s idle timeout"
        );
        assert!(!slot.is_loaded());
    }

    #[test]
    fn a_model_in_use_is_neither_unloaded_nor_lent_twice() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        let in_use = slot.get_or_load(|| Ok(7)).unwrap();
        assert!(!slot.unload_if_idle(Duration::ZERO), "must not wait for it");
        assert!(
            slot.get_or_load_within(Duration::ZERO, || Ok(8)).is_err(),
            "still busy when the wait is up"
        );
        drop(in_use);
        assert_eq!(
            *slot.get_or_load_within(Duration::ZERO, || Ok(8)).unwrap(),
            7
        );
    }

    #[test]
    fn a_bounded_wait_gets_the_model_once_the_other_user_is_done() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        let (taken_tx, taken) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            s.spawn(|| {
                let _in_use = slot.get_or_load(|| Ok(7)).unwrap();
                taken_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(100));
            });
            taken.recv().unwrap();
            let got = slot.get_or_load_within(Duration::from_secs(10), || Ok(8));
            assert_eq!(*got.unwrap(), 7);
        });
    }

    #[test]
    fn model_slot_reloads_after_being_unloaded() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        let load_calls = Mutex::new(0u32);
        let load = || {
            *load_calls.lock().unwrap() += 1;
            Ok(1)
        };

        slot.get_or_load(load).unwrap();
        slot.unload_if_idle(Duration::from_secs(0));
        slot.get_or_load(load).unwrap();

        assert_eq!(*load_calls.lock().unwrap(), 2);
    }

    #[test]
    fn model_slot_get_or_load_refreshes_last_used_so_it_is_not_idle() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        slot.get_or_load(|| Ok(1)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        // A fresh use just before the check must reset the idle clock.
        slot.get_or_load(|| Ok(1)).unwrap();
        assert!(!slot.unload_if_idle(Duration::from_millis(15)));
        assert!(slot.is_loaded());
    }
}
