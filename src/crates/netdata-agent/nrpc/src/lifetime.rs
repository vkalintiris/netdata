//! `NRPC_LIFETIME`'s dispatcher counter (`src/nrpc/nrpc-lifetime.h`): the gate a serving thread or a transport closes
//! when it ends. An acquire fails once the gate is retired and never waits; a retire marks the gate, then waits for
//! the holders to leave, so new arrivals cannot extend the drain. C's entry counter is the owner's `Arc`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// The retired mark, above any holder count.
const RETIRED: u32 = 1 << 31;

/// A dispatch gate: holders pass while it is alive.
#[derive(Debug, Default)]
pub struct Gate {
    state: AtomicU32,
}

/// One holder of a gate, released at its drop.
#[derive(Debug)]
pub struct Pass<'a> {
    gate: &'a Gate,
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        self.gate.state.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Gate {
    /// `nrpc_lifetime_dispatcher_acquire()`: a pass, none once the gate is retired.
    pub fn try_acquire(&self) -> Option<Pass<'_>> {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & RETIRED != 0 {
                return None;
            }
            match self.state.compare_exchange_weak(state, state + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Pass { gate: self }),
                Err(now) => state = now,
            }
        }
    }

    /// `nrpc_lifetime_alive()`: a plain load, for callers that only ask.
    pub fn is_alive(&self) -> bool {
        self.state.load(Ordering::Acquire) & RETIRED == 0
    }

    /// `nrpc_lifetime_retire()`: marked, then the holders drained. A holder on this thread would wait for itself, so
    /// a gate is never retired while its own thread holds a pass.
    pub fn retire(&self) {
        self.state.fetch_or(RETIRED, Ordering::AcqRel);
        while self.state.load(Ordering::Acquire) & !RETIRED != 0 {
            std::thread::sleep(Duration::from_micros(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;

    use super::*;

    /// A pass holds the retire until it is dropped; once the mark is set every acquire fails at once, a nested one
    /// included, without waiting.
    #[test]
    fn a_retire_drains_its_holders_and_refuses_new_ones() {
        let gate = Arc::new(Gate::default());
        assert!(gate.is_alive());
        let (held, release) = (mpsc::channel(), mpsc::channel::<()>());
        let holder = {
            let gate = Arc::clone(&gate);
            let (held, release) = (held.0, release.1);
            std::thread::spawn(move || {
                let _pass = gate.try_acquire().expect("alive");
                held.send(()).unwrap();
                release.recv().unwrap();
            })
        };
        held.1.recv().unwrap();
        let retiring = {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || gate.retire())
        };
        while gate.is_alive() {
            std::thread::yield_now();
        }
        // a dispatcher arriving during the drain, or one nested in a holder, is refused rather than queued
        assert!(gate.try_acquire().is_none());
        assert!(!retiring.is_finished());
        release.0.send(()).unwrap();
        holder.join().unwrap();
        retiring.join().unwrap();
        assert!(gate.try_acquire().is_none());
    }
}
