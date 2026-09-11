//! Connection factory (WAL, `foreign_keys`, `busy_timeout`), sqlite-vec
//! registration, and the migration runner. Implemented in M1 (see SPEC.md
//! §7 M1, §5.5 schema).

pub mod files;
pub mod roots;
