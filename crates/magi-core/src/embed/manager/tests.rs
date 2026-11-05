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
    assert!(matches!(err, Error::ChecksumMismatch { .. }));
    assert!(!dir.path().join("weights.bin.partial").exists());
    assert!(!dir.path().join("weights.bin").exists());

    let good_fetcher = FakeFetcher {
        full_content: content.clone(),
        resumed: true,
        calls: Mutex::new(0),
    };
    let path = download_with_fetcher(&good_fetcher, &file, dir.path(), &cancel, |_, _| {}).unwrap();
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

    let err = download_with_fetcher(&fetcher, &file, dir.path(), &cancel, |_, _| {}).unwrap_err();
    assert!(matches!(err, Error::DownloadCancelled));
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
fn tokenizer_unloads_when_idle_and_loads_once_it_appears() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokenizer.json");
    let slot = ModelSlot::new();

    assert!(tokenizer_in(&slot, &path).is_none(), "not downloaded yet");
    std::fs::write(
        &path,
        r#"{"version":"1.0","truncation":null,"padding":null,"added_tokens":[],
            "normalizer":null,"pre_tokenizer":{"type":"Whitespace"},"post_processor":null,
            "decoder":null,"model":{"type":"WordLevel",
            "vocab":{"<pad>":0,"[UNK]":1,"hello":2},"unk_token":"[UNK]"}}"#,
    )
    .unwrap();
    let in_use = tokenizer_in(&slot, &path).expect("picked up after download");
    assert!(slot.is_loaded());

    assert!(slot.unload_if_idle(Duration::ZERO));
    assert!(!slot.is_loaded());
    assert_eq!(
        in_use.encode("hello", false).unwrap().len(),
        1,
        "still usable"
    );
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
