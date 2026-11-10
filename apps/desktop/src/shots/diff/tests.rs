use image::{Rgba, RgbaImage};

use super::*;

fn plain(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba([240, 240, 240, 255]))
}

#[test]
fn the_same_picture_has_no_changed_pixels() {
    let c = compare(&plain(4, 3), &plain(4, 3));
    assert_eq!((c.changed, c.total, c.resized), (0, 12, false));
}

#[test]
fn a_changed_pixel_is_counted_and_marked() {
    let mut after = plain(4, 3);
    after.put_pixel(2, 1, Rgba([0, 0, 0, 255]));
    // Below the threshold: antialiasing noise, not a change.
    after.put_pixel(0, 0, Rgba([236, 240, 240, 255]));
    let c = compare(&plain(4, 3), &after);
    assert_eq!(c.changed, 1);
    assert_eq!(*c.image.get_pixel(2, 1), MARK);
    assert_ne!(*c.image.get_pixel(0, 0), MARK);
}

#[test]
fn a_resized_screen_counts_as_wholly_changed() {
    let c = compare(&plain(4, 3), &plain(4, 5));
    assert!(c.resized);
    assert_eq!((c.changed, c.total), (20, 20));
    assert_eq!(c.image.dimensions(), (4, 5));
}

#[test]
fn the_report_lists_changes_first_then_new_gone_and_same() {
    let entries = vec![
        Entry::same("a-same"),
        Entry::changed("b-small", 1, 100, false),
        Entry::new_shot("c-new"),
        Entry::changed("d-big", 50, 100, false),
        Entry::gone("e-<gone>"),
    ];
    let html = report(&entries);
    let at = |name: &str| html.find(name).unwrap_or_else(|| panic!("{name} missing"));
    assert!(at("d-big") < at("b-small"));
    assert!(at("b-small") < at("c-new"));
    assert!(at("c-new") < at("e-&lt;gone&gt;"));
    assert!(at("e-&lt;gone&gt;") < at("a-same"));
    assert!(html.contains("2 changed"), "{html}");
}

#[test]
fn a_blinking_caret_comes_out_the_same_whichever_phase_was_caught() {
    let mut caret = plain(4, 3);
    caret.put_pixel(1, 1, Rgba([20, 20, 20, 255]));
    let a = steady(&[plain(4, 3), caret.clone(), caret.clone()]);
    let b = steady(&[caret.clone(), plain(4, 3), plain(4, 3)]);
    assert_eq!(a, b);
    assert_eq!(steady(std::slice::from_ref(&caret)), caret);
}
