//! SPEC.md §7 M5 verification, against the real engine (threads, channels,
//! writer) with the fake embedder. Every wait polls a condition against a
//! deadline; nothing sleeps for a fixed time and hopes.

mod control;
mod platform;
mod startup;
mod watch;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use magi_core::config::Config;
use magi_core::db::{self, files, roots};
use magi_core::dto::{IndexState, PauseReason};
use magi_core::embed::{CountingEmbedder, FakeEmbedder, FakeImageEmbedder, ImageEmbedder};
use magi_core::features::Components;
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
            embedder: Arc::new(CountingEmbedder::new(FakeEmbedder)),
        }
    }

    fn root(&self) -> &Path {
        self.root_dir.path()
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.root().join(name), text).unwrap();
    }

    fn start(&self) -> EngineHandle {
        self.start_with(None)
    }

    fn start_with(&self, image: Option<Arc<dyn ImageEmbedder>>) -> EngineHandle {
        let mut config = Config::default();
        config.indexing.worker_threads = 2;
        // A laptop running the tests on battery must not pause them.
        config.indexing.pause_on_battery = false;
        Engine::start(
            &config,
            &self.db_path,
            Components {
                text: Some(self.embedder.clone()),
                image,
                ocr: None,
            },
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
            // One query: two could see a file get inserted, still pending, between them.
            self.count(
                "SELECT COUNT(*) FROM files WHERE NOT EXISTS
                   (SELECT 1 FROM files WHERE state IN ('pending', 'indexing'))",
            ) == files
        });
    }

    fn hits(&self, query: &str) -> usize {
        search_fts(&self.conn(), query, 10).unwrap().len()
    }

    /// Makes files waiting out a retry backoff due now, so a test does not
    /// sit through 30 s to 10 min delays. The engine's own logic still decides
    /// what happens on the retry.
    fn skip_backoff(&self) {
        self.conn()
            .execute(
                "UPDATE files SET next_attempt_at = 0
                 WHERE state = 'pending' AND next_attempt_at > 0",
                [],
            )
            .unwrap();
    }

    fn state_of(&self, name: &str) -> String {
        // `Engine::start` returns before the first scan has listed the file.
        wait_until(Duration::from_secs(30), "the file to be listed", || {
            self.count(&format!(
                "SELECT COUNT(*) FROM files WHERE file_name = '{name}'"
            )) > 0
        });
        self.conn()
            .query_row(
                "SELECT state FROM files WHERE file_name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn integrity_ok(&self) {
        let result: String = self
            .conn()
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(result, "ok");
    }
}

impl Env {
    fn second_root(&self) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        roots::add(&self.conn(), dir.path()).unwrap();
        dir
    }

    fn file_id(&self) -> i64 {
        self.count("SELECT id FROM files")
    }

    fn one_file_text(&self, column: &str) -> String {
        self.conn()
            .query_row(&format!("SELECT {column} FROM files"), [], |r| r.get(0))
            .unwrap()
    }
}

/// Denies the current user read access to `path`: mode 000 on Unix, a deny ACE
/// for reading data on Windows. Returns false when the denial did not take
/// effect, which is the case for root on Unix.
fn make_unreadable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap();
        let denied = std::process::Command::new("icacls")
            .arg(path)
            .args(["/deny", &format!("{user}:(RD)")])
            .output()
            .unwrap();
        assert!(denied.status.success(), "{denied:?}");
    }
    std::fs::read(path).is_err()
}

/// Undoes [`make_unreadable`].
fn make_readable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap();
        let removed = std::process::Command::new("icacls")
            .arg(path)
            .args(["/remove:d", &user])
            .output()
            .unwrap();
        assert!(removed.status.success(), "{removed:?}");
    }
}

/// Dates `path` a minute back: old enough to pass the scheduler's stability
/// check at once, so only what the test is about can hold it back.
fn backdate(path: &Path) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(60))
        .unwrap();
}
