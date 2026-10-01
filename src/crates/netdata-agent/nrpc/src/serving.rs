//! Serving handles (`src/nrpc/nrpc-serving.c`): the liveness token of a thread that registers functions. Every method
//! registered on a thread holds its handle, so when the thread finishes, all of them become unavailable at once; the
//! methods stay registered until something replaces or removes them.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::lifetime::{Gate, Pass};

/// `struct nrpc_serving_handle`.
#[derive(Debug, Default)]
pub struct Serving {
    gate: Gate,
    tid: AtomicU64,
}

impl Serving {
    /// `nrpc_serving_running()`.
    pub fn running(&self) -> bool {
        self.gate.is_alive()
    }

    /// `nrpc_serving_tid()`: the thread that last started it.
    pub fn tid(&self) -> u64 {
        self.tid.load(Ordering::Relaxed)
    }

    /// `nrpc_serving_dispatcher_acquire()`: a cancel or progress dispatch aimed at this thread, none once it finished.
    pub fn try_dispatch(&self) -> Option<Pass<'_>> {
        self.gate.try_acquire()
    }
}

/// The thread's handle; its drop at the thread's exit is `nrpc_serving_finished()` (C's thread-exit hook).
struct Current(Option<Arc<Serving>>);

impl Drop for Current {
    fn drop(&mut self) {
        if let Some(serving) = self.0.take() {
            serving.gate.retire();
        }
    }
}

thread_local! {
    /// `nrpc_thread_serving`.
    static CURRENT: RefCell<Current> = const { RefCell::new(Current(None)) };
}

/// `nrpc_serving_started()`: idempotent; the registration paths call it on every registration, which only refreshes
/// the tid.
pub fn started() {
    current();
}

/// `nrpc_serving_finished()`: every method this thread registered becomes unavailable, after the cancel and progress
/// dispatches aimed at it leave; a later registration on the thread starts a new handle.
pub fn finished() {
    let serving = CURRENT.with(|c| c.borrow_mut().0.take());
    if let Some(serving) = serving {
        serving.gate.retire();
    }
}

/// `nrpc_serving_current_thread_acquire()`: the thread's handle, started if it was not.
pub fn current() -> Arc<Serving> {
    CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        let serving = c.0.get_or_insert_with(Arc::default);
        serving.tid.store(netdata_agent_log::tid(), Ordering::Relaxed);
        Arc::clone(serving)
    })
}

/// Whether this thread has a handle and it is `serving` (none when the thread has no handle).
pub fn is_current(serving: &Arc<Serving>) -> Option<bool> {
    CURRENT.with(|c| c.borrow().0.as_ref().map(|current| Arc::ptr_eq(current, serving)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thread's methods share its handle until it finishes; the next registration starts a new one; a thread's exit
    /// finishes its handle.
    #[test]
    fn a_threads_handle_lives_until_it_finishes() {
        let first = current();
        assert!(Arc::ptr_eq(&first, &current()) && first.running());
        assert_eq!(is_current(&first), Some(true));
        finished();
        assert!(!first.running());
        assert_eq!(is_current(&first), None);
        let second = current();
        assert!(!Arc::ptr_eq(&first, &second) && second.running());
        assert_eq!(is_current(&first), Some(false));
        let other = std::thread::spawn(current).join().unwrap();
        assert!(!other.running(), "finished at its thread's exit");
    }
}
