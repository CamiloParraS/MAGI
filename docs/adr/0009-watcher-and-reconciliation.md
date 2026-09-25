# ADR-0009: Change detection — watchers as hints, reconciliation as truth

- **Status:** Accepted; implemented in `watch/{watcher,reconcile,poller}.rs`
- **Milestone:** M5

## Context

The index must converge to what is really on disk, including changes made while
the app was closed, events the OS dropped, network drives with no events, and
renames and moves that should not cost a re-embed. File-system events differ
by OS: FSEvents does not pair a rename's two halves, and `notify-debouncer-full`
can fold a rename of a just-created file into a plain create.

## Decision

**Events only say which paths to look at; the disk decides what happened.**

- **Watchers.** One `notify` watcher per root, debounced 2 s, started before
  the first scan so nothing is missed in between. A batch of events becomes
  `WriteJob::Paths`. Each path is looked at on disk (`reconcile::scan_paths`):
  a file is queued, a folder is walked, and anything known at or under the path
  that is gone, moved out or now excluded becomes a deletion candidate. Event
  kinds are never interpreted, so all three OSs behave the same. An overflow or
  watcher error falls back to a full rescan.
- **Reconciliation scans** at startup, every `reconcile_interval_hours`, after
  a wall-clock jump of more than 5 minutes, and on rescan. A scan compares size,
  mtime and `pipeline_version`; a changed file becomes `pending`, an unseen row
  becomes a deletion candidate. Roots are scanned together, so moves between
  roots are matched.
- **Rename and move detection by content hash.** A new path is hashed (blake3)
  only when a same-size, same-kind row exists whose own path is gone; that row
  is moved in place, keeping its chunks and vectors. Matching from the new side
  means the old path never has to be reported, which is what FSEvents needs.
  Deletion candidates are held for 5 s before deletion, so a move whose halves
  arrive in different batches (two roots' watchers) is still a move.
- **Hash-skip.** A changed mtime with the same content hash keeps the vectors.
- **Stability check.** A file modified under 3 s ago, or whose size changes
  between two stats 1 s apart, is looked at again later rather than indexed
  half-written.
- **Fallbacks.** A root whose watcher cannot start, a UNC path or a remote
  drive is polled every 15 minutes (`watch_failed`). A `missing` or
  `permission_denied` root keeps its rows, is hidden from search, and is
  re-probed every 30 s; when it returns it is reconciled without re-embedding.
- **Model change.** A stored text model, image model or OCR engine id that
  differs from the running one marks the affected files `pending` with
  `pipeline_version = 0`, so hash-skip cannot keep stale vectors.

## Alternatives considered

- **Mirroring the OS event semantics case by case** (create, modify, rename
  from/to). The macOS CI run showed renames arriving unpaired, and two
  attempts to pair them inside the scan transaction were replaced by the
  hash match above.
- **Trusting events alone.** Events are dropped on overflow, never arrive for
  changes made while the app was closed, and don't exist on many network
  drives; periodic reconciliation is needed anyway, so it is the source of
  truth.

## Consequences

- New dependencies: `notify` 8.2.0 and `notify-debouncer-full` 0.6.0.
- Files of kinds with no content hash (filename-only) can't be matched as
  moves; they are deleted and re-created, which costs one filename chunk.
- A deleted file stays searchable for up to 5 s (the hold).
- A root that returns, or whose watcher failed, is polled rather than watched
  until the next start.
- A same-size, same-mtime edit is only caught if a later scan hashes it.
- The 2 s debounce and the 5 s hold are constants, not settings.
