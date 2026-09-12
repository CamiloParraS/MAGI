//! magi-cli: dev/test CLI (doctor, roots, index, daemon, search, eval — added
//! milestone by milestone; see SPEC.md §7).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use magi_core::{db, paths};

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
