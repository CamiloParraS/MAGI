//! hash -> extract -> chunk -> embed -> write. Implemented starting M2

use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;

use crate::db::files::{FileRecord, upsert_file};
use crate::discovery::{self, Kind, WalkOptions};
use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::{Error, Result};
use crate::extract::code::CodeExtractor;
use crate::extract::filename::filename_chunk;
use crate::extract::office::OfficeExtractor;
use crate::extract::pdf::PdfExtractor;
use crate::extract::text::TextExtractor;
use crate::extract::{ExtractedDoc, Extractor, RawChunk};
use crate::ocr::{NoOcr, OcrEngine};

/// What an indexing run needs beyond the walk options: the models.
pub struct IndexContext<'a> {
    pub embedder: &'a dyn TextEmbedder,
    /// `Arc` because extraction runs on a detached thread that must own it.
    pub ocr: Arc<dyn OcrEngine>,
    /// `None` leaves `vec_image` empty: images are still found by their text.
    pub image_embedder: Option<Arc<dyn ImageEmbedder>>,
}

impl<'a> IndexContext<'a> {
    /// A context with no OCR engine and no image embedder; images still get
    /// QR payloads and a thumbnail.
    pub fn new(embedder: &'a dyn TextEmbedder) -> Self {
        Self {
            embedder,
            ocr: Arc::new(NoOcr),
            image_embedder: None,
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
    /// Kinds that get content extraction; others are indexed by filename only.
    pub file_types: Vec<Kind>,
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
        let thumb_key = store_thumbnail(&entry.path, &outcome);
        let mut chunks = std::mem::take(&mut outcome.doc.chunks);
        chunks.push(filename_chunk(rel_path));
        let embeddings = embed_chunks(embedder, &entry.path, &mut chunks, &mut outcome)?;

        let record = FileRecord {
            root_id,
            path: &entry.path,
            rel_path,
            file_name,
            ext,
            kind: outcome.kind.as_str(),
            size: entry.size,
            mtime_ns: entry.mtime_ns,
            lang: outcome.doc.lang.as_deref(),
            state: outcome.state,
            skip_reason: outcome.skip_reason,
            error: outcome.error.as_deref(),
            seen_scan_id: scan_id,
            content_hash: outcome.content_hash.as_ref().map(|h| h.as_slice()),
            thumb_key: thumb_key.as_deref(),
        };
        upsert_file(
            conn,
            &record,
            &chunks,
            &embeddings,
            outcome.doc.image_embedding.as_deref(),
        )?;

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
    if let Some(image_embedder) = &ctx.image_embedder {
        crate::db::meta::set(conn, "image_model_id", image_embedder.model_id())?;
    }
    Ok(summary)
}

/// Embeds `chunks` (content chunks, filename chunk last). One bad file must
/// not abort the run: if embedding fails, the file keeps only its filename
/// chunk (still searchable by name) and is marked `error`. If even that
/// fails, the embedder itself is broken and the error propagates rather than
/// erroring every remaining file.
fn embed_chunks(
    embedder: &dyn TextEmbedder,
    path: &Path,
    chunks: &mut Vec<RawChunk>,
    outcome: &mut FileOutcome,
) -> Result<Vec<Vec<f32>>> {
    let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    match embedder.embed_passages(&texts) {
        Ok(embeddings) => Ok(embeddings),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "embedding failed");
            outcome.state = "error";
            outcome.error = Some(e.to_string());
            chunks.drain(..chunks.len() - 1);
            embedder.embed_passages(&[chunks[0].text.as_str()])
        }
    }
}

/// A failed write costs the file its thumbnail, never its index entry.
fn store_thumbnail(path: &Path, outcome: &FileOutcome) -> Option<String> {
    let (hash, image) = outcome
        .content_hash
        .as_ref()
        .zip(outcome.doc.thumbnail.as_ref())?;
    crate::thumbs::store(hash, image)
        .inspect_err(
            |e| tracing::warn!(path = %path.display(), error = %e, "thumbnail not written"),
        )
        .ok()
}

struct FileOutcome {
    kind: Kind,
    state: &'static str,
    skip_reason: Option<&'static str>,
    error: Option<String>,
    content_hash: Option<[u8; 32]>,
    doc: ExtractedDoc,
}

fn indexed_no_chunks(kind: Kind) -> FileOutcome {
    FileOutcome {
        kind,
        state: "indexed",
        skip_reason: None,
        error: None,
        content_hash: None,
        doc: ExtractedDoc::default(),
    }
}

/// Not broken, just outside what is indexed: SPEC.md section 5.4's `skipped`.
fn skipped(kind: Kind, reason: &'static str) -> FileOutcome {
    FileOutcome {
        state: "skipped",
        skip_reason: Some(reason),
        ..indexed_no_chunks(kind)
    }
}

fn errored(kind: Kind, message: String) -> FileOutcome {
    FileOutcome {
        state: "error",
        error: Some(message),
        ..indexed_no_chunks(kind)
    }
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
    if size > max_size_bytes {
        return skipped(discovery::classify(path, &[]), "too_large");
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

    // `Other` has no extractor, and a kind the user disabled in `file_types`
    // is indexed by filename only: either way the bytes would be discarded,
    // so don't read them.
    if kind == Kind::Other || !options.file_types.contains(&kind) {
        return indexed_no_chunks(kind);
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return errored(kind, e.to_string()),
    };
    let content_hash = Some(*blake3::hash(&bytes).as_bytes());

    let (path_buf, ocr, image_embedder, max_megapixels) = (
        path.to_path_buf(),
        Arc::clone(&ctx.ocr),
        ctx.image_embedder.clone(),
        options.max_image_megapixels,
    );
    let extracted = super::isolate::run(path.to_path_buf(), move || {
        let (path, bytes) = (path_buf.as_path(), bytes.as_slice());
        match kind {
            Kind::Text => TextExtractor.extract(path, bytes),
            Kind::Code => CodeExtractor.extract(path, bytes),
            Kind::Office => OfficeExtractor.extract(path, bytes),
            Kind::Pdf => {
                let mut doc = PdfExtractor.extract(path, bytes)?;
                // A PDF we can read but not render is still searchable.
                doc.thumbnail = crate::extract::pdf::first_page_thumbnail(bytes)
                    .inspect_err(
                        |e| tracing::warn!(path = %path.display(), error = %e, "no PDF thumbnail"),
                    )
                    .ok();
                Ok(doc)
            }
            Kind::Image => crate::extract::image::extract_image(
                path,
                bytes,
                max_megapixels,
                &*ocr,
                image_embedder.as_deref(),
            ),
            Kind::Other => Ok(ExtractedDoc::default()),
        }
    });

    match extracted {
        Ok(doc) => FileOutcome {
            content_hash,
            doc,
            ..indexed_no_chunks(kind)
        },
        // SPEC.md section 5.2: images over `max_image_megapixels` are
        // skipped. A real 75 MP panorama is not an error to retry.
        Err(Error::ImageTooLarge { .. }) => FileOutcome {
            content_hash,
            ..skipped(kind, "image_too_large")
        },
        Err(e) => FileOutcome {
            content_hash,
            ..errored(kind, e.to_string())
        },
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
            file_types: vec![Kind::Text, Kind::Code, Kind::Pdf, Kind::Office, Kind::Image],
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
    fn image_over_the_megapixel_cap_is_skipped_not_errored() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        // Declares 20000 x 20000 (400 MP) in 1.1 MB, so it is over the default
        // 64 MP cap and refused from its header.
        let bomb = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/edge/bomb.png");
        fs::copy(bomb, root_path.join("panorama.png")).unwrap();

        // The test default caps files at 1 MB and the bomb is 1.1 MB: raise it
        // so the image, not the file-size check, is what refuses it.
        let options = IndexRootOptions {
            max_file_size_mb: 10,
            ..default_options()
        };
        let summary = index_root(
            &mut conn,
            root_id,
            &root_path,
            &options,
            1,
            &IndexContext::new(&FakeEmbedder),
        )
        .unwrap();

        assert_eq!((summary.skipped, summary.errored), (1, 0));
        let row = db::files::get_by_path(&conn, &root_path.join("panorama.png"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "skipped");
        assert_eq!(row.skip_reason.as_deref(), Some("image_too_large"));
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
        options.file_types.retain(|&k| k != Kind::Text);
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
}
