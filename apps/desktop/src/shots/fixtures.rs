//! The profile every shot runs on: a fresh data folder, a small folder of
//! sample files, and a host that has indexed them. Same names and contents
//! every run, so two runs on one machine draw the same pixels.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use magi_core::dto::IndexState;
use magi_core::host::{Host, HostPaths};

/// Sample files: a name and its text. Short, but long enough that the
/// search results, previews and "whole file" sizes have something to show.
const TEXT_FILES: &[(&str, &str)] = &[
    (
        "Lease agreement.txt",
        "Residential lease between Ana Torres (tenant) and Northside Homes (landlord).\n\n\
         1. Rent. The tenant shall pay rent of 1,450 each month, due on the first day.\n\
         2. Deposit. A security deposit of one month's rent is held until the lease ends.\n\
         3. Renewal. The lease renews for twelve months unless either party gives sixty \
         days' written notice.\n\
         4. Repairs. The landlord repairs heating, plumbing and the roof; the tenant \
         reports damage within seven days.\n",
    ),
    (
        "Rent receipts 2026.md",
        "# Rent receipts 2026\n\n\
         - January: 1,450 paid on the 1st, bank transfer.\n\
         - February: 1,450 paid on the 2nd, bank transfer.\n\
         - March: 1,450 paid on the 1st; the deposit receipt is attached.\n",
    ),
    (
        "Meeting notes.txt",
        "Team meeting, Friday 3 pm, room 4.\n\
         Agenda: the contract renewal terms, the new office lease, and the travel budget.\n\
         Action: Sam drafts the renewal letter by Wednesday.\n",
    ),
    (
        "budget.rs",
        "/// Monthly costs, in whole units.\n\
         pub struct Budget {\n    pub rent: u32,\n    pub travel: u32,\n}\n\n\
         impl Budget {\n    pub fn total(&self) -> u32 {\n        self.rent + self.travel\n    }\n}\n",
    ),
];

/// The output folder's `profile` subfolder: data, config and the sample root.
pub struct Profile {
    pub host: Host,
    /// The host's `config.toml`.
    pub config: PathBuf,
}

/// With `demo`, a models folder: [`super::demo`]'s files over real models
/// instead of the samples and the fake embedder.
pub fn start(dir: &Path, demo: Option<&Path>) -> anyhow::Result<Profile> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    let (data, config, root) = (dir.join("data"), dir.join("config"), dir.join("Documents"));
    for d in [&data, &config, &root] {
        std::fs::create_dir_all(d)?;
    }
    let want = match demo {
        Some(models) => {
            super::demo::write_files(&root)?;
            super::demo::link_models(models, &data)?;
            std::fs::write(config.join("config.toml"), super::demo::CONFIG)?;
            super::demo::FILES.len() as u64
        }
        None => {
            for (name, text) in TEXT_FILES {
                std::fs::write(root.join(name), text)?;
            }
            write_picture(&root.join("Whiteboard.png"))?;
            write_picture_dark(&root.join("Receipt scan.png"))?;
            TEXT_FILES.len() as u64 + 2
        }
    };
    // SAFETY: set before the host or GPUI starts any thread that reads them.
    unsafe {
        std::env::set_var("MAGI_DATA_DIR", &data);
        std::env::set_var("MAGI_CONFIG_DIR", &config);
        if demo.is_none() {
            std::env::set_var("MAGI_FAKE_EMBEDDER", "1");
        }
        std::env::set_var("MAGI_FAKE_ASK", "1");
    }
    let host = Host::start(
        HostPaths {
            db: data.join("magi.db"),
            config: config.join("config.toml"),
        },
        |_| {},
    )?;
    wait_for(Duration::from_secs(60), "the engine", || {
        host.engine().is_ok()
    })?;
    host.engine()?.add_root(&root)?;
    let engine = host.engine()?;
    // Real models read every photo and page: minutes, not seconds.
    let limit = Duration::from_secs(if demo.is_some() { 1200 } else { 120 });
    wait_for(limit, "indexing the samples", || {
        engine
            .status()
            .is_ok_and(|s| s.state == IndexState::Idle && s.indexed + s.skipped + s.errors >= want)
    })?;
    Ok(Profile {
        host,
        config: config.join("config.toml"),
    })
}

fn wait_for(limit: Duration, what: &str, mut done: impl FnMut() -> bool) -> anyhow::Result<()> {
    let started = Instant::now();
    while !done() {
        if started.elapsed() > limit {
            bail!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// A light "whiteboard": a few coloured bars on white.
fn write_picture(path: &Path) -> anyhow::Result<()> {
    let image = image::RgbImage::from_fn(640, 420, |x, y| {
        let bar = (y / 70) % 3;
        if (40..600).contains(&x) && y % 70 > 20 && x < 160 + 140 * (bar + 1) {
            [[46, 110, 220], [230, 120, 40], [40, 160, 90]][bar as usize].into()
        } else {
            [248, 248, 244].into()
        }
    });
    image.save(path)?;
    Ok(())
}

/// A darker "scan": grey paper with darker lines of "text".
fn write_picture_dark(path: &Path) -> anyhow::Result<()> {
    let image = image::RgbImage::from_fn(420, 600, |x, y| {
        if (40..380).contains(&x) && y % 28 < 6 && (60..560).contains(&y) {
            [70, 70, 76].into()
        } else {
            [214, 210, 200].into()
        }
    });
    image.save(path)?;
    Ok(())
}
