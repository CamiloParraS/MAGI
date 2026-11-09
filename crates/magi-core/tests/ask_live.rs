//! Asks the deployed Magi server for real (plan, Day 3). Ignored by default:
//! it needs a deployed Worker and a test account, which CI does not have.
//!
//! ```text
//! MAGI_LIVE_SUPABASE_URL=https://<ref>.supabase.co
//! MAGI_LIVE_ANON_KEY=<publishable or anon key>
//! MAGI_LIVE_WORKER_URL=https://magi-ask.<subdomain>.workers.dev
//! MAGI_LIVE_EMAIL=... MAGI_LIVE_PASSWORD=...
//! cargo test -p magi-core --test ask_live -- --ignored --nocapture
//! ```
//!
//! One question counts against the account's 20 a day.

use std::sync::Arc;

use magi_core::ask::{AskService, AskSource, Endpoints, MemoryStore, UreqTransport};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

#[test]
#[ignore = "needs the deployed server and a test account"]
fn the_deployed_server_answers_a_question_about_a_source() {
    let endpoints = Endpoints {
        supabase_url: var("MAGI_LIVE_SUPABASE_URL").trim_end_matches('/').into(),
        anon_key: var("MAGI_LIVE_ANON_KEY"),
        worker_url: var("MAGI_LIVE_WORKER_URL").trim_end_matches('/').into(),
    };
    let service = AskService::new(
        Box::new(UreqTransport::new()),
        Box::new(Arc::new(MemoryStore::new())),
        Ok(endpoints),
    );
    service
        .sign_in(&var("MAGI_LIVE_EMAIL"), &var("MAGI_LIVE_PASSWORD"))
        .unwrap();

    let sources = vec![AskSource {
        id: "1".into(),
        file_name: "meeting.txt".into(),
        text: "The team meeting is on Friday at 3pm in room 4.".into(),
    }];
    let answer = service.ask("When is the team meeting?", &sources).unwrap();
    println!("{answer:#?}");

    assert!(answer.answer.contains("Friday") || answer.answer.contains("3"));
    assert!(answer.cited.iter().all(|id| id == "1"));
    assert!(answer.used >= 1 && answer.limit == 20);
}
