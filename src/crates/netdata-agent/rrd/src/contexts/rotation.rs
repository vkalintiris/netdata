//! The contexts' deep pass after a dbengine rotation: `rrdcontext_db_rotation()` (`rrdcontext.c`) arms a deadline,
//! and the RRDCONTEXT worker, before its per-host loop, recomputes every host's retention and then collects every
//! host's garbage once the deadline passed (`rrdcontext_main()` in `rrdcontext-worker.c`). D75.6, D77.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::flags;
use crate::host::{Host, Hosts};

/// `FULL_RETENTION_SCAN_DELAY_AFTER_DB_ROTATION_SECS`, in microseconds.
const PASS_DELAY_UT: u64 = 120 * 1_000_000;

/// `rrdcontext_next_db_rotation_ut`: the wall-clock deadline of the next deep pass, 0 when none is armed; and
/// `extreme_cardinality.db_rotations`, the rotations the engine made.
#[derive(Debug, Default)]
pub struct DbRotation {
    next_ut: AtomicU64,
    rotations: AtomicUsize,
}

impl DbRotation {
    /// `rrdcontext_db_rotation()`: a rotation at `now_ut` moves the deadline to 120 s later, whatever it was, and
    /// counts (`rrdcontext_count_db_rotation()`). It takes no lock and records nothing, so the engine's deletion thread
    /// may call it at any time.
    pub fn rotated(&self, now_ut: u64) {
        self.next_ut
            .store(now_ut + PASS_DELAY_UT, Ordering::Relaxed);
        self.rotations.fetch_add(1, Ordering::Relaxed);
    }

    /// The rotations counted so far, which enable the extreme cardinality protection.
    pub fn rotations(&self) -> usize {
        self.rotations.load(Ordering::Relaxed)
    }

    /// The armed deadline, once `now_ut` is past it.
    pub fn due(&self, now_ut: u64) -> Option<u64> {
        let deadline = self.next_ut.load(Ordering::Relaxed);
        (deadline != 0 && now_ut > deadline).then_some(deadline)
    }

    /// Clears the deadline a pass processed, unless a rotation during the pass armed a new one, which drives the next.
    pub fn done(&self, deadline: u64) {
        let _ = self
            .next_ut
            .compare_exchange(deadline, 0, Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// The worker's check before its per-host loop: once the armed deadline passed at `now_ut`, every host's retention is
/// recomputed (`rrdcontext_recalculate_retention_all_hosts()`), then every host's garbage collected
/// (`rrdcontext_garbage_collect_for_all_hosts()`), each context the collection removes passed to `delete_from_sql`
/// with its host and hub version; then the deadline is cleared. Hosts whose contexts are still loading are left out
/// (D77.3). `running` false stops the pass between hosts and inside a collection, as C's service checks. Whether a
/// pass ran.
pub fn deep_pass(
    hosts: &Hosts,
    now_ut: u64,
    running: &dyn Fn() -> bool,
    mut delete_from_sql: impl FnMut(&Host, &str, u64),
) -> bool {
    let slot = Arc::clone(hosts.storage().db_rotation());
    let Some(deadline) = slot.due(now_ut) else {
        return false;
    };
    let loaded: Vec<Arc<Host>> = hosts
        .all()
        .into_iter()
        .filter(|h| !h.is_pending_context_load())
        .collect();
    for host in &loaded {
        if !running() {
            break;
        }
        host.contexts()
            .recalculate_host_retention(flags::REASON_DB_ROTATION);
    }
    for host in &loaded {
        if !running() {
            break;
        }
        host.contexts()
            .garbage_collect(running, |id, version| delete_from_sql(host, id, version));
    }
    slot.done(deadline);
    true
}

#[cfg(test)]
mod tests;
