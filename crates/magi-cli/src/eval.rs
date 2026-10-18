//! `magi-cli eval`: indexes a corpus and reports recall@5, recall@10, and
//! MRR for FTS-only, vector-only, and hybrid search against a
//! `queries.jsonl` file, broken down by `lang` bucket (SPEC.md §7 M3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use magi_core::index::pipeline::{IndexContext, IndexRootOptions, index_root};
use magi_core::search::fts::search_fts;
use magi_core::search::vector::{search_vector_image, search_vector_text};
use magi_core::{config, db, search};
use serde::Deserialize;

const FETCH_LIMIT: u32 = 10;

#[derive(Deserialize)]
struct EvalQuery {
    query: String,
    lang: String,
    expected: Vec<String>,
    /// Hits that are neither right nor wrong, removed before ranking: a
    /// `cross` query's same-language twin, which correctly ranks first.
    #[serde(default)]
    ignore: Vec<String>,
}

#[derive(Default, Clone, Copy)]
struct Accum {
    recall_at_5: f64,
    recall_at_10: f64,
    mrr: f64,
    n: usize,
}

impl Accum {
    fn add(&mut self, rank: Option<usize>) {
        self.n += 1;
        if let Some(rank) = rank {
            if rank <= 5 {
                self.recall_at_5 += 1.0;
            }
            if rank <= 10 {
                self.recall_at_10 += 1.0;
            }
            self.mrr += 1.0 / rank as f64;
        }
    }

    fn report(&self, label: &str) {
        if self.n == 0 {
            return;
        }
        println!(
            "  {label:<8} n={:<3} recall@5={:.3}  recall@10={:.3}  mrr={:.3}",
            self.n,
            self.recall_at_5 / self.n as f64,
            self.recall_at_10 / self.n as f64,
            self.mrr / self.n as f64,
        );
    }
}

fn load_queries(path: &Path) -> anyhow::Result<Vec<EvalQuery>> {
    let content = std::fs::read_to_string(path)?;
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

/// `path` relative to `corpus_dir`, with forward slashes, to compare
/// against `queries.jsonl`'s `expected` entries regardless of OS.
fn relative_slash_path(path: &Path, corpus_dir: &Path) -> String {
    path.strip_prefix(corpus_dir)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// 1-based rank of the first hit whose path is in `expected`, if any, with
/// the `ignore`d hits taken out of the list first.
fn first_match_rank(hit_paths: &[String], expected: &[String], ignore: &[String]) -> Option<usize> {
    hit_paths
        .iter()
        .filter(|p| !ignore.contains(p))
        .position(|p| expected.contains(p))
        .map(|i| i + 1)
}

pub fn eval_cmd(queries_path: PathBuf, corpus_dir: PathBuf) -> anyhow::Result<()> {
    let queries = load_queries(&queries_path)?;
    println!(
        "loaded {} queries from {}",
        queries.len(),
        queries_path.display()
    );

    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("eval.db");
    let mut conn = db::open(&db_path)?;
    let root = db::roots::add(&conn, &corpus_dir)?;

    let embedder = crate::embedder_from_env()?;
    let image_embedder = crate::image_embedder_from_env();
    let options = IndexRootOptions::from_config(&config::IndexingConfig::default())?;
    let summary = index_root(
        &mut conn,
        root.id,
        &root.path,
        &options,
        &IndexContext {
            embedder: Some(embedder.clone()),
            ocr: Some(crate::ocr_from_env()),
            image_gate: Default::default(),
            image_embedder: Some(image_embedder.clone()),
        },
    )?;
    println!(
        "indexed corpus: indexed={} skipped={} errors={}",
        summary.indexed, summary.skipped, summary.errored
    );

    for mode in ["fts", "vector", "visual", "hybrid"] {
        println!("\n=== {mode} ===");
        let mut overall = Accum::default();
        let mut by_lang: BTreeMap<String, Accum> = BTreeMap::new();

        for q in &queries {
            // An ignored hit must not cost the query a top-10 slot.
            let limit = FETCH_LIMIT + q.ignore.len() as u32;
            // All three modes go through `rank_and_boost`, so the
            // baselines are the same ranking function as hybrid with one
            // input list emptied — not a different one. Comparing raw
            // `search_vector_text` (no boosts) against boosted hybrid is
            // what made the old M3 item-4 numbers uninterpretable; see
            // docs/eval.md.
            let hits = match mode {
                "fts" => {
                    let fts = search_fts(&conn, &q.query, search::FTS_FETCH_LIMIT)?;
                    search::rank_and_boost(&q.query, &fts, &[], &[], limit)
                }
                "vector" => {
                    let embedding = embedder.embed_query(&q.query)?;
                    let vector = search_vector_text(&conn, &embedding, search::VECTOR_FETCH_LIMIT)?;
                    search::rank_and_boost(&q.query, &[], &vector, &[], limit)
                }
                "visual" => {
                    let embedding = image_embedder.embed_query(&q.query)?;
                    let visual = search_vector_image(
                        &conn,
                        &embedding,
                        search::IMAGE_FETCH_LIMIT,
                        search::IMAGE_MIN_COSINE,
                    )?;
                    search::rank_and_boost(&q.query, &[], &[], &visual, limit)
                }
                "hybrid" => search::hybrid_search(
                    &conn,
                    Some(embedder.as_ref()),
                    Some(image_embedder.as_ref()),
                    &q.query,
                    limit,
                )?,
                _ => unreachable!(),
            };
            let hit_paths: Vec<String> = hits
                .into_iter()
                .map(|h| relative_slash_path(&h.path, &root.path))
                .collect();

            let rank = first_match_rank(&hit_paths, &q.expected, &q.ignore);
            // Bucket averages hide which query failed; list the misses.
            if mode == "hybrid" && rank.is_none_or(|r| r > 5) {
                println!(
                    "  miss [{}] {:?}: rank {rank:?}, top hit {}",
                    q.lang,
                    q.query,
                    hit_paths.first().map_or("-", String::as_str)
                );
            }
            overall.add(rank);
            by_lang.entry(q.lang.clone()).or_default().add(rank);
        }

        overall.report("overall");
        for (lang, acc) in &by_lang {
            acc.report(lang);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ignored_hits_do_not_take_a_rank() {
        let hits = paths(&["en/meeting_notes.txt", "es/notas_reunion.txt"]);
        let expected = paths(&["es/notas_reunion.txt"]);
        assert_eq!(first_match_rank(&hits, &expected, &[]), Some(2));
        let ignore = paths(&["en/meeting_notes.txt"]);
        assert_eq!(first_match_rank(&hits, &expected, &ignore), Some(1));
    }
}
