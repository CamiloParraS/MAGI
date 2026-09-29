use gpui_kit::{AppContext as _, TestAppContext};

struct Counter(u32);

#[gpui_kit::test]
fn the_gpui_test_platform_runs_headless(cx: &mut TestAppContext) {
    let counter = cx.new(|_| Counter(1));
    counter.update(cx, |c, _| c.0 += 1);
    assert_eq!(counter.read_with(cx, |c, _| c.0), 2);
}
