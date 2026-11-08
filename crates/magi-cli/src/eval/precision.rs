//! Precision (docs/issues/0002): what a result list shows besides the
//! answer. For each query it keeps the hits and the two candidate scales of
//! `search::strength`, then sweeps the strong/weak thresholds:
//!
//! - `neg` queries (no answer in the corpus): how many lists hold no strong
//!   result, i.e. would read "Nothing closely related found".
//! - positive queries: the false-weak rate, the share whose expected file is
//!   found but labelled weak (folded). This is the number to keep near zero.

use magi_core::search::SearchHit;
use magi_core::search::strength::{Scale, Scales, Strength, Thresholds, fold_strength, strength};

const TEXT_Z: &[f64] = &[1.0, 1.5, 2.0, 2.5, 3.0, 3.5];
const IMAGE_Z: &[f64] = &[2.0, 2.5, 3.0, 3.5, 4.0, 5.0];
/// Raw cosine thresholds: e5 text cosines sit near 0.8, SigLIP's near 0.1
/// (`search::IMAGE_MIN_COSINE` is 0.10).
const TEXT_COSINE: &[f64] = &[0.80, 0.82, 0.84, 0.86, 0.88, 0.90];
const IMAGE_COSINE: &[f64] = &[0.10, 0.12, 0.14, 0.16];
const IDENTITY: Scale = Scale {
    mean: 0.0,
    deviation: 1.0,
};

/// One query's result list under one fusion, ready to be labelled.
pub struct Judged {
    pub query: String,
    pub lang: String,
    pub fresh: bool,
    pub hits: Vec<SearchHit>,
    /// Index in `hits` of the first expected file, ignored hits skipped.
    pub expected_at: Option<usize>,
    /// Scale from a fixed sample of each table (`SCALE_SAMPLE` rows).
    pub sample: Scales,
    /// Scale from the nearest rows the RRF lists fetch (100 chunks, 50 images).
    pub fetched: Scales,
}

impl Judged {
    pub fn is_neg(&self) -> bool {
        self.lang == "neg"
    }
}

#[derive(Default)]
struct Tally {
    neg: usize,
    neg_nothing: usize,
    found: usize,
    false_weak: usize,
    fresh_found: usize,
    fresh_false_weak: usize,
    /// Positive queries whose whole list is weak.
    pos_nothing: usize,
}

fn tally(records: &[Judged], scales: impl Fn(&Judged) -> Scales, t: &Thresholds) -> Tally {
    let mut tally = Tally::default();
    for r in records {
        let s = scales(r);
        let label = |h: &SearchHit| strength(h, &s, t);
        let any_strong = r.hits.iter().any(|h| label(h) == Strength::Strong);
        if r.is_neg() {
            tally.neg += 1;
            tally.neg_nothing += usize::from(!any_strong);
            continue;
        }
        tally.pos_nothing += usize::from(!any_strong);
        if let Some(i) = r.expected_at {
            let weak = label(&r.hits[i]) == Strength::Weak;
            tally.found += 1;
            tally.false_weak += usize::from(weak);
            if r.fresh {
                tally.fresh_found += 1;
                tally.fresh_false_weak += usize::from(weak);
            }
        }
    }
    tally
}

/// What the shipped fold does to lists of the app's default length.
#[derive(Default)]
pub struct FoldTally {
    positives: usize,
    positive_results: usize,
    positive_weak: usize,
    found: usize,
    answer_folded: usize,
    fresh_found: usize,
    fresh_answer_folded: usize,
    negs: usize,
    neg_results: usize,
    neg_weak: usize,
    neg_all_weak: usize,
}

impl FoldTally {
    pub fn add(&mut self, hits: &[SearchHit], expected_at: Option<usize>, neg: bool, fresh: bool) {
        let weak = hits
            .iter()
            .filter(|h| fold_strength(h) == Strength::Weak)
            .count();
        if neg {
            self.negs += 1;
            self.neg_results += hits.len();
            self.neg_weak += weak;
            self.neg_all_weak += usize::from(weak == hits.len());
            return;
        }
        self.positives += 1;
        self.positive_results += hits.len();
        self.positive_weak += weak;
        if let Some(i) = expected_at {
            let folded = fold_strength(&hits[i]) == Strength::Weak;
            self.found += 1;
            self.answer_folded += usize::from(folded);
            if fresh {
                self.fresh_found += 1;
                self.fresh_answer_folded += usize::from(folded);
            }
        }
    }

    pub fn report(&self, limit: u32) {
        let per = |total: usize, n: usize| total as f64 / n.max(1) as f64;
        println!(
            "  shipped fold, top {limit}: positives {:.1} results/query, {:.1} folded; answer folded {}/{} (fresh {}/{})",
            per(self.positive_results, self.positives),
            per(self.positive_weak, self.positives),
            self.answer_folded,
            self.found,
            self.fresh_answer_folded,
            self.fresh_found,
        );
        println!(
            "  shipped fold, top {limit}: neg {:.1} results/query, {:.1} folded; whole list folded {}/{}",
            per(self.neg_results, self.negs),
            per(self.neg_weak, self.negs),
            self.neg_all_weak,
            self.negs,
        );
    }
}

/// The neg bucket's list sizes, then the threshold sweep for both scales.
pub fn report(records: &[Judged]) {
    let negs: Vec<&Judged> = records.iter().filter(|r| r.is_neg()).collect();
    if !negs.is_empty() {
        let results: usize = negs.iter().map(|r| r.hits.len()).sum();
        println!(
            "  neg      n={:<3} results/query={:.1}",
            negs.len(),
            results as f64 / negs.len() as f64
        );
    }
    type Pick = fn(&Judged) -> Scales;
    let sweeps: [(&str, Pick, &[f64], &[f64]); 3] = [
        ("sample", |r| r.sample, TEXT_Z, IMAGE_Z),
        ("fetched", |r| r.fetched, TEXT_Z, IMAGE_Z),
        // No scale: the thresholds are raw cosines (`z == cosine`).
        (
            "cosine",
            |_| Scales {
                text: Some(IDENTITY),
                image: Some(IDENTITY),
            },
            TEXT_COSINE,
            IMAGE_COSINE,
        ),
    ];
    for (name, pick, text_grid, image_grid) in sweeps {
        println!(
            "  label sweep, {name} scale (text / image threshold: neg nothing-close, false-weak all, false-weak fresh, positives all-weak)"
        );
        for &text_z in text_grid {
            for &image_z in image_grid {
                let t = tally(records, pick, &Thresholds { text_z, image_z });
                println!(
                    "    {text_z:.2} / {image_z:.2}: neg {}/{}  false-weak {}/{}  fresh {}/{}  all-weak {}",
                    t.neg_nothing,
                    t.neg,
                    t.false_weak,
                    t.found,
                    t.fresh_false_weak,
                    t.fresh_found,
                    t.pos_nothing,
                );
            }
        }
    }
}

/// Per neg query: the best z any of its results reaches, text and visual,
/// on the sample scale, and whether keyword search found anything.
pub fn report_neg_detail(records: &[Judged]) {
    println!("  neg detail (sample scale): best text z / best image z / keyword hit");
    for r in records.iter().filter(|r| r.is_neg()) {
        let best = |sim: fn(&SearchHit) -> Option<f64>, scale: Option<Scale>| {
            scale.and_then(|s| {
                r.hits
                    .iter()
                    .filter_map(sim)
                    .map(|c| s.z(c))
                    .max_by(f64::total_cmp)
            })
        };
        let fmt = |z: Option<f64>| z.map_or("-".to_string(), |z| format!("{z:.2}"));
        let keyword = r.hits.iter().any(|h| {
            h.match_sources
                .contains(&magi_core::dto::MatchSource::Keyword)
        });
        println!(
            "    {:<28} {:>6} {:>6} {}",
            r.query,
            fmt(best(|h| h.text_similarity, r.sample.text)),
            fmt(best(|h| h.image_similarity, r.sample.image)),
            if keyword { "yes" } else { "no" },
        );
    }
}
