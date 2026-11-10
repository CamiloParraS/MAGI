//! The Ask window end to end on the test platform, against the in-process
//! fake server (`MAGI_FAKE_ASK=1`): no account and no network.

use std::time::Instant;

use async_channel::Receiver;
use gpui_kit::{AppContext as _, Modifiers, TestAppContext, VisualTestContext, WeakEntity};
use magi_core::discovery::Kind;
use magi_core::dto::{MatchSource, SearchResponse, SearchResult, Snippet};
use magi_core::host::{Host, HostPaths};
use magi_core::search::strength::Strength;
use magi_desktop::app::{AppEvent, AskSlot, show_ask};
use magi_desktop::ask::view::AskView;
use magi_desktop::ask::view::sign_in::SignInForm;
use magi_desktop::ask::{Phase, Problem, Request, TurnState, request};
use magi_desktop::i18n::Lang;
use magi_desktop::live::Live;
use magi_desktop::search::view::{Opening, SearchView};
use magi_desktop::theme::Backdrop;

fn hit(file_id: i64) -> SearchResult {
    SearchResult {
        file_id,
        path: format!("C:\\docs\\{file_id}.txt"),
        file_name: format!("{file_id}.txt"),
        kind: Kind::Text,
        score: 1.0,
        snippet: Some(Snippet {
            text: "the tenant shall pay rent".into(),
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

fn start_host(dir: &std::path::Path) -> Host {
    // SAFETY: every test in this binary sets the same values before reading.
    unsafe {
        std::env::set_var("MAGI_DATA_DIR", dir);
        std::env::set_var("MAGI_FAKE_EMBEDDER", "1");
        std::env::set_var("MAGI_FAKE_ASK", "1");
    }
    Host::start(
        HostPaths {
            db: dir.join("magi.db"),
            config: dir.join("config.toml"),
        },
        |_| {},
    )
    .unwrap()
}

fn asked(rx: &Receiver<AppEvent>) -> Option<Request> {
    match rx.try_recv() {
        Ok(AppEvent::Ask(request)) => Some(request),
        _ => None,
    }
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn every_entry_point_opens_ask_in_both_layouts(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let host = start_host(dir.path());
    cx.update(|cx| {
        gpui_kit::init(cx);
        SearchView::bind_keys(cx);
    });
    for detail_pane in [false, true] {
        host.update_settings(&serde_json::json!({ "ui": { "detail_pane": detail_pane } }))
            .unwrap();
        let (tx, rx) = async_channel::unbounded();
        let live = cx.new(|_| Live::new(Lang::En, "system".into()));
        let window = cx.add_window(|window, cx| {
            SearchView::new(
                host.clone(),
                live,
                tx,
                Backdrop::Solid,
                Opening {
                    at: Instant::now(),
                    teach: None,
                },
                window,
                cx,
            )
        });
        // Nothing to ask about yet: the shortcut does nothing.
        let mut vcx = VisualTestContext::from_window(*window, cx);
        vcx.run_until_parked();
        vcx.simulate_keystrokes("alt-enter");
        assert!(asked(&rx).is_none(), "asked with no results");

        // "Nothing close", folded away by the user: nothing is on screen to
        // ask about.
        window
            .update(&mut vcx, |view, _, cx| {
                let query = view.state_mut().set_query("zzz").unwrap();
                let weak = |id| SearchResult {
                    strength: Strength::Weak,
                    ..hit(id)
                };
                view.state_mut().apply(
                    query.generation,
                    Ok(SearchResponse {
                        results: vec![weak(1), weak(2)],
                        took_ms: 1,
                    }),
                );
                // They open on their own; the user folds them away.
                view.state_mut().toggle_weak();
                cx.notify();
            })
            .unwrap();
        vcx.run_until_parked();
        vcx.simulate_keystrokes("alt-enter");
        assert!(asked(&rx).is_none(), "asked with only folded results");
        click("ask-footer", &mut vcx);
        assert!(asked(&rx).is_none(), "the footer button is off too");

        window
            .update(&mut vcx, |view, _, cx| {
                let query = view.state_mut().set_query("rent").unwrap();
                view.state_mut().apply(
                    query.generation,
                    Ok(SearchResponse {
                        results: vec![hit(1), hit(2)],
                        took_ms: 1,
                    }),
                );
                cx.notify();
            })
            .unwrap();
        vcx.run_until_parked();

        vcx.simulate_keystrokes("alt-enter");
        let request = asked(&rx).expect("Alt+Enter asks");
        assert_eq!(request.question, "rent");
        assert_eq!(request.sources.len(), 2);

        click("ask-footer", &mut vcx);
        assert!(asked(&rx).is_some(), "the footer button asks");

        if detail_pane {
            click("ask-pane", &mut vcx);
            assert!(asked(&rx).is_some(), "the pane button asks");
        } else {
            assert!(
                vcx.debug_bounds("ask-pane").is_none(),
                "the pane button belongs to the detail pane"
            );
        }
        window
            .update(&mut vcx, |_, window, _| window.remove_window())
            .unwrap();
        vcx.run_until_parked();
    }
    host.shutdown();
}

#[gpui_kit::test]
fn asking_twice_keeps_one_window_with_the_new_contents(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let host = start_host(dir.path());
    cx.update(gpui_kit::init);
    let live = cx.new(|_| Live::new(Lang::En, "system".into()));
    let mut slot: AskSlot = None;

    cx.update(|cx| {
        show_ask(
            &mut slot,
            &host,
            &live,
            Backdrop::Solid,
            request("rent", &[hit(1)]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    let first: WeakEntity<AskView> = slot.as_ref().unwrap().1.clone();

    cx.update(|cx| {
        show_ask(
            &mut slot,
            &host,
            &live,
            Backdrop::Solid,
            request("deposit", &[hit(2), hit(3)]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1, "still one window");
    let view = first.upgrade().expect("the same window was reused");
    view.read_with(cx, |view, _| {
        assert_eq!(view.model().question(), "deposit");
        assert_eq!(view.model().sources().len(), 2);
    });

    // Closed by the user: the next ask opens a new one.
    let (handle, _) = slot.as_ref().unwrap().clone();
    handle
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        show_ask(
            &mut slot,
            &host,
            &live,
            Backdrop::Solid,
            request("lease", &[hit(4)]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    let reopened = slot.as_ref().unwrap().1.upgrade().unwrap();
    reopened.read_with(cx, |view, _| {
        assert_eq!(view.model().question(), "lease");
    });
    host.shutdown();
}

/// With no such account, a wrong-login answer proves that both fields were
/// filled and that Enter submitted them.
#[gpui_kit::test]
fn sign_in_works_from_the_keyboard_alone(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let host = start_host(dir.path());
    cx.update(gpui_kit::init);
    cx.update(AskView::bind_keys);
    let live = cx.new(|_| Live::new(Lang::En, "system".into()));
    let mut slot: AskSlot = None;
    cx.update(|cx| {
        show_ask(
            &mut slot,
            &host,
            &live,
            Backdrop::Solid,
            request("rent", &[hit(1)]),
            cx,
        );
    });
    cx.run_until_parked();
    let (handle, view) = slot.as_ref().unwrap().clone();
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();

    vcx.simulate_input("a@b.co");
    vcx.simulate_keystrokes("tab");
    vcx.simulate_input("secret-pass");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    let view = view.upgrade().unwrap();
    view.read_with(&vcx, |view, cx| {
        assert_eq!(view.model().phase(), Phase::SignIn { expired: false });
        let form = view.sign_in_form().read(cx);
        assert_eq!(form.problem(), Some(Problem::WrongLogin));
        assert!(!form.working());
    });
    host.shutdown();
}

/// Settings › Account's form signs in for every window, the Ask window too.
#[gpui_kit::test]
fn signing_in_from_settings_opens_the_chat(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let host = start_host(dir.path());
    cx.update(gpui_kit::init);
    let live = cx.new(|_| Live::new(Lang::En, "system".into()));
    let ask = cx.add_window({
        let (host, live) = (host.clone(), live.clone());
        move |window, cx| {
            AskView::new(
                host,
                live,
                Backdrop::Solid,
                request("rent", &[hit(1)]),
                window,
                cx,
            )
        }
    });
    let settings_form = cx.add_window({
        let (host, live) = (host.clone(), live.clone());
        move |window, cx| SignInForm::new(host, live, window, cx)
    });
    settings_form
        .update(cx, |form, window, cx| {
            form.authenticate("a@b.co".into(), "secret-pass".into(), true, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        live.read_with(cx, |live, _| live.account.signed_in_as.clone()),
        Some("a@b.co".to_owned())
    );
    let phase = ask.update(cx, |view, _, _| view.model().phase()).unwrap();
    assert_eq!(phase, Phase::Chat);
    host.shutdown();
}

#[gpui_kit::test]
fn the_whole_chat_runs_against_the_fake_server(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let host = start_host(dir.path());
    cx.update(gpui_kit::init);
    let live = cx.new(|_| Live::new(Lang::En, "system".into()));
    let the_live = live.clone();
    let window = cx.add_window(|window, cx| {
        AskView::new(
            host.clone(),
            live,
            Backdrop::Solid,
            request("rent", &[hit(1), hit(2)]),
            window,
            cx,
        )
    });
    let read = |cx: &mut TestAppContext| {
        window
            .update(cx, |view, _, _| {
                (
                    view.model().phase(),
                    view.model().turns().to_vec(),
                    view.model().needs_disclosure(),
                )
            })
            .unwrap()
    };
    assert!(matches!(read(cx).0, Phase::SignIn { .. }));

    window
        .update(cx, |view, window, cx| {
            view.sign_in_form().clone().update(cx, |form, cx| {
                form.authenticate("a@b.co".into(), "secret-pass".into(), true, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let (phase, turns, needs_disclosure) = read(cx);
    assert_eq!(phase, Phase::Chat);
    assert!(turns.is_empty());
    assert!(
        needs_disclosure,
        "shown above the box before the first question"
    );
    assert_eq!(
        the_live.read_with(cx, |live, _| live.account.signed_in_as.clone()),
        Some("a@b.co".to_owned()),
        "Settings sees who signed in"
    );

    // The box opens with the search text: Enter or Send asks it.
    window
        .update(cx, |view, window, cx| view.send(window, cx))
        .unwrap();
    cx.run_until_parked();
    let (_, turns, needs_disclosure) = read(cx);
    assert!(!needs_disclosure);
    assert!(
        host.account_state().unwrap().disclosed,
        "the first question saved it"
    );
    assert_eq!(turns.len(), 1);
    let TurnState::Answered(answer) = &turns[0].state else {
        panic!("no answer: {:?}", turns[0].state);
    };
    assert_eq!(answer.text, "Fake answer to: rent");
    assert_eq!(answer.cited, [0]);
    assert_eq!((answer.used, answer.limit), (1, 20));

    // A follow-up is a second turn under the first.
    window
        .update(cx, |view, window, cx| {
            view.set_question("and the deposit?", window, cx);
            view.send(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let (_, turns, _) = read(cx);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].question, "and the deposit?");
    assert!(matches!(turns[1].state, TurnState::Answered(_)));
    host.shutdown();
}
