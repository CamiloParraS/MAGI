//! Ask Magi through the host (ADR-0014): signed out, nothing reaches the
//! network or the keychain; signed in, `signed_in_as` follows the session.

use std::sync::Arc;
use std::time::{Duration, Instant};

use magi_core::Error;
use magi_core::ask::{AskError, AskFile, AskService, AskSource, FakeBackend, MemoryStore, SendAs};
use magi_core::config::Config;
use magi_core::dto::SearchRequest;
use magi_core::host::{Host, HostPaths};

struct Rig {
    _dir: tempfile::TempDir,
    host: Host,
    backend: FakeBackend,
    store: Arc<MemoryStore>,
}

fn rig() -> Rig {
    // SAFETY: every test sets the same value before its host starts.
    unsafe { std::env::set_var("MAGI_FAKE_EMBEDDER", "1") };
    let dir = tempfile::tempdir().unwrap();
    let paths = HostPaths {
        db: dir.path().join("magi.db"),
        config: dir.path().join("config.toml"),
    };
    let mut config = Config::default();
    config.indexing.pause_on_battery = false;
    std::fs::write(&paths.config, toml::to_string(&config).unwrap()).unwrap();
    let backend = FakeBackend::new();
    let store = Arc::new(MemoryStore::new());
    let ask = AskService::new(
        Box::new(backend.clone()),
        Box::new(store.clone()),
        Ok(FakeBackend::endpoints()),
    );
    let host = Host::start_with_ask(paths, |_| {}, ask).unwrap();
    let start = Instant::now();
    while host.engine().is_err() {
        assert!(start.elapsed() < Duration::from_secs(30), "engine start");
        std::thread::sleep(Duration::from_millis(50));
    }
    Rig {
        _dir: dir,
        host,
        backend,
        store,
    }
}

fn source() -> Vec<AskSource> {
    vec![AskSource {
        id: "1".into(),
        file_name: "a.txt".into(),
        text: "hello".into(),
    }]
}

#[test]
fn a_signed_out_start_and_search_make_zero_requests() {
    let rig = rig();
    rig.host
        .search(&SearchRequest {
            query: "anything".into(),
            limit: None,
        })
        .unwrap();
    assert_eq!(rig.host.account_state().unwrap().signed_in_as, None);
    assert_eq!(rig.backend.requests(), 0, "no request signed out");
    assert_eq!(rig.store.loads(), 0, "no keychain read signed out");

    let err = rig.host.ask("q", &source()).unwrap_err();
    assert!(matches!(err, Error::Ask(AskError::SignedOut)));
    assert_eq!(rig.backend.requests(), 0);
    rig.host.shutdown();
}

#[test]
fn sign_in_state_follows_the_session_and_is_kept_in_the_config() {
    let rig = rig();
    let state = rig.host.sign_up("a@example.com", "hunter22").unwrap();
    assert_eq!(state.signed_in_as.as_deref(), Some("a@example.com"));
    assert!(!state.disclosed);
    assert!(rig.host.accept_ask_disclosure().unwrap().disclosed);
    assert_eq!(rig.host.ask("q", &source()).unwrap().used, 1);

    let state = rig.host.sign_out().unwrap();
    assert_eq!(state.signed_in_as, None);
    assert!(state.disclosed, "signing out keeps the disclosure accepted");
    rig.host.shutdown();
}

#[test]
fn a_session_the_server_rejects_returns_the_window_to_sign_in() {
    let rig = rig();
    rig.host.sign_up("a@example.com", "hunter22").unwrap();
    rig.backend.revoke_all_sessions();
    let err = rig.host.ask("q", &source()).unwrap_err();
    assert!(matches!(err, Error::Ask(AskError::SessionExpired)));
    assert_eq!(rig.host.account_state().unwrap().signed_in_as, None);
    rig.host.shutdown();
}

#[test]
fn delete_account_signs_out_here_too() {
    let rig = rig();
    rig.host.sign_up("a@example.com", "hunter22").unwrap();
    assert_eq!(rig.host.delete_account().unwrap().signed_in_as, None);
    assert!(rig.store.peek().is_none());
    rig.host.shutdown();
}

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(start.elapsed() < Duration::from_secs(60), "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_whole_file_and_a_picture_go_as_chosen_and_never_with_the_path() {
    let rig = rig();
    let root = tempfile::tempdir().unwrap();
    let long: String = (0..400).map(|i| format!("word{i} ")).collect();
    std::fs::write(
        root.path().join("notes.txt"),
        format!("lease terms {long} the end"),
    )
    .unwrap();
    let picture = image::RgbImage::from_pixel(2048, 1024, image::Rgb([200, 30, 30]));
    picture.save(root.path().join("red.png")).unwrap();
    rig.host.engine().unwrap().add_root(root.path()).unwrap();
    let notes = root.path().join("notes.txt");
    let red = root.path().join("red.png");
    wait_for("indexed", || {
        // The file's row comes before its text does.
        rig.host
            .ask_file_at(&notes)
            .unwrap()
            .is_some_and(|f| !f.preview.is_empty())
            && rig.host.ask_file_at(&red).unwrap().is_some()
    });
    let notes = rig.host.ask_file_at(&notes).unwrap().unwrap();
    let red = rig.host.ask_file_at(&red).unwrap().unwrap();
    assert_eq!(red.kind, magi_core::discovery::Kind::Image);
    assert!(notes.preview.starts_with("lease terms"));
    // What a name-only search result's chip shows: the same opening text.
    assert_eq!(rig.host.ask_preview(notes.file_id).unwrap(), notes.preview);
    assert_eq!(rig.host.ask_preview(-1).unwrap(), "");
    let size = rig.host.ask_whole_size(notes.file_id).unwrap();
    assert!(!size.cut && size.chars > 2000, "{size:?}");
    // Outside every root: not something Ask Magi can use.
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert!(rig.host.ask_file_at(outside.path()).unwrap().is_none());

    rig.host.sign_up("a@example.com", "hunter22").unwrap();
    let file = |info: &magi_core::dto::AskFileInfo, send| AskFile {
        file_id: info.file_id,
        file_name: info.file_name.clone(),
        fallback: String::new(),
        send,
    };
    rig.host
        .ask_about(
            "what does it say?",
            &[file(&notes, SendAs::Whole), file(&red, SendAs::Image)],
        )
        .unwrap();
    let body = rig.backend.last_ask().unwrap();
    let text = body["sources"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("lease terms") && text.ends_with("the end"),
        "{text}"
    );
    assert_eq!(
        text.matches("word399").count(),
        1,
        "the overlap is taken out"
    );
    assert_eq!(body["images"][0]["id"], "2");
    let sent = rig.host.ask_whole_size(notes.file_id).unwrap();
    assert_eq!(sent.chars, text.encode_utf16().count());
    let raw = body.to_string();
    let folder = root.path().file_name().unwrap().to_string_lossy();
    assert!(!raw.contains(folder.as_ref()), "no folder name is sent");
    rig.host.shutdown();
}
