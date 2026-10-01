// SPDX-License-Identifier: GPL-3.0-or-later

//! libnetdata's `completion` (`src/libnetdata/completion/completion.c`) as its job counter: one thread marks jobs
//! done, another waits for one after those it has seen, so a job marked while the waiter was busy is not lost.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// `struct completion`'s `completed_jobs`, its mutex and its condition.
#[derive(Debug, Default)]
pub struct Completion {
    jobs: Mutex<u64>,
    cv: Condvar,
}

impl Completion {
    /// `completion_mark_complete_a_job()`: one more job, every waiter woken.
    pub fn mark(&self) {
        *self.jobs.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.cv.notify_all();
    }

    /// `completion_wait_for_a_job_with_timeout()`: returns once more than `seen` jobs are done, or at the timeout;
    /// the jobs done so far.
    pub fn wait(&self, seen: u64, timeout: Duration) -> u64 {
        let jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        let (jobs, _) = self
            .cv
            .wait_timeout_while(jobs, timeout, |jobs| *jobs <= seen)
            .unwrap_or_else(PoisonError::into_inner);
        *jobs
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// A job marked before the wait, as during a pass, makes the next wait return at once.
    #[test]
    fn a_job_is_counted() {
        let c = Completion::default();
        c.mark();
        let started = Instant::now();
        assert_eq!(c.wait(0, Duration::from_secs(5)), 1);
        assert!(started.elapsed() < Duration::from_secs(1));
        // nothing new: the wait times out
        let started = Instant::now();
        assert_eq!(c.wait(1, Duration::from_millis(50)), 1);
        assert!(started.elapsed() >= Duration::from_millis(50));
    }
}
