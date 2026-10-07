use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, TestAppContext};
use magi_core::config::Onboarding;
use magi_core::features::Feature;
use magi_core::host::{Host, HostPaths};
use magi_desktop::app::AppEvent;
use magi_desktop::i18n::Lang;
use magi_desktop::onboarding::{OnboardingView, Step};
use magi_desktop::search::view::Live;

#[gpui_kit::test]
fn onboarding_saves_the_chosen_features_and_the_login_choice(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary that reads these variables.
    unsafe {
        std::env::set_var("MAGI_DATA_DIR", dir.path());
        std::env::set_var("MAGI_FAKE_EMBEDDER", "1");
    }
    let host = Host::start(
        HostPaths {
            db: dir.path().join("magi.db"),
            config: dir.path().join("config.toml"),
        },
        |_| {},
    )
    .unwrap();
    let started = Instant::now();
    while host.engine().is_err() {
        assert!(started.elapsed() < Duration::from_secs(30), "engine start");
        std::thread::sleep(Duration::from_millis(50));
    }
    host.engine().unwrap().add_root(root.path()).unwrap();
    cx.update(gpui_kit::init);
    let (tx, rx) = async_channel::unbounded();
    let live = cx.new(|_| {
        let mut live = Live::new(Lang::En, "system".into());
        live.features = host.features().status().unwrap();
        live
    });
    let open = |cx: &mut TestAppContext| {
        let (host, live, tx) = (host.clone(), live.clone(), tx.clone());
        cx.add_window(|window, cx| OnboardingView::new(host, live, tx, window, cx))
    };
    let saved = || host.settings().unwrap().ui.onboarding;
    // A step saved by the old four-screen flow resumes on the one setup screen.
    host.update_settings(&serde_json::json!({ "ui": { "onboarding": Onboarding::Features } }))
        .unwrap();
    let window = open(cx);
    let step = |cx: &mut TestAppContext| window.update(cx, |view, _, _| view.step()).unwrap();
    assert_eq!(step(cx), Step::Setup);

    // One confirm applies everything: unchecking both defaults means keyword
    // search only and nothing downloaded; "Start with your computer" is on by
    // default and the shell applies it.
    assert!(!host.settings().unwrap().ui.launch_at_login);
    window
        .update(cx, |view, w, cx| {
            view.toggle_feature(Feature::Meaning, false, cx);
            view.toggle_feature(Feature::ImageText, false, cx);
            view.next(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(step(cx), Step::Indexing);
    let features = host.settings().unwrap().features;
    assert!(!features.meaning && !features.image_text && !features.image_visual);
    // Every saved step reaches the shell; the last one finishes onboarding.
    let last = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            AppEvent::Ui(ui) => Some(ui),
            _ => None,
        })
        .last()
        .expect("the shell got no ui settings");
    assert!(last.launch_at_login);
    assert_eq!(last.onboarding, Onboarding::Done);
    assert_eq!(saved(), Onboarding::Done);

    // The finish step's Change: registered by the shell, then saved.
    let key = gpui_kit::Keystroke::parse("ctrl-alt-j").unwrap();
    window
        .update(cx, |view, w, cx| view.record(&key, w, cx))
        .unwrap();
    cx.run_until_parked();
    let Ok(AppEvent::Hotkey(spec, reply)) = rx.try_recv() else {
        panic!("the shell was not asked to register it");
    };
    assert_eq!(spec, "Ctrl+Alt+J");
    reply.try_send(true).unwrap();
    cx.run_until_parked();
    assert_eq!(host.settings().unwrap().ui.hotkey, "Ctrl+Alt+J");
    host.shutdown();
}
