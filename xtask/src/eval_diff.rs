//! `eval-all` saves one eval report per corpus and `eval-diff` compares two
//! saved runs against the adoption rule (`eval/README.md`).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Query file and corpus folder of each eval.
const CORPORA: [(&str, &str); 3] = [
    ("queries", "corpus"),
    ("random_names", "random_names"),
    ("realistic_names", "realistic_names"),
];
const METRICS: [&str; 3] = ["recall@5", "recall@10", "mrr"];
/// Reports print three decimals; a smaller gap is rounding.
const EPSILON: f64 = 0.0005;

struct Bucket {
    name: String,
    values: [f64; 3],
}

#[derive(Debug, Default, PartialEq)]
struct Comparison {
    /// Buckets that moved, one line each, and fold lines that changed.
    lines: Vec<String>,
    /// `bucket metric` for every drop.
    regressions: Vec<String>,
}

/// The lines of the shipped ranking's section of a report.
fn hybrid_section(report: &str) -> impl Iterator<Item = &str> {
    report
        .lines()
        .skip_while(|l| l.trim() != "=== hybrid (rrf) ===")
        .skip(1)
        .take_while(|l| !l.starts_with("==="))
}

fn parse_bucket(line: &str) -> Option<Bucket> {
    let (name, rest) = line.trim().split_once(" n=")?;
    let mut values = [0.0; 3];
    for (value, metric) in values.iter_mut().zip(METRICS) {
        let (_, after) = rest.split_once(&format!("{metric}="))?;
        *value = after.split_whitespace().next()?.parse().ok()?;
    }
    Some(Bucket {
        name: name.trim().to_string(),
        values,
    })
}

fn buckets(report: &str) -> Vec<Bucket> {
    hybrid_section(report).filter_map(parse_bucket).collect()
}

fn fold_lines(report: &str) -> Vec<&str> {
    hybrid_section(report)
        .map(str::trim)
        .filter(|l| l.starts_with("shipped fold"))
        .collect()
}

fn compare(before: &str, after: &str) -> Comparison {
    let mut result = Comparison::default();
    let (old, new) = (buckets(before), buckets(after));
    for bucket in &old {
        let Some(current) = new.iter().find(|b| b.name == bucket.name) else {
            result
                .lines
                .push(format!("{}  only in one run", bucket.name));
            result.regressions.push(format!("{} missing", bucket.name));
            continue;
        };
        compare_bucket(bucket, current, &mut result);
    }
    for bucket in new.iter().filter(|b| !old.iter().any(|o| o.name == b.name)) {
        result
            .lines
            .push(format!("{}  only in one run", bucket.name));
    }
    let (old_fold, new_fold) = (fold_lines(before), fold_lines(after));
    for (was, now) in old_fold.iter().zip(&new_fold).filter(|(a, b)| a != b) {
        result.lines.push(format!("was: {was}"));
        result.lines.push(format!("now: {now}"));
    }
    result
}

fn compare_bucket(old: &Bucket, new: &Bucket, result: &mut Comparison) {
    let mut moved = String::new();
    let mut dropped = false;
    for (metric, (was, now)) in METRICS.iter().zip(old.values.iter().zip(new.values)) {
        if (now - was).abs() <= EPSILON {
            continue;
        }
        moved.push_str(&format!("  {metric} {was:.3} -> {now:.3}"));
        if now < *was {
            dropped = true;
            result.regressions.push(format!("{} {metric}", old.name));
        }
    }
    if !moved.is_empty() {
        let mark = if dropped { "  REGRESSED" } else { "" };
        result.lines.push(format!("{}{moved}{mark}", old.name));
    }
}

fn reports_dir(run: &str) -> PathBuf {
    Path::new("target/eval").join(run)
}

/// Runs the release `magi-cli` eval on every corpus present and saves each
/// report under `target/eval/<run>/`. Two corpora are local-only.
pub fn eval_all(run: Option<&str>) -> Result<()> {
    let run = run.context("usage: eval-all <run-name>")?;
    let dir = reports_dir(run);
    fs::create_dir_all(&dir)?;
    let cli = format!("target/release/magi-cli{}", std::env::consts::EXE_SUFFIX);
    for (queries, corpus) in CORPORA {
        let folder = format!("fixtures/{corpus}");
        if !Path::new(&folder).is_dir() {
            println!("{queries}: skipped, {folder} is not on this machine");
            continue;
        }
        println!("{queries}: running");
        let output = Command::new(&cli)
            .args([
                "eval",
                &format!("eval/{queries}.jsonl"),
                "--corpus",
                &folder,
            ])
            .output()
            .with_context(|| format!("running {cli}"))?;
        if !output.status.success() {
            bail!(
                "{queries}: eval failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fs::write(dir.join(format!("{queries}.txt")), output.stdout)?;
    }
    println!("saved in {}", dir.display());
    Ok(())
}

/// Prints what moved between two `eval-all` runs and every regression.
pub fn eval_diff(before: Option<&str>, after: Option<&str>) -> Result<()> {
    let (Some(before), Some(after)) = (before, after) else {
        bail!("usage: eval-diff <before-run> <after-run>");
    };
    let mut regressions = Vec::new();
    let mut compared = 0;
    let mut out = String::new();
    for (queries, _) in CORPORA {
        let name = format!("{queries}.txt");
        let a = reports_dir(before).join(&name);
        let b = reports_dir(after).join(&name);
        let (Ok(a), Ok(b)) = (fs::read_to_string(a), fs::read_to_string(b)) else {
            writeln!(out, "--- {queries}: skipped, not in both runs")?;
            continue;
        };
        compared += 1;
        let comparison = compare(&a, &b);
        writeln!(out, "--- {queries}")?;
        if comparison.lines.is_empty() {
            writeln!(out, "  no change")?;
        }
        for line in comparison.lines {
            writeln!(out, "  {line}")?;
        }
        regressions.extend(
            comparison
                .regressions
                .into_iter()
                .map(|r| format!("{queries}: {r}")),
        );
    }
    if compared == 0 {
        bail!("no corpus is in both runs (target/eval/{before}, target/eval/{after})");
    }
    print!("{out}");
    let total = CORPORA.len();
    match regressions.is_empty() {
        true => println!("\nno bucket regresses ({compared} of {total} corpora)"),
        false => println!(
            "\nREGRESSED ({}, {compared} of {total} corpora):\n  {}",
            regressions.len(),
            regressions.join("\n  ")
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(overall_mrr: &str, cross_r10: &str) -> String {
        [
            "=== fts ===".to_string(),
            "  overall  n=9 recall@5=0.100  recall@10=0.100  mrr=0.100".to_string(),
            String::new(),
            "=== hybrid (rrf) ===".to_string(),
            "  miss [cross] \"a\": rank None, top hit b.txt".to_string(),
            format!("  overall  n=9 recall@5=0.900  recall@10=0.900  mrr={overall_mrr}"),
            format!("  cross    n=3 recall@5=0.500  recall@10={cross_r10}  mrr=0.500"),
            "  as shown n=9 recall@5=0.900  recall@10=0.900  mrr=0.800".to_string(),
            "  shipped fold, top 30: neg 30.0 results/query, 1.0 folded".to_string(),
            "  neg      n=18  results/query=10.0".to_string(),
            String::new(),
            "=== hybrid (zscore) ===".to_string(),
            "  overall  n=9 recall@5=0.000  recall@10=0.000  mrr=0.000".to_string(),
        ]
        .join("\n")
    }

    #[test]
    fn identical_runs_have_nothing_to_report() {
        let r = report("0.800", "0.600");
        assert_eq!(compare(&r, &r), Comparison::default());
    }

    #[test]
    fn a_gain_is_shown_and_is_not_a_regression() {
        let c = compare(&report("0.800", "0.600"), &report("0.800", "0.800"));
        assert_eq!(c.lines, ["cross  recall@10 0.600 -> 0.800"]);
        assert!(c.regressions.is_empty());
    }

    #[test]
    fn any_drop_is_a_regression_even_when_another_metric_rises() {
        let c = compare(&report("0.800", "0.600"), &report("0.750", "0.800"));
        assert_eq!(c.regressions, ["overall mrr"]);
        assert_eq!(c.lines[0], "overall  mrr 0.800 -> 0.750  REGRESSED");
    }

    #[test]
    fn only_the_shipped_ranking_is_compared() {
        let a = report("0.800", "0.600");
        let b = a
            .replace("mrr=0.000", "mrr=0.900")
            .replace("mrr=0.100", "mrr=0.050");
        assert_eq!(compare(&a, &b), Comparison::default());
    }

    #[test]
    fn a_changed_fold_line_is_shown() {
        let a = report("0.800", "0.600");
        let b = a.replace("1.0 folded", "9.0 folded");
        let c = compare(&a, &b);
        assert_eq!(c.lines.len(), 2);
        assert!(c.lines[1].contains("9.0 folded"));
        assert!(c.regressions.is_empty());
    }

    #[test]
    fn a_bucket_in_one_run_only_is_flagged() {
        let a = report("0.800", "0.600");
        let b = a.replace(
            "  as shown n=9 recall@5=0.900  recall@10=0.900  mrr=0.800\n",
            "",
        );
        let c = compare(&a, &b);
        assert_eq!(c.lines, ["as shown  only in one run"]);
        assert_eq!(c.regressions, ["as shown missing"]);
    }
}
