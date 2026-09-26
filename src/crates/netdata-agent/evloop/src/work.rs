//! libuv's thread pool (`uv_queue_work()`), which the C agent sizes by `[global] libuv worker threads` and whose
//! threads name themselves `UV_WORKER[n]`, from 1, as they run their first job. Jobs run in the order queued, each on
//! whichever thread is free. Threads start as jobs find every running thread busy, up to the size: C starts them all
//! at once, which nothing outside the process sees. C's uv threads write no thread-created records, so these write
//! none either.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    jobs: VecDeque<Job>,
    started: usize,
    idle: usize,
}

struct Inner {
    size: usize,
    stack_size: usize,
    state: Mutex<State>,
    wake: Condvar,
}

/// The pool; clones share it. Its threads live as long as the process, as libuv's do.
#[derive(Clone)]
pub struct WorkPool {
    inner: Arc<Inner>,
}

impl WorkPool {
    /// A pool of up to `size` threads (at least one), each with a stack of `stack_size` bytes.
    pub fn new(size: usize, stack_size: usize) -> WorkPool {
        WorkPool {
            inner: Arc::new(Inner {
                size: size.max(1),
                stack_size,
                state: Mutex::default(),
                wake: Condvar::new(),
            }),
        }
    }

    /// `uv_queue_work()`: runs `job` on a pool thread. Fails only when no thread runs and none can start.
    pub fn queue(&self, job: impl FnOnce() + Send + 'static) -> io::Result<()> {
        let mut state = self.inner.lock();
        state.jobs.push_back(Box::new(job));
        if state.jobs.len() > state.idle && state.started < self.inner.size {
            let n = state.started + 1;
            let inner = Arc::clone(&self.inner);
            match std::thread::Builder::new()
                .name(format!("UV_WORKER[{n}]"))
                .stack_size(self.inner.stack_size)
                .spawn(move || inner.run())
            {
                Ok(_) => state.started = n,
                Err(err) if state.started == 0 => {
                    state.jobs.clear();
                    return Err(err);
                }
                // the running threads take the job in turn
                Err(_) => {}
            }
        }
        drop(state);
        self.inner.wake.notify_one();
        Ok(())
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(&self) {
        let mut state = self.lock();
        loop {
            if let Some(job) = state.jobs.pop_front() {
                drop(state);
                job();
                state = self.lock();
                continue;
            }
            state.idle += 1;
            state = self
                .wake
                .wait_while(state, |s| s.jobs.is_empty())
                .unwrap_or_else(PoisonError::into_inner);
            state.idle -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn threads_start_as_needed_up_to_the_size_and_carry_c_names() {
        let pool = WorkPool::new(2, 256 * 1024);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (name_tx, name_rx) = mpsc::channel();
        for _ in 0..3 {
            let (release_rx, name_tx) = (Arc::clone(&release_rx), name_tx.clone());
            pool.queue(move || {
                name_tx
                    .send(std::thread::current().name().unwrap().to_string())
                    .unwrap();
                let _ = release_rx.lock().unwrap().recv();
            })
            .unwrap();
        }
        let mut names = vec![
            name_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            name_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        ];
        names.sort();
        assert_eq!(names, ["UV_WORKER[1]", "UV_WORKER[2]"]);
        // the third job waits for a free thread: the pool is full
        assert!(name_rx.recv_timeout(Duration::from_millis(200)).is_err());
        release_tx.send(()).unwrap();
        let third = name_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(third == "UV_WORKER[1]" || third == "UV_WORKER[2]");
        assert_eq!(pool.inner.lock().started, 2);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
    }

    #[test]
    fn an_idle_thread_takes_the_next_job_without_a_new_thread() {
        let pool = WorkPool::new(4, 256 * 1024);
        for _ in 0..3 {
            let (tx, rx) = mpsc::channel();
            pool.queue(move || tx.send(()).unwrap()).unwrap();
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            // let the thread go back to waiting
            while pool.inner.lock().idle == 0 {
                std::thread::yield_now();
            }
        }
        assert_eq!(pool.inner.lock().started, 1);
    }
}
