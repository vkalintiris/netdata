//! `exit_initiated` (`src/libnetdata/exit/exit_initiated.c`): the process's exit reasons, non-zero from the moment
//! the exit starts. Every service loop tests it, as C's `service_running()` does for every service (D110); the daemon
//! names the reasons and learns the system-shutdown and update ones.

use std::sync::atomic::{AtomicU32, Ordering};

static EXIT_INITIATED: AtomicU32 = AtomicU32::new(0);

/// `exit_initiated_get()`.
pub fn reasons() -> u32 {
    EXIT_INITIATED.load(Ordering::Relaxed)
}

/// `exit_initiated_add()`.
pub fn add(reason: u32) {
    EXIT_INITIATED.fetch_or(reason, Ordering::Relaxed);
}

/// `exit_initiated_init()`'s reset.
pub fn clear() {
    EXIT_INITIATED.store(0, Ordering::Relaxed);
}

/// `!service_running(X)` of any X: the exit started.
pub fn initiated() -> bool {
    reasons() != 0
}
