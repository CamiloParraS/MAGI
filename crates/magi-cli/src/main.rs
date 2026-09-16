//! magi-cli: dev/test CLI (doctor, roots, index, daemon, search, eval — added
//! milestone by milestone; see SPEC.md §7).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use magi_core::embed::FakeEmbedder;
use magi_core::index::pipeline::{IndexRootOptions, index_root};
use magi_core::search::fts::search_fts;
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
    /// Search indexed content.
    Search {
        query: String,
        #[arg(long, default_value = "fts")]
        mode: String,
        #[arg(long, default_value_t = 30)]
        limit: u32,
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
        Command::Search { query, mode, limit } => search_cmd(&query, &mode, limit)?,
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

fn embedder_from_env() -> anyhow::Result<FakeEmbedder> {
    if std::env::var("MAGI_FAKE_EMBEDDER").as_deref() == Ok("1") {
        Ok(FakeEmbedder)
    } else {
        anyhow::bail!(
            "no real text embedder yet (lands in a later M3 slice); \
             set MAGI_FAKE_EMBEDDER=1 to index/search with the fake one"
        )
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
    // ponytail: fixed scan_id since reconciliation (M5) doesn't exist yet;
    // each one-shot `index` run reuses id 1.
    let summary = index_root(
        &mut conn,
        root_row.id,
        &root_row.path,
        &options,
        1,
        &embedder,
    )?;

    println!(
        "indexed: {}  skipped: {}  errors: {}",
        summary.indexed, summary.skipped, summary.errored
    );
    Ok(())
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
            let hits = magi_core::search::hybrid_search(&conn, &embedder, query, limit)?;
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
