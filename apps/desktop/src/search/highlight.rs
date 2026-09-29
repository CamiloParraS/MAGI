//! Snippet highlight ranges → GPUI highlight runs.

use std::ops::Range;

use gpui_kit::HighlightStyle;

/// `highlights` are byte ranges from `magi_core::dto::Snippet`. Ends past the
/// text are clamped; ranges that are empty or not on char boundaries are
/// dropped, so a bad range never panics the renderer.
pub fn runs(
    text: &str,
    highlights: &[[u32; 2]],
    style: HighlightStyle,
) -> Vec<(Range<usize>, HighlightStyle)> {
    highlights
        .iter()
        .map(|&[start, end]| (start as usize)..(end as usize).min(text.len()))
        .filter(|r| {
            r.start < r.end && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)
        })
        .map(|r| (r, style))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_map_to_runs_with_the_style() {
        let style = HighlightStyle::default();
        let got = runs("rent payment", &[[0, 4], [5, 12]], style);
        assert_eq!(
            got.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            vec![0..4, 5..12]
        );
    }

    #[test]
    fn ranges_past_the_end_or_inside_a_char_are_clamped_or_dropped() {
        let style = HighlightStyle::default();
        // "é" is 2 bytes: 1..2 cuts it; 3..99 is clamped to the end (6).
        let got = runs("éabcd", &[[1, 2], [3, 99], [4, 4]], style);
        assert_eq!(
            got.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            vec![3..6]
        );
    }
}
