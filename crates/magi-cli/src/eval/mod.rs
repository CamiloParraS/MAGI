//! `magi-cli eval`: indexes a corpus and reports recall@5, recall@10, and
//! MRR for FTS-only, vector-only, and hybrid search against a
//! `queries.jsonl` file, broken down by `lang` bucket (SPEC.md §7 M3).
//! Hybrid runs twice on the same index: the shipped RRF fusion and the
//! z-score reranker (`search::zfusion`, GitHub issue #11). Queries in the
//! `neg` bucket have no answer and are scored for precision only
//! (`precision`, docs/issues/0002); `fresh` queries are also reported apart.

mod precision;
mod rerank;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use magi_core::index::pipeline::{IndexContext, IndexRootOptions, index_root};
use magi_core::search::fts::search_fts;
use magi_core::search::strength::Scales;
use magi_core::search::vector::{search_vector_image, search_vector_text};
use magi_core::search::zfusion::{ZFusionParams, search_zscore, strong_count};
use magi_core::search::{FileHit, QueryVectors};
use magi_core::{config, db, search};
use precision::Judged;
use serde::Deserialize;

const FETCH_LIMIT: u32 = 10;
/// Files fetched per list when explaining a miss: past the 100 hybrid reads,
/// so "beyond 100" and "not found" differ. Corpora here are ~120 files.
const DIAGNOSE_DEPTH: u32 = 500;

#[derive(Deserialize)]
struct EvalQuery {
    query: String,
    lang: String,
    expected: Vec<String>,
    /// Hits that are neither right nor wrong, removed before ranking: a
    /// `cross` query's same-language twin, which correctly ranks first.
    #[serde(default)]
    ignore: Vec<String>,
    /// Written after the 2026-11-05 decisions, so no threshold was tuned on
    /// it (docs/issues/0002).
    #[serde(default)]
    fresh: bool,
}

impl EvalQuery {
    fn is_neg(&self) -> bool {
        self.lang == "neg"
    }
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

/// `expected`'s rank in one input list as `"<rank> (<chunk source>)"`, or
/// `"-"` when the list does not have it. Ignored hits take no rank.
fn list_rank(hits: &[FileHit], corpus_dir: &Path, q: &EvalQuery) -> String {
    let paths: Vec<String> = hits
        .iter()
        .map(|h| relative_slash_path(&h.path, corpus_dir))
        .collect();
    match first_match_rank(&paths, &q.expected, &q.ignore) {
        Some(rank) => {
            let hit = hits.iter().find(|h| {
                q.expected
                    .contains(&relative_slash_path(&h.path, corpus_dir))
            });
            match hit.and_then(|h| h.source) {
                Some(source) => format!("{rank} ({source:?})"),
                None => rank.to_string(),
            }
        }
        None => "-".to_string(),
    }
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

    let modes = [
        Mode::Fts,
        Mode::Vector,
        Mode::Visual,
        Mode::Rrf,
        // The reranker variant of GitHub #11; the other z-score variants are
        // recorded in docs/eval.md (2026-11-05) and no longer run.
        Mode::ZScore(z_params(1.0, true, true, false)),
        // The same, scaled from a fixed sample of each table: what scales
        // past sqlite-vec's 4,096-row KNN cap (`ZFusionParams::sample_scale`).
        Mode::ZScore(z_params(1.0, true, true, true)),
    ];
    for mode in modes {
        println!("\n=== {} ===", mode.label());
        let mut overall = Accum::default();
        let mut by_lang: BTreeMap<String, Accum> = BTreeMap::new();
        let mut fresh = Accum::default();
        let mut fresh_by_lang: BTreeMap<String, Accum> = BTreeMap::new();
        let mut folds = FoldStats::default();
        let mut judged: Vec<Judged> = Vec::new();

        for q in &queries {
            // An ignored hit must not cost the query a top-10 slot.
            let limit = FETCH_LIMIT + q.ignore.len() as u32;
            // All three modes go through `rank_and_boost`, so the
            // baselines are the same ranking function as hybrid with one
            // input list emptied — not a different one. Comparing raw
            // `search_vector_text` (no boosts) against boosted hybrid is
            // what made the old M3 item-4 numbers uninterpretable; see
            // docs/eval.md.
            let fused = matches!(mode, Mode::Rrf | Mode::ZScore(_));
            let vectors = if fused {
                QueryVectors::embed(
                    Some(embedder.as_ref()),
                    Some(image_embedder.as_ref()),
                    &q.query,
                )?
            } else {
                QueryVectors::default()
            };
            let hits = match mode {
                Mode::Fts => {
                    let fts = search_fts(&conn, &q.query, search::FTS_FETCH_LIMIT)?;
                    search::rank_and_boost(&q.query, &fts, &[], &[], limit)
                }
                Mode::Vector => {
                    let embedding = embedder.embed_query(&q.query)?;
                    let vector = search_vector_text(&conn, &embedding, search::VECTOR_FETCH_LIMIT)?;
                    search::rank_and_boost(&q.query, &[], &vector, &[], limit)
                }
                Mode::Visual => {
                    let embedding = image_embedder.embed_query(&q.query)?;
                    let visual = search_vector_image(
                        &conn,
                        &embedding,
                        search::IMAGE_FETCH_LIMIT,
                        search::IMAGE_MIN_COSINE,
                    )?;
                    search::rank_and_boost(&q.query, &[], &[], &visual, limit)
                }
                Mode::Rrf => search::search_with(&conn, &vectors, &q.query, limit)?,
                Mode::ZScore(params) => search_zscore(&conn, &vectors, &q.query, limit, &params)?,
            };
            let scores: Vec<f64> = hits.iter().map(|h| h.score).collect();
            let hit_paths: Vec<String> = hits
                .iter()
                .map(|h| relative_slash_path(&h.path, &root.path))
                .collect();

            let rank = first_match_rank(&hit_paths, &q.expected, &q.ignore);
            if fused {
                let sample = Scales::sample(&conn, &vectors)?;
                let fetched = Scales::fetched(&conn, &vectors)?;
                judged.push(Judged {
                    query: q.query.clone(),
                    lang: q.lang.clone(),
                    fresh: q.fresh,
                    expected_at: hit_paths.iter().position(|p| q.expected.contains(p)),
                    hits,
                    sample,
                    fetched,
                });
            }
            if q.is_neg() {
                continue;
            }
            folds.add(&scores, rank);
            // Bucket averages hide which query failed; list the misses.
            if mode == Mode::Rrf && rank.is_none_or(|r| r > 5) {
                println!(
                    "  miss [{}] {:?}: rank {rank:?}, top hit {}",
                    q.lang,
                    q.query,
                    hit_paths.first().map_or("-", String::as_str)
                );
                // Where the answer sits in each input list, fetched past
                // what hybrid reads: tells "fusion buried it" from "no list
                // found it", e.g. reranker vs. embedder work.
                let fts = search_fts(&conn, &q.query, DIAGNOSE_DEPTH)?;
                let vector =
                    search_vector_text(&conn, &embedder.embed_query(&q.query)?, DIAGNOSE_DEPTH)?;
                let visual = search_vector_image(
                    &conn,
                    &image_embedder.embed_query(&q.query)?,
                    DIAGNOSE_DEPTH,
                    search::IMAGE_MIN_COSINE,
                )?;
                println!(
                    "    lists: keyword {}, text-vector {}, visual {}",
                    list_rank(&fts, &root.path, q),
                    list_rank(&vector, &root.path, q),
                    list_rank(&visual, &root.path, q),
                );
            }
            overall.add(rank);
            by_lang.entry(q.lang.clone()).or_default().add(rank);
            if q.fresh {
                fresh.add(rank);
                fresh_by_lang.entry(q.lang.clone()).or_default().add(rank);
            }
        }

        overall.report("overall");
        for (lang, acc) in &by_lang {
            acc.report(lang);
        }
        fresh.report("fresh");
        for (lang, acc) in &fresh_by_lang {
            acc.report(&format!("f-{lang}"));
        }
        if matches!(mode, Mode::Rrf | Mode::ZScore(_)) {
            folds.report();
            precision::report(&judged);
            if mode == Mode::Rrf {
                precision::report_neg_detail(&judged);
            }
        }
    }

    if let Some(dir) = std::env::var_os("MAGI_RERANKER_DIR") {
        rerank::run(
            Path::new(&dir),
            &queries,
            |query, limit| {
                let vectors = QueryVectors::embed(
                    Some(embedder.as_ref()),
                    Some(image_embedder.as_ref()),
                    query,
                )?;
                Ok(search::search_with(&conn, &vectors, query, limit)?)
            },
            |file_id| Ok(search::rerank::file_text(&conn, file_id)?),
            |hit| relative_slash_path(&hit.path, &root.path),
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Fts,
    Vector,
    Visual,
    /// The shipped hybrid search: RRF over the three lists.
    Rrf,
    /// `search::zfusion` with these parameters (GitHub issue #9).
    ZScore(ZFusionParams),
}

impl Mode {
    fn label(self) -> String {
        match self {
            Mode::Fts => "fts".into(),
            Mode::Vector => "vector".into(),
            Mode::Visual => "visual".into(),
            Mode::Rrf => "hybrid (rrf)".into(),
            Mode::ZScore(p) => format!(
                "hybrid (zscore, gate {:.1}{}{}{})",
                p.z_gate,
                if p.image_min_cosine.is_some() {
                    ", image floor"
                } else {
                    ""
                },
                if p.words_need_gate {
                    ", words gated"
                } else {
                    ""
                },
                if p.sample_scale { ", sample scale" } else { "" },
            ),
        }
    }
}

/// `image_floor`: the RRF path's `IMAGE_MIN_COSINE` on images;
/// `words_gated`: coverage under the gate adds nothing; `sample`: the
/// scale comes from a fixed sample of each table.
fn z_params(gate: f64, image_floor: bool, words_gated: bool, sample: bool) -> ZFusionParams {
    ZFusionParams {
        z_gate: gate,
        image_min_cosine: image_floor.then_some(search::IMAGE_MIN_COSINE),
        words_need_gate: words_gated,
        sample_scale: sample,
        ..ZFusionParams::default()
    }
}

/// How long result lists are, and what folding everything under half the
/// top score into "weaker matches" would hide (issue #9, item 5): that
/// rule's false-weak count, kept to compare with `precision`'s labels.
#[derive(Default)]
struct FoldStats {
    queries: usize,
    results: usize,
    empty: usize,
    folded: usize,
    /// Queries whose expected file was found but would be folded.
    answer_folded: usize,
}

impl FoldStats {
    fn add(&mut self, scores: &[f64], rank: Option<usize>) {
        let strong = strong_count(scores, 0.5);
        self.queries += 1;
        self.results += scores.len();
        self.empty += usize::from(scores.is_empty());
        self.folded += scores.len() - strong;
        // `rank` skips ignored hits, so this can only undercount.
        self.answer_folded += usize::from(rank.is_some_and(|r| r > strong));
    }

    fn report(&self) {
        let n = self.queries.max(1) as f64;
        println!(
            "  results/query={:.1} (of {FETCH_LIMIT})  empty={}  folded under 0.5x top={:.1}/query  answer folded={}",
            self.results as f64 / n,
            self.empty,
            self.folded as f64 / n,
            self.answer_folded,
        );
    }
}

#[cfg(test)]
mod tests;
