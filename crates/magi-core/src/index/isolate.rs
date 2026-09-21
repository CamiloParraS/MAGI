//! Runs one extraction on its own thread so a hang becomes a timeout and a
//! panic becomes an error: one bad file never kills the engine (SPEC.md §7
//! M2 "Extraction isolation").

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::error::{Error, Result};

pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Timed-out extraction threads that may still be running before new
/// extractions are refused. A hung thread can't be killed, and each one
/// pins its file's bytes and any decoded image, so this caps that memory.
const MAX_STUCK_THREADS: usize = 4;

/// Timed-out extraction threads, kept so they can be joined once they
/// finish instead of being forgotten. Rust can't kill a thread; the best
/// available is to notice when a stuck one ends and to stop starting new
/// work while too many are still alive.
///
/// ponytail: a thread that never finishes holds its slot until restart;
/// past [`MAX_STUCK_THREADS`] of those, extraction is refused. Real
/// reclamation needs a subprocess per extraction.
struct StuckThreads(Mutex<Vec<JoinHandle<()>>>);

static STUCK: StuckThreads = StuckThreads(Mutex::new(Vec::new()));

impl StuckThreads {
    /// Joins the ones that have finished; returns how many still run.
    fn reap(&self) -> usize {
        let mut threads = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (finished, running): (Vec<_>, Vec<_>) =
            threads.drain(..).partition(|t| t.is_finished());
        for thread in finished {
            let _ = thread.join();
        }
        *threads = running;
        threads.len()
    }

    fn add(&self, thread: JoinHandle<()>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(thread);
    }
}

/// Runs `work` for the file at `path` under [`EXTRACTION_TIMEOUT`].
pub fn run<T: Send + 'static>(
    path: PathBuf,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    run_with(&STUCK, EXTRACTION_TIMEOUT, path, work)
}

fn run_with<T: Send + 'static>(
    stuck: &StuckThreads,
    timeout: Duration,
    path: PathBuf,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let stuck_now = stuck.reap();
    if stuck_now >= MAX_STUCK_THREADS {
        return Err(Error::ExtractionBacklog {
            path,
            stuck: stuck_now,
        });
    }

    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(AssertUnwindSafe(work));
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => {
            let _ = thread.join();
            match result {
                Ok(work_result) => work_result,
                Err(panic_payload) => Err(Error::ExtractionPanicked {
                    path,
                    message: panic_message(&panic_payload),
                }),
            }
        }
        Err(_timed_out) => {
            stuck.add(thread);
            Err(Error::ExtractionTimeout {
                path,
                seconds: timeout.as_secs(),
            })
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn timed_out_thread_is_reaped_once_it_finishes_and_new_work_is_refused_while_they_pile_up() {
        let stuck = StuckThreads(Mutex::new(Vec::new()));
        let release = Arc::new(AtomicBool::new(false));
        let path = PathBuf::from("hang.bin");
        let hang = |release: Arc<AtomicBool>| {
            move || -> Result<()> {
                while !release.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(())
            }
        };

        for _ in 0..MAX_STUCK_THREADS {
            let err = run_with(
                &stuck,
                Duration::from_millis(20),
                path.clone(),
                hang(release.clone()),
            )
            .unwrap_err();
            assert!(matches!(err, Error::ExtractionTimeout { .. }), "{err:?}");
        }
        let err = run_with(&stuck, Duration::from_secs(5), path.clone(), || Ok(())).unwrap_err();
        assert!(matches!(err, Error::ExtractionBacklog { .. }), "{err:?}");

        release.store(true, Ordering::Relaxed);
        while stuck.reap() > 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        run_with(&stuck, Duration::from_secs(5), path, || Ok(())).unwrap();
    }
}
