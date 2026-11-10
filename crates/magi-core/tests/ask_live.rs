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

use magi_core::ask::{
    AskImage, AskPayload, AskService, AskSource, Endpoints, MemoryStore, UreqTransport,
};

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

#[test]
#[ignore = "reaches the deployed server"]
fn the_deployed_server_takes_whole_files_and_pictures_and_rejects_oversized_ones() {
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
    let email = format!("magi-live-big-{stamp}@{domain}");
    service.sign_up(&email, "live-test-pass-42").unwrap();

    // The fact sits at the very end of a 60,000-unit source (a whole file).
    let mut text = "Filler about shipping and invoices. ".repeat(1_800);
    text.truncate(59_940);
    text.push_str(" The vault code is 7391.");
    let big = |id: &str, text: String| AskSource {
        id: id.into(),
        file_name: "notes.txt".into(),
        text,
    };
    let fits = service.ask("What is the vault code?", &[big("1", text.clone())]);
    // Over the per-source cap, and over the total: both must be refused.
    let too_long = service.ask("What is the vault code?", &[big("1", "x".repeat(60_001))]);
    let too_much = service.ask(
        "What is the vault code?",
        &["1", "2", "3"].map(|id| big(id, "x".repeat(40_001))),
    );
    // A picture, as the app sends it: plain red, so the answer is clear.
    let red = image::RgbImage::from_pixel(1600, 900, image::Rgb([220, 20, 20]));
    let picture = AskPayload {
        sources: Vec::new(),
        images: vec![AskImage {
            id: "1".into(),
            file_name: "swatch.jpg".into(),
            data: magi_core::ask::image::encode(&red).unwrap(),
        }],
    };
    let seen = service.ask_with("What colour fills this picture? One word.", &picture);
    let deleted = service.delete_account();

    let answer = fits.unwrap();
    println!("{answer:#?}");
    assert!(answer.answer.contains("7391"), "{}", answer.answer);
    assert!(too_long.is_err() && too_much.is_err());
    let seen = seen.unwrap();
    println!("{seen:#?}");
    assert!(
        seen.answer.to_lowercase().contains("red"),
        "{}",
        seen.answer
    );
    deleted.unwrap();
}
