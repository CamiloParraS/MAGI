//! SPEC.md §7 M5 verification, against the real engine (threads, channels,
//! writer) with the fake embedder. Items that need a file-system watcher come
//! with Slice 4. Every wait polls a condition against a deadline; nothing sleeps
//! for a fixed time and hopes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use magi_core::config::Config;
use magi_core::db::{self, files, roots};
use magi_core::embed::{CountingEmbedder, FakeEmbedder};
use magi_core::ocr::NoOcr;
use magi_core::search::fts::search_fts;
use magi_core::{Engine, EngineHandle};
use rusqlite::Connection;

fn isolate_data_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("magi-incremental-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: every test calls this before touching the environment, and
        // `Once` finishes it before any of them proceeds.
        unsafe { std::env::set_var("MAGI_DATA_DIR", dir) };
    });
}

/// Polls `cond` until it holds, panicking with `what` at the deadline.
fn wait_until(deadline: Duration, what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(50));
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
        let conn = db::open(&db_path).unwrap();
        roots::add(&conn, root_dir.path()).unwrap();
        Self {
            _db_dir: db_dir,
            root_dir,
            db_path,
            embedder: Arc::new(FakeEmbedder::counting()),
        }
    }

    fn root(&self) -> &Path {
        self.root_dir.path()
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.root().join(name), text).unwrap();
    }

    fn start(&self) -> EngineHandle {
        let mut config = Config::default();
        config.indexing.worker_threads = 2;
        Engine::start(
            &config,
            &self.db_path,
            self.embedder.clone(),
            None,
            Arc::new(NoOcr),
        )
        .unwrap()
    }

    fn conn(&self) -> Connection {
        db::open(&self.db_path).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Nothing left in the queue.
    fn drained(&self) -> bool {
        self.count("SELECT COUNT(*) FROM files WHERE state IN ('pending', 'indexing')") == 0
    }

    fn wait_drained(&self, files: i64) {
        wait_until(Duration::from_secs(120), "the queue to drain", || {
            self.drained() && self.count("SELECT COUNT(*) FROM files") == files
        });
    }

    fn hits(&self, query: &str) -> usize {
        search_fts(&self.conn(), query, 10).unwrap().len()
    }

    fn integrity_ok(&self) {
        let result: String = self
            .conn()
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(result, "ok");
    }
}

/// Item 9: a burst of 1,000 files is indexed exactly once each: no duplicate
/// rows, and every chunk was embedded once (none twice).
#[test]
fn a_burst_of_1000_files_is_indexed_exactly_once() {
    let env = Env::new();
    for i in 0..1000 {
        env.write(
            &format!("note_{i:04}.txt"),
            &format!("burst file number {i}"),
        );
    }
    let engine = env.start();

    env.wait_drained(1000);

    assert_eq!(env.count("SELECT COUNT(DISTINCT path) FROM files"), 1000);
    assert_eq!(engine.stats().indexed, 1000);
    assert_eq!(
        env.embedder.chunks() as i64,
        env.count("SELECT COUNT(*) FROM chunks"),
        "each chunk embedded exactly once"
    );
    assert_eq!(
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'"),
        1000
    );

    // A second run over the same folder finds nothing to do.
    engine.shutdown();
    let embedded = env.embedder.chunks();
    let engine = env.start();
    engine.rescan().recv().unwrap().unwrap();
    env.wait_drained(1000);
    assert_eq!(env.embedder.chunks(), embedded, "nothing embedded again");
    assert_eq!(env.count("SELECT COUNT(*) FROM files"), 1000);
}

/// Item 10: a file that is still being written is left alone, then indexed
/// once with its final content.
#[test]
fn a_file_written_slowly_is_indexed_once_with_its_final_content() {
    let env = Env::new();
    let engine = env.start();
    let path = env.root().join("slow.txt");

    let writer = {
        let path = path.clone();
        std::thread::spawn(move || {
            use std::io::Write;
            let mut file = std::fs::File::create(&path).unwrap();
            for i in 0..10 {
                writeln!(file, "partial line {i}").unwrap();
                file.flush().unwrap();
                std::thread::sleep(Duration::from_millis(500));
            }
            writeln!(file, "finalmarker").unwrap();
        })
    };
    // The file is seen (and queued) while it is still growing.
    wait_until(Duration::from_secs(10), "the file to appear", || {
        path.exists()
    });
    while !writer.is_finished() {
        engine.rescan().recv().unwrap().unwrap();
        assert_eq!(
            env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'"),
            0,
            "not indexed while it is still being written"
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    writer.join().unwrap();
    engine.rescan().recv().unwrap().unwrap();

    wait_until(Duration::from_secs(60), "slow.txt to be indexed", || {
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'") == 1
    });
    assert_eq!(env.hits("finalmarker"), 1, "the final content is indexed");
    assert_eq!(engine.stats().indexed, 1, "once");
    assert_eq!(
        env.embedder.chunks() as i64,
        env.count("SELECT COUNT(*) FROM chunks"),
        "no partial version was ever embedded"
    );
}

/// Item 11, in process: rows left `indexing` (the state a killed process
/// leaves behind) are reset and completed on the next start, and the database
/// is intact. The real process kill needs `magi-cli daemon` (Slice 7).
#[test]
fn rows_left_indexing_are_reset_and_completed_on_restart() {
    let env = Env::new();
    for i in 0..30 {
        env.write(&format!("crash_{i}.txt"), &format!("crash text {i}"));
    }
    // What a killed engine leaves: rows the pipeline had taken, never finished.
    let scan = {
        let mut conn = env.conn();
        magi_core::watch::reconcile::reconcile_all(
            &mut conn,
            &magi_core::index::pipeline::IndexRootOptions::from_config(&Config::default().indexing)
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(scan.inserted, 30);
    env.conn()
        .execute("UPDATE files SET state = 'indexing' WHERE id % 2 = 0", [])
        .unwrap();
    assert!(env.count("SELECT COUNT(*) FROM files WHERE state = 'indexing'") > 0);

    let _engine = env.start();

    env.wait_drained(30);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'"),
        30
    );
    assert_eq!(env.hits("crash"), 10, "search returns its limit of 10");
    env.integrity_ok();
}

/// Stopping in the middle of a large queue leaves a consistent database, and
/// the next start finishes the job.
#[test]
fn shutting_down_mid_queue_then_restarting_completes_the_index() {
    let env = Env::new();
    for i in 0..300 {
        env.write(&format!("mid_{i:03}.txt"), &format!("interrupted text {i}"));
    }
    let engine = env.start();
    wait_until(Duration::from_secs(60), "some files to be indexed", || {
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'") > 0
    });
    engine.shutdown();
    env.integrity_ok();

    let _engine = env.start();
    env.wait_drained(300);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'"),
        300
    );
    assert_eq!(
        env.embedder.chunks() as i64,
        env.count("SELECT COUNT(*) FROM chunks"),
        "no file was embedded twice across the restart"
    );
    env.integrity_ok();
}

/// Item 14: a file that cannot be extracted becomes `error`; the others carry
/// on. (The permission-denied half needs Slice 5's platform work.)
#[test]
fn an_unreadable_file_becomes_error_and_the_others_continue() {
    let env = Env::new();
    let truncated =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/edge/truncated.pdf");
    std::fs::copy(truncated, env.root().join("a_broken.pdf")).unwrap();
    env.write("z_fine.txt", "still gets indexed");
    let _engine = env.start();

    env.wait_drained(2);

    let state = |name: &str| -> String {
        let path = magi_core::paths::canonicalize(env.root())
            .unwrap()
            .join(name);
        files::get_by_path(&env.conn(), &path)
            .unwrap()
            .unwrap()
            .state
    };
    assert_eq!(state("a_broken.pdf"), "error");
    assert_eq!(state("z_fine.txt"), "indexed");
    assert_eq!(env.hits("gets"), 1);
}

/// A file that disappears while queued is dropped, not stuck.
#[test]
fn a_file_deleted_before_it_is_processed_leaves_no_row() {
    let env = Env::new();
    env.write("keep.txt", "keeper");
    env.write("gone.txt", "short lived");
    let engine = env.start();
    std::fs::remove_file(env.root().join("gone.txt")).unwrap();
    engine.rescan().recv().unwrap().unwrap();

    env.wait_drained(1);

    assert_eq!(env.hits("short"), 0);
    assert_eq!(env.hits("keeper"), 1);
}
