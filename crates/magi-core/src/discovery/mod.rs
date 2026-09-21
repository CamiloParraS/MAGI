//! Walker with exclusions, hidden-file policy, size cap, symlink policy,
//! opaque bundles; classifier (extension + magic bytes). Implemented in M2
//! (see SPEC.md §7 M2).

pub mod classify;
pub mod walk;

pub use classify::{Kind, classify};
pub use walk::{WalkEntry, WalkOptions, stat, walk};
