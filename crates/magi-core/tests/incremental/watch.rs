//! A real watcher: nothing here calls `rescan()`, changes are noticed the way
//! they are in use. Latency is the 2 s debounce plus the 3 s settle window, so
//! these take a few seconds each.

use super::*;

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
