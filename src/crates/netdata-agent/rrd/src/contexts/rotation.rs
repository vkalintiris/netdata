//! The contexts' deep pass after a dbengine rotation: `rrdcontext_db_rotation()` (`rrdcontext.c`) arms a deadline,
//! and the RRDCONTEXT worker, before its per-host loop, recomputes every host's retention and then collects every
//! host's garbage once the deadline passed (`rrdcontext_main()` in `rrdcontext-worker.c`). D75.6, D77.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use super::flags;
use crate::host::{Host, Hosts};

/// `FULL_RETENTION_SCAN_DELAY_AFTER_DB_ROTATION_SECS`, in microseconds.
const PASS_DELAY_UT: u64 = 120 * 1_000_000;

/// `rrdcontext_next_db_rotation_ut`: the wall-clock deadline of the next deep pass, 0 when none is armed;
/// `rrdcontext_full_gc_rerun_requested`: a pass asked for while one was under way; and
/// `extreme_cardinality.db_rotations`, the rotations the engine made.
#[derive(Debug, Default)]
pub struct DbRotation {
    next_ut: AtomicU64,
    rerun: AtomicBool,
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

    /// `rrdcontext_request_full_gc()`: freed charts ask for a deep pass, so that the hosts without the dbengine drop
    /// their archived entries too. It is armed 120 s after `now_ut` only when none is, so that requests every sweep
    /// do not push it out for good; a request once an armed deadline has passed (the pass is under way, and may have
    /// walked the host already) asks for one more after it. Not a rotation.
    pub fn request_full_gc(&self, now_ut: u64) {
        if let Err(armed) =
            self.next_ut
                .compare_exchange(0, now_ut + PASS_DELAY_UT, Ordering::Relaxed, Ordering::Relaxed)
            && armed <= now_ut
        {
            self.rerun.store(true, Ordering::Relaxed);
        }
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

    /// Clears the deadline a pass processed, unless a rotation during the pass armed a new one, which drives the next;
    /// then a pass asked for during this one is armed 120 s after `now_ut` if none is.
    pub fn done(&self, deadline: u64, now_ut: u64) {
        let _ = self
            .next_ut
            .compare_exchange(deadline, 0, Ordering::Relaxed, Ordering::Relaxed);
        if self.rerun.swap(false, Ordering::Relaxed) {
            let _ = self.next_ut.compare_exchange(
                0,
                now_ut + PASS_DELAY_UT,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }
}

/// The worker's check before its per-host loop: once the armed deadline passed at `now_ut`, every host's retention is
/// recomputed (`rrdcontext_recalculate_retention_all_hosts()`), then every host's garbage collected
/// (`rrdcontext_garbage_collect_for_all_hosts()`), each context the collection removes passed to `delete_from_sql`
/// with its host and hub version; then the deadline is cleared. Hosts whose contexts wait for their load or are being
/// loaded are left out, each checked again right before its recompute and its collection: a host the index has just
/// created loads after it is listed (D77.3). `running` false stops the pass between hosts and contexts (C checks
/// inside them too). Whether a pass ran.
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
    let hosts = hosts.all();
    let loaded = |host: &Host| !host.is_pending_context_load() && !host.contexts().is_loading();
    for host in hosts.iter().filter(|h| loaded(h)) {
        if !running() {
            break;
        }
        host.contexts()
            .recalculate_host_retention_while(flags::REASON_DB_ROTATION, running);
    }
    for host in hosts.iter().filter(|h| loaded(h)) {
        if !running() {
            break;
        }
        host.contexts()
            .garbage_collect(running, |id, version| delete_from_sql(host, id, version));
    }
    slot.done(deadline, crate::clock::now_realtime_ut());
    true
}

#[cfg(test)]
mod tests;
