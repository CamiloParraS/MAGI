//! Permissions, missing roots, long paths and locked files.

use super::*;

/// Item 14: a file the user may not read is retried, then left in `error`;
/// the other files are indexed regardless.
#[test]
fn a_file_without_read_permission_becomes_error_and_the_others_continue() {
    let env = Env::new();
    env.write("secret.txt", "classified words");
    env.write("open.txt", "public words");
    if !make_unreadable(&env.root().join("secret.txt")) {
        eprintln!("skipped: the current user reads files regardless of permissions (root)");
        return;
    }
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
    assert_eq!(
        env.count("SELECT COUNT(*) FROM files WHERE error_code = 'permission_denied'"),
        1
    );

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
    // The startup reconcile probes it on the writer, after `start` returns.
    wait_until(Duration::from_secs(30), "the root to be missing", || {
        status() == "missing"
    });
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
