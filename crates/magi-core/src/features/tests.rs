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
    let err =
        download_entry(&wrong, root.path(), &e, &AtomicBool::new(false), |_, _| {}).unwrap_err();
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
    let err =
        download_entry(&fetcher, root.path(), &e, &AtomicBool::new(true), |_, _| {}).unwrap_err();
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
