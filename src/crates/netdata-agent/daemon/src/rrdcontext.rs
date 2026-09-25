//! The `RRDCONTEXT` thread, ported from `rrdcontext_main()` (`src/database/contexts/rrdcontext-worker.c`): once a
//! second, every host's contexts are post-processed.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use netdata_agent_rrd::host::Hosts;

/// `RRDCONTEXT_WORKER_THREAD_HEARTBEAT_USEC`.
const HEARTBEAT: Duration = Duration::from_secs(1);

pub struct Worker {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: JoinHandle<()>,
}

impl Worker {
    pub fn spawn(hosts: Arc<Hosts>, stack_size: usize) -> std::io::Result<Self> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("RRDCONTEXT".into())
            .stack_size(stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                let (stopped, wake) = &*signal;
                let mut stopped = stopped.lock().unwrap_or_else(PoisonError::into_inner);
                loop {
                    // heartbeat_next(): the next tick on the wall-clock grid of the period.
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default();
                    let wait = HEARTBEAT
                        - Duration::from_nanos((now.as_nanos() % HEARTBEAT.as_nanos()) as u64);
                    stopped = wake
                        .wait_timeout(stopped, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                    if *stopped {
                        break;
                    }
                    for host in hosts.all() {
                        host.contexts().worker_cycle();
                    }
                }
                drop(stopped);
                netdata_agent_log::thread_finished();
            })
            .map_err(|err| netdata_agent_evloop::thread_create_failed("RRDCONTEXT", &err))?;
        Ok(Worker { stop, thread })
    }

    /// Stops the thread, waiting at most `limit` as C's service wait does.
    pub fn stop_within(self, limit: Duration) {
        let (stopped, wake) = &*self.stop;
        *stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline && !self.thread.is_finished() {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
        }
    }
}
