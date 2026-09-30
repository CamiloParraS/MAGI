use std::time::Instant;

use gpui_kit::{AppContext as _, TestAppContext};
use magi_core::host::{Host, HostPaths};
use magi_desktop::i18n::Lang;
use magi_desktop::search::view::{Live, SearchView};
use magi_desktop::theme::Backdrop;

struct Counter(u32);

#[gpui_kit::test]
fn the_gpui_test_platform_runs_headless(cx: &mut TestAppContext) {
    let counter = cx.new(|_| Counter(1));
    counter.update(cx, |c, _| c.0 += 1);
    assert_eq!(counter.read_with(cx, |c, _| c.0), 2);
}

#[gpui_kit::test]
fn the_search_window_opens_empty_over_a_real_host(cx: &mut TestAppContext) {
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
    cx.update(|cx| {
        gpui_kit::init(cx);
        SearchView::bind_keys(cx);
    });
    let live = cx.new(|_| Live::new(Lang::En, "system".into()));
    let window = cx.add_window(|window, cx| {
        SearchView::new(
            host.clone(),
            live,
            async_channel::unbounded().0,
            Backdrop::Solid,
            Instant::now(),
            window,
            cx,
        )
    });
    window
        .update(cx, |view, _, _| {
            assert!(view.state().results().is_empty());
            assert_eq!(view.state().selected_index(), None);
        })
        .unwrap();
    host.shutdown();
}
