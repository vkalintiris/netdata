//! The BACKFILL queue, ported from `src/web/api/queries/backfill.c`: on a parent, the dimensions of a chart that
//! starts replicating backfill their tiers above 0 on a pool of threads, and the chart's last job asks for the
//! replication (decisions D72.2, D73). The daemon runs the threads; this is their queue and their loop.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use netdata_agent_log::{Priority, Source, nd_log};

use crate::chart::{Chart, Dim};
use crate::clock::now_realtime_s;
use crate::host::{Host, ReceiverSlot};

/// What a chart's last job runs: `backfill_callback_t` with the successful and failed dimensions.
pub type Callback = Box<dyn FnOnce(usize, usize) + Send>;

/// `LOG_WARNING_EVERY`: idle seconds between the main worker's warnings.
const LOG_WARNING_EVERY: usize = 10;

/// `struct backfill_request`: a chart's jobs, the receiver they belong to (C's host state id) and the callback.
struct Request {
    host: Arc<Host>,
    receiver: Weak<ReceiverSlot>,
    works: AtomicUsize,
    successful: AtomicUsize,
    failed: AtomicUsize,
    callback: Mutex<Option<Callback>>,
}

/// `struct backfill_dim_work`: one dimension's job.
struct DimWork {
    dim: Arc<Dim>,
    request: Arc<Request>,
}

#[derive(Default)]
struct Queue {
    running: bool,
    jobs: VecDeque<DimWork>,
    /// `completion` job id: bumped whenever jobs are added.
    job_id: u64,
}

/// `backfill_globals`.
pub struct BackfillQueue {
    queue: Mutex<Queue>,
    wake: Condvar,
    charts_added: AtomicUsize,
    callbacks_executed: AtomicUsize,
    /// `now_realtime_sec()`, which the jobs backfill up to.
    clock: fn() -> i64,
}

impl std::fmt::Debug for BackfillQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackfillQueue")
            .field("charts_added", &self.charts_added)
            .field("callbacks_executed", &self.callbacks_executed)
            .finish_non_exhaustive()
    }
}

impl Default for BackfillQueue {
    fn default() -> Self {
        BackfillQueue::new(now_realtime_s)
    }
}

impl BackfillQueue {
    pub fn new(clock: fn() -> i64) -> Self {
        BackfillQueue {
            queue: Mutex::default(),
            wake: Condvar::new(),
            charts_added: AtomicUsize::new(0),
            callbacks_executed: AtomicUsize::new(0),
            clock,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `backfill_thread()`'s thread count: half the CPUs, from 2 to 16.
    pub fn threads(cpus: usize) -> usize {
        (cpus / 2).clamp(2, 16)
    }

    /// The pool takes jobs from now on (`backfill_globals.running`).
    pub fn start(&self) {
        self.lock().running = true;
    }

    /// `backfill_request_add()`: a job per dimension of `chart` not backfilled yet, answered by `callback` once the
    /// last one ran. The callback comes back when nothing was queued: the pool is not running (read under its lock,
    /// where C reads it without, D73.7), the chart has no dimensions, or every one is backfilled.
    pub fn request_add(
        &self,
        host: &Arc<Host>,
        receiver: Weak<ReceiverSlot>,
        chart: &Chart,
        callback: Callback,
    ) -> Result<(), Callback> {
        let dims = chart.dims();
        if dims.is_empty() {
            return Err(callback);
        }
        // the dimensions' options before the queue's lock, which the workers wait on
        let pending: Vec<Arc<Dim>> = dims.into_iter().filter(|d| !d.is_backfilled()).collect();
        let mut queue = self.lock();
        if !queue.running || pending.is_empty() {
            return Err(callback);
        }
        let request = Arc::new(Request {
            host: Arc::clone(host),
            receiver,
            works: AtomicUsize::new(pending.len()),
            successful: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            callback: Mutex::new(Some(callback)),
        });
        self.charts_added.fetch_add(1, Ordering::Relaxed);
        queue.jobs.extend(pending.into_iter().map(|dim| DimWork {
            dim,
            request: Arc::clone(&request),
        }));
        queue.job_id += 1;
        drop(queue);
        self.wake.notify_all();
        Ok(())
    }

    /// `backfill_execute()`: nothing once the chart's receiver is gone (C fails to acquire the host's state);
    /// else every tier above 0 of the dimension, up to the wall clock.
    fn execute(&self, work: &DimWork) -> bool {
        if !work.request.host.is_receiver(&work.request.receiver) {
            return false;
        }
        work.dim.backfill_tiers(self.clock)
    }

    /// `backfill_dim_work_free()`: the chart's last job runs its callback.
    fn done(&self, work: DimWork, successful: bool) {
        let r = &work.request;
        if successful {
            r.successful.fetch_add(1, Ordering::Relaxed);
        } else {
            r.failed.fetch_add(1, Ordering::Relaxed);
        }
        if r.works.fetch_sub(1, Ordering::AcqRel) == 1 {
            let callback = r
                .callback
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(callback) = callback {
                self.callbacks_executed.fetch_add(1, Ordering::Relaxed);
                callback(
                    r.successful.load(Ordering::Relaxed),
                    r.failed.load(Ordering::Relaxed),
                );
            }
        }
    }

    /// `backfill_worker_thread()`: jobs in the order they came, while `running` holds; idle, a wait of at most a
    /// second for new ones. The main worker warns every ten idle seconds while charts wait for their callbacks.
    pub fn worker(&self, main: bool, running: &dyn Fn() -> bool) {
        let (mut warning, mut timeout, mut job_id) = (LOG_WARNING_EVERY, false, self.lock().job_id);
        while running() {
            let job = self.lock().jobs.pop_front();
            if let Some(work) = job {
                warning = LOG_WARNING_EVERY;
                let ok = self.execute(&work);
                self.done(work, ok);
                continue;
            }
            if main && timeout {
                let added = self.charts_added.load(Ordering::Relaxed);
                let executed = self.callbacks_executed.load(Ordering::Relaxed);
                if executed != added {
                    warning -= 1;
                }
                if executed != added && warning == 0 {
                    warning = LOG_WARNING_EVERY;
                    nd_log!(
                        Source::Daemon,
                        Priority::Warning,
                        "BACKFILL: the queue is empty, but the commands executed {executed} is not equal to the \
                         commands added {added}"
                    );
                }
            }
            let queue = self.lock();
            let (queue, _) = self
                .wake
                .wait_timeout_while(queue, Duration::from_secs(1), |q| {
                    q.job_id == job_id && q.jobs.is_empty()
                })
                .unwrap_or_else(PoisonError::into_inner);
            timeout = queue.job_id == job_id;
            job_id = queue.job_id;
        }
    }

    /// The end of `backfill_thread()`: no more jobs are taken, and the queued ones fail, their callbacks run.
    pub fn finish(&self) {
        let jobs = {
            let mut queue = self.lock();
            queue.running = false;
            std::mem::take(&mut queue.jobs)
        };
        for work in jobs {
            self.done(work, false);
        }
    }

    /// Jobs waiting.
    pub fn queued(&self) -> usize {
        self.lock().jobs.len()
    }
}

#[cfg(test)]
mod tests;
