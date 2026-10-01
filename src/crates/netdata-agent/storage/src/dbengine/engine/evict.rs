//! The caches' evictor threads (`pgc_evict_thread()`, D84.1): `MAIN_PGC` and `EXTENT_PGC` wait for their cache's
//! signal or a second, run its eviction pass, and after a pass that began above the aggressive threshold give memory
//! back to the system, at most once a second across them. They are never joined, as C's (whose `pgc_destroy()` runs
//! only in sanitizer builds): each ends, silently, once its engine is gone.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use netdata_agent_evloop::completion::Completion;
use netdata_agent_evloop::thread_create_failed;
use netdata_agent_log::{netdata_log_error, thread_created};

use super::cache::AGGRESSIVE;
use super::query::Dbengine;

/// `completion_wait_for_a_job_with_timeout()`'s wait.
const WAIT: Duration = Duration::from_secs(1);

/// Which cache a thread evicts.
#[derive(Debug, Clone, Copy)]
enum Evicts {
    Main,
    Extents,
}

/// When memory was last given back (`last_malloc_release_ut`), monotonic microseconds; 0 for never.
static LAST_RELEASE_UT: AtomicU64 = AtomicU64::new(0);

/// Whether a release at `now_ut` is due: none in the second after the last one.
fn release_due(last_ut: u64, now_ut: u64) -> bool {
    last_ut == 0 || last_ut + 1_000_000 <= now_ut
}

/// `mallocz_release_as_much_memory_to_the_system()`, at most once a second.
fn release_memory() {
    let now_ut = netdata_agent_sys::now_monotonic_usec();
    if release_due(LAST_RELEASE_UT.load(Ordering::Relaxed), now_ut) {
        LAST_RELEASE_UT.store(now_ut, Ordering::Relaxed);
        netdata_agent_sys::malloc_trim();
    }
}

/// Starts the engine's evictor threads; one that cannot start is logged and the engine runs without it.
pub(crate) fn spawn(engine: &Arc<Dbengine>, stack_size: usize) {
    for (name, evicts) in [("MAIN_PGC", Evicts::Main), ("EXTENT_PGC", Evicts::Extents)] {
        let weak = Arc::downgrade(engine);
        let wakeup = Arc::clone(match evicts {
            Evicts::Main => engine.main.wakeup(),
            Evicts::Extents => engine.extents.wakeup(),
        });
        let spawned = std::thread::Builder::new()
            .name(name.to_string())
            .stack_size(stack_size)
            .spawn(move || {
                thread_created();
                run(weak, &wakeup, evicts);
            });
        if let Err(err) = spawned {
            netdata_log_error!("{}", thread_create_failed(name, &err));
        }
    }
}

fn run(engine: Weak<Dbengine>, wakeup: &Completion, evicts: Evicts) {
    let mut seen = 0;
    loop {
        seen = wakeup.wait(seen, WAIT);
        let Some(engine) = engine.upgrade() else {
            return;
        };
        let per1000 = match evicts {
            Evicts::Main => engine.main.evict_pass(),
            Evicts::Extents => engine.extents.evict_pass(|| engine.main.extent_target()),
        };
        drop(engine);
        if per1000 > AGGRESSIVE {
            release_memory();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_goes_back_at_most_once_a_second() {
        assert!(release_due(0, 1));
        assert!(!release_due(1, 1_000_000));
        assert!(release_due(1, 1_000_001));
    }
}
