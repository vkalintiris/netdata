//! The replication sender's threads (`src/streaming/stream-replication-sender.c`): `REPLAY[1]`, a static thread,
//! and its workers `REPLAY[2..threads]`. Until the replication sender (milestone 7 commit 6) their queue is always
//! empty, so each waits a second at a time until the exit starts or they are stopped (D103.3, D110).

use std::io;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// `service_running(SERVICE_REPLICATION)`, with a wakeup for the wait.
#[derive(Debug, Default)]
struct Stop {
    stopped: Mutex<bool>,
    cv: Condvar,
}

impl Stop {
    /// Waits up to `d`; false once stopped.
    fn wait(&self, d: Duration) -> bool {
        let stopped = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let (stopped, _) =
            self.cv.wait_timeout_while(stopped, d, |s| !*s).unwrap_or_else(PoisonError::into_inner);
        !*stopped
    }

    fn request(&self) {
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.cv.notify_all();
    }
}

fn spawn(name: String, stack_size: usize, body: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.clone())
        .stack_size(stack_size)
        .spawn(move || {
            netdata_agent_log::thread_created();
            body();
            netdata_agent_log::thread_finished();
        })
        .map_err(|err| netdata_agent_evloop::thread_create_failed(&name, &err))
}

/// The running replication threads.
#[derive(Debug)]
pub struct ReplicationThreads {
    stop: Arc<Stop>,
    main: JoinHandle<()>,
}

impl ReplicationThreads {
    /// `replication_thread_main()`: `REPLAY[1]` starts `threads - 1` workers, and joins them when it ends.
    pub fn spawn(threads: usize, stack_size: usize) -> io::Result<ReplicationThreads> {
        Self::spawn_with(threads, stack_size, netdata_agent_sys::exit::initiated)
    }

    /// [`ReplicationThreads::spawn`] with `exiting` telling the exit's start (`service_running(SERVICE_REPLICATION)`,
    /// tested after each wait).
    fn spawn_with(threads: usize, stack_size: usize, exiting: fn() -> bool) -> io::Result<ReplicationThreads> {
        let stop = Arc::new(Stop::default());
        let main_stop = Arc::clone(&stop);
        let main = spawn("REPLAY[1]".to_string(), stack_size, move || {
            let workers: Vec<JoinHandle<()>> = (2..=threads)
                .filter_map(|n| {
                    let stop = Arc::clone(&main_stop);
                    // nd_thread_create() failures leave the slot empty, as C carries on
                    spawn(format!("REPLAY[{n}]"), stack_size, move || {
                        while !exiting() && stop.wait(Duration::from_secs(1)) {}
                    })
                    .ok()
                })
                .collect();
            while !exiting() && main_stop.wait(Duration::from_secs(1)) {}
            for worker in workers {
                let _ = worker.join();
            }
        })?;
        Ok(ReplicationThreads { stop, main })
    }

    /// `service_wait_exit(SERVICE_REPLICATION, limit)` once the exit started: the threads leave on their own and are
    /// waited for at most `limit` without being woken (not by one of them); then they are asked to stop and left.
    pub fn join_within(self, limit: Duration) {
        if self.main.thread().id() != std::thread::current().id() {
            let deadline = Instant::now() + limit;
            while Instant::now() < deadline && !self.main.is_finished() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.stop.request();
        if self.main.is_finished() {
            let _ = self.main.join();
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// Before the exit starts a join waits out its limit, then asks the threads to stop.
    #[test]
    fn a_join_waits_its_limit_then_asks() {
        let threads = ReplicationThreads::spawn_with(3, 256 * 1024, || false).unwrap();
        let started = Instant::now();
        threads.join_within(Duration::from_millis(200));
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    /// The exit's start ends them within their second's wait, before any stop.
    #[test]
    fn the_threads_end_when_the_exit_starts() {
        let threads = ReplicationThreads::spawn_with(3, 256 * 1024, || true).unwrap();
        let started = Instant::now();
        while !threads.main.is_finished() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(threads.main.is_finished() && started.elapsed() < Duration::from_millis(1500));
    }
}
