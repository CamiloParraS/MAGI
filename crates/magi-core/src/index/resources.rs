//! Resource policy (SPEC.md §5.3 memory-aware concurrency, §6.4 background
//! policy, NFR-13): how many extract workers to run, how long an idle model
//! stays loaded, and when indexing pauses for low memory or battery.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use sysinfo::System;

use crate::index::pipeline::IndexContext;
use crate::platform::{Os, PowerStatus};

const GIB: u64 = 1 << 30;
/// Available memory is checked this often (SPEC.md §5.3).
const CHECK: Duration = Duration::from_secs(10);
/// Battery status is read at most this often: on macOS it spawns `pmset`.
const BATTERY_CHECK: Duration = Duration::from_secs(60);
/// NFR-13: below this much available RAM, indexing pauses.
const LOW_AVAILABLE: u64 = GIB;

/// Total RAM in whole GB as sold: an "8 GB" machine reports about 7.8 GiB.
fn ram_gb(total_bytes: u64) -> u64 {
    total_bytes.saturating_add(GIB / 2) / GIB
}

/// Auto `worker_threads`: `min(physical_cores / 2, total_ram_gb / 4)`,
/// clamped to 1-4.
pub fn auto_workers(physical_cores: usize, total_bytes: u64) -> usize {
    (physical_cores / 2)
        .min((ram_gb(total_bytes) / 4) as usize)
        .clamp(1, 4)
}

/// Low-memory mode: total RAM of 8 GB or less.
pub fn low_memory_mode(total_bytes: u64) -> bool {
    ram_gb(total_bytes) <= 8
}

/// `idle_unload_minutes`, lowered to 2 in low-memory mode.
pub fn idle_unload(minutes: u32, total_bytes: u64) -> Duration {
    let minutes = if low_memory_mode(total_bytes) {
        minutes.min(2)
    } else {
        minutes
    };
    Duration::from_secs(u64::from(minutes) * 60)
}

/// Whether available RAM is low enough to pause (NFR-13). `0` means the OS
/// did not say, which is not treated as low.
pub fn memory_low(available_bytes: u64) -> bool {
    available_bytes > 0 && available_bytes < LOW_AVAILABLE
}

/// Total RAM, and the extract worker count for `worker_threads` (0 = auto).
pub fn machine(worker_threads: u32) -> (u64, usize) {
    let mut sys = System::new();
    sys.refresh_memory();
    let total = sys.total_memory();
    let workers = match worker_threads {
        0 => {
            let cores = System::physical_core_count()
                .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get()));
            auto_workers(cores, total)
        }
        n => n as usize,
    };
    (total, workers)
}

/// Why indexing is paused, if it is. While either is set the scheduler starts
/// no new files.
#[derive(Default)]
pub(crate) struct Pause {
    /// By the user; persisted in `meta.paused` (SPEC.md §6.4).
    pub user: AtomicBool,
    /// By the monitor: low memory, or on battery.
    pub resources: AtomicBool,
}

impl Pause {
    pub fn any(&self) -> bool {
        self.user.load(Ordering::SeqCst) || self.resources.load(Ordering::SeqCst)
    }
}

/// What the monitor thread watches and acts on.
pub(crate) struct Monitor {
    pub ctx: Arc<IndexContext>,
    pub pause: Arc<Pause>,
    /// Test hook for SPEC.md §7 M5 item 17: acts as if memory were low.
    pub force_low_memory: Arc<AtomicBool>,
    pub pause_on_battery: bool,
    pub idle: Duration,
}

/// The monitor thread: every [`CHECK`], or when woken, pauses or resumes
/// indexing, unloads the image model under memory pressure (NFR-13), and
/// unloads any model idle longer than `idle`. Ends once `stop` is set.
pub(crate) fn run(m: Monitor, stop: &AtomicBool, wake: Receiver<()>) {
    let mut sys = System::new();
    let mut battery: Option<(Instant, bool)> = None;
    while !stop.load(Ordering::SeqCst) {
        sys.refresh_memory();
        let low = m.force_low_memory.load(Ordering::SeqCst) || memory_low(sys.available_memory());
        let on_battery = m.pause_on_battery && {
            if battery.is_none_or(|(at, _)| at.elapsed() >= BATTERY_CHECK) {
                battery = Some((Instant::now(), Os.on_battery() == Some(true)));
            }
            battery.is_some_and(|(_, on)| on)
        };
        let pause = low || on_battery;
        if m.pause.resources.swap(pause, Ordering::SeqCst) != pause {
            tracing::info!(
                pause,
                low_memory = low,
                on_battery,
                "indexing pause changed"
            );
        }
        if let Some(image) = &m.ctx.image_embedder {
            image.unload_if_idle(if low { Duration::ZERO } else { m.idle });
        }
        m.ctx.embedder.unload_if_idle(m.idle);
        m.ctx.ocr.unload_if_idle(m.idle);
        let _ = wake.recv_timeout(CHECK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB8: u64 = 7_900 * 1024 * 1024; // what an 8 GB laptop reports

    #[test]
    fn auto_workers_follows_the_formula_and_clamps() {
        assert_eq!(auto_workers(4, GB8), 2, "the 8 GB reference machine");
        assert_eq!(auto_workers(16, 64 * GIB), 4, "capped at 4");
        assert_eq!(auto_workers(2, 16 * GIB), 1, "cores bound it");
        assert_eq!(auto_workers(1, 2 * GIB), 1, "never below 1");
    }

    #[test]
    fn eight_gb_or_less_is_low_memory_mode_with_a_two_minute_unload() {
        assert!(low_memory_mode(GB8));
        assert!(!low_memory_mode(16 * GIB));
        assert_eq!(idle_unload(5, GB8), Duration::from_secs(120));
        assert_eq!(idle_unload(1, GB8), Duration::from_secs(60));
        assert_eq!(idle_unload(5, 16 * GIB), Duration::from_secs(300));
    }

    #[test]
    fn under_one_gib_available_is_low_but_unknown_is_not() {
        assert!(memory_low(GIB - 1));
        assert!(!memory_low(GIB));
        assert!(!memory_low(0));
    }
}
