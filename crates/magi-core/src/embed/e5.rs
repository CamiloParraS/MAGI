//! `multilingual-e5-small` text embedder (`ort` + `tokenizers`). Implemented
//! in M3 (see SPEC.md §7 M3).

use std::path::PathBuf;
use std::sync::Mutex;

use ort::ep;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;
use tokenizers::Tokenizer;

use super::{TEXT_EMBEDDING_DIM, TextEmbedder, l2_normalize};
use crate::error::{Error, Result};

const MODEL_ID: &str = "intfloat/multilingual-e5-small";
const QUERY_PREFIX: &str = "query: ";
const PASSAGE_PREFIX: &str = "passage: ";
/// Chunks per inference call. A file's chunk count is unbounded (a 15 MB
/// text file yields ~5,700 chunks), and ONNX Runtime materializes a
/// `batch x seq_len x 384` f32 `last_hidden_state` for the whole batch —
/// ~4.5 GB at that size. Capping the batch keeps that intermediate at a
/// few MB regardless of file size (SPEC.md §5.3: "small batches").
const BATCH_CHUNKS: usize = 16;

/// Real `intfloat/multilingual-e5-small` embedder: `tokenizers` for
/// encoding (with the `"query: "`/`"passage: "` prefixes SPEC.md §3
/// requires), `ort` for ONNX inference (dynamically loading the vendored
/// ONNX Runtime library — see `xtask fetch-onnxruntime`), then mean
/// pooling over the attention mask and L2 normalization.
pub struct E5Embedder {
    session: Mutex<Session>,
    /// Shared with `chunk::count_tokens` (`embed::manager::shared_text_tokenizer`)
    /// rather than a private copy — ADR-0005 measured a second parse of the
    /// ~17 MB `tokenizer.json` at ~275 MB of peak RSS, pure duplication.
    tokenizer: &'static Tokenizer,
}

/// Resolves the vendored ONNX Runtime shared library: `MAGI_ONNXRUNTIME_PATH`
/// first (an explicit override), then the dev-time `vendor/onnxruntime/<target>/`
/// layout that `xtask fetch-onnxruntime` populates — mirrors
/// `extract::pdf::resolve_library_path`.
fn onnxruntime_library_path() -> PathBuf {
    if let Ok(path) = std::env::var("MAGI_ONNXRUNTIME_PATH") {
        return PathBuf::from(path);
    }
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
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
/// model that needs a session calls this first.
pub(crate) fn init_onnxruntime() -> Result<()> {
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

/// Builds a CPU session with the settings every model here shares: light
/// graph optimization, `threads` intra-op threads, and the CPU memory arena
/// off, so the pool is not sized for the largest input ever seen and kept
/// for the life of the process (ADR-0005 measured ~66 MB from it).
pub(crate) fn build_session(path: &std::path::Path, threads: usize) -> Result<Session> {
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

impl E5Embedder {
    /// Loads the model + tokenizer from `embed::manager::model_dir("text")`
    /// (populated by `ensure_model_file`/`import_offline_model_file`).
    /// Dynamically loading the ONNX Runtime library is process-wide and
    /// idempotent (`ort::init_from` no-ops after the first successful
    /// call), so constructing more than one `E5Embedder` is safe.
    pub fn load() -> Result<Self> {
        init_onnxruntime()?;

        let dir = crate::embed::manager::model_dir("text");
        let model_path = dir.join("model.onnx");

        // Shared with `chunk::count_tokens` rather than a private
        // `Tokenizer::from_file` copy — see the `tokenizer` field's doc
        // comment and ADR-0005.
        let tokenizer = crate::embed::manager::shared_text_tokenizer().ok_or_else(|| {
            Error::Model(format!(
                "no downloaded tokenizer under {} (run `just models` first)",
                dir.display()
            ))
        })?;

        let session = Session::builder()
            .map_err(|e| Error::Model(format!("creating session builder: {e}")))?
            .with_optimization_level(GraphOptimizationLevel::Level1)
            .map_err(|e| Error::Model(format!("setting optimization level: {e}")))?
            // The embed worker is architecturally single-threaded (SPEC.md
            // §5.3: one dedicated embed thread), so there's no batch-level
            // parallelism to exploit; no reason to let `ort` default to one
            // intra-op thread per logical core.
            .with_intra_threads(1)
            .map_err(|e| Error::Model(format!("setting intra-op thread count: {e}")))?
            // The CPU arena allocator grows a pool sized for the *largest*
            // batch seen and never shrinks it back — ADR-0005 measured
            // ~66 MB of peak RSS attributable to it. Only `.with_memory_pattern`
            // was tested there (a related but distinct setting); disabling the
            // arena itself trades a small per-inference allocation cost (the
            // embed worker isn't latency-sensitive at that scale) for not
            // holding onto that pool for the process's lifetime.
            .with_execution_providers([ep::CPU::default().with_arena_allocator(false).build()])
            .map_err(|e| Error::Model(format!("disabling the CPU memory arena: {e}")))?
            .commit_from_file(&model_path)
            .map_err(|e| {
                Error::Model(format!(
                    "loading model {} (run `just models` first): {e}",
                    model_path.display()
                ))
            })?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    /// Tokenizes, runs inference, and mean-pools + L2-normalizes each of
    /// `texts` (already prefixed by the caller). One vector per input,
    /// same order.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts.iter().map(String::as_str).collect(), true)
            .map_err(|e| Error::Model(format!("tokenizing: {e}")))?;

        let batch = encodings.len();
        let seq_len = encodings[0].len();
        let mut ids = Vec::with_capacity(batch * seq_len);
        let mut mask = Vec::with_capacity(batch * seq_len);
        let mut type_ids = Vec::with_capacity(batch * seq_len);
        for encoding in &encodings {
            ids.extend(encoding.get_ids().iter().map(|&x| x as i64));
            mask.extend(encoding.get_attention_mask().iter().map(|&x| x as i64));
            type_ids.extend(encoding.get_type_ids().iter().map(|&x| x as i64));
        }

        let input_ids = TensorRef::from_array_view(([batch, seq_len], &*ids))
            .map_err(|e| Error::Model(format!("building input_ids tensor: {e}")))?;
        let attention_mask = TensorRef::from_array_view(([batch, seq_len], &*mask))
            .map_err(|e| Error::Model(format!("building attention_mask tensor: {e}")))?;
        let token_type_ids = TensorRef::from_array_view(([batch, seq_len], &*type_ids))
            .map_err(|e| Error::Model(format!("building token_type_ids tensor: {e}")))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| Error::Model("ONNX session lock poisoned".to_string()))?;
        let outputs = session
            .run(ort::inputs! {
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
                "token_type_ids" => token_type_ids,
            })
            .map_err(|e| Error::Model(format!("running inference: {e}")))?;

        let hidden = outputs
            .get("last_hidden_state")
            .ok_or_else(|| Error::Model("model produced no last_hidden_state output".to_string()))?
            .try_extract_array::<f32>()
            .map_err(|e| Error::Model(format!("extracting last_hidden_state: {e}")))?;
        let hidden = hidden
            .into_dimensionality::<ndarray::Ix3>()
            .map_err(|e| Error::Model(format!("unexpected output shape: {e}")))?;
        let dim = hidden.shape()[2];

        // Mean pooling over the attention mask (SPEC.md §3): average the
        // unmasked token vectors, ignoring padding.
        let mut result = Vec::with_capacity(batch);
        for b in 0..batch {
            let mut sum = vec![0f32; dim];
            let mut count = 0f32;
            for t in 0..seq_len {
                if mask[b * seq_len + t] == 0 {
                    continue;
                }
                count += 1.0;
                for (d, slot) in sum.iter_mut().enumerate() {
                    *slot += hidden[[b, t, d]];
                }
            }
            if count > 0.0 {
                for v in sum.iter_mut() {
                    *v /= count;
                }
            }
            l2_normalize(&mut sum);
            result.push(sum);
        }
        Ok(result)
    }
}

impl TextEmbedder for E5Embedder {
    fn model_id(&self) -> &str {
        MODEL_ID
    }

    fn dim(&self) -> usize {
        TEXT_EMBEDDING_DIM
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH_CHUNKS) {
            let prefixed: Vec<String> = batch
                .iter()
                .map(|t| format!("{PASSAGE_PREFIX}{t}"))
                .collect();
            out.extend(self.embed(&prefixed)?);
        }
        Ok(out)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let prefixed = format!("{QUERY_PREFIX}{text}");
        self.embed(&[prefixed])?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Model("embedding a query produced no vector".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Requires the real model + tokenizer (`just models`) and the
    /// vendored ONNX Runtime library (`cargo xtask fetch-onnxruntime`),
    /// neither of which `just test`/CI provide — run manually with
    /// `MAGI_DATA_DIR` pointed at a directory containing them.
    #[test]
    #[ignore = "requires `just models` and `cargo xtask fetch-onnxruntime`"]
    fn real_model_embeds_plausible_vectors() {
        let embedder = E5Embedder::load().expect("load real model");
        assert_eq!(embedder.dim(), TEXT_EMBEDDING_DIM);
        assert_eq!(embedder.model_id(), MODEL_ID);

        let query = embedder.embed_query("electrician invoice").unwrap();
        assert_eq!(query.len(), TEXT_EMBEDDING_DIM);
        let norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "not L2-normalized: norm={norm}");

        let passages = embedder
            .embed_passages(&[
                "Factura de electricista: reparación del panel eléctrico.",
                "Receta de arepas con queso.",
            ])
            .unwrap();
        assert_eq!(passages.len(), 2);

        // Cross-lingual: the English query should sit closer to the
        // Spanish electrician invoice than to the unrelated recipe.
        let sim_invoice = cosine(&query, &passages[0]);
        let sim_recipe = cosine(&query, &passages[1]);
        assert!(
            sim_invoice > sim_recipe,
            "expected invoice ({sim_invoice}) closer than recipe ({sim_recipe})"
        );
    }

    /// More passages than `BATCH_CHUNKS`, so the batching loop runs more
    /// than once: every input must still get its own vector, in the same
    /// order. The bar is 0.99, not equality — `BatchLongest` padding plus
    /// int8 kernels make a vector depend slightly on what it was batched
    /// with (measured worst case 0.9956, the same band as int8-vs-fp32
    /// parity). A mis-split or mis-ordered batch would land near 0.5, so
    /// 0.99 still catches the failure this test exists for.
    #[test]
    #[ignore = "requires `just models` and `cargo xtask fetch-onnxruntime`"]
    fn batched_passages_match_one_at_a_time() {
        let embedder = E5Embedder::load().expect("load real model");
        let texts: Vec<String> = (0..BATCH_CHUNKS + 4)
            .map(|i| format!("chunk number {i} of the electrician invoice"))
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();

        let batched = embedder.embed_passages(&refs).unwrap();
        assert_eq!(batched.len(), refs.len());
        for (i, text) in refs.iter().enumerate() {
            let alone = embedder.embed_passages(&[text]).unwrap();
            let c = cosine(&batched[i], &alone[0]);
            assert!(c > 0.99, "batch position {i} diverged: cosine {c}");
        }
    }

    #[test]
    #[ignore = "requires `just models` and `cargo xtask fetch-onnxruntime`"]
    fn repeated_load_is_safe_in_the_same_process() {
        let _first = E5Embedder::load().expect("first load");
        let _second = E5Embedder::load().expect("second load");
    }
}
