//! ONNX Runtime loading and the session settings every model shares: the
//! text embedder, both SigLIP towers and both OCR networks.

use std::path::{Path, PathBuf};

use ort::ep;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

use crate::error::{Error, Result};
use crate::platform::{Os, PowerStatus};

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

/// Intra-op threads for the indexing models (e5, SigLIP vision, OCR): 4 on
/// AC power, 2 on battery or when the platform cannot tell (NFR-8).
/// Measured in docs/perf-investigation.md: e5 7-8 -> ~12 chunks/s at 4.
///
/// ponytail: read once per session load, so plugging in or unplugging
/// mid-index takes effect only after the next idle unload. Re-check per
/// batch if that matters.
pub(crate) fn indexing_threads() -> usize {
    threads_for(Os.on_battery())
}

fn threads_for(on_battery: Option<bool>) -> usize {
    if on_battery == Some(false) { 4 } else { 2 }
}

/// Builds a CPU session: light graph optimization, `threads` intra-op
/// threads, and the CPU memory arena on only if `arena`. The arena keeps a
/// pool sized for the largest input seen until the session is dropped (idle
/// unload). e5 wants it: without it every layer allocates and page-faults its
/// tensors afresh, which held e5 at ~7 chunks/s whatever the thread count.
/// SigLIP and OCR scale without it, so they skip the memory
/// (docs/perf-investigation.md, ADR-0005).
pub(crate) fn session(path: &Path, threads: usize, arena: bool) -> Result<Session> {
    let model = |what: &str, e: &dyn std::fmt::Display| Error::Model(format!("{what}: {e}"));
    Session::builder()
        .map_err(|e| model("creating session builder", &e))?
        .with_optimization_level(GraphOptimizationLevel::Level1)
        .map_err(|e| model("setting optimization level", &e))?
        .with_intra_threads(threads)
        .map_err(|e| model("setting intra-op thread count", &e))?
        .with_execution_providers([ep::CPU::default().with_arena_allocator(arena).build()])
        .map_err(|e| model("setting the CPU execution provider", &e))?
        .commit_from_file(path)
        .map_err(|e| {
            Error::Model(format!(
                "loading model {} (run `just models` first): {e}",
                path.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_threads_only_when_known_to_be_on_ac() {
        assert_eq!(threads_for(Some(false)), 4);
        assert_eq!(threads_for(Some(true)), 2);
        assert_eq!(threads_for(None), 2);
    }
}
