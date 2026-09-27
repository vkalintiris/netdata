//! The `RRDCONTEXT` thread, ported from `rrdcontext_main()` (`src/database/contexts/rrdcontext-worker.c`): once a
//! second, the deep pass a dbengine rotation armed, then every host's contexts post-processed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use netdata_agent_rrd::contexts;
use netdata_agent_rrd::host::{Host, Hosts};

/// `RRDCONTEXT_WORKER_THREAD_HEARTBEAT_USEC`.
const HEARTBEAT: Duration = Duration::from_secs(1);

/// `now_realtime_usec()`.
pub fn now_realtime_ut() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

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

pub struct Worker {
    stop: Arc<Stop>,
    thread: JoinHandle<()>,
}

impl Worker {
    /// `delete_from_sql` removes a context the deep pass collected from the host's context database
    /// (`rrdcontext_delete_from_sql_unsafe()`).
    pub fn spawn(
        hosts: Arc<Hosts>,
        stack_size: usize,
        delete_from_sql: impl Fn(&Host, &str, u64) + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(Stop::default());
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("RRDCONTEXT".into())
            .stack_size(stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                let running = || !signal.requested();
                loop {
                    // heartbeat_next(): the next tick on the wall-clock grid of the period.
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default();
                    let wait = HEARTBEAT
                        - Duration::from_nanos((now.as_nanos() % HEARTBEAT.as_nanos()) as u64);
                    if signal.wait(wait) {
                        break;
                    }
                    contexts::deep_pass(
                        &hosts,
                        now_realtime_ut(),
                        &running,
                        |host, id, version| delete_from_sql(host, id, version),
                    );
                    // a host whose contexts are still loading waits for the load, as in C
                    for host in hosts.all().iter().filter(|h| !h.is_pending_context_load()) {
                        if !running() {
                            break;
                        }
                        host.contexts().worker_cycle_while(&running);
                    }
                }
                netdata_agent_log::thread_finished();
            })
            .map_err(|err| netdata_agent_evloop::thread_create_failed("RRDCONTEXT", &err))?;
        Ok(Worker { stop, thread })
    }

    /// Stops the thread, waiting at most `limit` as C's service wait does.
    pub fn stop_within(self, limit: Duration) {
        self.stop.request();
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline && !self.thread.is_finished() {
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
            let started = std::time::Instant::now();
            while !working.requested() && started.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(1));
            }
            working.requested()
        });
        std::thread::sleep(Duration::from_millis(20));
        let started = std::time::Instant::now();
        stop.request();
        assert!(thread.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(1));
        let waited = std::time::Instant::now();
        assert!(stop.wait(Duration::from_secs(10)));
        assert!(
            waited.elapsed() < Duration::from_secs(1),
            "a waiter returns at once"
        );
    }
}
