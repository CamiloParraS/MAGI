//! Classifies a file into a [`Kind`] using its extension first, falling
//! back to magic-byte sniffing when the extension is missing or unknown
//! (see SPEC.md §7 M2, §5.5 `files.kind`).

use std::path::Path;

/// Matches the `files.kind` column in `migrations/0001_init.sql`; the serde
/// names are the same strings, used by `indexing.file_types`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Kind {
    Text,
    Code,
    Pdf,
    Office,
    Image,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Code => "code",
            Kind::Pdf => "pdf",
            Kind::Office => "office",
            Kind::Image => "image",
            Kind::Other => "other",
        }
    }
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "log", "csv", "json", "yaml", "yml", "toml", "ini", "cfg",
];

// Extensions for the tree-sitter grammars listed in SPEC.md §3 (Rust,
// Python, JS/TS, Java, C/C++, Go, C#), plus a few common languages without
// a grammar here — those fall back to line-window chunking (see
// `extract::code`) rather than being mis-classified as `Other`.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "mjs", "cjs", "ts", "tsx", "java", "c", "h", "cc", "cpp", "cxx",
    "hpp", "hh", "go", "cs", "rb", "php", "sh", "bash", "kt", "kts", "swift",
];

const PDF_EXTENSIONS: &[&str] = &["pdf"];
const OFFICE_EXTENSIONS: &[&str] = &["docx", "pptx", "xlsx"];
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "tif", "heic", "heif",
];

/// Camera RAW formats. Not supported in v1 (filename-only indexing), but
/// they must be named here: most are TIFF containers, so magic-byte sniffing
/// would otherwise call them images and hand them to a TIFF decoder that
/// cannot read them, turning every RAW file into an index error.
const RAW_EXTENSIONS: &[&str] = &[
    "arw", "cr2", "cr3", "crw", "dng", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "srw",
    "x3f",
];

/// Classifies `path`, using `bytes` (a small header read, not the whole
/// file) for magic-byte sniffing when the extension doesn't resolve.
pub fn classify(path: &Path, bytes: &[u8]) -> Kind {
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && let Some(kind) = classify_extension(&ext.to_ascii_lowercase())
    {
        return kind;
    }
    if let Some(info) = infer::get(bytes)
        && let Some(kind) = classify_extension(info.extension())
    {
        return kind;
    }
    Kind::Other
}

fn classify_extension(ext: &str) -> Option<Kind> {
    if TEXT_EXTENSIONS.contains(&ext) {
        Some(Kind::Text)
    } else if CODE_EXTENSIONS.contains(&ext) {
        Some(Kind::Code)
    } else if PDF_EXTENSIONS.contains(&ext) {
        Some(Kind::Pdf)
    } else if OFFICE_EXTENSIONS.contains(&ext) {
        Some(Kind::Office)
    } else if IMAGE_EXTENSIONS.contains(&ext) {
        Some(Kind::Image)
    } else if RAW_EXTENSIONS.contains(&ext) {
        // `Some`, so `classify` stops here instead of sniffing.
        Some(Kind::Other)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(classify(&PathBuf::from("notes.txt"), b""), Kind::Text);
        assert_eq!(classify(&PathBuf::from("main.rs"), b""), Kind::Code);
        assert_eq!(classify(&PathBuf::from("report.pdf"), b""), Kind::Pdf);
        assert_eq!(classify(&PathBuf::from("sheet.xlsx"), b""), Kind::Office);
        assert_eq!(classify(&PathBuf::from("photo.png"), b""), Kind::Image);
        assert_eq!(classify(&PathBuf::from("archive.zip"), b""), Kind::Other);
    }

    #[test]
    fn camera_raw_is_not_sniffed_into_an_image() {
        // A TIFF header, which is what an ARW/CR2/NEF starts with.
        let tiff = b"II*\x00\x08\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        for name in ["shot.arw", "SHOT.CR2", "shot.nef", "shot.dng"] {
            assert_eq!(classify(&PathBuf::from(name), tiff), Kind::Other, "{name}");
        }
        // The same bytes with no extension really are a TIFF.
        assert_eq!(classify(&PathBuf::from("shot"), tiff), Kind::Image);
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(classify(&PathBuf::from("NOTES.TXT"), b""), Kind::Text);
        assert_eq!(classify(&PathBuf::from("Main.RS"), b""), Kind::Code);
    }

    #[test]
    fn falls_back_to_magic_bytes_when_extension_is_missing() {
        let pdf_bytes = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n1 0 obj\n";
        assert_eq!(classify(&PathBuf::from("mystery"), pdf_bytes), Kind::Pdf);

        let png_bytes: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert_eq!(classify(&PathBuf::from("noext"), png_bytes), Kind::Image);
    }

    #[test]
    fn unknown_extension_and_unrecognized_bytes_is_other() {
        assert_eq!(
            classify(&PathBuf::from("data.xyz123"), b"not a known signature"),
            Kind::Other
        );
        assert_eq!(classify(&PathBuf::from("noext"), b""), Kind::Other);
    }
}
