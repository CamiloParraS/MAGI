//! Periodic safety nets (SPEC.md §5.4): a reconciliation every
//! `reconcile_interval_hours`, one after a wall-clock jump (sleep/resume),
//! and one every 15 minutes while any root has no working watcher. Every tick
//! also re-probes roots that were unreadable or missing (SPEC.md §6.1).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

use crate::index::pipeline::unix_now;
use crate::index::writer::WriteJob;

/// The clock is looked at, and lost roots re-probed, this often.
const TICK: Duration = Duration::from_secs(30);
/// A wall-clock gap between ticks over this is a sleep, resume or clock change.
const JUMP_SECS: i64 = 5 * 60;
/// Polling period for roots without a watcher.
const POLL_SECS: i64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Interval,
    ClockJump,
    Poll,
}

/// When a reconciliation is due. Times are wall-clock seconds passed in, so it
/// is testable without waiting.
pub struct Timers {
    last_tick: i64,
    last_scan: i64,
    interval_secs: i64,
}

impl Timers {
    /// `interval_hours == 0` turns the periodic scan off.
    pub fn new(interval_hours: u32, now: i64) -> Self {
        Self {
            last_tick: now,
            last_scan: now,
            interval_secs: i64::from(interval_hours) * 3600,
        }
    }

    /// Called once per tick. `unwatched` is whether any root has no watcher.
    pub fn tick(&mut self, now: i64, unwatched: bool) -> Option<Reason> {
        let jumped = (now - self.last_tick).abs() > JUMP_SECS;
        self.last_tick = now;
        let due = now - self.last_scan;
        let reason = if jumped {
            Reason::ClockJump
        } else if self.interval_secs > 0 && due >= self.interval_secs {
            Reason::Interval
        } else if unwatched && due >= POLL_SECS {
            Reason::Poll
        } else {
            return None;
        };
        self.last_scan = now;
        Some(reason)
    }
}

/// The ticker thread: asks the writer for a full scan whenever [`Timers`] says
/// one is due. `unwatched` is set while some root is polled instead of
/// watched. Ends when `stop` fires or is dropped.
pub(crate) fn run(
    stop: Receiver<()>,
    jobs: Sender<WriteJob>,
    interval_hours: u32,
    unwatched: Arc<AtomicBool>,
) {
    let mut timers = Timers::new(interval_hours, unix_now());
    while let Err(crossbeam_channel::RecvTimeoutError::Timeout) = stop.recv_timeout(TICK) {
        let _ = jobs.send(WriteJob::Reprobe);
        if let Some(reason) = timers.tick(unix_now(), unwatched.load(Ordering::SeqCst)) {
            tracing::info!(?reason, "scheduled reconciliation");
            let _ = jobs.send(WriteJob::Reconcile(None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_due_on_a_quiet_minute() {
        let mut t = Timers::new(6, 1_000_000);
        assert_eq!(t.tick(1_000_060, false), None);
    }

    #[test]
    fn the_periodic_scan_fires_after_the_interval_and_then_waits_again() {
        let mut t = Timers::new(6, 0);
        let mut now = 0;
        let mut fired = Vec::new();
        while now <= 13 * 3600 {
            now += 60;
            if let Some(reason) = t.tick(now, false) {
                fired.push((now, reason));
            }
        }
        assert_eq!(
            fired,
            vec![(6 * 3600, Reason::Interval), (12 * 3600, Reason::Interval)]
        );
    }

    #[test]
    fn a_wall_clock_jump_triggers_a_scan_and_restarts_the_interval() {
        let mut t = Timers::new(6, 0);
        assert_eq!(t.tick(60, false), None);
        // The machine slept for two hours.
        assert_eq!(t.tick(60 + 2 * 3600, false), Some(Reason::ClockJump));
        assert_eq!(t.tick(120 + 2 * 3600, false), None);
    }

    #[test]
    fn roots_without_a_watcher_are_polled_every_fifteen_minutes() {
        let mut t = Timers::new(6, 0);
        let mut polls = 0;
        for minute in 1..=45 {
            if t.tick(minute * 60, true) == Some(Reason::Poll) {
                polls += 1;
            }
        }
        assert_eq!(polls, 3);
        let mut healthy = Timers::new(6, 0);
        assert!((1..=45).all(|m| healthy.tick(m * 60, false).is_none()));
    }

    #[test]
    fn interval_zero_turns_the_periodic_scan_off() {
        let mut t = Timers::new(0, 0);
        assert!((1..=24 * 60).all(|m| t.tick(m * 60, false).is_none()));
    }
}
