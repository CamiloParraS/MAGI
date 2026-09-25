//! Is a file's index entry still right? Every rule that answers it lives here.
//!
//! Two moments ask. The scan compares a row with a stat ([`unchanged_on_disk`]):
//! different means queue it. The extract stage, for a queued file, decides
//! whether its content must be read again ([`keeps`]).
//!
//! A queued row's `state` is `pending`, so what the last index produced is read
//! from the columns queuing leaves alone: `error` (set until a result replaces
//! it), `skip_reason`, `content_hash` and `pipeline_version`. A model change
//! sets `pipeline_version = 0` ([`files::invalidate`](crate::db::files::invalidate)),
//! which fails every rule here.

use crate::db::files::StoredFile;
use crate::discovery::WalkEntry;

use super::PIPELINE_VERSION;

/// The scan's question: same root, size and mtime, indexed by this pipeline.
pub(crate) fn unchanged_on_disk(stored: &StoredFile, root_id: i64, entry: &WalkEntry) -> bool {
    stored.pipeline_version == PIPELINE_VERSION
        && stored.root_id == root_id
        && stored.size == entry.size
        && stored.mtime_ns == entry.mtime_ns
}

/// What the extract stage found about a queued file.
pub(crate) enum Found<'a> {
    /// A kind whose content is not read (unsupported, disabled, too large,
    /// cloud-only), and why it is skipped, if it is.
    NotRead {
        kind: &'a str,
        skip_reason: Option<&'a str>,
    },
    /// The blake3 of its bytes.
    Hashed(&'a [u8; 32]),
}

/// Whether the stored chunks and vectors are still right for what was found:
/// the last index of this file was clean, by this pipeline, of the same thing.
pub(crate) fn keeps(stored: &StoredFile, found: Found) -> bool {
    let clean = stored.pipeline_version == PIPELINE_VERSION && stored.error.is_none();
    clean
        && match found {
            Found::NotRead { kind, skip_reason } => {
                stored.kind == kind
                    && stored.content_hash.is_none()
                    && stored.skip_reason.as_deref() == skip_reason
            }
            Found::Hashed(hash) => {
                stored.skip_reason.is_none()
                    && stored.content_hash.as_deref() == Some(hash.as_slice())
            }
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const HASH: [u8; 32] = [7; 32];

    /// A text file indexed cleanly by the current pipeline, then queued again.
    fn indexed() -> StoredFile {
        StoredFile {
            id: 1,
            root_id: 1,
            kind: "text".into(),
            state: crate::db::files::FileState::Pending,
            skip_reason: None,
            error: None,
            size: 10,
            mtime_ns: 100,
            content_hash: Some(HASH.to_vec()),
            pipeline_version: PIPELINE_VERSION,
        }
    }

    /// A file indexed by name only (no content read).
    fn by_name() -> StoredFile {
        StoredFile {
            kind: "other".into(),
            content_hash: None,
            ..indexed()
        }
    }

    fn entry(size: u64, mtime_ns: i64) -> WalkEntry {
        WalkEntry {
            path: PathBuf::from("a.txt"),
            size,
            mtime_ns,
            is_dir: false,
            cloud_only: false,
        }
    }

    #[test]
    fn the_scan_queues_any_difference_in_root_size_mtime_or_pipeline() {
        let s = indexed();
        assert!(unchanged_on_disk(&s, 1, &entry(10, 100)));
        assert!(!unchanged_on_disk(&s, 2, &entry(10, 100)), "moved root");
        assert!(!unchanged_on_disk(&s, 1, &entry(11, 100)), "size");
        assert!(!unchanged_on_disk(&s, 1, &entry(10, 101)), "mtime");
        let stale = StoredFile {
            pipeline_version: 0,
            ..indexed()
        };
        assert!(
            !unchanged_on_disk(&stale, 1, &entry(10, 100)),
            "invalidated"
        );
    }

    #[test]
    fn same_bytes_keep_a_clean_index() {
        assert!(keeps(&indexed(), Found::Hashed(&HASH)));
        assert!(!keeps(&indexed(), Found::Hashed(&[8; 32])), "new bytes");
    }

    /// The bug this module was made for: a queued `error` row looks `pending`,
    /// and its bytes still match.
    #[test]
    fn same_bytes_do_not_keep_an_error_a_skip_or_a_stale_pipeline() {
        let cases = [
            StoredFile {
                error: Some("embedding failed".into()),
                ..indexed()
            },
            StoredFile {
                skip_reason: Some("image_too_large".into()),
                ..indexed()
            },
            StoredFile {
                pipeline_version: 0,
                ..indexed()
            },
            StoredFile {
                content_hash: None,
                ..indexed()
            },
        ];
        for stored in &cases {
            assert!(!keeps(stored, Found::Hashed(&HASH)));
        }
    }

    #[test]
    fn a_file_not_read_keeps_when_it_would_be_indexed_the_same_way() {
        let not_read = |kind, skip_reason| Found::NotRead { kind, skip_reason };
        assert!(keeps(&by_name(), not_read("other", None)));
        assert!(!keeps(&by_name(), not_read("code", None)), "kind changed");
        assert!(!keeps(&by_name(), not_read("other", Some("too_large"))));
        assert!(
            !keeps(&indexed(), not_read("text", None)),
            "had content, now disabled: its content chunks must go"
        );
        let errored = StoredFile {
            error: Some("gave up".into()),
            ..by_name()
        };
        assert!(!keeps(&errored, not_read("other", None)));
    }
}
