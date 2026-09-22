//! Bounds concurrent image decodes: a large image is the memory peak of
//! indexing, so at most 2 decode at once and at most 1 of those is large
//! (docs/m5-plan.md Slice 3).

use std::sync::{Condvar, Mutex, PoisonError};

const MAX_DECODES: u32 = 2;
const MAX_LARGE: u32 = 1;

#[derive(Default)]
pub struct ImageGate {
    /// `(decoding now, of which large)`.
    state: Mutex<(u32, u32)>,
    freed: Condvar,
}

/// Held while an image is being decoded; releases its slot on drop.
pub struct Permit<'a> {
    gate: &'a ImageGate,
    large: bool,
}

impl ImageGate {
    /// Blocks until a decode of this size may start.
    pub fn acquire(&self, large: bool) -> Permit<'_> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.0 >= MAX_DECODES || (large && state.1 >= MAX_LARGE) {
            state = self
                .freed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.0 += 1;
        state.1 += u32::from(large);
        Permit { gate: self, large }
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.0 -= 1;
        state.1 -= u32::from(self.large);
        self.gate.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Never more than 2 decodes, and never more than 1 large one, however
    /// many threads ask.
    #[test]
    fn caps_total_and_large_decodes() {
        let gate = Arc::new(ImageGate::default());
        let (now, now_large) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let peaks = Arc::new((AtomicU32::new(0), AtomicU32::new(0)));
        let threads: Vec<_> = (0..12)
            .map(|i| {
                let (gate, now, now_large, peaks) =
                    (gate.clone(), now.clone(), now_large.clone(), peaks.clone());
                std::thread::spawn(move || {
                    let large = i % 2 == 0;
                    let _permit = gate.acquire(large);
                    let total = now.fetch_add(1, Ordering::SeqCst) + 1;
                    let big =
                        now_large.fetch_add(u32::from(large), Ordering::SeqCst) + u32::from(large);
                    peaks.0.fetch_max(total, Ordering::SeqCst);
                    peaks.1.fetch_max(big, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    now.fetch_sub(1, Ordering::SeqCst);
                    now_large.fetch_sub(u32::from(large), Ordering::SeqCst);
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert!(peaks.0.load(Ordering::SeqCst) <= MAX_DECODES);
        assert!(peaks.1.load(Ordering::SeqCst) <= MAX_LARGE);
    }
}
