//! Stage 1, extract: decide whether a file needs reading, then read,
//! extract and chunk it. CPU and I/O only, no database.

use std::path::Path;
use std::sync::Arc;

use crate::db::files::FileState;
use crate::discovery::{self, Kind, WalkEntry};
use crate::dto::FileErrorCode;
use crate::error::Error;
use crate::extract::code::CodeExtractor;
use crate::extract::filename::filename_chunk;
use crate::extract::office::OfficeExtractor;
use crate::extract::pdf::PdfExtractor;
use crate::extract::text::TextExtractor;
use crate::extract::{ExtractedDoc, Extractor};
use crate::features::Feature;
use crate::index::change::{self, Found};
use crate::ocr::{NoOcr, OcrEngine};

use super::{Extracted, Fresh, IndexContext, IndexRootOptions, Job};

/// Pixels above which an image counts as large for the decode gate (12 MP).
const LARGE_IMAGE_PIXELS: u64 = 12_000_000;

const SNIFF_HEADER_LEN: usize = 8192;

/// Stage 1, extract: is it unchanged ([`change::keeps`])? Otherwise read,
/// extract and chunk it. CPU and I/O only, no database.
pub(crate) fn prepare(ctx: &IndexContext, options: &IndexRootOptions, job: &Job) -> Extracted {
    let entry = &job.entry;
    let s = &job.stored;
    let unreadable = |e: std::io::Error| Extracted::Retry {
        code: FileErrorCode::classify_io(&e),
        message: e.to_string(),
    };

    let max_size_bytes = options.max_file_size_mb.saturating_mul(1024 * 1024);
    let mut features_missing = if ctx.embedder.is_none() {
        Feature::Meaning.bit()
    } else {
        0
    };
    // `Plan::Done` covers filename-only files and disabled kinds: they can
    // only miss meaning.
    let mut outcome = match plan_entry(entry, max_size_bytes, options) {
        Plan::Unreadable(e) => return unreadable(e),
        Plan::Done(outcome) => {
            let found = Found::NotRead {
                kind: outcome.kind.as_str(),
                skip_reason: outcome.skip_reason,
            };
            if change::keeps(s, found) {
                return Extracted::Keep {
                    state: outcome.state,
                };
            }
            *outcome
        }
        Plan::Extract(kind) => {
            // Touched or re-saved with the same content: keep the chunks and
            // vectors, update only size and mtime (SPEC.md §5.4 step 4). A row
            // never hashed goes straight to extraction, which hashes it anyway.
            if s.content_hash.is_some() {
                match hash_file(&entry.path) {
                    Ok(hash) if change::keeps(s, Found::Hashed(&hash)) => {
                        return Extracted::Keep {
                            state: FileState::Indexed,
                        };
                    }
                    Ok(_) => {}
                    Err(e) => return unreadable(e),
                }
            }
            if kind == Kind::Image {
                if ctx.ocr.is_none() {
                    features_missing |= Feature::ImageText.bit();
                }
                if ctx.image_embedder.is_none() {
                    features_missing |= Feature::ImageVisual.bit();
                }
            }
            match extract_entry(&entry.path, kind, options, ctx) {
                Ok(outcome) => outcome,
                Err(e) => return unreadable(e),
            }
        }
    };
    let rel_path = entry
        .path
        .strip_prefix(&job.root_path)
        .unwrap_or(&entry.path);
    let thumb_key = store_thumbnail(&entry.path, &outcome);
    let mut chunks = std::mem::take(&mut outcome.doc.chunks);
    chunks.push(filename_chunk(rel_path));
    Extracted::Fresh(Box::new(Fresh {
        outcome,
        chunks,
        thumb_key,
        features_missing,
    }))
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

pub(super) struct FileOutcome {
    pub(super) kind: Kind,
    pub(super) state: FileState,
    pub(super) skip_reason: Option<&'static str>,
    pub(super) error: Option<(FileErrorCode, String)>,
    pub(super) content_hash: Option<[u8; 32]>,
    pub(super) doc: ExtractedDoc,
}

pub(super) fn indexed_no_chunks(kind: Kind) -> FileOutcome {
    FileOutcome {
        kind,
        state: FileState::Indexed,
        skip_reason: None,
        error: None,
        content_hash: None,
        doc: ExtractedDoc::default(),
    }
}

/// Not broken, just outside what is indexed: SPEC.md section 5.4's `skipped`.
pub(super) fn skipped(kind: Kind, reason: &'static str) -> FileOutcome {
    FileOutcome {
        state: FileState::Skipped,
        skip_reason: Some(reason),
        ..indexed_no_chunks(kind)
    }
}

pub(super) fn errored(kind: Kind, code: FileErrorCode, message: String) -> FileOutcome {
    FileOutcome {
        state: FileState::Error,
        error: Some((code, message)),
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

/// What to do with a file before reading its content.
pub(super) enum Plan {
    /// Could not be opened to tell what it is.
    Unreadable(std::io::Error),
    /// Settled without reading it: too large or not extracted.
    Done(Box<FileOutcome>),
    /// Read it and run this kind's extractor.
    Extract(Kind),
}

pub(super) fn plan_entry(
    entry: &WalkEntry,
    max_size_bytes: u64,
    options: &IndexRootOptions,
) -> Plan {
    let path = entry.path.as_path();
    // Before anything that opens it: its bytes are in the cloud.
    if entry.cloud_only {
        return Plan::Done(Box::new(skipped(
            discovery::classify(path, &[]),
            "cloud_only",
        )));
    }
    // A package bundle (`.app`, `.photoslibrary`): the walk yields the folder
    // as one item, so it is found by name. There is nothing to read.
    if entry.is_dir {
        return Plan::Done(Box::new(skipped(Kind::Other, "bundle")));
    }
    if entry.size > max_size_bytes {
        return Plan::Done(Box::new(skipped(
            discovery::classify(path, &[]),
            "too_large",
        )));
    }

    // Classification from the extension alone needs no I/O; only sniff a
    // small header when the extension doesn't resolve a kind.
    let mut kind = discovery::classify(path, &[]);
    if kind == Kind::Other {
        match read_header(path, SNIFF_HEADER_LEN) {
            Ok(header) => kind = discovery::classify(path, &header),
            Err(e) => return Plan::Unreadable(e),
        }
    }

    // `Other` has no extractor, and a kind the user disabled in `file_types`
    // is indexed by filename only: either way the bytes would be discarded,
    // so don't read them.
    if kind == Kind::Other || !options.file_types.contains(&kind) {
        return Plan::Done(Box::new(indexed_no_chunks(kind)));
    }
    Plan::Extract(kind)
}

/// blake3 of the file, streamed so a touched large file is not loaded whole
/// just to learn it did not change.
pub(crate) fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(*hasher.finalize().as_bytes())
}

fn extract_entry(
    path: &Path,
    kind: Kind,
    options: &IndexRootOptions,
    ctx: &IndexContext,
) -> std::io::Result<FileOutcome> {
    let bytes = std::fs::read(path)?;
    // Hashed from the bytes about to be extracted, not carried over from the
    // skip check: the file may have changed in between.
    let content_hash = Some(*blake3::hash(&bytes).as_bytes());

    // Decoding an image is the memory peak of indexing: bound how many run at
    // once, and how many of them are large.
    let _permit = (kind == Kind::Image).then(|| {
        let large = crate::extract::image::probe_dimensions(path, &bytes)
            .is_ok_and(|(w, h)| u64::from(w) * u64::from(h) > LARGE_IMAGE_PIXELS);
        ctx.image_gate.acquire(large)
    });
    let (path_buf, ocr, image_embedder, max_megapixels) = (
        path.to_path_buf(),
        ctx.ocr
            .clone()
            .unwrap_or_else(|| Arc::new(NoOcr) as Arc<dyn OcrEngine>),
        ctx.image_embedder.clone(),
        options.max_image_megapixels,
    );
    let extracted = crate::index::isolate::run(path.to_path_buf(), move || {
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

    Ok(match extracted {
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
            ..errored(kind, FileErrorCode::classify(&e), e.to_string())
        },
    })
}
