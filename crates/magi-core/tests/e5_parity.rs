//! Parity between `embed::e5::E5Embedder` (Rust, ONNX) and
//! `tools/reference_embeddings.py`'s output (Python, PyTorch via
//! sentence-transformers) for the same sentences — SPEC.md §7 M3: "cosine
//! >= 0.99 for every fixture sentence".
//!
//! Requires the real model/tokenizer (`just models`) and the vendored
//! ONNX Runtime library (`cargo xtask fetch-onnxruntime`), neither of
//! which `just test`/CI provide, so every test here is `#[ignore]`d by
//! default — run manually with `MAGI_DATA_DIR` pointed at a directory
//! containing them.

use std::path::Path;

use magi_core::embed::{E5Embedder, TextEmbedder};
use serde::Deserialize;

const MIN_COSINE: f32 = 0.99;

#[derive(Deserialize)]
struct ReferenceEntry {
    prefix: String,
    text: String,
    embedding: Vec<f32>,
}

fn reference_fixture() -> Vec<ReferenceEntry> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference_embeddings/e5_small.json");
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("reading {path:?} (run `uv run tools/reference_embeddings.py`): {e}")
    });
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("parsing {path:?}: {e}"))
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[test]
#[ignore = "requires `just models` and `cargo xtask fetch-onnxruntime`"]
fn rust_e5_matches_python_reference_within_tolerance() {
    let embedder = E5Embedder::load().expect("load real model");
    let reference = reference_fixture();
    assert!(!reference.is_empty(), "reference fixture is empty");

    let mut worst: Option<(f32, String)> = None;
    for entry in &reference {
        let ours = match entry.prefix.as_str() {
            "query" => embedder.embed_query(&entry.text).unwrap(),
            "passage" => embedder
                .embed_passages(std::slice::from_ref(&entry.text))
                .unwrap()
                .into_iter()
                .next()
                .unwrap(),
            other => panic!("unknown prefix {other:?} in reference fixture"),
        };
        assert_eq!(ours.len(), entry.embedding.len());

        let sim = cosine(&ours, &entry.embedding);
        assert!(
            sim >= MIN_COSINE,
            "{:?}:{:?} cosine {sim} below {MIN_COSINE}",
            entry.prefix,
            entry.text
        );
        if worst.as_ref().is_none_or(|(w, _)| sim < *w) {
            worst = Some((sim, entry.text.clone()));
        }
    }

    let (worst_sim, worst_text) = worst.unwrap();
    println!("worst-case cosine similarity: {worst_sim} ({worst_text:?})");
}
