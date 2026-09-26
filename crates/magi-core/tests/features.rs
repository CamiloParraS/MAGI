//! ADR-0010: the engine with any subset of search features, against the real
//! engine threads and the fake components.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use magi_core::config::Config;
use magi_core::db::{self, roots};
use magi_core::embed::{CountingEmbedder, FakeEmbedder};
use magi_core::features::Components;
use magi_core::ocr::OcrEngine;
use magi_core::search::{fts::search_fts, hybrid_search};
use magi_core::{Engine, EngineHandle};
use rusqlite::Connection;

fn isolate_data_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("magi-features-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: every test calls this first; `Once` finishes before any proceeds.
        unsafe { std::env::set_var("MAGI_DATA_DIR", dir) };
    });
}

fn wait_until(deadline: Duration, what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Reads the same text out of every image.
struct FakeOcr;

impl OcrEngine for FakeOcr {
    fn engine_id(&self) -> &str {
        "fake-ocr"
    }

    fn recognize(&self, _image: &image::RgbImage) -> magi_core::Result<String> {
        Ok("invoice forty two".into())
    }
}

struct Env {
    _db_dir: tempfile::TempDir,
    root_dir: tempfile::TempDir,
    db_path: PathBuf,
    embedder: Arc<CountingEmbedder<FakeEmbedder>>,
}

impl Env {
    fn new() -> Self {
        isolate_data_dir();
        let db_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("magi.db");
        let root_dir = tempfile::tempdir().unwrap();
        roots::add(&db::open(&db_path).unwrap(), root_dir.path()).unwrap();
        Self {
            _db_dir: db_dir,
            root_dir,
            db_path,
            embedder: Arc::new(CountingEmbedder::new(FakeEmbedder)),
        }
    }

    fn root(&self) -> &Path {
        self.root_dir.path()
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.root().join(name), text).unwrap();
    }

    fn write_png(&self, name: &str) {
        image::RgbImage::from_pixel(64, 64, image::Rgb([200, 30, 30]))
            .save(self.root().join(name))
            .unwrap();
    }

    fn start(&self, components: Components) -> EngineHandle {
        let mut config = Config::default();
        config.indexing.worker_threads = 2;
        config.indexing.pause_on_battery = false;
        Engine::start(&config, &self.db_path, components).unwrap()
    }

    fn text(&self) -> Components {
        Components {
            text: Some(self.embedder.clone()),
            ..Components::default()
        }
    }

    fn conn(&self) -> Connection {
        db::open(&self.db_path).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn wait_drained(&self, files: i64) {
        wait_until(Duration::from_secs(120), "the queue to drain", || {
            self.count("SELECT COUNT(*) FROM files WHERE state IN ('pending', 'indexing')") == 0
                && self.count("SELECT COUNT(*) FROM files") == files
        });
    }
}

#[test]
fn with_no_features_files_are_found_by_keyword_and_name() {
    let env = Env::new();
    env.write("notes.txt", "quarterly budget review");
    let _engine = env.start(Components::default());
    env.wait_drained(1);
    assert_eq!(env.count("SELECT COUNT(*) FROM vec_text"), 0);
    assert_eq!(search_fts(&env.conn(), "budget", 10).unwrap().len(), 1);
    assert_eq!(
        search_fts(&env.conn(), "notes", 10).unwrap().len(),
        1,
        "found by name"
    );
    assert_eq!(
        hybrid_search(&env.conn(), None, None, "budget", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn turning_ocr_off_keeps_the_text_already_read_and_re_reads_nothing() {
    let env = Env::new();
    env.write_png("scan.png");
    let with_ocr = Components {
        ocr: Some(Arc::new(FakeOcr)),
        ..env.text()
    };
    let engine = env.start(with_ocr);
    env.wait_drained(1);
    engine.shutdown();
    assert_eq!(
        env.count("SELECT COUNT(*) FROM chunks WHERE source = 'ocr'"),
        1
    );
    let embedded = env.embedder.chunks();

    let _engine = env.start(env.text());
    env.wait_drained(1);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM chunks WHERE source = 'ocr'"),
        1,
        "OCR text kept"
    );
    assert_eq!(env.embedder.chunks(), embedded, "nothing re-indexed");
    assert_eq!(search_fts(&env.conn(), "invoice", 10).unwrap().len(), 1);
}
