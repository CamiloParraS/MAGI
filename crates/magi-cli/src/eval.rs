//! `magi-cli eval`: indexes a corpus and reports recall@5, recall@10, and
//! MRR for FTS-only, vector-only, and hybrid search against a
//! `queries.jsonl` file, broken down by `lang` (SPEC.md §7 M3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use magi_core::index::pipeline::{IndexRootOptions, index_root};
use magi_core::search::fts::search_fts;
use magi_core::search::vector::search_vector_text;
use magi_core::{config, db, search};
use serde::Deserialize;

const FETCH_LIMIT: u32 = 10;

#[derive(Deserialize)]
struct EvalQuery {
    query: String,
    lang: String,
    expected: Vec<String>,
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

/// 1-based rank of the first hit whose path is in `expected`, if any.
fn first_match_rank(hit_paths: &[String], expected: &[String]) -> Option<usize> {
    hit_paths
        .iter()
        .position(|p| expected.iter().any(|e| e == p))
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
    let options = IndexRootOptions::from_config(&config::IndexingConfig::default())?;
    let summary = index_root(
        &mut conn,
        root.id,
        &root.path,
        &options,
        1,
        embedder.as_ref(),
    )?;
    println!(
        "indexed corpus: indexed={} skipped={} errors={}",
        summary.indexed, summary.skipped, summary.errored
    );

    for mode in ["fts", "vector", "hybrid"] {
        println!("\n=== {mode} ===");
        let mut overall = Accum::default();
        let mut by_lang: BTreeMap<String, Accum> = BTreeMap::new();

        for q in &queries {
            let hit_paths: Vec<String> = match mode {
                "fts" => search_fts(&conn, &q.query, FETCH_LIMIT)?
                    .into_iter()
                    .map(|h| relative_slash_path(&h.path, &root.path))
                    .collect(),
                "vector" => {
                    let embedding = embedder.embed_query(&q.query)?;
                    search_vector_text(&conn, &embedding, FETCH_LIMIT)?
                        .into_iter()
                        .map(|h| relative_slash_path(&h.path, &root.path))
                        .collect()
                }
                "hybrid" => search::hybrid_search(&conn, embedder.as_ref(), &q.query, FETCH_LIMIT)?
                    .into_iter()
                    .map(|h| relative_slash_path(&h.path, &root.path))
                    .collect(),
                _ => unreachable!(),
            };

            let rank = first_match_rank(&hit_paths, &q.expected);
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
