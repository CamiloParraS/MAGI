//! Reconciliation scans: diff a walk of a root against its rows (SPEC.md §5.4
//! startup sequence step 4) and detect moves. Plain functions over a
//! connection; the runtime and the watcher call them.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rusqlite::Connection;

use crate::db::{files, meta, roots};
use crate::discovery;
use crate::error::Result;
use crate::index::change;
use crate::index::pipeline::{IndexRootOptions, hash_file};
use crate::platform::{FsProbe, PermissionProbe, RootAccess};

/// The id for the next scan, one more than the last (`meta.last_scan_id`).
/// Rows carry the id of the scan that last saw them; an older one means gone.
pub fn next_scan_id(conn: &Connection) -> Result<i64> {
    let id = meta::get(conn, "last_scan_id")?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
        + 1;
    meta::set(conn, "last_scan_id", &id.to_string())?;
    Ok(id)
}

/// Why a running scan should stop early: the engine is shutting down, or a
/// root was switched off or removed. A stopped root keeps its rows as they
/// were; nothing it did not get to see is deleted.
#[derive(Default)]
pub struct ScanControl {
    stop: AtomicBool,
    off: Mutex<Vec<i64>>,
    /// Files found by the scan under way, for the status to show.
    seen: AtomicU64,
}

impl ScanControl {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn switch_off(&self, root_id: i64) {
        let mut off = self.off.lock().unwrap_or_else(|p| p.into_inner());
        if !off.contains(&root_id) {
            off.push(root_id);
        }
    }

    pub fn switch_on(&self, root_id: i64) {
        self.off
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|id| *id != root_id);
    }

    /// Forgets the last scan's count, for a scan about to start.
    pub fn reset_seen(&self) {
        self.seen.store(0, Ordering::Relaxed);
    }

    /// Files found so far by the scan that last started.
    pub fn seen(&self) -> u64 {
        self.seen.load(Ordering::Relaxed)
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn root_stopped(&self, root_id: i64) -> bool {
        self.stopped()
            || self
                .off
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(&root_id)
    }
}

enum EntryOutcome {
    Inserted,
    Changed,
    Unchanged,
    Moved,
}

/// Brings one file's row in line with what was found on disk.
fn reconcile_entry(
    conn: &Connection,
    root_id: i64,
    root_path: &Path,
    entry: &discovery::WalkEntry,
    scan_id: i64,
) -> Result<EntryOutcome> {
    Ok(match files::get_stored(conn, &entry.path)? {
        None => {
            let rel = entry.path.strip_prefix(root_path).unwrap_or(&entry.path);
            if let Some(id) = find_old_home(conn, entry)? {
                let to = files::MoveTarget {
                    path: &entry.path,
                    rel_path: rel,
                    size: entry.size,
                    mtime_ns: entry.mtime_ns,
                };
                files::rename_file_to(conn, id, root_id, to, scan_id)?;
                return Ok(EntryOutcome::Moved);
            }
            files::insert_pending(
                conn,
                root_id,
                &entry.path,
                rel,
                entry.size,
                entry.mtime_ns,
                scan_id,
            )?;
            EntryOutcome::Inserted
        }
        Some(s) if !change::unchanged_on_disk(&s, root_id, entry) => {
            files::mark_changed(conn, s.id, root_id, entry.size, entry.mtime_ns, scan_id)?;
            EntryOutcome::Changed
        }
        Some(s) => {
            files::mark_seen(conn, s.id, scan_id)?;
            EntryOutcome::Unchanged
        }
    })
}

/// An unknown path whose bytes match an indexed row whose own path is gone is
/// that row, renamed or moved (SPEC.md §5.4 step 4): it takes the new location
/// and keeps its chunks and vectors. Matching from the new side means the old
/// path never has to be reported, which matters: `notify-debouncer-full` folds
/// a rename of a just-created file into a plain create of the new path.
///
/// Only same-size rows whose path is gone are candidates, so the new file is
/// read only when a move is plausible. A row never hashed (filename-only
/// kinds) cannot be matched; its replacement costs one filename chunk.
fn find_old_home(conn: &Connection, entry: &discovery::WalkEntry) -> Result<Option<i64>> {
    // Hashing reads the bytes, which for a placeholder means downloading them.
    if entry.cloud_only {
        return Ok(None);
    }
    let kind = discovery::classify(&entry.path, &[]);
    let candidates: Vec<_> = files::hashed_of_size(conn, entry.size, kind.as_str())?
        .into_iter()
        .filter(|(_, path, _)| !path.exists())
        .collect();
    if candidates.is_empty() {
        return Ok(None);
    }
    let Ok(hash) = hash_file(&entry.path) else {
        return Ok(None);
    };
    Ok(candidates
        .into_iter()
        .find(|(_, _, h)| h.as_slice() == hash)
        .map(|(id, ..)| id))
}

/// What [`scan_paths`] found and left undecided.
pub struct PathScan {
    pub summary: ScanSummary,
    /// Rows at or under the paths that were not found there.
    pub unseen: Vec<Unseen>,
}

/// A row a scan did not find: a deletion candidate until a later scan sees
/// it again (a move claimed it, or it was recreated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unseen {
    pub id: i64,
    /// The scan that missed it.
    pub scan_id: i64,
}

impl Unseen {
    /// Kept undecided until `until`, then deleted if still unseen.
    pub fn hold_until(self, until: std::time::Instant) -> Held {
        Held {
            id: self.id,
            scan_id: self.scan_id,
            until,
        }
    }
}

/// Reconciles only `paths` (what the watcher reported): each is looked at on
/// disk and a folder is walked. What is new or changed is queued and moves are
/// followed. Whatever was at or under a path but is no longer found there
/// (deleted, moved away, now excluded) is returned undecided, so the caller can
/// hold it for a moment: a move between roots is reported by two watchers that
/// debounce independently, so the removal can arrive first. A path outside
/// every usable root is ignored; one that is a root itself triggers a full
/// [`reconcile_all`], which probes it.
pub fn scan_paths(
    conn: &mut Connection,
    options: &IndexRootOptions,
    paths: &[PathBuf],
) -> Result<PathScan> {
    let usable: Vec<_> = roots::list(conn)?
        .into_iter()
        .filter(|r| r.indexable())
        .collect();
    if paths.iter().any(|p| usable.iter().any(|r| r.path == *p)) {
        return Ok(PathScan {
            summary: reconcile_all(conn, options)?,
            // `reconcile_all` settled its own deletions: nothing to hold.
            unseen: Vec::new(),
        });
    }
    let scan_id = next_scan_id(conn)?;
    let walk_options = options.walk_options();
    let mut summary = ScanSummary::default();
    let tx = conn.transaction()?;

    let mut scanned = Vec::new();
    for path in paths {
        let Some(root) = usable.iter().find(|r| path.starts_with(&r.path)) else {
            continue;
        };
        let entries = if !discovery::is_wanted(&root.path, path, &walk_options) {
            Vec::new()
        } else {
            match std::fs::metadata(path) {
                Ok(m) if m.is_dir() => discovery::walk_under(&root.path, path, &walk_options)?,
                Ok(m) if m.is_file() => discovery::stat(path).into_iter().collect(),
                _ => Vec::new(),
            }
        };
        for entry in &entries {
            summary.count(reconcile_entry(&tx, root.id, &root.path, entry, scan_id)?);
        }
        scanned.push(path);
    }
    // After every path is marked, so a file that turned up under one path is
    // not mistaken for a deletion under another.
    let mut unseen = Vec::new();
    for path in scanned {
        unseen.extend(files::unseen_under(&tx, path, scan_id)?);
    }
    unseen.sort_unstable();
    unseen.dedup();
    tx.commit()?;
    Ok(PathScan {
        summary,
        unseen: unseen
            .into_iter()
            .map(|id| Unseen { id, scan_id })
            .collect(),
    })
}

/// A deletion candidate waiting to see whether its bytes turn up elsewhere.
pub struct Held {
    pub id: i64,
    pub scan_id: i64,
    pub until: std::time::Instant,
}

/// Settles held candidates. One seen again since (claimed by a move, or
/// recreated, as an editor's save does) is dropped from the list; one still
/// unseen when its time is up is deleted. Returns how many were deleted.
pub fn settle_held(
    conn: &mut Connection,
    held: &mut Vec<Held>,
    now: std::time::Instant,
) -> Result<u32> {
    let mut removed = 0;
    for item in std::mem::take(held) {
        if !files::is_unseen_since(conn, item.id, item.scan_id)? {
            continue;
        }
        if item.until <= now {
            files::delete_file(conn, item.id)?;
            removed += 1;
        } else {
            held.push(item);
        }
    }
    Ok(removed)
}

/// Walks `root_path` and brings its rows in line, in one transaction: an
/// unknown file is a move (see [`find_old_home`]) or is inserted `pending`;
/// one whose size or mtime differ, or that was indexed by an older pipeline,
/// is marked `pending` with its new stat; anything else is only marked seen.
/// Nothing is extracted or deleted: the rows it did not see are returned as
/// deletion candidates for [`remove_unseen`], so a move into a root scanned
/// later can still claim them.
pub fn reconcile_root(
    conn: &mut Connection,
    root_id: i64,
    root_path: &Path,
    options: &IndexRootOptions,
    scan_id: i64,
) -> Result<(ScanSummary, Vec<Unseen>)> {
    let stopped = || options.scan.root_stopped(root_id);
    let found = || {
        options.scan.seen.fetch_add(1, Ordering::Relaxed);
    };
    let Some(entries) = discovery::walk_until(
        root_path,
        root_path,
        &options.walk_options(),
        &stopped,
        &found,
    )?
    else {
        return Ok(Default::default());
    };
    let tx = conn.transaction()?;
    let mut summary = ScanSummary::default();
    for (n, entry) in entries.iter().enumerate() {
        // Dropping `tx` rolls the half-done scan back.
        if n % 256 == 0 && stopped() {
            return Ok(Default::default());
        }
        summary.count(reconcile_entry(&tx, root_id, root_path, entry, scan_id)?);
    }
    tx.execute(
        "UPDATE roots SET last_full_scan_at = unixepoch() WHERE id = ?1",
        [root_id],
    )?;
    let unseen = files::unseen(&tx, root_id, scan_id)?
        .into_iter()
        .map(|id| Unseen { id, scan_id })
        .collect();
    tx.commit()?;
    Ok((summary, unseen))
}

/// Totals from [`reconcile_all`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanSummary {
    pub inserted: u32,
    pub changed: u32,
    pub unchanged: u32,
    pub moved: u32,
    pub removed: u32,
}

impl std::ops::AddAssign for ScanSummary {
    fn add_assign(&mut self, other: Self) {
        self.inserted += other.inserted;
        self.changed += other.changed;
        self.unchanged += other.unchanged;
        self.moved += other.moved;
        self.removed += other.removed;
    }
}

impl ScanSummary {
    fn count(&mut self, outcome: EntryOutcome) {
        *match outcome {
            EntryOutcome::Inserted => &mut self.inserted,
            EntryOutcome::Changed => &mut self.changed,
            EntryOutcome::Unchanged => &mut self.unchanged,
            EntryOutcome::Moved => &mut self.moved,
        } += 1;
    }
}

/// One scan of every enabled root. See [`reconcile_roots`].
pub fn reconcile_all(conn: &mut Connection, options: &IndexRootOptions) -> Result<ScanSummary> {
    let enabled: Vec<i64> = roots::list(conn)?
        .into_iter()
        .filter(|r| r.enabled)
        .map(|r| r.id)
        .collect();
    reconcile_roots(conn, options, &enabled)
}

/// One scan of the enabled roots among `ids`: probe each, reconcile it, then
/// remove what none of them has claimed, so a move between them is followed
/// whichever is walked first. A root that is missing or unreadable is skipped
/// with its rows intact and its status set.
///
/// Scanning only some roots (one just added, enabled or readable again) is
/// safe: a file moved *into* them is matched from the new side
/// ([`find_old_home`]) wherever it came from, and a file moved out of them
/// into a root not scanned here was already claimed by that root's watcher (a
/// polled root may instead re-embed it once, at its next scan).
pub fn reconcile_roots(
    conn: &mut Connection,
    options: &IndexRootOptions,
    ids: &[i64],
) -> Result<ScanSummary> {
    let scan_id = next_scan_id(conn)?;
    options.scan.seen.store(0, Ordering::Relaxed);
    let started = std::time::Instant::now();
    let mut summary = ScanSummary::default();
    let mut unseen = Vec::new();
    let scanned = roots::list(conn)?
        .into_iter()
        .filter(|r| r.enabled && ids.contains(&r.id));
    for root in scanned {
        if options.scan.stopped() {
            break;
        }
        let access = FsProbe.probe(&root.path);
        roots::set_access(conn, root.id, &access)?;
        if access != RootAccess::Ok {
            continue;
        }
        let (found, gone) = reconcile_root(conn, root.id, &root.path, options, scan_id)?;
        summary += found;
        unseen.extend(gone);
    }
    // A scan that was cut short has not seen everything: delete nothing.
    if options.scan.stopped() {
        return Ok(summary);
    }
    summary.removed = remove_unseen(conn, unseen)?;
    tracing::info!(
        found = options.scan.seen(),
        inserted = summary.inserted,
        changed = summary.changed,
        removed = summary.removed,
        elapsed = ?started.elapsed(),
        "scan finished"
    );
    Ok(summary)
}

/// Re-probes roots that were unreadable or missing (SPEC.md §6.1 recovery,
/// every 30 s from the ticker). Those that are back are reconciled, which also
/// clears their status. `None` when nothing came back.
pub fn recover(conn: &mut Connection, options: &IndexRootOptions) -> Result<Option<ScanSummary>> {
    let back: Vec<i64> = roots::list(conn)?
        .into_iter()
        .filter(|r| r.enabled && !r.status.readable() && FsProbe.probe(&r.path) == RootAccess::Ok)
        .map(|r| r.id)
        .collect();
    if back.is_empty() {
        return Ok(None);
    }
    reconcile_roots(conn, options, &back).map(Some)
}

/// Deletes the candidates still unseen (a later root's walk may have claimed
/// one as a move) with everything derived from them. Returns how many were
/// deleted.
pub fn remove_unseen(conn: &mut Connection, unseen: Vec<Unseen>) -> Result<u32> {
    let now = std::time::Instant::now();
    let mut held = unseen.into_iter().map(|u| u.hold_until(now)).collect();
    settle_held(conn, &mut held, now)
}

#[cfg(test)]
mod tests;
