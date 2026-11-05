//! File and chunk repositories (SPEC.md §5.5). The queue and the scan
//! writes live in [`queue`] and [`scan`], re-exported here.

mod queue;
mod scan;

pub use queue::*;
pub use scan::*;

use std::path::{Path, PathBuf};

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Connection, params};

use crate::discovery::Kind;
use crate::dto::{FileDetails, FileErrorCode};
use crate::embed::embedding_to_blob;
use crate::error::Result;
use crate::extract::{ChunkSource, RawChunk};

/// Where a file is in the index (`files.state`): queued, in the pipeline, or
/// settled by its last result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileState {
    Pending,
    Indexing,
    Indexed,
    Skipped,
    Error,
}

impl FileState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Indexing => "indexing",
            Self::Indexed => "indexed",
            Self::Skipped => "skipped",
            Self::Error => "error",
        }
    }
}

impl ToSql for FileState {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for FileState {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "pending" => Ok(Self::Pending),
            "indexing" => Ok(Self::Indexing),
            "indexed" => Ok(Self::Indexed),
            "skipped" => Ok(Self::Skipped),
            "error" => Ok(Self::Error),
            other => Err(FromSqlError::Other(
                format!("unknown file state {other:?}").into(),
            )),
        }
    }
}

impl FromSql for Kind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "text" => Ok(Self::Text),
            "code" => Ok(Self::Code),
            "pdf" => Ok(Self::Pdf),
            "office" => Ok(Self::Office),
            "image" => Ok(Self::Image),
            "other" => Ok(Self::Other),
            other => Err(FromSqlError::Other(
                format!("unknown file kind {other:?}").into(),
            )),
        }
    }
}

impl FromSql for ChunkSource {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "body" => Ok(Self::Body),
            "ocr" => Ok(Self::Ocr),
            "qr" => Ok(Self::Qr),
            "filename" => Ok(Self::Filename),
            "code_symbol" => Ok(Self::CodeSymbol),
            other => Err(FromSqlError::Other(
                format!("unknown chunk source {other:?}").into(),
            )),
        }
    }
}

/// Everything needed to upsert one file's row.
pub struct FileRecord<'a> {
    pub root_id: i64,
    pub path: &'a Path,
    pub rel_path: &'a Path,
    pub file_name: &'a str,
    pub ext: Option<&'a str>,
    pub kind: &'a str,
    pub size: u64,
    pub mtime_ns: i64,
    pub lang: Option<&'a str>,
    pub state: FileState,
    pub skip_reason: Option<&'a str>,
    pub error: Option<(FileErrorCode, &'a str)>,
    pub seen_scan_id: i64,
    /// blake3 of the file's bytes; `None` when the content was never read.
    pub content_hash: Option<&'a [u8]>,
    /// Thumbnail cache key (see `crate::thumbs`).
    pub thumb_key: Option<&'a str>,
    /// `Feature::bit`s of the search features this file was indexed without.
    pub features_missing: i64,
}

/// Inserts or replaces `record`, its `chunks`, and their `vec_text`
/// embeddings in one transaction: the file row is upserted keyed on its
/// unique `path`, and any previous chunks/vectors for that file are
/// deleted before the new ones are inserted (idempotent re-indexing).
/// `embeddings[i]` is the vector for `chunks[i]` — same length, same order.
/// `embeddings` is `None` when meaning is not running: chunks are stored for
/// keyword search with no `vec_text` rows.
/// `image_embedding` replaces the file's `vec_image` row (or removes it
/// when `None`) in the same transaction.
///
/// Deletes `vec_text` rows before `chunks` (not after): the delete uses a
/// subquery over `chunks` to find which `vec_text` rows belong to this
/// file, so it must run while those `chunks` rows still exist (SPEC.md
/// §5.5: "Deletions MUST explicitly delete `vec_*` rows ... in the same
/// transaction" — virtual tables aren't covered by `FOREIGN KEY` cascades).
pub fn upsert_file(
    conn: &mut Connection,
    record: &FileRecord,
    chunks: &[RawChunk],
    embeddings: Option<&[Vec<f32>]>,
    image_embedding: Option<&[f32]>,
) -> Result<i64> {
    if let Some(embeddings) = embeddings
        && chunks.len() != embeddings.len()
    {
        // `zip` below would silently drop the excess, i.e. lose chunks
        // from the index with no error anywhere.
        return Err(crate::error::Error::Model(format!(
            "embedder returned {} embeddings for {}'s {} chunks",
            embeddings.len(),
            record.path.display(),
            chunks.len(),
        )));
    }
    let tx = conn.transaction()?;
    let path_str = record.path.to_string_lossy();
    let rel_path_str = record.rel_path.to_string_lossy();

    let file_id: i64 = tx.query_row(
        "INSERT INTO files (
            root_id, path, rel_path, file_name, ext, kind, size, mtime_ns,
            lang, state, skip_reason, error, pipeline_version, seen_scan_id, indexed_at,
            content_hash, thumb_key, features_missing, error_code
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?16, ?13, unixepoch(), ?14, ?15, ?17, ?18)
         ON CONFLICT(path) DO UPDATE SET
            root_id = excluded.root_id,
            rel_path = excluded.rel_path,
            file_name = excluded.file_name,
            ext = excluded.ext,
            kind = excluded.kind,
            size = excluded.size,
            mtime_ns = excluded.mtime_ns,
            lang = excluded.lang,
            state = excluded.state,
            skip_reason = excluded.skip_reason,
            error = excluded.error,
            error_code = excluded.error_code,
            content_hash = excluded.content_hash,
            thumb_key = excluded.thumb_key,
            features_missing = excluded.features_missing,
            pipeline_version = excluded.pipeline_version,
            attempts = 0,
            next_attempt_at = NULL,
            indexed_at = unixepoch()
         RETURNING id",
        params![
            record.root_id,
            path_str,
            rel_path_str,
            record.file_name,
            record.ext,
            record.kind,
            record.size as i64,
            record.mtime_ns,
            record.lang,
            record.state,
            record.skip_reason,
            record.error.map(|(_, text)| text),
            record.seen_scan_id,
            record.content_hash,
            record.thumb_key,
            crate::index::PIPELINE_VERSION,
            record.features_missing,
            record.error.map(|(code, _)| code),
        ],
        |row| row.get(0),
    )?;

    tx.execute(
        "DELETE FROM vec_text WHERE chunk_id IN (SELECT id FROM chunks WHERE file_id = ?1)",
        params![file_id],
    )?;
    tx.execute("DELETE FROM chunks WHERE file_id = ?1", params![file_id])?;
    tx.execute("DELETE FROM vec_image WHERE file_id = ?1", params![file_id])?;
    if let Some(embedding) = image_embedding {
        tx.execute(
            "INSERT INTO vec_image (file_id, embedding) VALUES (?1, vec_f32(?2))",
            params![file_id, embedding_to_blob(embedding)],
        )?;
    }
    // Prepared once, not per chunk: a single large file can carry
    // thousands. `RETURNING id` avoids a follow-up SELECT — and avoids
    // `tx.last_insert_rowid()`, which the `chunks_ai` trigger's own INSERT
    // into `chunks_fts` would shadow.
    {
        let mut insert_chunk = tx.prepare(
            "INSERT INTO chunks (file_id, ordinal, source, text, page, line_start, line_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             RETURNING id",
        )?;
        let mut insert_vector =
            tx.prepare("INSERT INTO vec_text (chunk_id, embedding) VALUES (?1, vec_f32(?2))")?;
        for (ordinal, chunk) in chunks.iter().enumerate() {
            let chunk_id: i64 = insert_chunk.query_row(
                params![
                    file_id,
                    ordinal as i64,
                    chunk.source.as_str(),
                    chunk.text,
                    chunk.page,
                    chunk.line_start,
                    chunk.line_end,
                ],
                |row| row.get(0),
            )?;
            if let Some(embeddings) = embeddings {
                insert_vector
                    .execute(params![chunk_id, embedding_to_blob(&embeddings[ordinal])])?;
            }
        }
    }

    tx.commit()?;
    Ok(file_id)
}

/// Where a file is, for `open_file`/`reveal_file`: the UI never sends paths.
pub fn path_of(conn: &Connection, file_id: i64) -> Result<PathBuf> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT path FROM files WHERE id = ?1",
        params![file_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(PathBuf::from)
    .ok_or(crate::error::Error::FileIdNotFound(file_id))
}

/// The selected result's size and page count, for the detail pane.
pub fn details_of(conn: &Connection, file_id: i64) -> Result<FileDetails> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT f.size, (SELECT MAX(page) FROM chunks WHERE file_id = f.id)
         FROM files f WHERE f.id = ?1",
        params![file_id],
        |row| {
            Ok(FileDetails {
                size: row.get::<_, i64>(0)?.max(0) as u64,
                pages: row.get(1)?,
            })
        },
    )
    .optional()?
    .ok_or(crate::error::Error::FileIdNotFound(file_id))
}

pub struct FileRow {
    pub id: i64,
    pub path: PathBuf,
    pub kind: String,
    pub state: String,
    pub skip_reason: Option<String>,
    pub lang: Option<String>,
}

pub fn get_by_path(conn: &Connection, path: &Path) -> Result<Option<FileRow>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, path, kind, state, skip_reason, lang FROM files WHERE path = ?1",
        params![path.to_string_lossy()],
        |row| {
            Ok(FileRow {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                kind: row.get(2)?,
                state: row.get(3)?,
                skip_reason: row.get(4)?,
                lang: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(crate::error::Error::Db)
}

/// The columns change detection needs about a file already in the index.
#[derive(Clone)]
pub struct StoredFile {
    pub id: i64,
    pub root_id: i64,
    pub kind: String,
    pub state: FileState,
    pub skip_reason: Option<String>,
    /// The last result's or attempt's error, kept until a result replaces it.
    pub error: Option<String>,
    pub size: u64,
    pub mtime_ns: i64,
    pub content_hash: Option<Vec<u8>>,
    pub pipeline_version: i64,
}

pub fn get_stored(conn: &Connection, path: &Path) -> Result<Option<StoredFile>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, root_id, kind, state, skip_reason, size, mtime_ns, content_hash,
                pipeline_version, error
         FROM files WHERE path = ?1",
        params![path.to_string_lossy()],
        |row| {
            Ok(StoredFile {
                id: row.get(0)?,
                root_id: row.get(1)?,
                kind: row.get(2)?,
                state: row.get(3)?,
                skip_reason: row.get(4)?,
                size: row.get::<_, i64>(5)? as u64,
                mtime_ns: row.get(6)?,
                content_hash: row.get(7)?,
                pipeline_version: row.get(8)?,
                error: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(crate::error::Error::Db)
}

#[cfg(test)]
mod tests;
