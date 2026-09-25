//! SPEC.md §7 M5 item 11 with a real process kill: `magi-cli daemon` killed
//! (SIGKILL / TerminateProcess) while files are `indexing` resets and finishes
//! them on the next start, and the database passes `integrity_check`.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const FILES: i64 = 40;

struct Dirs {
    data: tempfile::TempDir,
    config: tempfile::TempDir,
    root: tempfile::TempDir,
}

impl Dirs {
    fn cli(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_magi-cli"));
        cmd.env("MAGI_DATA_DIR", self.data.path())
            .env("MAGI_CONFIG_DIR", self.config.path())
            .env("MAGI_FAKE_EMBEDDER", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    fn daemon(&self) -> Child {
        self.cli().arg("daemon").spawn().unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        let conn = magi_core::db::open(&self.data.path().join("magi.db")).unwrap();
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }
}

fn wait_until(deadline: Duration, what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// About 1 MB of text per file, so each takes long enough to store that a
/// kill lands while some are `indexing`.
fn write_corpus(root: &Path) {
    for i in 0..FILES {
        let text = format!("marker{i} lorem ipsum dolor sit amet {i}\n").repeat(25_000);
        std::fs::write(root.join(format!("file{i}.txt")), text).unwrap();
    }
}

#[test]
fn a_daemon_killed_mid_index_finishes_on_restart_with_an_intact_database() {
    let dirs = Dirs {
        data: tempfile::tempdir().unwrap(),
        config: tempfile::tempdir().unwrap(),
        root: tempfile::tempdir().unwrap(),
    };
    // A laptop on battery must not pause the test.
    std::fs::write(
        dirs.config.path().join("config.toml"),
        "[indexing]\npause_on_battery = false\n",
    )
    .unwrap();
    write_corpus(dirs.root.path());
    let added = dirs
        .cli()
        .args(["roots", "add"])
        .arg(dirs.root.path())
        .status()
        .unwrap();
    assert!(added.success());

    let mut daemon = dirs.daemon();
    wait_until(Duration::from_secs(120), "a file being indexed", || {
        dirs.count("SELECT COUNT(*) FROM files WHERE state = 'indexing'") > 0
    });
    daemon.kill().unwrap();
    daemon.wait().unwrap();

    assert!(
        dirs.count("SELECT COUNT(*) FROM files WHERE state = 'indexing'") > 0,
        "the kill must land mid-index for this test to mean anything"
    );
    let conn = magi_core::db::open(&dirs.data.path().join("magi.db")).unwrap();
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    drop(conn);

    let mut daemon = dirs.daemon();
    wait_until(Duration::from_secs(300), "every file indexed", || {
        dirs.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'") == FILES
    });
    daemon.kill().unwrap();
    daemon.wait().unwrap();

    assert_eq!(dirs.count("SELECT COUNT(*) FROM files"), FILES);
    let chunks = dirs.count("SELECT COUNT(*) FROM chunks");
    assert_eq!(dirs.count("SELECT COUNT(*) FROM vec_text"), chunks);
    assert_eq!(dirs.count("SELECT COUNT(*) FROM chunks_fts"), chunks);
    assert_eq!(
        dirs.count(
            "SELECT COUNT(*) FROM files f
             WHERE NOT EXISTS (SELECT 1 FROM chunks c WHERE c.file_id = f.id)"
        ),
        0
    );
}
