//! Cross-encoder reranking spike (docs/archive/0001, ADR-0013: deferred). Not on the product
//! path: used by `magi-cli eval` and `magi-cli search --rerank` only, with a
//! model folder given by hand (`model.onnx`, `tokenizer.json`), outside
//! `models/manifest.toml` until the spike's result says to keep it.
//!
//! The model scores a (query, document) pair together and returns one raw
//! logit; higher means more relevant. Spiked with
//! `cross-encoder/mmarco-mMiniLMv2-L12-H384-v1` (qint8), see
//! docs/research/cross-lingual-reranker.md.

use std::path::Path;
use std::sync::Mutex;

use ort::session::Session;
use ort::value::TensorRef;
use rusqlite::{Connection, params};
use tokenizers::{EncodeInput, PaddingParams, Tokenizer, TruncationParams};

use crate::error::{Error, Result};

/// Tokens per (query, document) pair: the query plus the start of the
/// document. The fixture documents fit; longer files are cut. The spike
/// can override it with `MAGI_SPIKE_TOKENS` to price shorter pairs.
pub const MAX_PAIR_TOKENS: usize = 256;
/// Characters of a file's text read per document, before tokenizing.
const DOC_CHARS: usize = 1500;

pub struct CrossEncoder {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    token_type_ids: bool,
    max_tokens: usize,
}

impl CrossEncoder {
    /// Loads `dir/model.onnx` and `dir/tokenizer.json`.
    pub fn load(dir: &Path) -> Result<Self> {
        crate::onnx::init()?;
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| Error::Model(format!("loading the reranker tokenizer: {e}")))?;
        let max_tokens = std::env::var("MAGI_SPIKE_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MAX_PAIR_TOKENS);
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: max_tokens,
                ..Default::default()
            }))
            .map_err(|e| Error::Model(format!("setting truncation: {e}")))?;
        tokenizer.with_padding(Some(PaddingParams::default()));
        let session = crate::onnx::session(
            &dir.join("model.onnx"),
            crate::onnx::indexing_threads(),
            true,
        )?;
        let token_type_ids = session
            .inputs()
            .iter()
            .any(|input| input.name() == "token_type_ids");
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            token_type_ids,
            max_tokens,
        })
    }

    /// Tokens per pair, after the `MAGI_SPIKE_TOKENS` override.
    pub fn max_tokens(&self) -> usize {
        self.max_tokens
    }

    /// The model's input names, for the spike's first check.
    pub fn input_names(&self) -> Vec<String> {
        self.session
            .lock()
            .map(|s| s.inputs().iter().map(|i| i.name().to_string()).collect())
            .unwrap_or_default()
    }

    /// One logit per document, in order: how relevant each is to `query`.
    pub fn score(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let pairs: Vec<EncodeInput> = documents
            .iter()
            .map(|d| (query, d.as_str()).into())
            .collect();
        let encodings = self
            .tokenizer
            .encode_batch(pairs, true)
            .map_err(|e| Error::Model(format!("tokenizing pairs: {e}")))?;
        let batch = encodings.len();
        let seq_len = encodings[0].len();
        let mut ids = Vec::with_capacity(batch * seq_len);
        let mut mask = Vec::with_capacity(batch * seq_len);
        let mut type_ids = Vec::with_capacity(batch * seq_len);
        for e in &encodings {
            ids.extend(e.get_ids().iter().map(|&x| i64::from(x)));
            mask.extend(e.get_attention_mask().iter().map(|&x| i64::from(x)));
            type_ids.extend(e.get_type_ids().iter().map(|&x| i64::from(x)));
        }
        let shape = [batch, seq_len];
        let mut session = self
            .session
            .lock()
            .map_err(|_| Error::Model("reranker session lock poisoned".to_string()))?;
        let outputs = if self.token_type_ids {
            session.run(ort::inputs! {
                "input_ids" => tensor(shape, &ids, "input_ids")?,
                "attention_mask" => tensor(shape, &mask, "attention_mask")?,
                "token_type_ids" => tensor(shape, &type_ids, "token_type_ids")?,
            })
        } else {
            session.run(ort::inputs! {
                "input_ids" => tensor(shape, &ids, "input_ids")?,
                "attention_mask" => tensor(shape, &mask, "attention_mask")?,
            })
        }
        .map_err(|e| Error::Model(format!("running the reranker: {e}")))?;
        let logits = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| Error::Model(format!("extracting logits: {e}")))?;
        if logits.len() != batch {
            return Err(Error::Model(format!(
                "expected one logit per pair, got shape {:?}",
                logits.shape()
            )));
        }
        Ok(logits.iter().copied().collect())
    }
}

fn tensor<'a>(shape: [usize; 2], data: &'a [i64], what: &str) -> Result<TensorRef<'a, i64>> {
    TensorRef::from_array_view((shape, data))
        .map_err(|e| Error::Model(format!("building {what}: {e}")))
}

/// What the reranker reads for a file: its name, then its text chunks in
/// order (OCR and QR included), up to [`DOC_CHARS`] characters.
pub fn file_text(conn: &Connection, file_id: i64) -> Result<String> {
    let name: String = conn.query_row(
        "SELECT file_name FROM files WHERE id = ?1",
        params![file_id],
        |row| row.get(0),
    )?;
    let mut stmt = conn.prepare_cached(
        "SELECT text FROM chunks WHERE file_id = ?1 AND source <> 'filename' ORDER BY id",
    )?;
    let mut text = name;
    for chunk in stmt.query_map(params![file_id], |row| row.get::<_, String>(0))? {
        if text.len() >= DOC_CHARS {
            break;
        }
        text.push('\n');
        text.push_str(&chunk?);
    }
    Ok(text.chars().take(DOC_CHARS).collect())
}
