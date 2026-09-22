//! SPEC.md §7 M5 verification, against the real engine (threads, channels,
//! writer) with the fake embedder. Items that need a file-system watcher come
//! with Slice 4. Every wait polls a condition against a deadline; nothing sleeps
//! for a fixed time and hopes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use magi_core::config::Config;
use magi_core::db::{self, files, roots};
use magi_core::dto::IndexState;
use magi_core::embed::{CountingEmbedder, FakeEmbedder, FakeImageEmbedder, ImageEmbedder};
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
            self.embedder.clone(),
            image,
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

// ---- Slice 4: a real watcher. Nothing below calls `rescan()`: changes are
// noticed the way they are in use. Latency is the 2 s debounce plus the 3 s
// settle window, so these take a few seconds each.

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

/// Item 1: a new file is searchable within 10 s.
#[test]
fn a_new_file_is_searchable_within_ten_seconds() {
    let env = Env::new();
    let _engine = env.start();

    env.write("arrival.txt", "the zeppelin has landed");

    wait_until(
        Duration::from_secs(10),
        "the new file to be searchable",
        || env.hits("zeppelin") == 1,
    );
}

/// Item 2: a modified file's old chunks, FTS rows and vectors are gone.
#[test]
fn a_modified_file_replaces_its_old_content_everywhere() {
    let env = Env::new();
    env.write("edit.txt", "aardvark burrows");
    let _engine = env.start();
    wait_until(Duration::from_secs(30), "first version", || {
        env.hits("aardvark") == 1
    });

    env.write(
        "edit.txt",
        "platypus swims, and a much longer second version",
    );

    wait_until(Duration::from_secs(30), "second version", || {
        env.hits("platypus") == 1 && env.hits("aardvark") == 0
    });
    assert_eq!(env.count("SELECT COUNT(*) FROM files"), 1);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM vec_text"),
        env.count("SELECT COUNT(*) FROM chunks"),
        "no vector outlives its chunk"
    );
    assert_eq!(
        env.count("SELECT COUNT(*) FROM chunks"),
        2,
        "the body and the filename chunk, not the old ones too"
    );
}

/// Item 3: touching a file without changing it embeds nothing.
#[test]
fn touching_a_file_does_not_re_embed_it() {
    let env = Env::new();
    env.write("touch.txt", "steady content");
    let _engine = env.start();
    wait_until(Duration::from_secs(30), "the first index", || {
        env.hits("steady") == 1
    });
    let embedded = env.embedder.chunks();
    let path = env.root().join("touch.txt");
    let before = magi_core::discovery::stat(&path).unwrap().mtime_ns;

    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let after = magi_core::discovery::stat(&path).unwrap().mtime_ns;
    assert_ne!(before, after);

    wait_until(
        Duration::from_secs(30),
        "the new mtime to be recorded",
        || env.count("SELECT mtime_ns FROM files") == after && env.drained(),
    );
    assert_eq!(env.embedder.chunks(), embedded, "nothing was embedded");
}

/// Item 4: a rename inside a root updates the path and embeds nothing.
#[test]
fn a_rename_inside_a_root_updates_the_path_without_re_embedding() {
    let env = Env::new();
    env.write("before.txt", "portable words");
    let _engine = env.start();
    wait_until(Duration::from_secs(30), "the first index", || {
        env.hits("portable") == 1
    });
    let (id, embedded) = (env.file_id(), env.embedder.chunks());

    std::fs::rename(env.root().join("before.txt"), env.root().join("after.txt")).unwrap();

    wait_until(Duration::from_secs(30), "the rename to be followed", || {
        env.hits("after") == 1 && env.drained()
    });
    assert_eq!(env.one_file_text("file_name"), "after.txt");
    assert_eq!(
        (env.file_id(), env.count("SELECT COUNT(*) FROM files")),
        (id, 1)
    );
    assert_eq!(env.hits("before"), 0);
    assert_eq!(env.embedder.chunks(), embedded, "nothing was embedded");
}

/// Item 5: a move between two roots updates `root_id` and embeds nothing.
#[test]
fn a_move_between_roots_updates_the_root_without_re_embedding() {
    let env = Env::new();
    let other = env.second_root();
    env.write("traveller.txt", "wandering words");
    let _engine = env.start();
    wait_until(Duration::from_secs(30), "the first index", || {
        env.hits("wandering") == 1
    });
    let (id, embedded) = (env.file_id(), env.embedder.chunks());
    let first_root = env.count("SELECT root_id FROM files");

    std::fs::rename(
        env.root().join("traveller.txt"),
        other.path().join("traveller.txt"),
    )
    .unwrap();

    wait_until(Duration::from_secs(30), "the move to be followed", || {
        env.count("SELECT root_id FROM files") != first_root && env.drained()
    });
    assert_eq!(
        (env.file_id(), env.count("SELECT COUNT(*) FROM files")),
        (id, 1)
    );
    assert_eq!(env.embedder.chunks(), embedded, "nothing was embedded");
    assert_eq!(env.hits("wandering"), 1);
}

/// Item 6: a file moved out of every root is removed.
#[test]
fn a_file_moved_out_of_every_root_is_removed() {
    let env = Env::new();
    env.write("leaving.txt", "departing words");
    let _engine = env.start();
    wait_until(Duration::from_secs(30), "the first index", || {
        env.hits("departing") == 1
    });
    let outside = tempfile::tempdir().unwrap();

    std::fs::rename(
        env.root().join("leaving.txt"),
        outside.path().join("leaving.txt"),
    )
    .unwrap();

    wait_until(Duration::from_secs(30), "the file to be removed", || {
        env.count("SELECT COUNT(*) FROM files") == 0
    });
    assert_eq!(env.hits("departing"), 0);
}

/// Item 7: a deleted file is gone from every table.
#[test]
fn a_deleted_file_is_removed_from_every_table() {
    let env = Env::new();
    env.write("doomed.txt", "condemned words");
    let _engine = env.start();
    wait_until(Duration::from_secs(30), "the first index", || {
        env.hits("condemned") == 1
    });

    std::fs::remove_file(env.root().join("doomed.txt")).unwrap();

    wait_until(Duration::from_secs(30), "the file to be removed", || {
        env.count("SELECT COUNT(*) FROM files") == 0
    });
    for table in ["chunks", "vec_text", "vec_image"] {
        assert_eq!(
            env.count(&format!("SELECT COUNT(*) FROM {table}")),
            0,
            "{table}"
        );
    }
    assert_eq!(env.hits("condemned"), 0, "and not in the FTS index");
}

/// Item 9, with a live watcher: 1,000 files created while the engine runs are
/// each indexed exactly once.
#[test]
fn a_burst_of_1000_files_created_while_running_is_indexed_exactly_once() {
    let env = Env::new();
    let engine = env.start();

    for i in 0..1000 {
        env.write(
            &format!("live_{i:04}.txt"),
            &format!("live burst number {i}"),
        );
    }

    env.wait_drained(1000);
    assert_eq!(env.count("SELECT COUNT(DISTINCT path) FROM files"), 1000);
    assert_eq!(engine.stats().indexed, 1000);
    assert_eq!(
        env.embedder.chunks() as i64,
        env.count("SELECT COUNT(*) FROM chunks"),
        "every chunk embedded exactly once"
    );
}

/// Item 10, with a live watcher: a file written for 5 s is indexed once with
/// its final content, and never in a half-written state.
#[test]
fn a_file_written_slowly_under_a_watcher_is_indexed_once() {
    let env = Env::new();
    let engine = env.start();
    let path = env.root().join("growing.txt");

    {
        use std::io::Write;
        let mut file = std::fs::File::create(&path).unwrap();
        for i in 0..10 {
            writeln!(file, "partial line {i}").unwrap();
            file.flush().unwrap();
            std::thread::sleep(Duration::from_millis(500));
            assert_eq!(
                env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'"),
                0,
                "not indexed while it is being written"
            );
        }
        writeln!(file, "finalmarker").unwrap();
    }

    wait_until(Duration::from_secs(30), "the final content", || {
        env.hits("finalmarker") == 1
    });
    assert_eq!(engine.stats().indexed, 1, "once");
    assert_eq!(
        env.embedder.chunks() as i64,
        env.count("SELECT COUNT(*) FROM chunks"),
        "no partial version was ever embedded"
    );
}

/// Item 13: adding an exclusion purges the files it covers; removing it
/// indexes them again.
#[test]
fn changing_an_exclusion_purges_and_restores_files() {
    let env = Env::new();
    env.write("keep.txt", "kept words");
    env.write("draft.txt", "drafted words");
    let engine = env.start();
    wait_until(Duration::from_secs(30), "both files", || {
        env.hits("kept") == 1 && env.hits("drafted") == 1
    });

    let mut config = Config::default().indexing;
    config.exclude_globs.push("**/draft.txt".into());
    engine
        .apply_indexing_config(&config)
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    wait_until(Duration::from_secs(30), "the excluded file to go", || {
        env.hits("drafted") == 0
    });
    assert_eq!(env.hits("kept"), 1);

    engine
        .apply_indexing_config(&Config::default().indexing)
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    wait_until(Duration::from_secs(30), "the file to come back", || {
        env.hits("drafted") == 1
    });
}

/// Denies the current user read access to `path`: mode 000 on Unix, a deny ACE
/// for reading data on Windows.
fn make_unreadable(path: &Path) {
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
    assert!(std::fs::read(path).is_err(), "the file is still readable");
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

/// Item 14: a file the user may not read is retried, then left in `error`;
/// the other files are indexed regardless.
#[test]
fn a_file_without_read_permission_becomes_error_and_the_others_continue() {
    let env = Env::new();
    env.write("secret.txt", "classified words");
    env.write("open.txt", "public words");
    make_unreadable(&env.root().join("secret.txt"));
    let engine = env.start();

    wait_until(Duration::from_secs(60), "secret.txt to give up", || {
        env.skip_backoff();
        env.state_of("secret.txt") == "error"
    });
    wait_until(Duration::from_secs(60), "open.txt to be indexed", || {
        env.state_of("open.txt") == "indexed"
    });
    assert_eq!(env.hits("public"), 1);
    assert_eq!(env.hits("classified"), 0);
    assert_eq!(engine.status().unwrap().errors, 1);

    // Readable again: only a manual retry brings it back.
    make_readable(&env.root().join("secret.txt"));
    assert_eq!(engine.retry_errors().unwrap(), 1);
    wait_until(Duration::from_secs(30), "secret.txt to be indexed", || {
        env.state_of("secret.txt") == "indexed"
    });
    assert_eq!(env.hits("classified"), 1);
}

/// Item 15: a root that disappears keeps its index (hidden from search), and
/// when it is back it is picked up by the 30 s re-probe with nothing embedded
/// again.
#[test]
fn a_missing_root_keeps_its_index_and_resumes_without_re_embedding() {
    let env = Env::new();
    env.write("kept.txt", "durable content");
    let engine = env.start();
    env.wait_drained(1);
    engine.shutdown();
    let embedded = env.embedder.chunks();
    let status = || -> String {
        env.conn()
            .query_row("SELECT status FROM roots", [], |r| r.get(0))
            .unwrap()
    };

    let away = env.root().with_extension("away");
    std::fs::rename(env.root(), &away).unwrap();
    let _engine = env.start();
    assert_eq!(status(), "missing");
    assert_eq!(env.count("SELECT COUNT(*) FROM files"), 1, "index kept");
    assert_eq!(env.hits("durable"), 0, "hidden while missing");

    std::fs::rename(&away, env.root()).unwrap();
    wait_until(Duration::from_secs(90), "the root to be back", || {
        status() == "ok" && env.drained()
    });
    assert_eq!(env.hits("durable"), 1);
    assert_eq!(env.embedder.chunks(), embedded, "nothing embedded again");
}

/// SPEC.md §6.2: a path over the old 260-character limit is indexed like any
/// other (and on Unix, nothing special happens at all).
#[test]
fn a_file_under_a_path_over_260_characters_is_indexed() {
    let env = Env::new();
    let mut dir = env.root().to_path_buf();
    for i in 0..5 {
        dir.push(format!("{i}-{}", "d".repeat(60)));
    }
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("deep.txt");
    assert!(file.as_os_str().len() > 260);
    std::fs::write(&file, "abyssal content").unwrap();

    let _engine = env.start();
    env.wait_drained(1);
    assert_eq!(env.state_of("deep.txt"), "indexed");
    assert_eq!(env.hits("abyssal"), 1);
}

/// Item 16 (Windows): a file another program holds open without sharing is
/// retried as often as it takes, never given up on, and indexed once released.
#[cfg(windows)]
#[test]
fn a_locked_file_is_retried_and_indexed_after_release() {
    use std::os::windows::fs::OpenOptionsExt;

    let env = Env::new();
    env.write("held.txt", "exclusive content");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(env.root().join("held.txt"))
        .unwrap();
    let _engine = env.start();

    // Far more tries than the three that would put a broken file in `error`.
    for n in 1..=6 {
        env.skip_backoff();
        wait_until(Duration::from_secs(30), &format!("lock retry {n}"), || {
            env.count(
                "SELECT COUNT(*) FROM files
                 WHERE file_name = 'held.txt' AND state = 'pending' AND next_attempt_at > 0",
            ) == 1
        });
    }
    assert_eq!(env.state_of("held.txt"), "pending");

    drop(lock);
    wait_until(Duration::from_secs(30), "held.txt to be indexed", || {
        env.skip_backoff();
        env.state_of("held.txt") == "indexed"
    });
    assert_eq!(env.hits("exclusive"), 1);
}

/// An image model that says whether it is loaded, so a test can watch it
/// being unloaded. Only a forced unload (idle `0`) clears it: the real idle
/// timeout never passes during a test.
#[derive(Default)]
struct TrackedImageModel(AtomicBool);

impl ImageEmbedder for TrackedImageModel {
    fn model_id(&self) -> &str {
        "fake-image-v1"
    }

    fn dim(&self) -> usize {
        FakeImageEmbedder.dim()
    }

    fn embed_image(&self, image: &image::RgbImage) -> magi_core::Result<Vec<f32>> {
        self.0.store(true, Ordering::SeqCst);
        FakeImageEmbedder.embed_image(image)
    }

    fn embed_query(&self, text: &str) -> magi_core::Result<Vec<f32>> {
        FakeImageEmbedder.embed_query(text)
    }

    fn unload_if_idle(&self, idle: Duration) {
        if idle.is_zero() {
            self.0.store(false, Ordering::SeqCst);
        }
    }
}

/// Item 17: memory pressure (forced through the test hook) pauses indexing
/// and unloads the image model; clearing it resumes indexing.
#[test]
fn memory_pressure_pauses_indexing_and_unloads_the_image_model() {
    let env = Env::new();
    image::RgbImage::from_pixel(64, 64, image::Rgb([200, 30, 30]))
        .save(env.root().join("red.png"))
        .unwrap();
    let image = Arc::new(TrackedImageModel::default());
    let engine = env.start_with(Some(image.clone()));
    env.wait_drained(1);
    assert!(image.0.load(Ordering::SeqCst), "the image was embedded");

    engine.simulate_low_memory(true);
    wait_until(Duration::from_secs(30), "pause and unload", || {
        engine.is_paused() && !image.0.load(Ordering::SeqCst)
    });
    env.write("later.txt", "postponed content");
    backdate(&env.root().join("later.txt"));
    wait_until(Duration::from_secs(30), "later.txt to be queued", || {
        env.count("SELECT COUNT(*) FROM files WHERE file_name = 'later.txt'") == 1
    });
    // A negative can only be shown by waiting: the scheduler looks at the
    // queue every 250 ms, so 3 s gives it a dozen chances to start the file.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(env.state_of("later.txt"), "pending");

    engine.simulate_low_memory(false);
    env.wait_drained(2);
    assert!(!engine.is_paused());
    assert_eq!(env.hits("postponed"), 1);
}

/// Item 12 through the engine: a root added while running is indexed and
/// watched, a disabled one is hidden and re-read when enabled, and a removed
/// one leaves no row behind.
#[test]
fn roots_are_added_disabled_and_removed_while_running() {
    let env = Env::new();
    env.write("home.txt", "home words");
    let engine = env.start();
    env.wait_drained(1);

    let extra = tempfile::tempdir().unwrap();
    std::fs::write(extra.path().join("first.txt"), "added words").unwrap();
    let root = engine.add_root(extra.path()).unwrap();
    assert_eq!((root.enabled, root.status.as_str()), (true, "ok"));
    assert!(engine.add_root(extra.path()).is_err(), "already a root");
    env.wait_drained(2);
    std::fs::write(extra.path().join("second.txt"), "watched words").unwrap();
    wait_until(Duration::from_secs(30), "the new root's watcher", || {
        env.hits("watched") == 1
    });

    engine.set_root_enabled(root.id, false).unwrap();
    assert_eq!(env.hits("added"), 0, "a disabled root is hidden");
    std::fs::write(extra.path().join("third.txt"), "disabled words").unwrap();
    engine.set_root_enabled(root.id, true).unwrap();
    wait_until(Duration::from_secs(30), "the re-enabled root", || {
        env.hits("added") == 1 && env.hits("disabled") == 1
    });

    engine.remove_root(root.id).unwrap();
    assert!(engine.remove_root(root.id).is_err(), "already removed");
    assert_eq!(env.count("SELECT COUNT(*) FROM files"), 1);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM chunks c JOIN files f ON f.id = c.file_id"),
        env.count("SELECT COUNT(*) FROM chunks")
    );
    assert_eq!(
        env.count("SELECT COUNT(*) FROM vec_text"),
        env.count("SELECT COUNT(*) FROM chunks")
    );
    assert_eq!(env.hits("words"), 1, "only home.txt is left");
    assert_eq!(engine.status().unwrap().roots.len(), 1);
}

/// SPEC.md §6.4: a user pause holds new work back, survives a restart, and
/// resuming indexes what waited.
#[test]
fn a_pause_survives_a_restart_and_resuming_indexes_what_waited() {
    let env = Env::new();
    let engine = env.start();
    engine.pause().unwrap();
    assert!(engine.is_paused());
    assert_eq!(engine.status().unwrap().state, IndexState::Paused);
    engine.shutdown();

    env.write("waiting.txt", "held words");
    backdate(&env.root().join("waiting.txt"));
    let engine = env.start();
    assert!(engine.is_paused(), "the pause was persisted");
    // A negative can only be shown by waiting: see the item 17 test.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(env.state_of("waiting.txt"), "pending");

    engine.resume().unwrap();
    env.wait_drained(1);
    assert_eq!(env.hits("held"), 1);
    engine.shutdown();
    assert!(!env.start().is_paused(), "the resume was persisted");
}

/// Status and its events: the counts follow the queue, and a subscriber hears
/// about a change without asking.
#[test]
fn status_counts_the_queue_and_events_follow_changes() {
    let env = Env::new();
    env.write("a.txt", "alpha words");
    let engine = env.start();
    env.wait_drained(1);
    let events = engine.subscribe();

    env.write("b.txt", "beta words");
    wait_until(Duration::from_secs(30), "an idle event after b.txt", || {
        events
            .try_iter()
            .any(|s| s.state == IndexState::Idle && s.indexed == 2)
    });
    let status = engine.status().unwrap();
    assert_eq!(
        (status.state, status.queued, status.indexed, status.errors),
        (IndexState::Idle, 0, 2, 0)
    );
    assert_eq!(status.current_file, None);
    assert_eq!(status.roots.len(), 1);
    assert_eq!(status.roots[0].status, "ok");
}
