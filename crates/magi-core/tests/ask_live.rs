//! Asks the deployed Magi server for real (plan, Day 3). Ignored by default:
//! it reaches the network, which CI should not.
//!
//! ```text
//! cargo test -p magi-core --test ask_live -- --ignored --nocapture
//! ```
//!
//! It signs up a throwaway account, asks one question, then deletes the
//! account, so it leaves nothing behind. Set `MAGI_LIVE_EMAIL_DOMAIN` if the
//! Supabase project rejects the default `example.com`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use magi_core::ask::{AskService, AskSource, Endpoints, MemoryStore, UreqTransport};

#[test]
#[ignore = "reaches the deployed server"]
fn the_deployed_server_answers_a_question_about_a_source() {
    let service = AskService::new(
        Box::new(UreqTransport::new()),
        Box::new(Arc::new(MemoryStore::new())),
        Endpoints::compiled(),
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let domain = std::env::var("MAGI_LIVE_EMAIL_DOMAIN").unwrap_or_else(|_| "example.com".into());
    let email = format!("magi-live-{stamp}@{domain}");
    service.sign_up(&email, "live-test-pass-42").unwrap();

    let sources = vec![AskSource {
        id: "1".into(),
        file_name: "meeting.txt".into(),
        text: "The team meeting is on Friday at 3pm in room 4.".into(),
    }];
    let asked = service.ask("When is the team meeting?", &sources);
    // Delete first, so a failed assertion does not leave the account behind.
    let deleted = service.delete_account();

    let answer = asked.unwrap();
    println!("{answer:#?}");
    assert!(answer.answer.contains("Friday") || answer.answer.contains("3"));
    assert!(answer.cited.iter().all(|id| id == "1"));
    assert_eq!((answer.used, answer.limit), (1, 20));
    deleted.unwrap();
}
