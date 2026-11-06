//! The cross-encoder spike of docs/issues/0001, run when
//! `MAGI_RERANKER_DIR` names a folder with `model.onnx` and
//! `tokenizer.json`. RRF's top results are scored once per query, then
//! reordered (pure rerank, or a rank-blend with RRF) and graded like the
//! other modes on their top 10. Also: the scoring time, and the "nothing
//! closely related" sweep of docs/issues/0003 on the raw logit.
//!
//! Default: every hit in the top 75 is scored. With `MAGI_SPIKE_TEXT_ONLY`
//! set, only the non-image hits of the top 30 are: a text model cannot
//! judge a photo, so images keep their RRF slots and only text hits are
//! reordered among the slots they hold.

use std::collections::BTreeMap;
use std::time::Instant;

use magi_core::discovery::Kind;
use magi_core::dto::MatchSource;
use magi_core::search::SearchHit;
use magi_core::search::rerank::CrossEncoder;

use super::{Accum, EvalQuery, FETCH_LIMIT, first_match_rank};

const LOGIT_THRESHOLDS: &[f32] = &[-8.0, -6.0, -4.0, -3.0, -2.0, -1.0, 0.0, 2.0];
/// Buckets whose answers are text, where a text cross-encoder can judge.
const TEXT_BUCKETS: &[&str] = &["en", "es", "cross", "kw"];

#[derive(Clone, Copy)]
enum Variant {
    Rrf,
    Pure(usize),
    Blend(usize),
}

impl Variant {
    fn label(self) -> String {
        match self {
            Variant::Rrf => "rrf (same lists)".into(),
            Variant::Pure(k) => format!("rerank top {k}"),
            Variant::Blend(k) => format!("blend top {k}"),
        }
    }

    /// Indexes into the RRF list, in this variant's order. Within the top
    /// `k`, the scored hits are re-sorted into the slots scored hits held;
    /// unscored hits (and everything past `k`) keep their place.
    fn order(self, scores: &[Option<f32>]) -> Vec<usize> {
        let n = scores.len();
        let mut order: Vec<usize> = (0..n).collect();
        let (k, blend) = match self {
            Variant::Rrf => return order,
            Variant::Pure(k) => (k.min(n), false),
            Variant::Blend(k) => (k.min(n), true),
        };
        let slots: Vec<usize> = (0..k).filter(|&i| scores[i].is_some()).collect();
        let score = |i: usize| scores[i].unwrap_or(f32::NEG_INFINITY);
        let mut by_score = slots.clone();
        by_score.sort_by(|&a, &b| score(b).total_cmp(&score(a)).then(a.cmp(&b)));
        let sorted = if blend {
            // RRF over the two orders of the scored hits: RRF's and the model's.
            let model_rank = |i: usize| by_score.iter().position(|&j| j == i).unwrap_or(k);
            let rrf_rank = |i: usize| slots.iter().position(|&j| j == i).unwrap_or(k);
            let fused =
                |i: usize| 1.0 / (60.0 + rrf_rank(i) as f64) + 1.0 / (60.0 + model_rank(i) as f64);
            let mut blended = slots.clone();
            blended.sort_by(|&a, &b| fused(b).total_cmp(&fused(a)).then(a.cmp(&b)));
            blended
        } else {
            by_score
        };
        for (slot, i) in slots.into_iter().zip(sorted) {
            order[slot] = i;
        }
        order
    }
}

struct Scored<'a> {
    q: &'a EvalQuery,
    paths: Vec<String>,
    keyword: Vec<bool>,
    /// The model's logit, for the hits it scored.
    scores: Vec<Option<f32>>,
}

/// `search(query, limit)` is RRF; `text(file_id)` what the model reads;
/// `rel(hit)` the hit's corpus-relative path.
pub fn run(
    dir: &std::path::Path,
    queries: &[EvalQuery],
    search: impl Fn(&str, u32) -> anyhow::Result<Vec<SearchHit>>,
    text: impl Fn(i64) -> anyhow::Result<String>,
    rel: impl Fn(&SearchHit) -> String,
) -> anyhow::Result<()> {
    let text_only = std::env::var_os("MAGI_SPIKE_TEXT_ONLY").is_some();
    let (depth, variants): (usize, &[Variant]) = if text_only {
        (30, &[Variant::Rrf, Variant::Pure(30), Variant::Blend(30)])
    } else {
        (
            75,
            &[
                Variant::Rrf,
                Variant::Pure(30),
                Variant::Pure(75),
                Variant::Blend(30),
                Variant::Blend(75),
            ],
        )
    };
    println!(
        "\n=== cross-encoder rerank{} ({}) ===",
        if text_only { ", text hits only" } else { "" },
        dir.display()
    );
    let load = Instant::now();
    let model = CrossEncoder::load(dir)?;
    println!(
        "  loaded in {:.0} ms; inputs {:?}; {} tokens per pair",
        load.elapsed().as_secs_f64() * 1000.0,
        model.input_names(),
        model.max_tokens()
    );

    let mut scored = Vec::new();
    let (mut ms30, mut ms_all, mut pairs30) = (Vec::new(), Vec::new(), Vec::new());
    for q in queries {
        let hits = search(&q.query, depth as u32 + q.ignore.len() as u32)?;
        let scorable: Vec<usize> = (0..hits.len())
            .filter(|&i| !text_only || hits[i].kind != Kind::Image)
            .collect();
        let docs: Vec<String> = scorable
            .iter()
            .map(|&i| text(hits[i].file_id))
            .collect::<anyhow::Result<_>>()?;
        let split = scorable.iter().take_while(|&&i| i < 30).count();
        let t = Instant::now();
        let mut logits = model.score(&q.query, &docs[..split])?;
        let head = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        logits.extend(model.score(&q.query, &docs[split..])?);
        ms30.push(head);
        pairs30.push(split as f64);
        ms_all.push(head + t.elapsed().as_secs_f64() * 1000.0);
        let mut scores = vec![None; hits.len()];
        for (&i, logit) in scorable.iter().zip(logits) {
            scores[i] = Some(logit);
        }
        scored.push(Scored {
            q,
            paths: hits.iter().map(&rel).collect(),
            keyword: hits
                .iter()
                .map(|h| h.match_sources.contains(&MatchSource::Keyword))
                .collect(),
            scores,
        });
    }
    let pct = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(f64::total_cmp);
        v.get(((v.len() as f64 - 1.0) * p).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "  scoring time per query (warm): top 30 p50 {:.0} / p95 {:.0} ms ({:.0} pairs p50), top {depth} p50 {:.0} / p95 {:.0} ms",
        pct(&mut ms30, 0.5),
        pct(&mut ms30, 0.95),
        pct(&mut pairs30, 0.5),
        pct(&mut ms_all, 0.5),
        pct(&mut ms_all, 0.95)
    );

    for &variant in variants {
        println!("  -- {} --", variant.label());
        let mut overall = Accum::default();
        let mut fresh = Accum::default();
        let mut by_lang: BTreeMap<&str, Accum> = BTreeMap::new();
        for s in scored.iter().filter(|s| !s.q.is_neg()) {
            let top: Vec<String> = variant
                .order(&s.scores)
                .into_iter()
                .map(|i| s.paths[i].clone())
                .filter(|p| !s.q.ignore.contains(p))
                .take(FETCH_LIMIT as usize)
                .collect();
            let rank = first_match_rank(&top, &s.q.expected, &[]);
            overall.add(rank);
            by_lang.entry(&s.q.lang).or_default().add(rank);
            if s.q.fresh {
                fresh.add(rank);
            }
        }
        overall.report("overall");
        for (lang, acc) in &by_lang {
            acc.report(lang);
        }
        fresh.report("fresh");
    }

    // 0003: a hit is strong when keyword search found it or the model
    // scored it at the threshold or above; unscored hits (images in
    // text-only mode) are not judged. On the top 10 of "rerank top 30".
    println!(
        "  label sweep, logit (neg nothing-close, false-weak text buckets, false-weak all judged, fresh)"
    );
    for &t in LOGIT_THRESHOLDS {
        let (mut neg, mut neg_nothing) = (0, 0);
        let (mut text_found, mut text_weak, mut found, mut weak, mut fresh_found, mut fresh_weak) =
            (0, 0, 0, 0, 0, 0);
        for s in &scored {
            let top: Vec<usize> = Variant::Pure(30)
                .order(&s.scores)
                .into_iter()
                .filter(|&i| !s.q.ignore.contains(&s.paths[i]))
                .take(FETCH_LIMIT as usize)
                .collect();
            let strong = |i: usize| s.keyword[i] || s.scores[i].is_some_and(|l| l >= t);
            if s.q.is_neg() {
                neg += 1;
                neg_nothing += usize::from(!top.iter().any(|&i| strong(i)));
                continue;
            }
            let Some(&i) = top.iter().find(|&&i| s.q.expected.contains(&s.paths[i])) else {
                continue;
            };
            if !s.keyword[i] && s.scores[i].is_none() {
                continue;
            }
            let w = usize::from(!strong(i));
            found += 1;
            weak += w;
            if TEXT_BUCKETS.contains(&s.q.lang.as_str()) {
                text_found += 1;
                text_weak += w;
            }
            if s.q.fresh {
                fresh_found += 1;
                fresh_weak += w;
            }
        }
        println!(
            "    {t:>5.1}: neg {neg_nothing}/{neg}  false-weak text {text_weak}/{text_found}  all {weak}/{found}  fresh {fresh_weak}/{fresh_found}"
        );
    }

    println!("  neg detail: best logit in the top 30");
    for s in scored.iter().filter(|s| s.q.is_neg()) {
        let best = s
            .scores
            .iter()
            .take(30)
            .flatten()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        println!("    {:<28} {best:>6.2}", s.q.query);
    }
    Ok(())
}
