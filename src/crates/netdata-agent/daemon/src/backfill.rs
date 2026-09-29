//! The `BACKFILL` static thread, ported from `backfill_thread()` (`src/web/api/queries/backfill.c`): on a parent,
//! `BACKFILL[0]` and its workers run the storage's BACKFILL queue until the exit starts (decisions D72.2, D73).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_rrd::backfill::BackfillQueue;
use netdata_agent_rrd::storage::StorageLayout;

use crate::shutdown;

/// `NETDATA_MAIN_THREAD_RUNNING`, `_EXITING` and `_EXITED`.
const RUNNING: u8 = 0;
const EXITING: u8 = 1;
const EXITED: u8 = 2;

pub struct Thread {
    state: Arc<AtomicU8>,
    thread: JoinHandle<()>,
}

fn not_exiting() -> bool {
    !shutdown::exiting()
}

impl Thread {
    /// `backfill_thread()`: named `BACKFILL` and tagged `BACKFILL[0]` once it runs, with half the CPUs' threads (2 to
    /// 16) working the queue until the exit starts; then the queued jobs fail.
    pub fn spawn(
        storage: Arc<StorageLayout>,
        cpus: usize,
        stack_size: usize,
    ) -> std::io::Result<Self> {
        let state = Arc::new(AtomicU8::new(RUNNING));
        let exited = Arc::clone(&state);
        let thread = std::thread::Builder::new()
            .name("BACKFILL".into())
            .stack_size(stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                netdata_agent_log::thread_tag_set("BACKFILL[0]");
                let queue = storage.backfill_queue();
                queue.start();
                let workers: Vec<JoinHandle<()>> = (1..BackfillQueue::threads(cpus))
                    .filter_map(|k| {
                        let name = format!("BACKFILL[{k}]");
                        let storage = Arc::clone(&storage);
                        std::thread::Builder::new()
                            .name(name.clone())
                            .stack_size(stack_size)
                            .spawn(move || {
                                netdata_agent_log::thread_created();
                                storage.backfill_queue().worker(false, &not_exiting);
                                netdata_agent_log::thread_finished();
                            })
                            .map_err(|err| {
                                let err = netdata_agent_evloop::thread_create_failed(&name, &err);
                                nd_log!(Source::Daemon, Priority::Err, "{err}");
                            })
                            .ok()
                    })
                    .collect();
                queue.worker(true, &not_exiting);
                exited.store(EXITING, Ordering::Release);
                for worker in workers {
                    let _ = worker.join();
                }
                queue.finish();
                netdata_agent_log::thread_finished();
                exited.store(EXITED, Ordering::Release);
            })
            .map_err(|err| netdata_agent_evloop::thread_create_failed("BACKFILL", &err))?;
        Ok(Thread { state, thread })
    }

    /// The service wait of "stop collectors and streaming threads": at most until `deadline`; the thread when it did
    /// not finish.
    pub fn stop_by(self, deadline: Instant) -> Option<Self> {
        // not waited for by itself (an exit on this thread)
        if self.thread.thread().id() == std::thread::current().id() {
            return Some(self);
        }
        while Instant::now() < deadline && !self.thread.is_finished() {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
            None
        } else {
            Some(self)
        }
    }

    /// `cancel_main_threads()`: a thread whose loop still runs is named.
    pub fn cancel(&self) {
        if self.state.load(Ordering::Acquire) == RUNNING {
            netdata_agent_log::netdata_log_info!("EXIT: Stopping main thread: BACKFILL");
        }
    }
}
