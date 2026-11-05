use super::*;

#[test]
fn a_held_session_lock_fails_fast_instead_of_blocking() {
    let m = Mutex::new(());
    let held = lock(&m, "detector").unwrap();
    assert!(matches!(lock(&m, "detector"), Err(Error::Model(_))));
    drop(held);
    assert!(lock(&m, "detector").is_ok());
}

#[test]
fn dict_parses_quoting_styles() {
    let yml = "A:\n  b: 1\nPostProcess:\n  character_dict:\n  - '0'\n  - a\n  - ''''\n  - ' '\n  - \"\\\\\"\n  - 'á'\nnext: 1\n";
    assert_eq!(parse_dict(yml), ["0", "a", "'", " ", "\\", "á"]);
}

#[test]
fn ctc_collapses_repeats_and_drops_blanks() {
    let classes: Vec<String> = ["", "a", "b"].map(String::from).to_vec();
    // a a _ a b b  ->  "aab": the blank separates the two real 'a's.
    assert_eq!(ctc_decode([1, 1, 0, 1, 2, 2].into_iter(), &classes), "aab");
}

#[test]
fn detects_a_tilted_bar_and_ignores_specks() {
    let (w, h) = (200, 100);
    let mut prob = vec![0f32; w * h];
    // A 150 x 6 bar tilted about 5 degrees (slope 0.09), then a speck.
    for x in 20..170 {
        let top = 40 + ((x - 20) as f32 * 0.09) as usize;
        for y in top..top + 6 {
            prob[y * w + x] = 0.9;
        }
    }
    prob[2 * w + 2] = 0.9;
    let boxes = boxes_from_map(&prob, w, h);
    assert_eq!(boxes.len(), 1);
    let b = &boxes[0];
    // Long side runs along the text, at roughly the tilt angle, and is
    // not inflated by the tilt the way an axis-aligned box would be.
    assert!(b.w > 150.0 && b.h < 30.0, "w={} h={}", b.w, b.h);
    assert!((b.uy.atan2(b.ux) - 0.09).abs() < 0.03);
}

#[test]
fn lines_group_by_row_and_read_left_to_right() {
    let line = |x: f32, cy: f32| TextLine {
        origin: [x, cy - 5.0],
        du: [40.0, 0.0],
        dv: [0.0, 10.0],
        cy,
        height: 10.0,
    };
    let rows = group_lines(vec![line(50.0, 5.0), line(0.0, 6.0), line(0.0, 35.0)]);
    let xs: Vec<Vec<f32>> = rows
        .iter()
        .map(|r| r.iter().map(|l| l.origin[0]).collect())
        .collect();
    assert_eq!(xs, vec![vec![0.0, 50.0], vec![0.0]]);
}

/// Needs `just models` (ocr slot) and the vendored ONNX Runtime.
#[test]
#[ignore = "requires `just models` and `cargo xtask fetch-onnxruntime`"]
fn reads_spanish_screenshot_with_accents() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/images/screenshot_es.png");
    let img = image::open(path).expect("fixture").to_rgb8();
    let text = PaddleOcr::load().expect("load").recognize(&img).unwrap();
    for want in [
        "¿Qué son las letras con tilde?",
        "español",
        "sílaba",
        "pingüino",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
}
