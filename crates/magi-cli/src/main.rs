//! magi-cli: dev/test CLI (doctor, roots, index, daemon, search, eval — added
//! milestone by milestone; see SPEC.md §7).

mod eval;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use magi_core::dto::{IndexStatus, RootStatus};
use magi_core::embed::{E5Embedder, FakeEmbedder, TextEmbedder};
use magi_core::embed::{FakeImageEmbedder, ImageEmbedder, SigLipEmbedder};
use magi_core::index::pipeline::{IndexContext, IndexRootOptions, index_root};
use magi_core::ocr::{NoOcr, OcrEngine, paddle::PaddleOcr};
use magi_core::search::fts::search_fts;
use magi_core::{Engine, config, db, paths};
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Parser)]
#[command(name = "magi-cli")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print resolved directories and database diagnostics.
    Doctor,
    /// Manage indexed root folders.
    Roots {
        #[command(subcommand)]
        action: RootsAction,
    },
    /// One-shot index of a root folder (no watcher; see SPEC.md §7 M2).
    Index { root: PathBuf },
    /// Run the engine headless until Ctrl-C: watch the roots, index, and
    /// print the status whenever it changes (SPEC.md §7 M5).
    Daemon {
        /// Also print this process's CPU and memory use once a minute.
        #[arg(long)]
        stats: bool,
    },
    /// Search indexed content.
    Search {
        query: String,
        #[arg(long, default_value = "fts")]
        mode: String,
        #[arg(long, default_value_t = 30)]
        limit: u32,
    },
    /// Index `--corpus` and report recall@5, recall@10, and MRR for
    /// fts/vector/hybrid search against a queries.jsonl file.
    Eval {
        queries: PathBuf,
        #[arg(long)]
        corpus: PathBuf,
    },
}

#[derive(Subcommand)]
enum RootsAction {
    Add { path: PathBuf },
    List,
    Remove { id: i64 },
}

fn db_path() -> PathBuf {
    paths::data_dir().join("magi.db")
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => doctor()?,
        Command::Roots { action } => roots(action)?,
        Command::Index { root } => index_cmd(root)?,
        Command::Daemon { stats } => daemon_cmd(stats)?,
        Command::Search { query, mode, limit } => search_cmd(&query, &mode, limit)?,
        Command::Eval { queries, corpus } => eval::eval_cmd(queries, corpus)?,
    }
    Ok(())
}

fn doctor() -> anyhow::Result<()> {
    println!("data dir:   {}", paths::data_dir().display());
    println!("config dir: {}", paths::config_dir().display());
    println!("cache dir:  {}", paths::cache_dir().display());

    std::fs::create_dir_all(paths::data_dir())?;
    let conn = db::open(&db_path())?;
    println!("sqlite version: {}", db::sqlite_version(&conn)?);
    println!("ENABLE_FTS5 = {}", db::fts5_enabled(&conn)? as i32);
    println!("vec_version: {}", db::vec_version(&conn)?);

    if let Some(limit) = magi_core::platform::inotify_watch_limit() {
        println!("inotify max_user_watches: {limit}");
    }
    Ok(())
}

fn roots(action: RootsAction) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths::data_dir())?;
    let conn = db::open(&db_path())?;
    match action {
        RootsAction::Add { path } => {
            let root = db::roots::add(&conn, &path)?;
            println!("added root {} ({})", root.id, root.path.display());
        }
        RootsAction::List => {
            for root in db::roots::list(&conn)? {
                println!(
                    "{}\t{}\t{}\t{}",
                    root.id,
                    root.path.display(),
                    root.enabled,
                    root.status
                );
            }
        }
        RootsAction::Remove { id } => {
            db::roots::remove(&conn, id)?;
            println!("removed root {id}");
        }
    }
    Ok(())
}

fn embedder_from_env() -> anyhow::Result<std::sync::Arc<dyn TextEmbedder>> {
    if std::env::var("MAGI_FAKE_EMBEDDER").as_deref() == Ok("1") {
        Ok(std::sync::Arc::new(FakeEmbedder))
    } else {
        Ok(std::sync::Arc::new(E5Embedder::load().map_err(|e| {
            anyhow::anyhow!(
                "loading the real text embedder failed: {e}\n\
                 run `just models` and `cargo xtask fetch-onnxruntime` first, \
                 or set MAGI_FAKE_EMBEDDER=1 to index/search with the fake one"
            )
        })?))
    }
}

/// Real OCR unless the fake embedder is on (tests stay deterministic and
/// model-free). Missing OCR models are not fatal: images just get no
/// recognized text.
fn ocr_from_env() -> std::sync::Arc<dyn OcrEngine> {
    if std::env::var("MAGI_FAKE_EMBEDDER").as_deref() == Ok("1") {
        return std::sync::Arc::new(NoOcr);
    }
    match PaddleOcr::load() {
        Ok(ocr) => std::sync::Arc::new(ocr),
        Err(e) => {
            eprintln!("warning: OCR unavailable ({e}); images will be indexed without text");
            std::sync::Arc::new(NoOcr)
        }
    }
}

/// The fake one under `MAGI_FAKE_EMBEDDER=1`; otherwise SigLIP, which loads
/// nothing until the first image or query (and degrades if its models are
/// missing).
fn image_embedder_from_env() -> std::sync::Arc<dyn ImageEmbedder> {
    if std::env::var("MAGI_FAKE_EMBEDDER").as_deref() == Ok("1") {
        std::sync::Arc::new(FakeImageEmbedder)
    } else {
        std::sync::Arc::new(SigLipEmbedder::new())
    }
}

fn index_cmd(root: PathBuf) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths::data_dir())?;
    let mut conn = db::open(&db_path())?;
    let config = config::load()?;
    let embedder = embedder_from_env()?;

    let root_row = match db::roots::add(&conn, &root) {
        Ok(r) => r,
        Err(magi_core::Error::RootAlreadyExists(canonical)) => db::roots::list(&conn)?
            .into_iter()
            .find(|r| r.path == canonical)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "root {} vanished after being reported as already registered",
                    canonical.display()
                )
            })?,
        Err(e) => return Err(e.into()),
    };

    let options = IndexRootOptions::from_config(&config.indexing)?;
    let summary = index_root(
        &mut conn,
        root_row.id,
        &root_row.path,
        &options,
        &IndexContext {
            embedder: Some(embedder.clone()),
            ocr: Some(ocr_from_env()),
            image_gate: Default::default(),
            image_embedder: Some(image_embedder_from_env()),
        },
    )?;

    println!(
        "indexed: {}  unchanged: {}  moved: {}  removed: {}  skipped: {}  errors: {}",
        summary.indexed,
        summary.unchanged,
        summary.moved,
        summary.removed,
        summary.skipped,
        summary.errored
    );
    Ok(())
}

fn daemon_cmd(stats: bool) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths::data_dir())?;
    let config = config::load()?;
    let engine = Engine::start(
        &config,
        &db_path(),
        magi_core::features::Components {
            text: Some(embedder_from_env()?),
            image: Some(image_embedder_from_env()),
            ocr: Some(ocr_from_env()),
        },
    )?;
    let stop = Arc::new(AtomicBool::new(false));
    ctrlc::set_handler({
        let stop = stop.clone();
        move || stop.store(true, Ordering::SeqCst)
    })?;

    let events = engine.subscribe();
    let started = Instant::now();
    let mut last: Option<IndexStatus> = None;
    let mut print = |status: &IndexStatus| {
        if last.as_ref() == Some(status) {
            return;
        }
        print_status(started.elapsed(), status);
        if last.as_ref().is_none_or(|l| l.roots != status.roots) {
            print_roots(&status.roots);
        }
        last = Some(status.clone());
    };
    print(&engine.status()?);
    let mut sampler = stats.then(Sampler::new).transpose()?;
    while !stop.load(Ordering::SeqCst) {
        if let Ok(status) = events.recv_timeout(Duration::from_millis(500)) {
            print(&status);
        }
        if let Some(sampler) = sampler.as_mut() {
            sampler.tick();
        }
    }
    println!("stopping...");
    engine.shutdown();
    println!("stopped");
    Ok(())
}

fn print_status(elapsed: Duration, s: &IndexStatus) {
    println!(
        "[{:>6}s] {:<8}  queued {}  indexed {}  skipped {}  errors {}{}",
        elapsed.as_secs(),
        format!("{:?}", s.state).to_lowercase(),
        s.queued,
        s.indexed,
        s.skipped,
        s.errors,
        s.current_file
            .as_deref()
            .map(|f| format!("  {f}"))
            .unwrap_or_default()
    );
}

fn print_roots(roots: &[RootStatus]) {
    for root in roots {
        let enabled = if root.enabled { "" } else { " (disabled)" };
        println!("  root {} {}: {}{enabled}", root.id, root.path, root.status);
    }
}

/// Prints this process's CPU, resident and private memory once a minute (`--stats`),
/// for the SPEC.md §7 M5 idle-CPU benchmark. CPU is averaged over the minute;
/// 100% is one full core.
struct Sampler {
    sys: System,
    pid: Pid,
    next: Instant,
}

impl Sampler {
    const EVERY: Duration = Duration::from_secs(60);

    fn new() -> anyhow::Result<Self> {
        let pid = sysinfo::get_current_pid().map_err(|e| anyhow::anyhow!(e))?;
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        Ok(Self {
            sys,
            pid,
            next: Instant::now() + Self::EVERY,
        })
    }

    fn tick(&mut self) {
        if Instant::now() < self.next {
            return;
        }
        self.next += Self::EVERY;
        self.sys
            .refresh_processes(ProcessesToUpdate::Some(&[self.pid]), true);
        if let Some(p) = self.sys.process(self.pid) {
            // On Windows sysinfo's `virtual_memory` is `PrivateUsage`: unlike
            // the working set, it doesn't drop when Windows trims pages.
            // Elsewhere it's the virtual size, so read it only on Windows.
            println!(
                "stats: cpu {:.2}% of one core, rss {} MB, private {} MB",
                p.cpu_usage(),
                p.memory() / (1024 * 1024),
                p.virtual_memory() / (1024 * 1024)
            );
        }
    }
}

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

fn search_cmd(query: &str, mode: &str, limit: u32) -> anyhow::Result<()> {
    let conn = db::open(&db_path())?;
    match mode {
        "fts" => {
            let hits = search_fts(&conn, query, limit)?;
            if hits.is_empty() {
                println!("no results");
            }
            for hit in hits {
                println!("{}\t{}", hit.path.display(), hit.snippet);
            }
        }
        "hybrid" => {
            let embedder = embedder_from_env()?;
            let hits = magi_core::search::hybrid_search(
                &conn,
                Some(&OneShotQuery(embedder.as_ref())),
                Some(image_embedder_from_env().as_ref()),
                query,
                limit,
            )?;
            if hits.is_empty() {
                println!("no results");
            }
            for hit in hits {
                println!(
                    "{}\t{:.4}\t{:?}\t{}",
                    hit.path.display(),
                    hit.score,
                    hit.match_sources,
                    hit.snippet
                );
            }
        }
        other => anyhow::bail!("unsupported search mode {other:?}; use \"fts\" or \"hybrid\""),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<&'static str>>);

    impl TextEmbedder for Recorder {
        fn model_id(&self) -> &str {
            "recorder"
        }
        fn dim(&self) -> usize {
            1
        }
        fn embed_passages(&self, texts: &[&str]) -> magi_core::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0]).collect())
        }
        fn embed_query(&self, _text: &str) -> magi_core::Result<Vec<f32>> {
            self.0.lock().unwrap().push("embed");
            Ok(vec![1.0])
        }
        fn unload_if_idle(&self, idle: Duration) {
            assert_eq!(idle, Duration::ZERO);
            self.0.lock().unwrap().push("unload");
        }
    }

    #[test]
    fn one_shot_embedder_unloads_right_after_the_query() {
        let inner = Recorder::default();
        let v = OneShotQuery(&inner).embed_query("q").unwrap();
        assert_eq!(v, vec![1.0]);
        assert_eq!(*inner.0.lock().unwrap(), vec!["embed", "unload"]);
    }
}
