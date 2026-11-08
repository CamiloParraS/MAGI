//! File names that say nothing: a text embedding of `IMG_0494.JPG` lands near
//! short queries by chance, and RRF then rewards the file for being in a
//! second list (`docs/eval.md`, 2026-11-08).

/// Prefixes cameras and phones write, which read as words but mean nothing.
const CAMERA_PREFIXES: &[&str] = &["img", "dsc", "dscn", "dscf", "pxl", "pict", "mvimg", "vid"];

/// Whether `file_name` has no word in it: digits, hashes, UUIDs and camera
/// names. Generic words (`Screenshot`, `Untitled`) count as words on purpose:
/// a list of them would have to guess which ones a user's files really mean.
pub(super) fn is_opaque_name(file_name: &str) -> bool {
    let stem = match file_name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file_name,
    };
    let stem = stem.to_lowercase();
    let has_digit = stem.chars().any(|c| c.is_ascii_digit());
    !stem
        .split(|c: char| !c.is_alphabetic())
        .any(|run| is_word(run, has_digit))
}

/// `in_digits`: the name has digits, so a run of hex letters is a hash's.
fn is_word(run: &str, in_digits: bool) -> bool {
    let long_enough = run.chars().count() >= 3 || (!run.is_empty() && !run.is_ascii());
    let hash = in_digits && run.chars().all(|c| c.is_ascii_hexdigit());
    long_enough && !hash && !CAMERA_PREFIXES.contains(&run)
}

#[cfg(test)]
mod tests {
    use super::is_opaque_name;

    #[test]
    fn names_with_no_word_are_opaque() {
        for name in [
            "IMG_0494.JPG",
            "IMG-20240101-WA0001.jpg",
            "DSC_1234.JPG",
            "PXL_20240726_203533123.jpg",
            "5f705a2e5458b0fe.pdf",
            "123e4567-e89b-12d3-a456-426614174000.png",
            "20240726_203533.jpg",
            "8.jpg",
        ] {
            assert!(is_opaque_name(name), "{name}");
        }
    }

    #[test]
    fn names_with_a_word_are_not() {
        for name in [
            "invoice.pdf",
            "IMG_beach.jpg",
            "Screenshot_20240726-203533.png",
            "Untitled-6.txt",
            "New Text Document (2).txt",
            "canción.mp3",
        ] {
            assert!(!is_opaque_name(name), "{name}");
        }
    }

    /// `dad` and `cafe` are made of hex digits, but a hash has digits too.
    #[test]
    fn a_word_spelled_in_hex_letters_is_still_a_word() {
        for name in ["dad.jpg", "cafe.txt", "FACE.png"] {
            assert!(!is_opaque_name(name), "{name}");
        }
    }

    /// Chinese and Japanese words are often two characters.
    #[test]
    fn short_non_latin_words_count() {
        assert!(!is_opaque_name("报告.pdf"));
    }

    #[test]
    fn a_dotfile_is_named_by_what_follows_the_dot() {
        assert!(!is_opaque_name(".gitignore"));
    }
}
