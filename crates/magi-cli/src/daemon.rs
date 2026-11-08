//! `magi-cli daemon`: the engine, headless, until Ctrl-C.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use magi_core::dto::{IndexStatus, RootStatus};
use magi_core::features::load_components;
use magi_core::{Engine, config, paths};
use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::db_path;

pub(crate) fn daemon_cmd(stats: bool) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths::data_dir())?;
    let config = config::load()?;
    let engine = Engine::start(&config, &db_path(), load_components(&config.features))?;
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
        println!(
            "  root {} {}: {}{enabled}, {} indexed",
            root.id, root.path, root.status, root.indexed
        );
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
