//! `magi-cli search`: one query, printed, then the process exits.

use std::time::Duration;

use magi_core::embed::TextEmbedder;
use magi_core::features::{Components, load_components};
use magi_core::search::fts::search_fts;
use magi_core::{config, db};

use crate::db_path;

/// Frees the text model and its shared tokenizer as soon as the query is
/// embedded, so they aren't still resident while the SigLIP text tower and
/// its Gemma tokenizer load (NFR-12: ~490 MB off the peak). One-shot CLI
/// only; a long-lived process keeps them cached.
struct OneShotQuery<'a>(&'a dyn TextEmbedder);

impl TextEmbedder for OneShotQuery<'_> {
    fn model_id(&self) -> &str {
        self.0.model_id()
    }
    fn dim(&self) -> usize {
        self.0.dim()
    }
    fn embed_passages(&self, texts: &[&str]) -> magi_core::Result<Vec<Vec<f32>>> {
        self.0.embed_passages(texts)
    }
    fn embed_query(&self, text: &str) -> magi_core::Result<Vec<f32>> {
        let v = self.0.embed_query(text);
        self.0.unload_if_idle(Duration::ZERO);
        magi_core::embed::manager::unload_text_tokenizer_if_idle(Duration::ZERO);
        v
    }
}

/// The terminal shows highlights as `[term]`.
fn show(snippet: &str) -> String {
    use magi_core::search::fts::{HIGHLIGHT_END, HIGHLIGHT_START};
    snippet
        .replace(HIGHLIGHT_START, "[")
        .replace(HIGHLIGHT_END, "]")
}

pub(crate) fn search_cmd(
    query: &str,
    mode: &str,
    limit: u32,
    rerank: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let conn = db::open(&db_path())?;
    match mode {
        "fts" => {
            let hits = search_fts(&conn, query, limit)?;
            if hits.is_empty() {
                println!("no results");
            }
            for hit in hits {
                println!("{}\t{}", hit.path.display(), show(&hit.snippet));
            }
        }
        "hybrid" => {
            let Components { text, image, .. } = load_components(&config::load()?.features);
            let Some(embedder) = text else {
                anyhow::bail!(
                    "search by meaning is not installed; run: magi-cli features enable meaning"
                );
            };
            let mut hits = magi_core::search::hybrid_search(
                &conn,
                Some(&OneShotQuery(embedder.as_ref())),
                image.as_deref(),
                query,
                limit,
            )?;
            if let Some(dir) = rerank {
                // As the app would: the reranker is built after the query
                // is embedded, then reorders the results it was given.
                // `MAGI_SPIKE_UNLOAD_SIGLIP`: free SigLIP's sessions first.
                if std::env::var_os("MAGI_SPIKE_UNLOAD_SIGLIP").is_some()
                    && let Some(image) = &image
                {
                    image.unload_if_idle(Duration::ZERO);
                }
                let model = magi_core::search::rerank::CrossEncoder::load(dir)?;
                let docs = hits
                    .iter()
                    .map(|h| magi_core::search::rerank::file_text(&conn, h.file_id))
                    .collect::<magi_core::Result<Vec<_>>>()?;
                let scores = model.score(query, &docs)?;
                for (hit, score) in hits.iter_mut().zip(scores) {
                    hit.score = f64::from(score);
                }
                hits.sort_by(|a, b| b.score.total_cmp(&a.score));
            }
            if hits.is_empty() {
                println!("no results");
            }
            for hit in hits {
                println!(
                    "{}\t{:.4}\t{:?}\t{}",
                    hit.path.display(),
                    hit.score,
                    hit.match_sources,
                    show(&hit.snippet)
                );
            }
        }
        other => anyhow::bail!("unsupported search mode {other:?}; use \"fts\" or \"hybrid\""),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
