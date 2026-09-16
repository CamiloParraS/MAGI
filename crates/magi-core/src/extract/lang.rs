//! Per-file language detection, stored as ISO 639-1 (see SPEC.md §5.5
//! `files.lang`). `whatlang` reports ISO 639-3; `isolang` maps that down to
//! ISO 639-1 for the languages that have one.

/// Detects the dominant language of `text`, returning its ISO 639-1 code
/// (e.g. `"en"`, `"es"`). Returns `None` when the text is too short/mixed
/// for a confident guess, or when the detected language has no ISO 639-1
/// code.
pub fn detect_lang(text: &str) -> Option<String> {
    let info = whatlang::detect(text)?;
    let code_639_3 = info.lang().code();
    let language = isolang::Language::from_639_3(code_639_3)?;
    language.to_639_1().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_english() {
        let text = "The quick brown fox jumps over the lazy dog near the riverbank every morning.";
        assert_eq!(detect_lang(text), Some("en".to_string()));
    }

    #[test]
    fn detects_spanish() {
        let text = "El veloz murciélago hindú comía feliz cardillo y kiwi en la página web.";
        assert_eq!(detect_lang(text), Some("es".to_string()));
    }

    #[test]
    fn empty_text_has_no_detected_language() {
        assert_eq!(detect_lang(""), None);
    }
}
