//! Hybrid search orchestration (FTS + text vector + image vector, fused
//! with RRF). Implemented in M3 (text) and M4 (image) — see SPEC.md §5.6.

pub mod fts;
pub mod fuse;
pub mod vector;
