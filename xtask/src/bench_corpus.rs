//! `xtask bench-corpus`: builds a synthetic 100k-chunk index and measures
//! search latency against SPEC.md §7 M3's NFR-2 (warm, p95 <= 300 ms) and
//! NFR-3 (cold, <= 3 s) targets (see SPEC.md §2.2, §8's "Performance" row).
//!
//! Corpus content and vectors are synthetic (random, not real embeddings) —
//! this benchmarks search latency at realistic index *size*, not retrieval
//! quality (that's `magi-cli eval`'s job, against the real embedder and a
//! real fixture corpus). Only the query side uses the real `E5Embedder`,
//! since that's the actual cost path NFR-2/NFR-3 care about.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use magi_core::db;
use magi_core::db::files::{FileRecord, upsert_file};
use magi_core::embed::TEXT_EMBEDDING_DIM;
use magi_core::embed::e5::E5Embedder;
use magi_core::extract::RawChunk;
use magi_core::search;

const CHUNKS_PER_FILE: usize = 20;
const TOTAL_CHUNKS: usize = 100_000;
const FILE_COUNT: usize = TOTAL_CHUNKS / CHUNKS_PER_FILE;
const WARM_QUERIES: usize = 200;
const WORDS_PER_CHUNK: usize = 40;

const VOCAB: &[&str] = &[
    "invoice",
    "electrician",
    "recipe",
    "arepas",
    "meeting",
    "notes",
    "travel",
    "itinerary",
    "doctor",
    "appointment",
    "lease",
    "agreement",
    "grocery",
    "list",
    "bug",
    "report",
    "birthday",
    "party",
    "car",
    "maintenance",
    "book",
    "club",
    "quarterly",
    "budget",
    "contract",
    "apartment",
    "system",
    "design",
    "review",
    "project",
    "update",
    "plan",
    "schedule",
    "client",
    "payment",
    "service",
    "repair",
    "panel",
    "electrical",
    "kitchen",
];

const QUERIES: &[&str] = &[
    "electrician invoice",
    "receta de arepas",
    "travel itinerary",
    "grocery list",
    "bug report",
    "car maintenance",
    "lease agreement",
    "birthday party",
    "book club",
    "meeting notes",
];

/// xorshift64* — deterministic, dependency-free PRNG. Only needs to spread
/// synthetic corpus content/vectors realistically, not to be cryptographic.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_unit_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }

    fn pick<'a>(&mut self, words: &[&'a str]) -> &'a str {
        words[(self.next_u64() as usize) % words.len()]
    }
}

fn synthetic_text(rng: &mut Rng, words: usize) -> String {
    (0..words)
        .map(|_| rng.pick(VOCAB))
        .collect::<Vec<_>>()
        .join(" ")
}

fn random_unit_vector(rng: &mut Rng, dim: usize) -> Vec<f32> {
    let mut v: Vec<f32> = (0..dim).map(|_| rng.next_unit_f32()).collect();
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted_ms.len() as f64 - 1.0)).round() as usize;
    sorted_ms[idx.min(sorted_ms.len() - 1)]
}

pub fn bench_corpus() -> Result<()> {
    println!(
        "building synthetic corpus: {FILE_COUNT} files x {CHUNKS_PER_FILE} chunks = {TOTAL_CHUNKS} chunks"
    );
    let root_dir = tempfile::tempdir()?;
    let db_dir = tempfile::tempdir()?;
    let mut conn = db::open(&db_dir.path().join("bench.db"))?;
    let root = db::roots::add(&conn, root_dir.path())?;

    let mut rng = Rng(0x9E3779B97F4A7C15);
    let build_start = Instant::now();
    for i in 0..FILE_COUNT {
        let file_name = format!("file_{i:05}.txt");
        let path = root_dir.path().join(&file_name);
        let rel = PathBuf::from(&file_name);
        let chunks: Vec<RawChunk> = (0..CHUNKS_PER_FILE)
            .map(|_| RawChunk::body(synthetic_text(&mut rng, WORDS_PER_CHUNK)))
            .collect();
        let embeddings: Vec<Vec<f32>> = (0..CHUNKS_PER_FILE)
            .map(|_| random_unit_vector(&mut rng, TEXT_EMBEDDING_DIM))
            .collect();
        let record = FileRecord {
            root_id: root.id,
            path: &path,
            rel_path: &rel,
            file_name: &file_name,
            ext: Some("txt"),
            kind: "text",
            size: 0,
            mtime_ns: i as i64,
            lang: Some("en"),
            state: magi_core::db::files::FileState::Indexed,
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
        };
        upsert_file(&mut conn, &record, &chunks, &embeddings, None)?;
        if i > 0 && i % 1000 == 0 {
            println!("  ... {i}/{FILE_COUNT} files");
        }
    }
    println!(
        "corpus built in {:.1}s",
        build_start.elapsed().as_secs_f64()
    );

    // Fold the WAL back into the main database before measuring. Bulk-loading
    // 100k rows through 5,000 transactions leaves a large WAL that every
    // reader has to consult, and SQLite's own checkpoint landing inside the
    // measured window made warm p95 bimodal across runs (231-478 ms for the
    // same code). A real index isn't mid-bulk-load when a query arrives, so
    // checkpointing here measures steady-state search rather than
    // write-ahead-log drain.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;

    println!("\nloading real E5Embedder (cold: model not yet in memory)...");
    let cold_start = Instant::now();
    let embedder = E5Embedder::load()?;
    let first_hits = search::hybrid_search(&conn, &embedder, None, QUERIES[0], 30)?;
    let cold_ms = cold_start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "cold latency (model load + first search): {cold_ms:.1} ms  [{} hits]  -- NFR-3 target: <= 3000 ms",
        first_hits.len()
    );

    let mut warm_ms = Vec::with_capacity(WARM_QUERIES);
    for i in 0..WARM_QUERIES {
        let query = QUERIES[i % QUERIES.len()];
        let t = Instant::now();
        search::hybrid_search(&conn, &embedder, None, query, 30)?;
        warm_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    warm_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = percentile(&warm_ms, 50.0);
    let p95 = percentile(&warm_ms, 95.0);
    let max = warm_ms.last().copied().unwrap_or(0.0);
    println!(
        "warm latency over {WARM_QUERIES} queries: p50={p50:.1} ms  p95={p95:.1} ms  max={max:.1} ms  -- NFR-2 target: p95 <= 300 ms"
    );

    println!(
        "\nNFR-3 (cold <= 3000 ms): {}",
        if cold_ms <= 3000.0 { "PASS" } else { "FAIL" }
    );
    println!(
        "NFR-2 (warm p95 <= 300 ms): {}",
        if p95 <= 300.0 { "PASS" } else { "FAIL" }
    );

    Ok(())
}
