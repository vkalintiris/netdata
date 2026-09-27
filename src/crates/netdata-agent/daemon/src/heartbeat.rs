//! The shell of the daemon's periodic threads (C's `heartbeat_t` loops): a named thread that wakes on the wall-clock
//! grid of its period until a stop request, with the start and end records every thread writes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The stop request: a flag the work checks without a lock, and the condition the idle thread waits on.
#[derive(Default)]
struct Stop {
    requested: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
}

impl Stop {
    fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// Waits for `wait` unless a stop comes first; whether one did.
    fn wait(&self, wait: Duration) -> bool {
        let guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .wake
            .wait_timeout_while(guard, wait, |_| !self.requested())
            .unwrap_or_else(PoisonError::into_inner);
        self.requested()
    }

    fn request(&self) {
        self.requested.store(true, Ordering::Release);
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.wake.notify_all();
    }
}

/// What a periodic thread's body waits on.
pub struct Ticker {
    stop: Arc<Stop>,
    period: Duration,
}

impl Ticker {
    /// `heartbeat_next()` without randomness: waits for the next tick on the wall-clock grid of the period; false
    /// once a stop came.
    pub fn next(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let wait =
            self.period - Duration::from_nanos((now.as_nanos() % self.period.as_nanos()) as u64);
        !self.stop.wait(wait)
    }

    /// Whether no stop was requested.
    pub fn running(&self) -> bool {
        !self.stop.requested()
    }
}

pub struct Thread {
    stop: Arc<Stop>,
    thread: JoinHandle<()>,
}

impl Thread {
    /// Starts `name`, whose `body` waits on a ticker of `period`.
    pub fn spawn(
        name: &str,
        stack_size: usize,
        period: Duration,
        body: impl FnOnce(&Ticker) + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(Stop::default());
        let ticker = Ticker {
            stop: Arc::clone(&stop),
            period,
        };
        let thread = std::thread::Builder::new()
            .name(name.into())
            .stack_size(stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                body(&ticker);
                netdata_agent_log::thread_finished();
            })
            .map_err(|err| netdata_agent_evloop::thread_create_failed(name, &err))?;
        Ok(Thread { stop, thread })
    }

    /// Stops the thread, waiting at most `limit` as C's service wait does.
    pub fn stop_within(self, limit: Duration) {
        self.stop.request();
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline && !self.thread.is_finished() {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stop reaches a thread in the middle of its work at once: the flag needs no lock the work holds.
    #[test]
    fn a_stop_is_seen_during_work() {
        let stop = Arc::new(Stop::default());
        let working = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            while !working.requested() && started.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(1));
            }
            working.requested()
        });
        std::thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        stop.request();
        assert!(thread.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(1));
        let waited = Instant::now();
        assert!(stop.wait(Duration::from_secs(10)));
        assert!(
            waited.elapsed() < Duration::from_secs(1),
            "a waiter returns at once"
        );
    }

    /// A ticker wakes on its period's grid and stops at once when asked; the thread's stop waits for its end.
    #[test]
    fn a_thread_ticks_until_it_is_stopped() {
        let (sender, ticks) = std::sync::mpsc::channel();
        let thread = Thread::spawn("TICKS", 64 * 1024, Duration::from_millis(20), move |t| {
            while t.next() {
                let _ = sender.send(SystemTime::now());
            }
        })
        .unwrap();
        let first = ticks.recv_timeout(Duration::from_secs(5)).unwrap();
        let since_epoch = first.duration_since(UNIX_EPOCH).unwrap();
        assert!(since_epoch.as_millis() % 20 < 15, "{since_epoch:?}");
        let started = Instant::now();
        thread.stop_within(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
