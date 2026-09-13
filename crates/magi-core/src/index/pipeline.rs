//! hash -> extract -> chunk -> embed -> write. Implemented starting M2
//! (see SPEC.md §5.4 processing a pending file).
//!
//! M2 implements a one-shot walk -> classify -> extract -> chunk -> write
//! pass (`index_root`, used by `magi-cli index`). Hashing, the pending
//! state machine, and the worker/scheduler threads are M5.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use rusqlite::Connection;

use crate::db::files::{FileRecord, upsert_file};
use crate::discovery::{self, Kind, WalkOptions};
use crate::error::{Error, Result};
use crate::extract::code::CodeExtractor;
use crate::extract::filename::filename_chunk;
use crate::extract::office::OfficeExtractor;
use crate::extract::pdf::PdfExtractor;
use crate::extract::text::TextExtractor;
use crate::extract::{ExtractedDoc, Extractor, RawChunk};

/// Per-file extraction timeout (see SPEC.md §5.4 step 5).
pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Bytes read from the file head for magic-byte sniffing when the
/// extension doesn't resolve a [`Kind`].
const SNIFF_HEADER_LEN: usize = 8192;

pub struct IndexRootOptions {
    pub exclude_globs: Vec<glob::Pattern>,
    pub include_hidden: bool,
    pub follow_symlinks: bool,
    pub max_file_size_mb: u64,
}

impl IndexRootOptions {
    /// Compiles the config's exclude globs, reusing the same
    /// `glob::Pattern` validation as `Config::validate`.
    pub fn from_config(config: &crate::config::IndexingConfig) -> Result<Self> {
        let exclude_globs = config
            .exclude_globs
            .iter()
            .map(|glob| {
                glob::Pattern::new(glob).map_err(|e| Error::InvalidGlob {
                    glob: glob.clone(),
                    reason: e.to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            exclude_globs,
            include_hidden: config.include_hidden,
            follow_symlinks: config.follow_symlinks,
            max_file_size_mb: config.max_file_size_mb,
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IndexSummary {
    pub indexed: u32,
    pub skipped: u32,
    pub errored: u32,
}

/// Walks `root_path`, extracts and chunks every file, and writes the
/// results to the database. One-shot, no watcher — incremental
/// re-indexing and the pending/indexing state machine land in M5.
pub fn index_root(
    conn: &mut Connection,
    root_id: i64,
    root_path: &Path,
    options: &IndexRootOptions,
    scan_id: i64,
) -> Result<IndexSummary> {
    let walk_options = WalkOptions {
        exclude_globs: options.exclude_globs.clone(),
        include_hidden: options.include_hidden,
        follow_symlinks: options.follow_symlinks,
    };
    let entries = discovery::walk(root_path, &walk_options)?;
    let max_size_bytes = options.max_file_size_mb.saturating_mul(1024 * 1024);

    let mut summary = IndexSummary::default();
    for entry in &entries {
        let rel_path = entry.path.strip_prefix(root_path).unwrap_or(&entry.path);
        let file_name = entry
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let ext = entry.path.extension().and_then(|e| e.to_str());

        let outcome = process_entry(&entry.path, entry.size, max_size_bytes);

        let mut chunks = outcome.chunks;
        chunks.push(filename_chunk(rel_path));

        let record = FileRecord {
            root_id,
            path: &entry.path,
            rel_path,
            file_name,
            ext,
            kind: outcome.kind.as_str(),
            size: entry.size,
            mtime_ns: entry.mtime_ns,
            lang: outcome.lang.as_deref(),
            state: outcome.state,
            skip_reason: outcome.skip_reason,
            error: outcome.error.as_deref(),
            seen_scan_id: scan_id,
        };
        upsert_file(conn, &record, &chunks)?;

        match outcome.state {
            "indexed" => summary.indexed += 1,
            "skipped" => summary.skipped += 1,
            _ => summary.errored += 1,
        }
    }
    Ok(summary)
}

struct FileOutcome {
    kind: Kind,
    state: &'static str,
    skip_reason: Option<&'static str>,
    error: Option<String>,
    chunks: Vec<RawChunk>,
    lang: Option<String>,
}

fn indexed_no_chunks(kind: Kind) -> FileOutcome {
    FileOutcome {
        kind,
        state: "indexed",
        skip_reason: None,
        error: None,
        chunks: Vec::new(),
        lang: None,
    }
}

fn errored(kind: Kind, message: String) -> FileOutcome {
    FileOutcome {
        kind,
        state: "error",
        skip_reason: None,
        error: Some(message),
        chunks: Vec::new(),
        lang: None,
    }
}

/// Which extractor (if any) handles a [`Kind`]. A single exhaustive match
/// over `Kind` here — rather than separate `has_extractor` and
/// `extract_for_kind` matches — means the compiler forces both "should we
/// read this file's bytes" and "how do we extract it" to stay in sync
/// whenever a `Kind` variant is added.
enum Dispatch {
    Text,
    Pdf,
    Office,
    Code,
    /// Image extraction (OCR/QR/embeddings) lands in M4; until then it
    /// falls back to filename-only indexing, same as an unsupported
    /// (`Other`) file.
    None,
}

fn dispatch_for(kind: Kind) -> Dispatch {
    match kind {
        Kind::Text => Dispatch::Text,
        Kind::Pdf => Dispatch::Pdf,
        Kind::Office => Dispatch::Office,
        Kind::Code => Dispatch::Code,
        Kind::Image | Kind::Other => Dispatch::None,
    }
}

/// Whether `extract_for_kind` does real content extraction for `kind`.
/// Kinds without one fall back to filename-only indexing, so there's no
/// point reading their full file content.
fn has_extractor(kind: Kind) -> bool {
    !matches!(dispatch_for(kind), Dispatch::None)
}

fn read_header(path: &Path, len: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; len];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

fn process_entry(path: &Path, size: u64, max_size_bytes: u64) -> FileOutcome {
    if size > max_size_bytes {
        return FileOutcome {
            kind: discovery::classify(path, &[]),
            state: "skipped",
            skip_reason: Some("too_large"),
            error: None,
            chunks: Vec::new(),
            lang: None,
        };
    }

    // Classification from the extension alone needs no I/O; only sniff a
    // small header when the extension doesn't resolve a kind.
    let mut kind = discovery::classify(path, &[]);
    if kind == Kind::Other {
        match read_header(path, SNIFF_HEADER_LEN) {
            Ok(header) => kind = discovery::classify(path, &header),
            Err(e) => return errored(Kind::Other, e.to_string()),
        }
    }

    // No extractor for this kind yet: skip reading the rest of the file,
    // since the bytes would just be discarded (see `has_extractor`).
    if !has_extractor(kind) {
        return indexed_no_chunks(kind);
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return errored(kind, e.to_string()),
    };

    match extract_with_isolation(kind, path.to_path_buf(), bytes) {
        Ok(doc) => FileOutcome {
            kind,
            state: "indexed",
            skip_reason: None,
            error: None,
            chunks: doc.chunks,
            lang: doc.lang,
        },
        Err(e) => errored(kind, e.to_string()),
    }
}

fn extract_for_kind(kind: Kind, path: &Path, bytes: &[u8]) -> Result<ExtractedDoc> {
    match dispatch_for(kind) {
        Dispatch::Text => TextExtractor.extract(path, bytes),
        Dispatch::Pdf => PdfExtractor.extract(path, bytes),
        Dispatch::Office => OfficeExtractor.extract(path, bytes),
        Dispatch::Code => CodeExtractor.extract(path, bytes),
        Dispatch::None => Ok(ExtractedDoc::default()),
    }
}

/// Runs extraction on a worker thread so a hang can be treated as a
/// timeout, and contains panics so one bad file never kills the engine
/// (see SPEC.md §7 M2 "Extraction isolation").
fn extract_with_isolation(kind: Kind, path: PathBuf, bytes: Vec<u8>) -> Result<ExtractedDoc> {
    let (tx, rx) = mpsc::channel();
    let thread_path = path.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            extract_for_kind(kind, &thread_path, &bytes)
        }));
        let _ = tx.send(result);
    });

    match rx.recv_timeout(EXTRACTION_TIMEOUT) {
        Ok(Ok(doc_result)) => doc_result,
        Ok(Err(panic_payload)) => Err(Error::ExtractionPanicked {
            path,
            message: panic_message(&panic_payload),
        }),
        Err(_timed_out) => Err(Error::ExtractionTimeout {
            path,
            seconds: EXTRACTION_TIMEOUT.as_secs(),
        }),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::fs;

    /// Returns `(db_dir, root_dir, conn, root_id)`. Both temp dirs must
    /// stay in scope for the duration of the test, or they're deleted.
    fn open_test_db() -> (tempfile::TempDir, tempfile::TempDir, Connection, i64) {
        let db_dir = tempfile::tempdir().unwrap();
        let conn = db::open(&db_dir.path().join("magi.db")).unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        let root = db::roots::add(&conn, root_dir.path()).unwrap();
        (db_dir, root_dir, conn, root.id)
    }

    fn default_options() -> IndexRootOptions {
        IndexRootOptions {
            exclude_globs: vec![glob::Pattern::new("**/node_modules/**").unwrap()],
            include_hidden: false,
            follow_symlinks: false,
            max_file_size_mb: 1,
        }
    }

    #[test]
    fn indexes_text_files_and_makes_them_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(
            root_path.join("english.txt"),
            "The quick brown fox jumps over the lazy dog.",
        )
        .unwrap();
        fs::write(
            root_path.join("spanish.txt"),
            "El veloz murciélago hindú comía feliz cardillo y kiwi.",
        )
        .unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        assert_eq!(summary.indexed, 2);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.errored, 0);
        assert_eq!(db::files::count_files(&conn).unwrap(), 2);

        let hits = crate::search::fts::search_fts(&conn, "murciélago", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn oversized_file_is_skipped_with_reason() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let huge = "a".repeat(2 * 1024 * 1024); // 2 MB > 1 MB cap
        fs::write(root_path.join("huge.txt"), huge).unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        assert_eq!(summary.skipped, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("huge.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "skipped");
        assert_eq!(row.skip_reason.as_deref(), Some("too_large"));
    }

    #[test]
    fn empty_file_is_indexed_with_only_a_filename_chunk() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("empty.txt"), "").unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        assert_eq!(summary.indexed, 1);
        let hits = crate::search::fts::search_fts(&conn, "empty", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn unsupported_file_type_is_indexed_by_filename_only() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("archive.zip"), b"PK\x03\x04").unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        assert_eq!(summary.indexed, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("archive.zip"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "indexed");
        assert_eq!(row.kind, "other");
        let hits = crate::search::fts::search_fts(&conn, "archive", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn excluded_directory_is_never_indexed() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::create_dir(root_path.join("node_modules")).unwrap();
        fs::write(root_path.join("node_modules/pkg.js"), "ignored").unwrap();
        fs::write(root_path.join("keep.txt"), "keep me").unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        assert_eq!(summary.indexed, 1);
        assert_eq!(db::files::count_files(&conn).unwrap(), 1);
    }

    #[test]
    fn reindexing_is_idempotent() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "alpha content").unwrap();
        fs::write(root_path.join("b.txt"), "beta content").unwrap();

        index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();
        let first_count = db::files::count_files(&conn).unwrap();
        index_root(&mut conn, root_id, &root_path, &default_options(), 2).unwrap();
        let second_count = db::files::count_files(&conn).unwrap();

        assert_eq!(first_count, second_count);
        assert_eq!(second_count, 2);
    }

    #[test]
    fn pdf_files_are_extracted_per_page_and_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/pdf/report.pdf");
        fs::copy(&fixture, root_path.join("report.pdf")).unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        let row = db::files::get_by_path(&conn, &root_path.join("report.pdf"))
            .unwrap()
            .unwrap();
        assert_eq!(summary.indexed, 1);
        assert_eq!(row.state, "indexed");
        assert_eq!(row.kind, "pdf");
        assert_eq!(row.lang.as_deref(), Some("en"));

        let hits = crate::search::fts::search_fts(&conn, "Expenses", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn password_protected_pdf_is_indexed_by_filename_only_via_error_path() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/edge/password_protected.pdf");
        fs::copy(&fixture, root_path.join("password_protected.pdf")).unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        assert_eq!(summary.errored, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("password_protected.pdf"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "error");
        // Still findable by name even though content extraction failed.
        let hits = crate::search::fts::search_fts(&conn, "protected", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn office_files_are_extracted_and_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let office_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/office");
        fs::copy(office_dir.join("notes.docx"), root_path.join("notes.docx")).unwrap();
        fs::copy(
            office_dir.join("kickoff.pptx"),
            root_path.join("kickoff.pptx"),
        )
        .unwrap();
        fs::copy(
            office_dir.join("inventory.xlsx"),
            root_path.join("inventory.xlsx"),
        )
        .unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        assert_eq!(summary.indexed, 3);
        for name in ["notes.docx", "kickoff.pptx", "inventory.xlsx"] {
            let row = db::files::get_by_path(&conn, &root_path.join(name))
                .unwrap()
                .unwrap();
            assert_eq!(row.state, "indexed");
            assert_eq!(row.kind, "office");
        }

        // Not "warehouse" alone: it appears in both notes.docx ("the new
        // warehouse") and inventory.xlsx ("Warehouse A"/"Warehouse B").
        assert_eq!(
            crate::search::fts::search_fts(&conn, "migration", 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::search::fts::search_fts(&conn, "presupuesto", 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::search::fts::search_fts(&conn, "widget", 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn code_files_are_chunked_by_symbol_and_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let code_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/code");
        fs::copy(code_dir.join("sample.rs"), root_path.join("sample.rs")).unwrap();
        fs::copy(code_dir.join("muestra.py"), root_path.join("muestra.py")).unwrap();

        let summary = index_root(&mut conn, root_id, &root_path, &default_options(), 1).unwrap();

        assert_eq!(summary.indexed, 2);
        for name in ["sample.rs", "muestra.py"] {
            let row = db::files::get_by_path(&conn, &root_path.join(name))
                .unwrap()
                .unwrap();
            assert_eq!(row.state, "indexed");
            assert_eq!(row.kind, "code");
        }

        assert_eq!(
            crate::search::fts::search_fts(&conn, "origin", 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::search::fts::search_fts(&conn, "calculadora", 10)
                .unwrap()
                .len(),
            1
        );
    }
}
