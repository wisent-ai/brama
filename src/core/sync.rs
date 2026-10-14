//! Holding a process-wide lock after a holder panicked.
//!
//! A panic while a `std::sync::Mutex` is held poisons it, and every later
//! `lock()` then answers an error. Refusing on that error turns one panic into
//! a refusal that lasts until the process restarts: routing that never reads
//! its pool again, route files that are never written again. What these locks
//! guard is either nothing (they serialize a file write) or a value replaced
//! whole, so the state a panicking holder left is still a state; the lock is
//! taken back, its poison cleared, and the recovery logged with its name.

use std::sync::{Mutex, MutexGuard};

/// The guard of `mutex`, taken back from a poisoned lock instead of refused.
pub fn held<'a, T>(mutex: &'a Mutex<T>, name: &str) -> MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(
                event = "lock_poisoned_recovered",
                lock = name,
                "a holder of this lock panicked; the lock is taken back"
            );
            mutex.clear_poison();
            poisoned.into_inner()
        }
    }
}
