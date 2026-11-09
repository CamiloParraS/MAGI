use std::sync::Arc;

use super::*;

const EMAIL: &str = "a@example.com";
const PASSWORD: &str = "hunter22";

fn sources() -> Vec<AskSource> {
    vec![AskSource {
        id: "7".into(),
        file_name: "notes.txt".into(),
        text: "The meeting is on Friday.".into(),
    }]
}

struct Rig {
    backend: FakeBackend,
    store: Arc<MemoryStore>,
    service: AskService,
}

fn rig() -> Rig {
    let backend = FakeBackend::new();
    let store = Arc::new(MemoryStore::new());
    let service = AskService::new(
        Box::new(backend.clone()),
        Box::new(store.clone()),
        Ok(FakeBackend::endpoints()),
    );
    Rig {
        backend,
        store,
        service,
    }
}

/// A new run of the app: empty memory, same keychain and server.
fn restart(rig: &Rig) -> AskService {
    AskService::new(
        Box::new(rig.backend.clone()),
        Box::new(rig.store.clone()),
        Ok(FakeBackend::endpoints()),
    )
}

fn signed_up() -> Rig {
    let rig = rig();
    rig.service.sign_up(EMAIL, PASSWORD).unwrap();
    rig
}

#[test]
fn signed_out_makes_no_request_and_reads_nothing_until_asked() {
    let rig = rig();
    assert_eq!(rig.backend.requests(), 0);
    assert_eq!(rig.store.loads(), 0, "the keychain is not read at launch");

    let err = rig.service.ask("q", &sources()).unwrap_err();
    assert_eq!(err, AskError::SignedOut);
    assert_eq!(rig.backend.requests(), 0, "signed out reaches no server");
    assert_eq!(rig.store.loads(), 1, "asking is when the keychain is read");
}

#[test]
fn sign_up_then_ask_answers_and_cites_a_sent_source() {
    let rig = signed_up();
    let answer = rig.service.ask("When?", &sources()).unwrap();
    assert_eq!(answer.cited, vec!["7".to_string()]);
    assert_eq!((answer.used, answer.limit), (1, 20));
    assert_eq!(rig.store.peek().unwrap().email, EMAIL);
    assert_eq!(
        rig.store.loads(),
        0,
        "a live session never reads the keychain"
    );
}

#[test]
fn wrong_password_taken_email_and_weak_password_are_told_apart() {
    let rig = signed_up();
    assert_eq!(
        rig.service.sign_in(EMAIL, "nope-nope").unwrap_err(),
        AskError::InvalidCredentials
    );
    assert_eq!(
        rig.service.sign_up(EMAIL, PASSWORD).unwrap_err(),
        AskError::EmailTaken
    );
    assert!(matches!(
        rig.service.sign_up("b@example.com", "123").unwrap_err(),
        AskError::WeakPassword(_)
    ));
}

#[test]
fn an_expired_access_token_is_refreshed_and_saved_before_the_question_is_sent() {
    let rig = rig();
    rig.backend.set_lifetime_secs(0);
    rig.service.sign_up(EMAIL, PASSWORD).unwrap();
    rig.backend.set_lifetime_secs(3600);
    let before = rig.store.peek().unwrap().refresh_token;
    let saves_before = rig.store.saves();

    rig.service.ask("q", &sources()).unwrap();

    assert_eq!(rig.backend.count("grant_type=refresh_token"), 1);
    assert_ne!(rig.store.peek().unwrap().refresh_token, before);
    assert_eq!(rig.store.saves(), saves_before + 1);
    // The refresh response is spent on the second token, which the question used.
    let log = rig.backend.log();
    let refresh = log
        .iter()
        .position(|l| l.contains("refresh_token"))
        .unwrap();
    let ask = log.iter().position(|l| l.ends_with("/ask")).unwrap();
    assert!(refresh < ask);
}

#[test]
fn a_new_refresh_token_reaches_the_keychain_before_the_question_goes_out() {
    // Records "keychain saved" in the same order as server requests.
    struct Noting(FakeBackend, MemoryStore);
    impl CredentialStore for Noting {
        fn load(&self) -> Result<Option<StoredLogin>, String> {
            self.1.load()
        }
        fn save(&self, login: &StoredLogin) -> Result<(), String> {
            self.0.note("keychain saved");
            self.1.save(login)
        }
        fn clear(&self) -> Result<(), String> {
            self.1.clear()
        }
    }
    let backend = FakeBackend::new();
    backend.set_lifetime_secs(0);
    let service = AskService::new(
        Box::new(backend.clone()),
        Box::new(Noting(backend.clone(), MemoryStore::new())),
        Ok(FakeBackend::endpoints()),
    );
    service.sign_up(EMAIL, PASSWORD).unwrap();
    backend.set_lifetime_secs(3600);
    service.ask("q", &sources()).unwrap();

    let log = backend.log();
    let refresh = log
        .iter()
        .position(|l| l.contains("refresh_token"))
        .unwrap();
    let saved_after = log
        .iter()
        .skip(refresh)
        .position(|l| l == "note keychain saved")
        .unwrap();
    let ask = log.iter().position(|l| l.ends_with("/ask")).unwrap();
    assert!(refresh + saved_after < ask, "{log:?}");
}

#[test]
fn concurrent_questions_spend_a_refresh_token_once() {
    let rig = rig();
    rig.backend.set_lifetime_secs(0);
    rig.service.sign_up(EMAIL, PASSWORD).unwrap();
    // The first refresh yields a token that is valid for an hour, so the
    // other threads find it instead of refreshing again.
    rig.backend.set_lifetime_secs(3600);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let service = rig.service.clone();
            std::thread::spawn(move || service.ask("q", &sources()))
        })
        .collect();
    for t in threads {
        t.join().unwrap().unwrap();
    }
    assert_eq!(rig.backend.count("grant_type=refresh_token"), 1);
}

#[test]
fn a_restart_signs_back_in_from_the_keychain_with_one_read() {
    let rig = signed_up();
    let second_run = restart(&rig);
    assert_eq!(rig.store.loads(), 0);

    let answer = second_run.ask("q", &sources()).unwrap();
    assert_eq!(answer.used, 1);
    assert_eq!(rig.store.loads(), 1);
    assert_eq!(rig.backend.count("grant_type=refresh_token"), 1);

    second_run.ask("q", &sources()).unwrap();
    assert_eq!(rig.store.loads(), 1, "memory serves the next question");
}

#[test]
fn a_revoked_session_clears_the_keychain_and_asks_for_a_sign_in() {
    let rig = signed_up();
    let second_run = restart(&rig);
    rig.backend.revoke_all_sessions();

    assert_eq!(
        second_run.ask("q", &sources()).unwrap_err(),
        AskError::SessionExpired
    );
    assert!(rig.store.peek().is_none());
    assert_eq!(
        second_run.ask("q", &sources()).unwrap_err(),
        AskError::SignedOut
    );
}

#[test]
fn a_denied_keychain_keeps_this_run_signed_in_but_not_the_next() {
    let rig = rig();
    rig.store.set_failing(true);
    rig.service.sign_up(EMAIL, PASSWORD).unwrap();
    rig.service.ask("q", &sources()).unwrap();

    assert_eq!(
        restart(&rig).ask("q", &sources()).unwrap_err(),
        AskError::SignedOut
    );
}

#[test]
fn sign_out_forgets_the_session_and_later_questions_make_no_request() {
    let rig = signed_up();
    rig.service.sign_out();
    assert!(rig.store.peek().is_none());
    let requests = rig.backend.requests();

    assert_eq!(
        rig.service.ask("q", &sources()).unwrap_err(),
        AskError::SignedOut
    );
    assert_eq!(rig.backend.requests(), requests);
}

#[test]
fn delete_account_removes_it_everywhere() {
    let rig = signed_up();
    rig.service.delete_account().unwrap();
    assert!(rig.store.peek().is_none());
    assert_eq!(
        rig.service.sign_in(EMAIL, PASSWORD).unwrap_err(),
        AskError::InvalidCredentials
    );
}

#[test]
fn the_twenty_first_question_hits_the_daily_limit() {
    let rig = signed_up();
    for _ in 0..20 {
        rig.service.ask("q", &sources()).unwrap();
    }
    assert_eq!(
        rig.service.ask("q", &sources()).unwrap_err(),
        AskError::DailyLimit {
            used: 20,
            limit: 20,
            everyone: false
        }
    );
}

#[test]
fn a_build_without_its_public_values_reports_not_configured() {
    let service = AskService::new(
        Box::new(FakeBackend::new()),
        Box::new(MemoryStore::new()),
        Err(AskError::NotConfigured),
    );
    assert_eq!(
        service.sign_in(EMAIL, PASSWORD).unwrap_err(),
        AskError::NotConfigured
    );
    assert_eq!(
        service.ask("q", &sources()).unwrap_err(),
        AskError::NotConfigured
    );
}

#[test]
fn offline_is_a_network_error_and_keeps_the_login() {
    struct Offline;
    impl Transport for Offline {
        fn send(&self, _: &HttpRequest) -> Result<HttpResponse, String> {
            Err("dns error".into())
        }
    }
    let store = Arc::new(MemoryStore::new());
    store
        .save(&StoredLogin {
            refresh_token: "r".into(),
            email: EMAIL.into(),
        })
        .unwrap();
    let service = AskService::new(
        Box::new(Offline),
        Box::new(store.clone()),
        Ok(FakeBackend::endpoints()),
    );
    assert!(matches!(
        service.ask("q", &sources()).unwrap_err(),
        AskError::Network(_)
    ));
    assert!(
        store.peek().is_some(),
        "a dropped connection is not a sign-out"
    );
}

#[test]
fn citations_the_server_invents_are_dropped() {
    struct Liar;
    impl Transport for Liar {
        fn send(&self, _: &HttpRequest) -> Result<HttpResponse, String> {
            Ok(HttpResponse {
                status: 200,
                body: r#"{"answer":"x","cited":["7","999"],"used":1,"limit":20}"#.into(),
            })
        }
    }
    let session = Session {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: u64::MAX,
        email: EMAIL.into(),
    };
    let endpoints = FakeBackend::endpoints();
    let answer = AuthedClient::new(&Liar, &endpoints, &session)
        .ask("q", &sources())
        .unwrap();
    assert_eq!(answer.cited, vec!["7".to_string()]);
}

#[test]
fn a_session_does_not_print_its_tokens() {
    let session = Session {
        access_token: "secret-access".into(),
        refresh_token: "secret-refresh".into(),
        expires_at: 1,
        email: EMAIL.into(),
    };
    let shown = format!("{session:?}");
    assert!(!shown.contains("secret"), "{shown}");
}
