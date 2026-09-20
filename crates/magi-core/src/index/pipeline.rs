//! hash -> extract -> chunk -> embed -> write. Implemented starting M2

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::Connection;

use crate::db::files::{FileRecord, upsert_file};
use crate::discovery::{self, Kind, WalkOptions};
use crate::embed::TextEmbedder;
use crate::error::{Error, Result};
use crate::extract::code::CodeExtractor;
use crate::extract::filename::filename_chunk;
use crate::extract::office::OfficeExtractor;
use crate::extract::pdf::PdfExtractor;
use crate::extract::text::TextExtractor;
use crate::extract::{ExtractedDoc, Extractor, RawChunk};
use crate::ocr::{NoOcr, OcrEngine};

pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Timed-out extraction threads that may still be running before new
/// extractions are refused. A hung thread can't be killed, and each one
/// pins its file's bytes and any decoded image, so this caps that memory.
const MAX_STUCK_THREADS: usize = 4;

/// What an indexing run needs beyond the walk options: the models.
pub struct IndexContext<'a> {
    pub embedder: &'a dyn TextEmbedder,
    /// `Arc` because extraction runs on a detached thread that must own it.
    pub ocr: Arc<dyn OcrEngine>,
}

impl<'a> IndexContext<'a> {
    /// A context with no OCR engine; images still get QR payloads and a
    /// thumbnail.
    pub fn new(embedder: &'a dyn TextEmbedder) -> Self {
        Self {
            embedder,
            ocr: Arc::new(NoOcr),
        }
    }
}

const SNIFF_HEADER_LEN: usize = 8192;

pub struct IndexRootOptions {
    pub exclude_globs: Vec<glob::Pattern>,
    pub include_hidden: bool,
    pub follow_symlinks: bool,
    pub max_file_size_mb: u64,
    pub max_image_megapixels: u32,
    /// Kinds (`Kind::as_str`) that get content extraction; others are
    /// indexed by filename only.
    pub file_types: Vec<String>,
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
            max_image_megapixels: config.max_image_megapixels,
            file_types: config.file_types.clone(),
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IndexSummary {
    pub indexed: u32,
    pub skipped: u32,
    pub errored: u32,
}

/// Walks `root_path`, extracts, chunks, embeds, and writes every file to
/// the database. One-shot, no watcher — incremental re-indexing and the
/// pending/indexing state machine land in M5.
pub fn index_root(
    conn: &mut Connection,
    root_id: i64,
    root_path: &Path,
    options: &IndexRootOptions,
    scan_id: i64,
    ctx: &IndexContext,
) -> Result<IndexSummary> {
    let embedder = ctx.embedder;
    let walk_options = WalkOptions {
        exclude_globs: options.exclude_globs.clone(),
        include_hidden: options.include_hidden,
        follow_symlinks: options.follow_symlinks,
    };
    let entries = discovery::walk(root_path, &walk_options)?;
    let max_size_bytes = options.max_file_size_mb.saturating_mul(1024 * 1024);
    let previous_model_id = crate::db::meta::get(conn, "text_model_id")?;

    let mut summary = IndexSummary::default();
    for entry in &entries {
        let rel_path = entry.path.strip_prefix(root_path).unwrap_or(&entry.path);
        let file_name = entry
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let ext = entry.path.extension().and_then(|e| e.to_str());

        let mut outcome = process_entry(&entry.path, entry.size, max_size_bytes, options, ctx);
        let thumb_key = write_thumbnail(&entry.path, &outcome);
        let mut chunks = std::mem::take(&mut outcome.chunks);
        chunks.push(filename_chunk(rel_path));

        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let embeddings = match embedder.embed_passages(&texts) {
            Ok(e) => e,
            Err(e) => {
                // One bad file must not abort the run: keep only the
                // filename chunk (still searchable by name) and mark error.
                tracing::warn!(path = %entry.path.display(), error = %e, "embedding failed");
                outcome.state = "error";
                outcome.error = Some(e.to_string());
                chunks.drain(..chunks.len() - 1);
                let texts = [chunks[0].text.as_str()];
                // If even the filename chunk fails, the embedder itself is
                // broken: propagate rather than error every remaining file.
                embedder.embed_passages(&texts)?
            }
        };

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
            content_hash: outcome.content_hash.as_ref().map(|h| h.as_slice()),
            thumb_key: thumb_key.as_deref(),
        };
        // ponytail: no visual embedder until SigLIP lands (M4 slice 3).
        upsert_file(conn, &record, &chunks, &embeddings, None)?;

        match outcome.state {
            "indexed" => summary.indexed += 1,
            "skipped" => summary.skipped += 1,
            _ => summary.errored += 1,
        }
    }

    if let Some(previous) = &previous_model_id
        && previous != embedder.model_id()
    {
        tracing::warn!(
            previous_model = %previous,
            current_model = %embedder.model_id(),
            "text embedding model changed since the last index; vec_text now mixes vectors from two models until affected files are re-embedded (re-embed triggering lands in M5)"
        );
    }
    crate::db::meta::set(conn, "text_model_id", embedder.model_id())?;
    Ok(summary)
}

/// Content-hash-keyed, so an unchanged or duplicate image is not rewritten.
/// A failed write costs the file its thumbnail, never its index entry.
fn write_thumbnail(path: &Path, outcome: &FileOutcome) -> Option<String> {
    let (hash, image) = outcome
        .content_hash
        .as_ref()
        .zip(outcome.thumbnail.as_ref())?;
    let key = crate::thumbs::thumb_key(hash);
    let thumb_path = crate::thumbs::thumb_path(&key);
    if !thumb_path.exists()
        && let Err(e) = crate::thumbs::write_thumbnail(&thumb_path, image)
    {
        tracing::warn!(path = %path.display(), error = %e, "thumbnail not written");
        return None;
    }
    Some(key)
}

struct FileOutcome {
    kind: Kind,
    state: &'static str,
    skip_reason: Option<&'static str>,
    error: Option<String>,
    chunks: Vec<RawChunk>,
    lang: Option<String>,
    content_hash: Option<[u8; 32]>,
    thumbnail: Option<image::RgbImage>,
}

fn indexed_no_chunks(kind: Kind) -> FileOutcome {
    FileOutcome {
        kind,
        state: "indexed",
        skip_reason: None,
        error: None,
        chunks: Vec::new(),
        lang: None,
        content_hash: None,
        thumbnail: None,
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
        content_hash: None,
        thumbnail: None,
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
    Image,
    /// No extractor: filename-only indexing.
    None,
}

fn dispatch_for(kind: Kind) -> Dispatch {
    match kind {
        Kind::Text => Dispatch::Text,
        Kind::Pdf => Dispatch::Pdf,
        Kind::Office => Dispatch::Office,
        Kind::Code => Dispatch::Code,
        Kind::Image => Dispatch::Image,
        Kind::Other => Dispatch::None,
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

fn process_entry(
    path: &Path,
    size: u64,
    max_size_bytes: u64,
    options: &IndexRootOptions,
    ctx: &IndexContext,
) -> FileOutcome {
    let file_types = &options.file_types;
    if size > max_size_bytes {
        return FileOutcome {
            kind: discovery::classify(path, &[]),
            state: "skipped",
            skip_reason: Some("too_large"),
            error: None,
            chunks: Vec::new(),
            lang: None,
            content_hash: None,
            thumbnail: None,
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
    // Also filename-only when the user disabled this kind in `file_types`.
    if !has_extractor(kind) || !file_types.iter().any(|t| t == kind.as_str()) {
        return indexed_no_chunks(kind);
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return errored(kind, e.to_string()),
    };

    let content_hash = *blake3::hash(&bytes).as_bytes();
    let job = ExtractJob {
        kind,
        path: path.to_path_buf(),
        bytes,
        ocr: Arc::clone(&ctx.ocr),
        max_megapixels: options.max_image_megapixels,
    };
    let mut outcome = match extract_with_isolation(job) {
        Ok(extracted) => FileOutcome {
            kind,
            state: "indexed",
            skip_reason: None,
            error: None,
            chunks: extracted.doc.chunks,
            lang: extracted.doc.lang,
            content_hash: None,
            thumbnail: extracted.thumbnail,
        },
        Err(e) => errored(kind, e.to_string()),
    };
    outcome.content_hash = Some(content_hash);
    outcome
}

/// An [`ExtractedDoc`] plus the thumbnail derived from the same read.
#[derive(Default)]
struct Extracted {
    doc: ExtractedDoc,
    thumbnail: Option<image::RgbImage>,
}

/// Owned inputs for the extraction thread.
struct ExtractJob {
    kind: Kind,
    path: PathBuf,
    bytes: Vec<u8>,
    ocr: Arc<dyn OcrEngine>,
    max_megapixels: u32,
}

fn extract_for_kind(job: &ExtractJob) -> Result<Extracted> {
    let (path, bytes) = (job.path.as_path(), job.bytes.as_slice());
    let plain = |doc| Extracted {
        doc,
        thumbnail: None,
    };
    match dispatch_for(job.kind) {
        Dispatch::Text => TextExtractor.extract(path, bytes).map(plain),
        Dispatch::Pdf => {
            let mut extracted = plain(PdfExtractor.extract(path, bytes)?);
            // A PDF we can read but not render is still searchable.
            match crate::extract::pdf::first_page_thumbnail(bytes) {
                Ok(thumbnail) => extracted.thumbnail = Some(thumbnail),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "no PDF thumbnail"),
            }
            Ok(extracted)
        }
        Dispatch::Office => OfficeExtractor.extract(path, bytes).map(plain),
        Dispatch::Code => CodeExtractor.extract(path, bytes).map(plain),
        Dispatch::Image => {
            crate::extract::image::extract_image(path, bytes, job.max_megapixels, &*job.ocr).map(
                |artifacts| Extracted {
                    doc: artifacts.doc,
                    thumbnail: Some(artifacts.thumbnail),
                },
            )
        }
        Dispatch::None => Ok(Extracted::default()),
    }
}

/// Timed-out extraction threads, kept so they can be joined once they
/// finish instead of being forgotten. Rust can't kill a thread; the best
/// available is to notice when a stuck one ends and to stop starting new
/// work while too many are still alive.
///
/// ponytail: a thread that never finishes holds its slot until restart;
/// past [`MAX_STUCK_THREADS`] of those, extraction is refused. Real
/// reclamation needs a subprocess per extraction.
struct StuckThreads(Mutex<Vec<JoinHandle<()>>>);

static STUCK: StuckThreads = StuckThreads(Mutex::new(Vec::new()));

impl StuckThreads {
    /// Joins the ones that have finished; returns how many still run.
    fn reap(&self) -> usize {
        let mut threads = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (finished, running): (Vec<_>, Vec<_>) =
            threads.drain(..).partition(|t| t.is_finished());
        for thread in finished {
            let _ = thread.join();
        }
        *threads = running;
        threads.len()
    }

    fn add(&self, thread: JoinHandle<()>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(thread);
    }
}

/// Runs extraction on a worker thread so a hang can be treated as a
/// timeout, and contains panics so one bad file never kills the engine
/// (see SPEC.md §7 M2 "Extraction isolation").
fn extract_with_isolation(job: ExtractJob) -> Result<Extracted> {
    let path = job.path.clone();
    run_isolated(&STUCK, EXTRACTION_TIMEOUT, path, move || {
        extract_for_kind(&job)
    })
}

fn run_isolated<T: Send + 'static>(
    stuck: &StuckThreads,
    timeout: Duration,
    path: PathBuf,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let stuck_now = stuck.reap();
    if stuck_now >= MAX_STUCK_THREADS {
        return Err(Error::ExtractionBacklog {
            path,
            stuck: stuck_now,
        });
    }

    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(AssertUnwindSafe(work));
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => {
            let _ = thread.join();
            match result {
                Ok(work_result) => work_result,
                Err(panic_payload) => Err(Error::ExtractionPanicked {
                    path,
                    message: panic_message(&panic_payload),
                }),
            }
        }
        Err(_timed_out) => {
            stuck.add(thread);
            Err(Error::ExtractionTimeout {
                path,
                seconds: timeout.as_secs(),
            })
        }
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
    use crate::embed::{FakeEmbedder, TextEmbedder};
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

    /// `index_root` with the defaults every test but two uses.
    fn index_fake(conn: &mut Connection, root_id: i64, root_path: &Path) -> IndexSummary {
        index_root(
            conn,
            root_id,
            root_path,
            &default_options(),
            1,
            &IndexContext::new(&FakeEmbedder),
        )
        .unwrap()
    }

    fn default_options() -> IndexRootOptions {
        IndexRootOptions {
            exclude_globs: vec![glob::Pattern::new("**/node_modules/**").unwrap()],
            include_hidden: false,
            follow_symlinks: false,
            max_file_size_mb: 1,
            max_image_megapixels: 64,
            file_types: ["text", "code", "pdf", "office", "image"]
                .map(String::from)
                .to_vec(),
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

        let summary = index_fake(&mut conn, root_id, &root_path);

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

        let summary = index_fake(&mut conn, root_id, &root_path);

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

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        let hits = crate::search::fts::search_fts(&conn, "empty", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn unsupported_file_type_is_indexed_by_filename_only() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("archive.zip"), b"PK\x03\x04").unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

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

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        assert_eq!(db::files::count_files(&conn).unwrap(), 1);
    }

    #[test]
    fn reindexing_is_idempotent() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "alpha content").unwrap();
        fs::write(root_path.join("b.txt"), "beta content").unwrap();

        index_fake(&mut conn, root_id, &root_path);
        let first_count = db::files::count_files(&conn).unwrap();
        index_root(
            &mut conn,
            root_id,
            &root_path,
            &default_options(),
            2,
            &IndexContext::new(&FakeEmbedder),
        )
        .unwrap();
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

        let summary = index_fake(&mut conn, root_id, &root_path);

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

        let summary = index_fake(&mut conn, root_id, &root_path);

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

        let summary = index_fake(&mut conn, root_id, &root_path);

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

    /// Real files from an actual nested user document tree (not
    /// synthetic tempfile writes), edge-case fixture
    /// list: a genuinely empty file, a file over the size cap, and a
    /// realistically deep directory — see `fixtures/corpus/edge/`.
    #[test]
    fn real_empty_file_is_indexed_with_only_a_filename_chunk() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/edge/empty_real.gitignore");
        assert_eq!(fs::metadata(&fixture).unwrap().len(), 0);
        fs::copy(&fixture, root_path.join("empty_real.gitignore")).unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("empty_real.gitignore"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "indexed");
    }

    #[test]
    fn real_file_over_cap_is_skipped_with_reason() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/edge/huge_real.pdf");
        assert!(fs::metadata(&fixture).unwrap().len() > 1024 * 1024);
        fs::copy(&fixture, root_path.join("huge_real.pdf")).unwrap();

        // default_options() caps at 1 MB; huge_real.pdf is ~1.4 MB.
        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.skipped, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("huge_real.pdf"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "skipped");
        assert_eq!(row.skip_reason.as_deref(), Some("too_large"));
    }

    #[test]
    fn real_deeply_nested_file_is_indexed_and_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture_root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/edge/deep_real");
        let rel = Path::new(
            "Tercer Semestre/Fisica/asuntosSinImportancia/solarcalculator/solarcalculator/src/main/java/com/example/solarcalculator/model/calculateRequest.java",
        );
        // Rebuild component-by-component rather than a single `join(rel)`:
        // `rel`'s forward slashes survive verbatim inside a joined PathBuf
        // on Windows, producing mixed separators that don't string-match
        // the walker's own (all-backslash) path for the same file.
        let dest = rel
            .components()
            .fold(root_path.clone(), |acc, c| acc.join(c));
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::copy(fixture_root.join(rel), &dest).unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        let row = db::files::get_by_path(&conn, &dest).unwrap().unwrap();
        assert_eq!(row.state, "indexed");
        assert_eq!(row.kind, "code");
        assert_eq!(
            crate::search::fts::search_fts(&conn, "irradiacionSolar", 10)
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

        let summary = index_fake(&mut conn, root_id, &root_path);

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

    #[test]
    fn indexing_writes_vec_text_rows_and_records_model_id() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "hello there").unwrap();

        index_fake(&mut conn, root_id, &root_path);

        let chunk_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .unwrap();
        let vec_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_text", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vec_count, chunk_count);
        assert!(chunk_count > 0);

        assert_eq!(
            crate::db::meta::get(&conn, "text_model_id").unwrap(),
            Some(FakeEmbedder.model_id().to_string())
        );
    }

    /// A test-only `TextEmbedder` that delegates to `FakeEmbedder` for
    /// everything except `model_id`, so it can stand in for "a different
    /// embedding model" without a second real implementation.
    struct OtherFakeEmbedder;

    impl TextEmbedder for OtherFakeEmbedder {
        fn model_id(&self) -> &str {
            "other-fake-v1"
        }

        fn dim(&self) -> usize {
            FakeEmbedder.dim()
        }

        fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            FakeEmbedder.embed_passages(texts)
        }

        fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            FakeEmbedder.embed_query(text)
        }
    }

    /// Fails any batch containing a content chunk (more than one text);
    /// filename-only batches succeed.
    struct FlakyEmbedder;

    impl TextEmbedder for FlakyEmbedder {
        fn model_id(&self) -> &str {
            FakeEmbedder.model_id()
        }

        fn dim(&self) -> usize {
            FakeEmbedder.dim()
        }

        fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            if texts.len() > 1 {
                return Err(Error::Model("boom".into()));
            }
            FakeEmbedder.embed_passages(texts)
        }

        fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            FakeEmbedder.embed_query(text)
        }
    }

    #[test]
    fn disabled_file_type_is_indexed_by_filename_only() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "zebrafish").unwrap();

        let mut options = default_options();
        options.file_types.retain(|t| t != "text");
        let summary = index_root(
            &mut conn,
            root_id,
            &root_path,
            &options,
            1,
            &IndexContext::new(&FakeEmbedder),
        )
        .unwrap();

        assert_eq!(summary.indexed, 1);
        let fts = |q| crate::search::fts::search_fts(&conn, q, 10).unwrap().len();
        assert_eq!(fts("zebrafish"), 0);
        assert_eq!(fts("a"), 1);
    }

    #[test]
    fn embedding_failure_marks_file_errored_and_run_continues() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "hello there").unwrap();
        fs::write(root_path.join("b.bin"), [0u8; 4]).unwrap(); // filename-only: embeds fine

        let summary = index_root(
            &mut conn,
            root_id,
            &root_path,
            &default_options(),
            1,
            &IndexContext::new(&FlakyEmbedder),
        )
        .unwrap();

        assert_eq!(summary.errored, 1);
        assert_eq!(summary.indexed, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("a.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "error");
    }

    #[test]
    fn reindexing_with_a_different_embedder_overwrites_the_recorded_model_id() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "hello there").unwrap();

        index_fake(&mut conn, root_id, &root_path);
        assert_eq!(
            crate::db::meta::get(&conn, "text_model_id").unwrap(),
            Some(FakeEmbedder.model_id().to_string())
        );

        // Re-index with a different embedder: `previous_model_id` is read
        // (exercising that code path without panicking) and the stored id
        // still ends up overwritten with the new model's — this fix only
        // warns, it doesn't block the write. The re-embed *trigger* is M5's
        // job.
        index_root(
            &mut conn,
            root_id,
            &root_path,
            &default_options(),
            2,
            &IndexContext::new(&OtherFakeEmbedder),
        )
        .unwrap();
        assert_eq!(
            crate::db::meta::get(&conn, "text_model_id").unwrap(),
            Some(OtherFakeEmbedder.model_id().to_string())
        );
    }

    #[test]
    fn timed_out_thread_is_reaped_once_it_finishes_and_new_work_is_refused_while_they_pile_up() {
        let stuck = StuckThreads(Mutex::new(Vec::new()));
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let path = PathBuf::from("hang.bin");
        let hang = |release: Arc<std::sync::atomic::AtomicBool>| {
            move || -> Result<()> {
                while !release.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(())
            }
        };

        for _ in 0..MAX_STUCK_THREADS {
            let err = run_isolated(
                &stuck,
                Duration::from_millis(20),
                path.clone(),
                hang(release.clone()),
            )
            .unwrap_err();
            assert!(matches!(err, Error::ExtractionTimeout { .. }), "{err:?}");
        }
        let err =
            run_isolated(&stuck, Duration::from_secs(5), path.clone(), || Ok(())).unwrap_err();
        assert!(matches!(err, Error::ExtractionBacklog { .. }), "{err:?}");

        release.store(true, std::sync::atomic::Ordering::Relaxed);
        while stuck.reap() > 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        run_isolated(&stuck, Duration::from_secs(5), path, || Ok(())).unwrap();
    }
}
