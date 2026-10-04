use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, TestAppContext};
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
    let window =
        cx.add_window(|window, cx| OnboardingView::new(host.clone(), live, tx, window, cx));
    let step = |cx: &mut TestAppContext| window.update(cx, |view, _, _| view.step()).unwrap();
    assert_eq!(step(cx), Step::Folders);
    window.update(cx, |view, w, cx| view.next(w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(step(cx), Step::Features);

    // Unchecking both defaults: keyword search only, nothing downloaded.
    window
        .update(cx, |view, w, cx| {
            view.toggle_feature(Feature::Meaning, false, cx);
            view.toggle_feature(Feature::ImageText, false, cx);
            view.next(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(step(cx), Step::Background);
    let features = host.settings().unwrap().features;
    assert!(!features.meaning && !features.image_text && !features.image_visual);

    // "Start with your computer" is checked by default; the shell applies it.
    assert!(!host.settings().unwrap().ui.launch_at_login);
    window.update(cx, |view, w, cx| view.next(w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(step(cx), Step::Indexing);
    match rx.try_recv() {
        Ok(AppEvent::Ui(ui)) => assert!(ui.launch_at_login),
        _ => panic!("the shell got no ui settings"),
    }
    assert!(host.settings().unwrap().ui.launch_at_login);
    host.shutdown();
}
