//! Control while running: memory pressure, roots, pause, status, model changes.

use super::*;

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
    let status = engine.status().unwrap();
    assert_eq!(status.state, IndexState::Paused);
    assert_eq!(status.paused_for, Some(PauseReason::LowMemory));
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
    assert_eq!(engine.status().unwrap().paused_for, None);
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
    std::fs::create_dir(extra.path().join("sub")).unwrap();
    assert!(matches!(
        engine.add_root(&extra.path().join("sub")),
        Err(magi_core::Error::NestedRoot { .. })
    ));
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

/// Switching a big folder off while it indexes, then on again, must finish it.
#[test]
fn a_folder_switched_off_mid_index_finishes_when_switched_on() {
    let env = Env::new();
    for i in 0..1500 {
        env.write(
            &format!("note_{i:04}.txt"),
            &format!("toggle file number {i}"),
        );
    }
    let engine = env.start();
    let root = engine.status().unwrap().roots[0].id;

    wait_until(Duration::from_secs(60), "indexing to be under way", || {
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'") >= 100
    });
    engine.set_root_enabled(root, false).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let settled = env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'");
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'"),
        settled,
        "nothing is indexed while the folder is off"
    );
    assert_eq!(engine.status().unwrap().queued, 0, "and nothing is queued");

    engine.set_root_enabled(root, true).unwrap();
    env.wait_drained(1500);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'"),
        1500
    );
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
    assert_eq!(status.roots[0].status, magi_core::db::roots::Health::Ok);
}

/// Item 3b: a text model id that differs from the one stored at the last
/// index re-embeds every file even though no content changed; with the id
/// unchanged, a restart embeds nothing.
#[test]
fn a_changed_text_model_id_re_embeds_unchanged_files() {
    let env = Env::new();
    env.write("model_a.txt", "first model text");
    env.write("model_b.txt", "second model text");
    let engine = env.start();
    env.wait_drained(2);
    engine.shutdown();

    // Same model: the hash-skip holds.
    let embedded = env.embedder.chunks();
    let engine = env.start();
    // The DB already holds the files: wait out the startup scan, which runs first.
    engine.rescan().recv().unwrap().unwrap();
    env.wait_drained(2);
    engine.shutdown();
    assert_eq!(
        env.embedder.chunks(),
        embedded,
        "same model id: nothing embedded"
    );

    // The last index was made by another model.
    env.conn()
        .execute(
            "UPDATE meta SET value = 'some-older-model' WHERE key = 'text_model_id'",
            [],
        )
        .unwrap();
    let _engine = env.start();
    // The DB already holds the files: wait out the startup scan, which runs first.
    _engine.rescan().recv().unwrap().unwrap();
    env.wait_drained(2);
    assert_eq!(
        (env.embedder.chunks() - embedded) as i64,
        env.count("SELECT COUNT(*) FROM chunks"),
        "every chunk re-embedded once"
    );
    assert_eq!(
        env.count(
            "SELECT COUNT(*) FROM meta WHERE key = 'text_model_id' AND value = 'some-older-model'"
        ),
        0,
        "the running model's id replaced the old one"
    );
    assert_eq!(env.hits("model"), 2);
}

/// Item 8: files created, modified, deleted and renamed while the engine is
/// stopped are all picked up on restart, and the database matches the folder.
#[test]
fn changes_made_while_stopped_converge_on_restart() {
    let env = Env::new();
    env.write("edited.txt", "original wording");
    env.write("doomed.txt", "soon deleted");
    env.write("renamed.txt", "travelling content");
    let engine = env.start();
    env.wait_drained(3);
    engine.shutdown();
    let embedded = env.embedder.chunks();

    env.write("edited.txt", "rewritten with a longer replacement wording");
    std::fs::remove_file(env.root().join("doomed.txt")).unwrap();
    std::fs::rename(
        env.root().join("renamed.txt"),
        env.root().join("arrived.txt"),
    )
    .unwrap();
    env.write("created.txt", "brand new arrival");

    let _engine = env.start();
    // The DB already holds the files: wait out the startup scan, which runs first.
    _engine.rescan().recv().unwrap().unwrap();
    env.wait_drained(3);

    let mut on_disk: Vec<String> = std::fs::read_dir(env.root())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    on_disk.sort();
    let mut in_db: Vec<String> = env
        .conn()
        .prepare("SELECT file_name FROM files ORDER BY file_name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    in_db.sort();
    assert_eq!(in_db, on_disk);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM files WHERE state = 'indexed'"),
        3
    );

    assert_eq!(env.hits("original"), 0, "the old text is gone");
    assert_eq!(env.hits("rewritten"), 1);
    assert_eq!(env.hits("doomed"), 0);
    assert_eq!(env.hits("travelling"), 1);
    assert_eq!(env.hits("arrival"), 1);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM vec_text"),
        env.count("SELECT COUNT(*) FROM chunks"),
        "no orphaned vectors"
    );
    // Only the edited and the created file were embedded; the rename was not.
    assert_eq!(
        (env.embedder.chunks() - embedded) as i64,
        env.count(
            "SELECT COUNT(*) FROM chunks JOIN files ON files.id = chunks.file_id
             WHERE files.file_name IN ('edited.txt', 'created.txt')"
        ),
    );
    env.integrity_ok();
}
