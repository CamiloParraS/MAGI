//! Ask Magi through the host (ADR-0014): signed out, nothing reaches the
//! network or the keychain; signed in, `signed_in_as` follows the session.

use std::sync::Arc;
use std::time::{Duration, Instant};

use magi_core::Error;
use magi_core::ask::{AskError, AskService, AskSource, FakeBackend, MemoryStore};
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
