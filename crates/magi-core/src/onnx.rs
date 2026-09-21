//! ONNX Runtime loading and the session settings every model shares: the
//! text embedder, both SigLIP towers and both OCR networks.

use std::path::{Path, PathBuf};

use ort::ep;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

use crate::error::{Error, Result};

/// Resolves the vendored ONNX Runtime shared library: `MAGI_ONNXRUNTIME_PATH`
/// first (an explicit override), then the dev-time `vendor/onnxruntime/<target>/`
/// layout that `xtask fetch-onnxruntime` populates — mirrors
/// `extract::pdf::resolve_library_path`.
fn onnxruntime_library_path() -> PathBuf {
    if let Ok(path) = std::env::var("MAGI_ONNXRUNTIME_PATH") {
        return PathBuf::from(path);
    }
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(manifest_dir);
    workspace_root
        .join("vendor/onnxruntime")
        .join(crate::platform::onnxruntime_vendor_dir())
        .join("lib")
        .join(crate::platform::onnxruntime_library_filename())
}

/// Dynamically loads the vendored ONNX Runtime. Process-wide and idempotent
/// (`ort::init_from` no-ops after the first successful call), so every
/// model calls this before building a session.
pub(crate) fn init() -> Result<()> {
    let ort_lib = onnxruntime_library_path();
    ort::init_from(&ort_lib)
        .map_err(|e| {
            Error::Model(format!(
                "loading ONNX Runtime from {} (set MAGI_ONNXRUNTIME_PATH or run `cargo xtask fetch-onnxruntime`): {e}",
                ort_lib.display()
            ))
        })?
        .commit();
    Ok(())
}

/// Builds a CPU session: light graph optimization, `threads` intra-op
/// threads, and the CPU memory arena off. The arena grows a pool sized for
/// the largest input ever seen and never shrinks it (ADR-0005 measured
/// ~66 MB from it); disabling it trades a small per-inference allocation
/// cost for not holding that pool for the life of the process.
pub(crate) fn session(path: &Path, threads: usize) -> Result<Session> {
    let model = |what: &str, e: &dyn std::fmt::Display| Error::Model(format!("{what}: {e}"));
    Session::builder()
        .map_err(|e| model("creating session builder", &e))?
        .with_optimization_level(GraphOptimizationLevel::Level1)
        .map_err(|e| model("setting optimization level", &e))?
        .with_intra_threads(threads)
        .map_err(|e| model("setting intra-op thread count", &e))?
        .with_execution_providers([ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| model("disabling the CPU memory arena", &e))?
        .commit_from_file(path)
        .map_err(|e| {
            Error::Model(format!(
                "loading model {} (run `just models` first): {e}",
                path.display()
            ))
        })
}
