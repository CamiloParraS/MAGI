//! magi-cli: dev/test CLI (doctor, roots, index, daemon, search, eval — added
//! milestone by milestone; see SPEC.md §7).

mod daemon;
mod eval;
mod from_env;
mod search;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use magi_core::dto::FeatureStatus;
use magi_core::dto::Install;
use magi_core::features::{Feature, Features, load_components};
use magi_core::index::pipeline::{IndexContext, IndexRootOptions, index_root};
use magi_core::{config, db, paths};

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
        /// Spike (docs/issues/0001): rerank hybrid's results with the
        /// cross-encoder in this folder (`model.onnx`, `tokenizer.json`).
        #[arg(long)]
        rerank: Option<PathBuf>,
    },
    /// Index `--corpus` and report recall@5, recall@10, and MRR for
    /// fts/vector/hybrid search against a queries.jsonl file.
    Eval {
        queries: PathBuf,
        #[arg(long)]
        corpus: PathBuf,
    },
    /// Show and change the optional search features (ADR-0010).
    Features {
        #[command(subcommand)]
        action: FeaturesAction,
    },
}

#[derive(Subcommand)]
enum FeaturesAction {
    List,
    /// Turn a feature on, downloading it first if needed.
    Enable {
        feature: String,
    },
    Disable {
        feature: String,
        /// Also delete its downloaded files (indexed data is kept).
        #[arg(long)]
        delete_download: bool,
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
        Command::Daemon { stats } => daemon::daemon_cmd(stats)?,
        Command::Search {
            query,
            mode,
            limit,
            rerank,
        } => search::search_cmd(&query, &mode, limit, rerank.as_deref())?,
        Command::Eval { queries, corpus } => eval::eval_cmd(queries, corpus)?,
        Command::Features { action } => features_cmd(action)?,
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
    print_features(&Features::start(&db_path())?.status()?);
    Ok(())
}

fn features_cmd(action: FeaturesAction) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths::data_dir())?;
    let features = Features::start(&db_path())?;
    match action {
        FeaturesAction::List => print_features(&features.status()?),
        FeaturesAction::Enable { feature } => {
            let feature: Feature = feature.parse()?;
            features.set_enabled(feature, true)?;
            let installed = |s: &[FeatureStatus]| {
                s.iter()
                    .any(|x| x.feature == feature && matches!(x.install, Install::Installed { .. }))
            };
            if !installed(&features.status()?) {
                let rx = features.subscribe();
                features.download(feature)?;
                for s in rx {
                    let this = s
                        .iter()
                        .find(|x| x.feature == feature)
                        .map(|x| x.install.clone());
                    match this {
                        Some(Install::Downloading { bytes, total }) => println!(
                            "downloading {}: {} / {} MB",
                            feature.as_str(),
                            bytes >> 20,
                            total >> 20
                        ),
                        Some(Install::Failed { code }) => {
                            anyhow::bail!("download failed: {code:?}")
                        }
                        Some(Install::Installed { .. }) => break,
                        _ => {}
                    }
                }
            }
            println!("{} on", feature.as_str());
        }
        FeaturesAction::Disable {
            feature,
            delete_download,
        } => {
            let feature: Feature = feature.parse()?;
            features.set_enabled(feature, false)?;
            if delete_download {
                features.remove_download(feature)?;
            }
            println!("{} off", feature.as_str());
        }
    }
    Ok(())
}

/// One line per feature: name, on/off, install state, backfill.
fn print_features(status: &[FeatureStatus]) {
    for s in status {
        let install = match &s.install {
            Install::NotInstalled => format!("not installed ({} MB)", s.download_size >> 20),
            Install::Downloading { bytes, total } => {
                format!("downloading {} / {} MB", bytes >> 20, total >> 20)
            }
            Install::Installed { size_bytes } => format!("installed ({} MB)", size_bytes >> 20),
            Install::Failed { code } => format!("failed: {code:?}"),
        };
        let backfill = s
            .backfill
            .map(|b| format!("\tbackfill {}/{}", b.done, b.total))
            .unwrap_or_default();
        let on = if s.enabled { "on" } else { "off" };
        println!("{}\t{on}\t{install}{backfill}", s.feature.as_str());
    }
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

fn index_cmd(root: PathBuf) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths::data_dir())?;
    let mut conn = db::open(&db_path())?;
    let config = config::load()?;
    let components = load_components(&config.features);

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
            embedder: components.text,
            ocr: components.ocr,
            image_embedder: components.image,
            ..IndexContext::default()
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
