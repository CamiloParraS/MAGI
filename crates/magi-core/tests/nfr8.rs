//! NFR-8, carried over from M5: search latency while the first index runs,
//! with the real models from the dev data directory (`just models`). Run
//! `cargo test -p magi-core --release --test nfr8 -- --ignored --nocapture`
//! and record the output in docs/benchmarks.md.

use std::path::Path;
use std::time::{Duration, Instant};

use magi_core::config::Config;
use magi_core::dto::{IndexState, SearchRequest};
use magi_core::host::{Host, HostPaths};

fn summary(v: &mut [Duration]) -> String {
    v.sort();
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    format!(
        "n {} p50 {:?} p95 {:?} max {:?}",
        v.len(),
        at(0.5),
        at(0.95),
        v[v.len() - 1]
    )
}

#[test]
#[ignore = "needs the real models (just models); records NFR-8 numbers"]
fn search_latency_while_indexing() {
    let dir = tempfile::tempdir().unwrap();
    let paths = HostPaths {
        db: dir.path().join("magi.db"),
        config: dir.path().join("config.toml"),
    };
    let mut config = Config::default();
    config.features.image_visual = true;
    std::fs::write(&paths.config, toml::to_string(&config).unwrap()).unwrap();
    let host = Host::start(paths, |_| {}).unwrap();
    let engine = loop {
        if let Ok(e) = host.engine() {
            break e;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    engine
        .add_root(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus"))
        .unwrap();

    let queries = [
        "invoice",
        "receta de arepas",
        "a cat on a sofa",
        "quarterly report",
        "wifi password",
    ];
    let search = |q: &str| {
        let t = Instant::now();
        host.search(&SearchRequest {
            query: q.into(),
            limit: Some(30),
        })
        .unwrap();
        t.elapsed()
    };
    let mut busy = Vec::new();
    for q in queries.iter().cycle() {
        let s = engine.status().unwrap();
        if !busy.is_empty() && s.state == IndexState::Idle && s.queued == 0 {
            break;
        }
        busy.push(search(q));
        std::thread::sleep(Duration::from_millis(200)); // a person typing, debounced
    }
    let mut idle: Vec<_> = queries.iter().map(|q| search(q)).collect();
    println!("NFR-8 while indexing: {}", summary(&mut busy));
    println!("NFR-8 idle, warm:     {}", summary(&mut idle));
    host.shutdown();
}
