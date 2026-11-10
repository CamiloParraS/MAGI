use magi_core::ask::{AskAnswer, AskError};
use magi_core::discovery::Kind;
use magi_core::dto::{AccountState, MatchSource, SearchResult, Snippet};
use magi_core::search::strength::Strength;

use super::*;
use crate::i18n::Lang;

fn result(file_id: i64, kind: Kind, snippet: Option<&str>) -> SearchResult {
    SearchResult {
        file_id,
        path: format!("C:\\private\\folder\\{file_id}.bin"),
        file_name: format!("{file_id}.bin"),
        kind,
        score: 1.0,
        snippet: snippet.map(|text| Snippet {
            text: text.to_owned(),
            highlights: Vec::new(),
        }),
        page: None,
        thumb_path: None,
        modified_at: 0,
        match_sources: vec![MatchSource::Keyword],
        strength: Strength::Strong,
        copies: Vec::new(),
    }
}

fn texts(n: i64) -> Vec<SearchResult> {
    (1..=n)
        .map(|id| result(id, Kind::Text, Some("the tenant shall pay rent")))
        .collect()
}

fn signed_in() -> AccountState {
    AccountState {
        signed_in_as: Some("a@b.co".into()),
        disclosed: true,
    }
}

fn chat(n: i64) -> AskModel {
    AskModel::new(signed_in(), request("rent", &texts(n)))
}

fn answer(cited: &[&str]) -> AskAnswer {
    AskAnswer {
        answer: "It is due on the 14th.".into(),
        cited: cited.iter().map(|c| (*c).to_owned()).collect(),
        used: 4,
        limit: 20,
    }
}

// --- What the search window hands over ---

#[test]
fn it_takes_the_top_five_results_with_text_all_ticked() {
    let taken = request("rent", &texts(8));
    assert_eq!(taken.question, "rent");
    let ids: Vec<i64> = taken.sources.iter().map(|s| s.file_id).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5]);
    assert!(taken.sources.iter().all(|s| s.ticked));
}

#[test]
fn a_photo_with_no_text_goes_as_its_picture() {
    let mut results = vec![result(1, Kind::Image, None)];
    results.extend(texts(6).into_iter().map(|mut r| {
        r.file_id += 1;
        r
    }));
    let taken = request("rent", &results).sources;
    let ids: Vec<i64> = taken.iter().map(|s| s.file_id).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5]);
    assert_eq!(taken[0].send, SendAs::Image);
    assert_eq!(taken[1].send, SendAs::Passages);
}

#[test]
fn a_photo_with_text_in_it_is_taken() {
    let taken = request("receipt", &[result(1, Kind::Image, Some("Pago aprobado"))]);
    assert_eq!(taken.sources[0].text, "Pago aprobado");
}

#[test]
fn a_snippet_that_only_repeats_the_path_is_not_sent() {
    // Never full paths (ADR-0014): a file-name match quotes the path's words.
    let results = [
        result(1, Kind::Text, Some("private folder 1 bin")),
        result(2, Kind::Text, Some("   ")),
        result(3, Kind::Text, Some("the tenant shall pay")),
    ];
    let ids: Vec<i64> = request("q", &results)
        .sources
        .iter()
        .map(|s| s.file_id)
        .collect();
    assert_eq!(ids, [3]);
}

#[test]
fn text_is_cut_to_what_the_server_accepts_counting_as_it_does() {
    // The server counts UTF-16 units, so an emoji is two of them.
    assert_eq!(clip("ab😀cd", 3), "ab");
    assert_eq!(clip("ab😀cd", 4), "ab😀");
    assert_eq!(clip("abc", 3), "abc");

    let long = "é".repeat(MAX_SOURCE_LEN + 50);
    let mut results = texts(1);
    results[0].snippet = Some(Snippet {
        text: long,
        highlights: Vec::new(),
    });
    results[0].file_name = "n".repeat(MAX_FILE_NAME_LEN + 9);
    let taken = request(&"q".repeat(MAX_QUESTION_LEN + 7), &results);
    assert_eq!(utf16_len(&taken.sources[0].text), MAX_SOURCE_LEN);
    assert_eq!(utf16_len(&taken.sources[0].file_name), MAX_FILE_NAME_LEN);
    assert_eq!(utf16_len(&taken.question), MAX_QUESTION_LEN);
}

#[test]
fn asking_needs_something_on_screen_to_ask_about() {
    assert!(!can_ask(&[]));
    assert!(can_ask(&texts(1)));
    assert!(can_ask(&[result(1, Kind::Image, None)]));
}

// --- The window ---

#[test]
fn the_window_fits_a_short_screen_so_the_question_box_shows() {
    use gpui_kit::{px, size};
    use view::{MIN_SIZE, SIZE, fit_to};
    assert_eq!(fit_to(None), SIZE);
    assert_eq!(fit_to(Some(size(px(1920.), px(1040.)))), SIZE);
    // 1920x1080 at 125%, less the taskbar.
    let short = fit_to(Some(size(px(1536.), px(691.))));
    assert!(short.height < px(600.) && short.height >= MIN_SIZE.height);
    assert_eq!(short.width, SIZE.width);
    // Never under the minimum, however small the screen says it is.
    assert_eq!(
        fit_to(Some(size(px(800.), px(300.)))).height,
        MIN_SIZE.height
    );
    // A narrow screen narrows it too, never under the minimum.
    assert_eq!(fit_to(Some(size(px(800.), px(900.)))).width, px(720.));
    assert_eq!(fit_to(Some(size(px(300.), px(900.)))).width, MIN_SIZE.width);
}

// --- Where the window starts ---

#[test]
fn signed_out_starts_at_sign_in_and_signed_in_starts_at_the_chat() {
    let req = || request("rent", &texts(1));
    let out = AskModel::new(AccountState::default(), req());
    assert_eq!(out.phase(), Phase::SignIn { expired: false });
    assert_eq!(AskModel::new(signed_in(), req()).phase(), Phase::Chat);
    // The disclosure is no screen of its own: the chat shows it once.
    let new = AccountState {
        signed_in_as: Some("a@b.co".into()),
        disclosed: false,
    };
    let model = AskModel::new(new, req());
    assert_eq!(model.phase(), Phase::Chat);
    assert!(model.needs_disclosure());
    assert!(!chat(1).needs_disclosure());
}

#[test]
fn signing_in_anywhere_opens_the_chat() {
    let mut model = AskModel::new(AccountState::default(), request("rent", &texts(1)));
    model.sync_account(signed_in());
    assert_eq!(model.phase(), Phase::Chat);
}

#[test]
fn sign_in_needs_an_email_and_a_password() {
    assert!(credentials_ok(" a@b.co ", "secret"));
    assert!(!credentials_ok("nobody", "secret"));
    assert!(!credentials_ok("a@b.co", ""));
    assert!(!credentials_ok("", "secret"));
}

// --- Asking ---

#[test]
fn only_ticked_sources_go_numbered_in_the_order_they_are_sent() {
    let mut model = chat(4);
    model.toggle(1);
    let out = model.start("when is rent due?").unwrap();
    // Host::ask_about numbers them 1, 2, 3 in this order, like the chips.
    let sent: Vec<&str> = out.files.iter().map(|f| f.file_name.as_str()).collect();
    assert_eq!(sent, ["1.bin", "3.bin", "4.bin"]);
    assert_eq!(out.question, "when is rent due?");
    let turn = model.turns().last().unwrap();
    assert_eq!(turn.question, "when is rent due?");
    assert_eq!(turn.state, TurnState::Asking);
}

#[test]
fn the_first_question_records_the_disclosure_and_later_ones_do_not() {
    let mut model = AskModel::new(
        AccountState {
            signed_in_as: Some("a@b.co".into()),
            disclosed: false,
        },
        request("rent", &texts(1)),
    );
    let out = model.start("rent?").unwrap();
    assert!(out.accept_disclosure);
    assert!(!model.needs_disclosure());
    model.finished(out.generation, Ok(answer(&[])));
    assert!(!model.start("and the deposit?").unwrap().accept_disclosure);
}

#[test]
fn sending_is_blocked_until_there_is_a_question_and_a_ticked_file() {
    let mut model = chat(1);
    assert_eq!(model.blocked("  "), Some(Blocked::NoQuestion));
    assert_eq!(model.blocked("rent?"), None);
    let long = "x".repeat(MAX_QUESTION_LEN + 1);
    assert_eq!(model.blocked(&long), Some(Blocked::TooLong));
    assert!(model.start(&long).is_none());
    model.toggle(0);
    assert_eq!(model.blocked("rent?"), Some(Blocked::NoSources));
    assert!(model.start("rent?").is_none());
    assert!(model.turns().is_empty());
}

#[test]
fn one_question_at_a_time() {
    let mut model = chat(1);
    let out = model.start("rent?").unwrap();
    assert_eq!(model.blocked("and?"), Some(Blocked::Busy));
    assert!(model.start("and?").is_none());
    // The chips stay as they were sent while the answer comes.
    model.toggle(0);
    assert!(model.sources()[0].ticked);
    model.finished(out.generation, Ok(answer(&[])));
    assert_eq!(model.blocked("and?"), None);
}

#[test]
fn a_search_with_only_photos_can_ask_about_the_pictures() {
    let model = AskModel::new(signed_in(), request("q", &[result(1, Kind::Image, None)]));
    assert_eq!(model.sources().len(), 1);
    assert_eq!(model.blocked("what is this?"), None);
}

#[test]
fn a_send_that_is_off_says_why_unless_something_else_already_does() {
    let s = Lang::En.strings();
    assert_eq!(
        blocked_text(Blocked::TooLong, true, s),
        Some(s.ask_too_long)
    );
    assert_eq!(
        blocked_text(Blocked::NoSources, true, s),
        Some(s.ask_tick_one)
    );
    // An empty box is how a follow-up starts; the placeholder says what to do.
    assert_eq!(blocked_text(Blocked::NoQuestion, true, s), None);
    // The spinner and the limit's own message already say it.
    assert_eq!(blocked_text(Blocked::Busy, true, s), None);
    assert_eq!(blocked_text(Blocked::Limited, true, s), None);
    // With nothing to send, the chat says so once.
    for reason in [Blocked::NoSources, Blocked::TooLong] {
        assert_eq!(blocked_text(reason, false, s), None);
    }
}

// --- Answers and follow-ups ---

#[test]
fn citations_become_numbers_and_ones_not_sent_are_dropped() {
    let mut model = chat(3);
    let out = model.start("rent?").unwrap();
    model.finished(out.generation, Ok(answer(&["3", "1", "3", "9", "x", "0"])));
    let TurnState::Answered(reply) = &model.turns()[0].state else {
        panic!("no answer");
    };
    assert_eq!(reply.cited, [0, 2]);
    assert_eq!(reply.text, "It is due on the 14th.");
    assert_eq!(model.used(), Some((4, 20)));
}

#[test]
fn a_citation_points_at_what_that_turn_sent() {
    let mut model = chat(3);
    model.toggle(0);
    let out = model.start("rent?").unwrap();
    model.finished(out.generation, Ok(answer(&["1"])));
    let turn = &model.turns()[0];
    let ids: Vec<i64> = turn.sent.iter().map(|s| s.file_id).collect();
    assert_eq!(ids, [2, 3]);
    let TurnState::Answered(reply) = &turn.state else {
        panic!("no answer");
    };
    // Number 1 is the first file sent, not the first chip.
    assert_eq!(turn.sent[reply.cited[0]].file_id, 2);
}

#[test]
fn a_follow_up_adds_a_turn_and_keeps_the_earlier_ones() {
    let mut model = chat(2);
    let first = model.start("rent?").unwrap();
    model.finished(first.generation, Ok(answer(&["1"])));
    model.toggle(0);
    let second = model.start("and the deposit?").unwrap();
    assert_eq!(second.files.len(), 1, "the chips as they are now");
    model.finished(second.generation, Ok(answer(&["1"])));
    let questions: Vec<&str> = model.turns().iter().map(|t| t.question.as_str()).collect();
    assert_eq!(questions, ["rent?", "and the deposit?"]);
    assert_eq!(model.turns()[0].sent.len(), 2);
    assert_eq!(model.turns()[1].sent.len(), 1);
}

// --- Failures ---

#[test]
fn a_failure_stays_on_its_turn_and_can_be_tried_again() {
    let mut model = chat(1);
    let out = model.start("rent?").unwrap();
    model.finished(out.generation, Err(AskError::Network("down".into())));
    assert_eq!(model.turns()[0].state, TurnState::Failed(Problem::Network));
    // Try again resends this question as it went, in the same turn.
    let again = model.retry().unwrap();
    assert_eq!(again.question, "rent?");
    assert_eq!(model.turns().len(), 1);
    assert_eq!(model.turns()[0].state, TurnState::Asking);
    model.finished(again.generation, Ok(answer(&[])));
    assert!(model.retry().is_none(), "only a failed turn is retried");
}

#[test]
fn an_expired_session_returns_to_sign_in_and_keeps_the_chat() {
    for error in [AskError::SessionExpired, AskError::SignedOut] {
        let mut model = chat(1);
        let out = model.start("rent?").unwrap();
        model.finished(out.generation, Err(error));
        assert_eq!(model.phase(), Phase::SignIn { expired: true });
        assert_eq!(model.account().signed_in_as, None);
        // The disclosure was already accepted and stays so.
        assert!(model.account().disclosed);
        assert_eq!(
            model.turns()[0].state,
            TurnState::Failed(Problem::SessionEnded)
        );
        // Signed in again, the question is there to try again.
        model.sync_account(signed_in());
        assert_eq!(model.phase(), Phase::Chat);
        assert!(model.retry().is_some());
    }
}

#[test]
fn the_daily_limit_ends_the_chat_for_today() {
    let mut model = chat(1);
    let out = model.start("rent?").unwrap();
    model.finished(
        out.generation,
        Err(AskError::DailyLimit {
            used: 20,
            limit: 20,
            everyone: false,
        }),
    );
    assert_eq!(
        model.turns()[0].state,
        TurnState::Limit {
            limit: 20,
            everyone: false
        }
    );
    assert_eq!(model.blocked("more?"), Some(Blocked::Limited));
}

#[test]
fn every_failure_has_words_in_both_languages() {
    for problem in [
        Problem::WrongLogin,
        Problem::EmailTaken,
        Problem::WeakPassword,
        Problem::NeedsConfirmation,
        Problem::Network,
        Problem::Service,
        Problem::TooLarge,
        Problem::SessionEnded,
    ] {
        let (en, es) = (
            problem_text(problem, Lang::En.strings()),
            problem_text(problem, Lang::Es.strings()),
        );
        assert!(!en.is_empty(), "{problem:?}");
        assert_ne!(en, es, "{problem:?}");
    }
}

#[test]
fn errors_map_to_what_the_person_can_do() {
    assert_eq!(
        problem_of(&AskError::InvalidCredentials),
        Problem::WrongLogin
    );
    assert_eq!(problem_of(&AskError::EmailTaken), Problem::EmailTaken);
    assert_eq!(
        problem_of(&AskError::WeakPassword("short".into())),
        Problem::WeakPassword
    );
    assert_eq!(
        problem_of(&AskError::ConfirmationRequired),
        Problem::NeedsConfirmation
    );
    assert_eq!(problem_of(&AskError::TooLarge), Problem::TooLarge);
    assert_eq!(problem_of(&AskError::Network("x".into())), Problem::Network);
    assert_eq!(problem_of(&AskError::Upstream), Problem::Service);
    assert_eq!(problem_of(&AskError::NotConfigured), Problem::Service);
    assert_eq!(
        problem_of(&AskError::Rejected("x".into())),
        Problem::Service
    );
    assert_eq!(problem_of(&AskError::SessionExpired), Problem::SessionEnded);
}

// --- One window at a time ---

#[test]
fn asking_from_a_new_search_starts_a_new_chat() {
    let mut model = chat(2);
    let out = model.start("rent?").unwrap();
    model.finished(out.generation, Ok(answer(&["1"])));

    let results = vec![result(7, Kind::Pdf, Some("a new excerpt"))];
    model.replace(request("deposit", &results));
    assert_eq!(model.phase(), Phase::Chat);
    assert!(model.turns().is_empty());
    assert_eq!(model.question(), "deposit");
    assert_eq!(model.sources().len(), 1);
    assert_eq!(model.sources()[0].file_id, 7);
}

#[test]
fn an_answer_to_the_old_search_is_dropped_after_a_replace() {
    let mut model = chat(2);
    let out = model.start("rent?").unwrap();
    model.replace(request("deposit", &texts(1)));
    model.finished(out.generation, Ok(answer(&["1"])));
    assert!(model.turns().is_empty());
}

#[test]
fn a_replace_while_signed_out_waits_at_sign_in() {
    let mut model = AskModel::new(AccountState::default(), request("rent", &texts(1)));
    model.replace(request("deposit", &texts(2)));
    assert_eq!(model.phase(), Phase::SignIn { expired: false });
    assert_eq!(model.question(), "deposit");
    model.sync_account(signed_in());
    assert_eq!(model.phase(), Phase::Chat);
    assert_eq!(model.sources().len(), 2);
}

#[test]
fn signing_out_in_settings_returns_an_open_window_to_sign_in() {
    let mut model = chat(1);
    let out = model.start("rent?").unwrap();
    model.sync_account(AccountState {
        signed_in_as: None,
        disclosed: true,
    });
    assert_eq!(model.phase(), Phase::SignIn { expired: false });
    // Whatever was in flight no longer counts, and is not left spinning.
    assert!(model.turns().is_empty());
    model.finished(out.generation, Ok(answer(&["1"])));
    assert!(model.turns().is_empty());
}

#[test]
fn the_same_account_again_changes_nothing() {
    let mut model = chat(1);
    let out = model.start("rent?").unwrap();
    model.sync_account(signed_in());
    assert_eq!(model.turns()[0].state, TurnState::Asking);
    model.finished(out.generation, Ok(answer(&[])));
    assert!(matches!(model.turns()[0].state, TurnState::Answered(_)));
}

#[test]
fn an_answer_that_cites_nothing_does_not_point_at_cited_files() {
    for lang in [Lang::En, Lang::Es] {
        let s = lang.strings();
        let none = Answer {
            text: "x".into(),
            cited: vec![],
            used: 1,
            limit: 20,
        };
        let some = Answer {
            cited: vec![0],
            ..none.clone()
        };
        assert_eq!(ai_note(&some, s), s.ask_written_by_ai);
        assert_eq!(ai_note(&none, s), s.ask_written_by_ai_uncited);
        assert_ne!(s.ask_written_by_ai, s.ask_written_by_ai_uncited);
    }
}

mod attach;
