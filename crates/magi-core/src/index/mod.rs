//! Indexing: scheduler, pipeline, and DB writer. Implemented starting M2
//! (see SPEC.md §5.3 threads, §7).

pub mod pipeline;
pub mod scheduler;
pub mod writer;
