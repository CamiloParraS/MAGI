use gpui_kit::{AppContext as _, TestAppContext};
use magi_core::config::Language;
use magi_core::host::{Host, HostPaths};
use magi_desktop::app::AppEvent;
use magi_desktop::i18n::Lang;
use magi_desktop::search::view::Live;
use magi_desktop::settings::view::{Field, SettingsView};

#[gpui_kit::test]
fn a_saved_setting_reaches_the_shell_and_a_rejected_one_its_box(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
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
    cx.update(gpui_kit::init);
    let (tx, rx) = async_channel::unbounded();
    let live = cx.new(|_| Live::new(Lang::En, "system".into()));
    let window = cx.add_window(|window, cx| SettingsView::new(host.clone(), live, tx, window, cx));
    window
        .update(cx, |view, window, cx| {
            view.save(
                Field::Language,
                serde_json::json!({ "ui": { "language": "es" } }),
                window,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    match rx.try_recv() {
        Ok(AppEvent::Ui(ui)) => assert_eq!(ui.language, Language::Es),
        _ => panic!("the shell got no ui settings"),
    }
    assert_eq!(host.settings().unwrap().ui.language, Language::Es);

    // A change the core rejects is shown in its own box and not saved.
    let globs = host.settings().unwrap().indexing.exclude_globs;
    window
        .update(cx, |view, window, cx| {
            view.save(
                Field::Excludes,
                serde_json::json!({ "indexing": { "exclude_globs": ["**/[bad"] } }),
                window,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    let failed = window.update(cx, |view, _, _| view.failed()).unwrap();
    assert_eq!(failed, Some(Field::Excludes));
    assert_eq!(host.settings().unwrap().indexing.exclude_globs, globs);
    assert!(rx.try_recv().is_err(), "nothing to apply");

    // The hotkey recorder: the shell registers a shortcut before it is saved.
    let record = |key: &str, cx: &mut TestAppContext| {
        let key = gpui_kit::Keystroke::parse(key).unwrap();
        window
            .update(cx, |view, window, cx| view.record(&key, window, cx))
            .unwrap();
        cx.run_until_parked();
        rx.try_recv().ok()
    };
    assert!(record("escape", cx).is_none());
    assert!(record("shift-k", cx).is_none(), "not a shortcut");
    let Some(AppEvent::Hotkey(spec, reply)) = record("ctrl-alt-k", cx) else {
        panic!("the shell was not asked to register it");
    };
    assert_eq!(spec, "Ctrl+Alt+K");
    let default = host.settings().unwrap().ui.hotkey;
    reply.try_send(false).unwrap();
    cx.run_until_parked();
    assert_eq!(
        host.settings().unwrap().ui.hotkey,
        default,
        "taken: not saved"
    );
    let Some(AppEvent::Hotkey(_, reply)) = record("ctrl-alt-k", cx) else {
        panic!("no second try");
    };
    reply.try_send(true).unwrap();
    cx.run_until_parked();
    assert_eq!(host.settings().unwrap().ui.hotkey, "Ctrl+Alt+K");
    host.shutdown();
}
