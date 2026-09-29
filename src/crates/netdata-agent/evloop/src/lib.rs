//! Pools of OS threads, each running one `mio` event loop.
//!
//! This is the concurrency model of the C agent's web workers and stream threads: a fixed number of threads,
//! each owning the sockets registered on its own poller and running every handler inline, so a slow handler
//! delays the other sockets of that thread exactly as it does in C (decisions D8). There is no work stealing.
//!
//! A [`Worker`] is created per thread. It registers sources on the thread's [`mio::Registry`], reacts to their
//! readiness, receives messages other threads send it through the pool's [`PoolHandle`] (for example a socket
//! handed from a web worker to a stream thread), and arms timers.

#![forbid(unsafe_code)]

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use mio::{Events, Poll, Waker};

pub mod conn;
pub mod work;

pub use mio::event::Event;
pub use mio::{Interest, Registry, Token};

/// The token the loop reserves for its waker. Workers must not register sources with it.
pub const WAKER_TOKEN: Token = Token(usize::MAX);

/// Events fetched per poll call.
const EVENTS_CAPACITY: usize = 1024;

/// The per-thread logic run by a pool.
pub trait Worker: Send + 'static {
    /// Messages other threads can send to this worker.
    type Msg: Send + 'static;

    /// Called once on the worker's thread before the loop starts.
    fn start(&mut self, _cx: &mut Context<'_>) -> io::Result<()> {
        Ok(())
    }

    /// Where [`Worker::running`] is tested: after each wait, before its events (C's stream threads,
    /// `stream-thread.c:608`), or at the top of the loop, after the last wait's events are handled (C's
    /// `poll_events()`, the web servers).
    const EXIT_AT_LOOP_TOP: bool = false;

    /// Tested each time around the loop ([`Worker::EXIT_AT_LOOP_TOP`] says where): false ends the loop, as a stop does.
    /// A worker that tests a condition nothing wakes it for arms a timer. A worker must not send to its own index
    /// from its exit path, or a lazy pool would start it again for good.
    fn running(&self) -> bool {
        true
    }

    /// A registered source became ready.
    fn event(&mut self, cx: &mut Context<'_>, event: &Event);

    /// A message sent to this thread through the pool's handle.
    fn message(&mut self, cx: &mut Context<'_>, msg: Self::Msg);

    /// A message still queued when the loop ended itself ([`Worker::running`] false), before [`Worker::stop`]: C's
    /// exit paths take the queued work and drop the rest. By default it is handled as any message.
    fn exit_message(&mut self, cx: &mut Context<'_>, msg: Self::Msg) {
        self.message(cx, msg);
    }

    /// A timer armed with [`Context::add_timer`] expired.
    fn timer(&mut self, _cx: &mut Context<'_>, _timer: TimerId) {}

    /// Called once on the worker's thread when the pool stops, before the thread exits.
    fn stop(&mut self, _cx: &mut Context<'_>) {}

    /// The `errno` C's thread holds as it ends (what its cleanup's last close left), which the thread's end record
    /// carries.
    fn exit_errno(&self) -> i32 {
        0
    }
}

/// `nd_thread_create()`'s record of a thread that could not start, as the error's text: libuv reports the negated
/// errno.
pub fn thread_create_failed(name: &str, err: &io::Error) -> io::Error {
    io::Error::new(
        err.kind(),
        format!(
            "failed to create new thread for {name}. uv_thread_create() failed with code {}",
            -err.raw_os_error().unwrap_or(0)
        ),
    )
}

/// Identifies an armed timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimerId(u64);

/// What a worker can reach from inside its loop.
pub struct Context<'a> {
    registry: &'a Registry,
    timers: &'a mut Timers,
    index: usize,
}

impl Context<'_> {
    /// The registry of this thread's poller.
    pub fn registry(&self) -> &Registry {
        self.registry
    }

    /// This thread's index in its pool, `0..threads`.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Arms a one-shot timer that fires at or after `at`.
    pub fn add_timer(&mut self, at: Instant) -> TimerId {
        self.timers.add(at)
    }

    /// Disarms a timer; a timer that already fired or was cancelled is ignored.
    pub fn cancel_timer(&mut self, timer: TimerId) {
        self.timers.cancel(timer);
    }
}

#[derive(Default)]
struct Timers {
    next_id: u64,
    heap: BinaryHeap<Reverse<(Instant, u64)>>,
    cancelled: HashSet<u64>,
}

impl Timers {
    fn add(&mut self, at: Instant) -> TimerId {
        let id = self.next_id;
        self.next_id += 1;
        self.heap.push(Reverse((at, id)));
        TimerId(id)
    }

    fn cancel(&mut self, timer: TimerId) {
        if self.heap.iter().any(|Reverse((_, id))| *id == timer.0) {
            self.cancelled.insert(timer.0);
        }
    }

    fn next_deadline(&mut self) -> Option<Instant> {
        while let Some(Reverse((at, id))) = self.heap.peek().copied() {
            if self.cancelled.remove(&id) {
                self.heap.pop();
                continue;
            }
            return Some(at);
        }
        None
    }

    fn pop_due(&mut self, now: Instant) -> Option<TimerId> {
        while let Some(Reverse((at, id))) = self.heap.peek().copied() {
            if self.cancelled.remove(&id) {
                self.heap.pop();
                continue;
            }
            if at > now {
                return None;
            }
            self.heap.pop();
            return Some(TimerId(id));
        }
        None
    }
}

enum Envelope<M> {
    Msg(M),
    Stop,
}

struct Mailbox<M> {
    tx: Sender<Envelope<M>>,
    waker: Waker,
    /// The poller and queue of a thread that starts on its first message ([`Pool::spawn_lazy`]).
    idle: Mutex<Option<(Poll, Receiver<Envelope<M>>)>>,
}

/// Starts thread `index` of a lazy pool on its poller and queue.
type Starter<M> =
    dyn Fn(usize, Poll, Receiver<Envelope<M>>) -> io::Result<JoinHandle<()>> + Send + Sync;

/// What a lazy pool needs to start its threads, and the threads it started.
struct Lazy<M> {
    start: Box<Starter<M>>,
    started: Mutex<Vec<JoinHandle<()>>>,
}

/// Sends messages to the threads of a running pool. Cheap to clone and usable from any thread.
pub struct PoolHandle<M> {
    mailboxes: Arc<[Mailbox<M>]>,
    lazy: Option<Arc<Lazy<M>>>,
}

impl<M> fmt::Debug for PoolHandle<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolHandle")
            .field("threads", &self.mailboxes.len())
            .finish_non_exhaustive()
    }
}

impl<M> Clone for PoolHandle<M> {
    fn clone(&self) -> Self {
        Self {
            mailboxes: Arc::clone(&self.mailboxes),
            lazy: self.lazy.clone(),
        }
    }
}

impl<M: Send + 'static> PoolHandle<M> {
    /// The number of threads in the pool.
    pub fn threads(&self) -> usize {
        self.mailboxes.len()
    }

    /// Like [`PoolHandle::send`], but a lazy pool's thread that is not running is not started for it (C's opcodes to
    /// a stream thread slot with no thread): fails with the message then.
    pub fn send_if_running(&self, index: usize, msg: M) -> Result<(), M> {
        let Some(mailbox) = self.mailboxes.get(index) else {
            return Err(msg);
        };
        if self.lazy.is_some() {
            let idle = mailbox.idle.lock().unwrap_or_else(PoisonError::into_inner);
            if idle.is_some() {
                return Err(msg);
            }
            mailbox.tx.send(Envelope::Msg(msg)).map_err(|e| match e.0 {
                Envelope::Msg(msg) => msg,
                Envelope::Stop => unreachable!("only Msg envelopes are sent here"),
            })?;
            drop(idle);
            let _ = mailbox.waker.wake();
            return Ok(());
        }
        self.send(index, msg)
    }

    /// Queues `msg` for thread `index` and wakes it. Fails with the message when the index is out of range or the
    /// thread has already exited.
    pub fn send(&self, index: usize, msg: M) -> Result<(), M> {
        let Some(mailbox) = self.mailboxes.get(index) else {
            return Err(msg);
        };
        // a lazy thread starts (again) here; the message is queued under the same lock, which an ended thread takes to
        // give its queue back, so no message is left on a queue nobody runs
        let _idle = if let Some(lazy) = &self.lazy {
            let mut idle = mailbox.idle.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((poll, rx)) = idle.take() {
                match (lazy.start)(index, poll, rx) {
                    Ok(join) => lazy
                        .started
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(join),
                    // nd_thread_create(): the queue went with the failed thread, so the send below fails too
                    Err(err) => netdata_agent_log::netdata_log_error!("{err}"),
                }
            }
            Some(idle)
        } else {
            None
        };
        mailbox.tx.send(Envelope::Msg(msg)).map_err(|e| match e.0 {
            Envelope::Msg(msg) => msg,
            Envelope::Stop => unreachable!("only Msg envelopes are sent here"),
        })?;
        // only the push needs the lock
        drop(_idle);
        // A failed wake means the loop is gone; the message is then dropped with the channel.
        let _ = mailbox.waker.wake();
        Ok(())
    }
}

/// A running pool of event-loop threads.
pub struct Pool<M> {
    handle: PoolHandle<M>,
    threads: Vec<JoinHandle<()>>,
}

/// A pool thread panicked; its name is given.
#[derive(Debug)]
pub struct PoolPanicked(pub String);

impl fmt::Display for PoolPanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "event-loop thread {} panicked", self.0)
    }
}

impl std::error::Error for PoolPanicked {}

impl<M: Send + 'static> Pool<M> {
    /// Starts `threads` threads named by `name(index)`, each with a stack of `stack_size` bytes and running the worker
    /// `make(index)` returns.
    ///
    /// Workers are created on the calling thread, so a worker can be given handles of other pools.
    pub fn spawn<W>(
        threads: usize,
        stack_size: usize,
        name: impl Fn(usize) -> String,
        mut make: impl FnMut(usize) -> W,
    ) -> io::Result<Self>
    where
        W: Worker<Msg = M>,
    {
        let mut mailboxes = Vec::with_capacity(threads);
        let mut loops = Vec::with_capacity(threads);
        for index in 0..threads {
            let poll = Poll::new()?;
            let waker = Waker::new(poll.registry(), WAKER_TOKEN)?;
            let (tx, rx) = crossbeam_channel::unbounded();
            mailboxes.push(Mailbox {
                tx,
                waker,
                idle: Mutex::new(None),
            });
            loops.push((poll, rx, make(index)));
        }
        let handle = PoolHandle {
            mailboxes: mailboxes.into(),
            lazy: None,
        };

        let mut joins = Vec::with_capacity(threads);
        for (index, (poll, rx, worker)) in loops.into_iter().enumerate() {
            let spawned = std::thread::Builder::new()
                .name(name(index))
                .stack_size(stack_size)
                .spawn(move || {
                    let _ = run_loop(index, poll, rx, worker);
                });
            match spawned {
                Ok(join) => joins.push(join),
                Err(err) => {
                    // Stop the threads already running before reporting the failure.
                    let partial = Pool {
                        handle: handle.clone(),
                        threads: joins,
                    };
                    let _ = partial.stop();
                    return Err(thread_create_failed(&name(index), &err));
                }
            }
        }
        Ok(Pool {
            handle,
            threads: joins,
        })
    }

    /// Like [`Pool::spawn`], but each thread starts on its first message, as C starts its stream threads when a
    /// node is first assigned to them; `make` then runs on the new thread.
    pub fn spawn_lazy<W>(
        threads: usize,
        stack_size: usize,
        name: impl Fn(usize) -> String + Send + Sync + 'static,
        make: impl Fn(usize) -> W + Send + Sync + 'static,
    ) -> io::Result<Self>
    where
        W: Worker<Msg = M>,
    {
        let mut mailboxes = Vec::with_capacity(threads);
        for _ in 0..threads {
            let poll = Poll::new()?;
            let waker = Waker::new(poll.registry(), WAKER_TOKEN)?;
            let (tx, rx) = crossbeam_channel::unbounded();
            mailboxes.push(Mailbox {
                tx,
                waker,
                idle: Mutex::new(Some((poll, rx))),
            });
        }
        let mailboxes: Arc<[Mailbox<M>]> = mailboxes.into();
        let make = Arc::new(make);
        let parked = Arc::clone(&mailboxes);
        let start: Box<Starter<M>> = Box::new(move |index, poll, rx| {
            let (make, parked) = (Arc::clone(&make), Arc::clone(&parked));
            std::thread::Builder::new()
                .name(name(index))
                .stack_size(stack_size)
                .spawn(move || {
                    let mut parts = Some((poll, rx));
                    while let Some((poll, rx)) = parts.take() {
                        let Ended::Itself(poll, rx) = run_loop(index, poll, rx, make(index)) else {
                            break;
                        };
                        // back to idle for the next message to start it again (C starts an ended stream thread at
                        // its next assignment), unless one came meanwhile: `send()` queues under this lock
                        let mut idle = parked[index].idle.lock().unwrap_or_else(PoisonError::into_inner);
                        if rx.is_empty() {
                            *idle = Some((poll, rx));
                        } else {
                            parts = Some((poll, rx));
                        }
                    }
                })
                .map_err(|err| thread_create_failed(&name(index), &err))
        });
        Ok(Pool {
            handle: PoolHandle {
                mailboxes,
                lazy: Some(Arc::new(Lazy {
                    start,
                    started: Mutex::new(Vec::new()),
                })),
            },
            threads: Vec::new(),
        })
    }

    /// A handle for sending messages to this pool's threads.
    pub fn handle(&self) -> PoolHandle<M> {
        self.handle.clone()
    }

    /// Asks every thread to run its worker's `stop` and exit, and waits for all of them.
    pub fn stop(self) -> Result<(), PoolPanicked> {
        self.stop_within(None)
    }

    /// Like [`Pool::stop`], waiting at most `limit` (C's service waits): threads still running then are left to the
    /// exiting process. A pool thread that asks is not waited for, as C's waits skip their caller.
    pub fn stop_within(self, limit: Option<std::time::Duration>) -> Result<(), PoolPanicked> {
        self.ask_stop();
        self.wait(limit)
    }

    /// `service_wait_exit()` of threads that leave on their own once the exit started ([`Worker::running`]): waits
    /// at most `limit` without waking them, then asks the ones still running to stop and leaves them.
    pub fn join_within(self, limit: std::time::Duration) -> Result<(), PoolPanicked> {
        let deadline = Instant::now() + limit;
        let me = std::thread::current().id();
        let running = |pool: &Pool<M>| {
            let lazy = pool.handle.lazy.as_ref().map(|l| l.started.lock().unwrap_or_else(PoisonError::into_inner));
            pool.threads
                .iter()
                .chain(lazy.iter().flat_map(|l| l.iter()))
                .any(|j| !j.is_finished() && j.thread().id() != me)
        };
        while Instant::now() < deadline && running(&self) {
            // service_wait_exit()'s poll
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        self.ask_stop();
        self.wait(Some(std::time::Duration::ZERO))
    }

    fn ask_stop(&self) {
        for mailbox in self.handle.mailboxes.iter() {
            if mailbox.tx.send(Envelope::Stop).is_ok() {
                let _ = mailbox.waker.wake();
            }
        }
    }

    /// Joins the threads that end within `limit` (all of them without one); the others are left.
    fn wait(self, limit: Option<std::time::Duration>) -> Result<(), PoolPanicked> {
        let mut threads = self.threads;
        if let Some(lazy) = &self.handle.lazy {
            threads.append(&mut lazy.started.lock().unwrap_or_else(PoisonError::into_inner));
        }
        let me = std::thread::current().id();
        threads.retain(|join| join.thread().id() != me);
        if let Some(limit) = limit {
            let deadline = Instant::now() + limit;
            while Instant::now() < deadline && !threads.iter().all(JoinHandle::is_finished) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if !threads.iter().all(JoinHandle::is_finished) {
                // the threads left to the exiting process keep their wakers: a waker dropped before its thread saw
                // the stop's wake would leave it waiting for good
                std::mem::forget(Arc::clone(&self.handle.mailboxes));
            }
            threads.retain(JoinHandle::is_finished);
        }
        let mut result = Ok(());
        for join in threads {
            let name = join.thread().name().unwrap_or("unnamed").to_string();
            if join.join().is_err() && result.is_ok() {
                result = Err(PoolPanicked(name));
            }
        }
        result
    }
}

/// The exit path of a loop that ended itself: the queued messages first, as C's exit paths dequeue before their
/// cleanup, then the stop. Whether a stop was asked meanwhile.
fn end_itself<W: Worker>(worker: &mut W, cx: &mut Context<'_>, rx: &Receiver<Envelope<W::Msg>>) -> bool {
    let mut stop_asked = false;
    loop {
        match rx.try_recv() {
            Ok(Envelope::Msg(msg)) => worker.exit_message(cx, msg),
            Ok(Envelope::Stop) => stop_asked = true,
            Err(_) => break,
        }
    }
    worker.stop(cx);
    stop_asked
}

/// How a thread's loop ended: stopped (by the pool or a failure), or by itself with its poller and queue, which a lazy
/// pool keeps to start the thread again.
enum Ended<M> {
    Stopped,
    Itself(Poll, Receiver<Envelope<M>>),
}

/// `nd_thread_starting_point()` / `nd_thread_exit()` around the loop: C's records of every thread.
fn run_loop<W: Worker>(index: usize, poll: Poll, rx: Receiver<Envelope<W::Msg>>, mut worker: W) -> Ended<W::Msg> {
    netdata_agent_log::thread_created();
    let ended = run_worker(index, poll, rx, &mut worker);
    netdata_agent_log::thread_finished_errno(worker.exit_errno());
    ended
}

fn run_worker<W: Worker>(
    index: usize,
    mut poll: Poll,
    rx: Receiver<Envelope<W::Msg>>,
    worker: &mut W,
) -> Ended<W::Msg> {
    let mut timers = Timers::default();
    let mut events = Events::with_capacity(EVENTS_CAPACITY);

    {
        let mut cx = Context {
            registry: poll.registry(),
            timers: &mut timers,
            index,
        };
        if worker.start(&mut cx).is_err() {
            worker.stop(&mut cx);
            return Ended::Stopped;
        }
    }

    loop {
        if W::EXIT_AT_LOOP_TOP && !worker.running() {
            let mut cx = Context {
                registry: poll.registry(),
                timers: &mut timers,
                index,
            };
            let stop_asked = end_itself(worker, &mut cx, &rx);
            return if stop_asked { Ended::Stopped } else { Ended::Itself(poll, rx) };
        }
        let timeout = timers
            .next_deadline()
            .map(|at| at.saturating_duration_since(Instant::now()));
        if let Err(err) = poll.poll(&mut events, timeout) {
            if err.kind() != io::ErrorKind::Interrupted {
                let mut cx = Context {
                    registry: poll.registry(),
                    timers: &mut timers,
                    index,
                };
                worker.stop(&mut cx);
                return Ended::Stopped;
            }
        }

        let mut cx = Context {
            registry: poll.registry(),
            timers: &mut timers,
            index,
        };

        if !W::EXIT_AT_LOOP_TOP && !worker.running() {
            let stop_asked = end_itself(worker, &mut cx, &rx);
            return if stop_asked { Ended::Stopped } else { Ended::Itself(poll, rx) };
        }

        for event in events.iter() {
            if event.token() != WAKER_TOKEN {
                worker.event(&mut cx, event);
            }
        }

        // The waker only interrupts poll(); messages are drained on every iteration.
        let mut stopping = false;
        loop {
            match rx.try_recv() {
                Ok(Envelope::Msg(msg)) => worker.message(&mut cx, msg),
                Ok(Envelope::Stop) | Err(TryRecvError::Disconnected) => {
                    stopping = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        let now = Instant::now();
        while let Some(timer) = cx.timers.pop_due(now) {
            worker.timer(&mut cx, timer);
        }

        if stopping {
            worker.stop(&mut cx);
            return Ended::Stopped;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    /// std's default thread stack.
    const TEST_STACK: usize = 2 << 20;
    use std::time::Duration;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Seen {
        Start(usize),
        Msg(usize, u32),
        Timer(usize, u32),
        Stop(usize),
    }

    struct Recorder {
        log: Arc<Mutex<Vec<Seen>>>,
        timers: Vec<(TimerId, u32)>,
    }

    impl Worker for Recorder {
        type Msg = u32;

        fn start(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
            self.log.lock().unwrap().push(Seen::Start(cx.index()));
            Ok(())
        }

        fn event(&mut self, _cx: &mut Context<'_>, _event: &Event) {}

        fn message(&mut self, cx: &mut Context<'_>, msg: u32) {
            if msg >= 100 {
                // Arm a timer whose delay orders it: 100+n fires after 100+m when n > m.
                let at = Instant::now() + Duration::from_millis(u64::from(msg - 100) * 20);
                let id = cx.add_timer(at);
                self.timers.push((id, msg));
                if msg == 101 {
                    cx.cancel_timer(id);
                }
                return;
            }
            self.log.lock().unwrap().push(Seen::Msg(cx.index(), msg));
        }

        fn timer(&mut self, cx: &mut Context<'_>, timer: TimerId) {
            let msg = self
                .timers
                .iter()
                .find(|(id, _)| *id == timer)
                .map(|(_, m)| *m)
                .unwrap();
            self.log.lock().unwrap().push(Seen::Timer(cx.index(), msg));
        }

        fn stop(&mut self, cx: &mut Context<'_>) {
            self.log.lock().unwrap().push(Seen::Stop(cx.index()));
        }
    }

    fn recorder_pool(threads: usize) -> (Pool<u32>, Arc<Mutex<Vec<Seen>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let pool = Pool::spawn(
            threads,
            TEST_STACK,
            |i| format!("TEST[{i}]"),
            |_| Recorder {
                log: Arc::clone(&log),
                timers: Vec::new(),
            },
        )
        .unwrap();
        (pool, log)
    }

    fn wait_for<T: std::fmt::Debug>(log: &Arc<Mutex<Vec<T>>>, pred: impl Fn(&[T]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pred(&log.lock().unwrap()) {
            assert!(
                Instant::now() < deadline,
                "timed out; log: {:?}",
                log.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A lazy pool starts a thread only when it gets its first message; the others never run their workers.
    #[test]
    fn a_lazy_pool_starts_threads_on_their_first_message() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let theirs = Arc::clone(&log);
        let pool = Pool::spawn_lazy(
            2,
            TEST_STACK,
            |i| format!("lazy-{i}"),
            move |_| Recorder {
                log: Arc::clone(&theirs),
                timers: Vec::new(),
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(log.lock().unwrap().is_empty());
        pool.handle().send(1, 7).unwrap();
        pool.handle().send(1, 8).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while log.lock().unwrap().len() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        pool.stop().unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            [
                Seen::Start(1),
                Seen::Msg(1, 7),
                Seen::Msg(1, 8),
                Seen::Stop(1)
            ]
        );
    }

    #[test]
    fn messages_reach_their_thread_in_order_and_stop_runs_last() {
        let (pool, log) = recorder_pool(3);
        let handle = pool.handle();
        for msg in 0..10 {
            handle.send((msg % 3) as usize, msg).unwrap();
        }
        assert_eq!(handle.send(3, 99), Err(99));
        wait_for(&log, |l| {
            l.iter().filter(|s| matches!(s, Seen::Msg(..))).count() == 10
        });
        pool.stop().unwrap();

        let log = log.lock().unwrap();
        for thread in 0..3 {
            let seen: Vec<Seen> = log
                .iter()
                .filter(|s| match s {
                    Seen::Start(i) | Seen::Msg(i, _) | Seen::Timer(i, _) | Seen::Stop(i) => {
                        *i == thread
                    }
                })
                .cloned()
                .collect();
            let mut want = vec![Seen::Start(thread)];
            want.extend(
                (0..10u32)
                    .filter(|m| (*m as usize) % 3 == thread)
                    .map(|m| Seen::Msg(thread, m)),
            );
            want.push(Seen::Stop(thread));
            assert_eq!(seen, want, "thread {thread}");
        }
    }

    #[test]
    fn timers_fire_in_deadline_order_and_cancelled_ones_never_fire() {
        let (pool, log) = recorder_pool(1);
        let handle = pool.handle();
        // Armed out of order; 101 is cancelled right after arming.
        for msg in [103, 100, 101, 102] {
            handle.send(0, msg).unwrap();
        }
        wait_for(&log, |l| {
            l.iter().filter(|s| matches!(s, Seen::Timer(..))).count() == 3
        });
        std::thread::sleep(Duration::from_millis(60));
        pool.stop().unwrap();
        let timers: Vec<Seen> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|s| matches!(s, Seen::Timer(..)))
            .cloned()
            .collect();
        assert_eq!(
            timers,
            vec![
                Seen::Timer(0, 100),
                Seen::Timer(0, 102),
                Seen::Timer(0, 103)
            ]
        );
    }

    /// Every thread polls a clone of the same listener, as C web workers do; each connection is accepted by
    /// exactly one of them and served on that thread.
    struct Echo {
        listener: mio::net::TcpListener,
        clients: Vec<mio::net::TcpStream>,
        accepted: Arc<Mutex<Vec<usize>>>,
    }

    const LISTENER: Token = Token(0);

    impl Worker for Echo {
        type Msg = ();

        fn start(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
            cx.registry()
                .register(&mut self.listener, LISTENER, Interest::READABLE)
        }

        fn event(&mut self, cx: &mut Context<'_>, event: &Event) {
            if event.token() == LISTENER {
                loop {
                    match self.listener.accept() {
                        Ok((mut stream, _)) => {
                            let token = Token(self.clients.len() + 1);
                            cx.registry()
                                .register(&mut stream, token, Interest::READABLE)
                                .unwrap();
                            self.clients.push(stream);
                            self.accepted.lock().unwrap().push(cx.index());
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => panic!("accept: {e}"),
                    }
                }
                return;
            }
            let stream = &mut self.clients[event.token().0 - 1];
            let mut buf = [0u8; 64];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => stream.write_all(&buf[..n]).unwrap(),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => panic!("read: {e}"),
                }
            }
        }

        fn message(&mut self, _cx: &mut Context<'_>, _msg: ()) {}
    }

    #[test]
    fn shared_listener_accepts_each_connection_once() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let pool: Pool<()> = Pool::spawn(
            4,
            TEST_STACK,
            |i| format!("WEB[{i}]"),
            |_| Echo {
                listener: mio::net::TcpListener::from_std(listener.try_clone().unwrap()),
                clients: Vec::new(),
                accepted: Arc::clone(&accepted),
            },
        )
        .unwrap();

        const CONNECTIONS: usize = 32;
        for n in 0..CONNECTIONS {
            let mut client = std::net::TcpStream::connect(addr).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let msg = format!("hello {n}");
            client.write_all(msg.as_bytes()).unwrap();
            let mut back = vec![0u8; msg.len()];
            client.read_exact(&mut back).unwrap();
            assert_eq!(back, msg.as_bytes());
        }
        pool.stop().unwrap();
        let accepted = accepted.lock().unwrap();
        assert_eq!(accepted.len(), CONNECTIONS);
        assert!(accepted.iter().all(|i| *i < 4));
    }

    struct Panicker;

    impl Worker for Panicker {
        type Msg = ();
        fn event(&mut self, _cx: &mut Context<'_>, _event: &Event) {}
        fn message(&mut self, _cx: &mut Context<'_>, _msg: ()) {
            panic!("worker failure");
        }
    }

    /// A worker whose condition turned false (C's `service_running()`) at a wake of its loop: what is queued goes to
    /// its exit path, its stop runs and its thread ends before the pool is stopped.
    struct Leaver {
        running: Arc<std::sync::atomic::AtomicBool>,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Worker for Leaver {
        type Msg = ();
        fn running(&self) -> bool {
            self.running.load(std::sync::atomic::Ordering::Relaxed)
        }
        fn event(&mut self, _cx: &mut Context<'_>, _event: &Event) {}
        fn message(&mut self, _cx: &mut Context<'_>, _msg: ()) {
            self.log.lock().unwrap().push("message");
        }
        fn stop(&mut self, _cx: &mut Context<'_>) {
            self.log.lock().unwrap().push("stop");
        }
    }

    #[test]
    fn a_worker_that_stops_running_ends_its_thread() {
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (r, l) = (Arc::clone(&running), Arc::clone(&log));
        let pool: Pool<()> = Pool::spawn(1, TEST_STACK, |i| format!("L[{i}]"), move |_| Leaver {
            running: Arc::clone(&r),
            log: Arc::clone(&l),
        })
        .unwrap();
        pool.handle().send(0, ()).unwrap();
        wait_for(&log, |l| l.len() == 1);
        running.store(false, std::sync::atomic::Ordering::Relaxed);
        // the next wake ends it: the message that wakes it is not handled
        pool.handle().send(0, ()).unwrap();
        wait_for(&log, |l| l.len() == 3);
        // the message that woke it went to the exit path (by default, handled), then the stop
        assert_eq!(*log.lock().unwrap(), ["message", "message", "stop"]);
        assert!(pool.handle().send(0, ()).is_err(), "its mailbox is gone");
        pool.stop().unwrap();
        assert_eq!(*log.lock().unwrap(), ["message", "message", "stop"], "stop ran once");
    }

    /// A lazy pool's worker that tests a shared condition, recording what it handles by thread generation.
    struct Gen {
        generation: usize,
        running: Arc<std::sync::atomic::AtomicBool>,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Worker for Gen {
        type Msg = u32;
        fn running(&self) -> bool {
            self.running.load(std::sync::atomic::Ordering::Relaxed)
        }
        fn event(&mut self, _cx: &mut Context<'_>, _event: &Event) {}
        fn message(&mut self, _cx: &mut Context<'_>, msg: u32) {
            self.log.lock().unwrap().push(format!("{} message {msg}", self.generation));
        }
        fn exit_message(&mut self, _cx: &mut Context<'_>, msg: u32) {
            self.log.lock().unwrap().push(format!("{} exit message {msg}", self.generation));
        }
        fn stop(&mut self, _cx: &mut Context<'_>) {
            self.log.lock().unwrap().push(format!("{} stop", self.generation));
        }
    }

    /// A lazy thread that ended itself hands what was queued to its worker's exit path, and the next message starts
    /// it again with a new worker (C starts an ended stream thread at its next assignment).
    #[test]
    fn an_ended_lazy_thread_starts_again_at_its_next_message() {
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let log = Arc::new(Mutex::new(Vec::new()));
        let generations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (r, l, g) = (Arc::clone(&running), Arc::clone(&log), Arc::clone(&generations));
        let pool: Pool<u32> = Pool::spawn_lazy(1, TEST_STACK, |i| format!("G[{i}]"), move |_| Gen {
            generation: g.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            running: Arc::clone(&r),
            log: Arc::clone(&l),
        })
        .unwrap();
        let handle = pool.handle();
        handle.send(0, 1).unwrap();
        wait_for(&log, |l| l.len() == 1);
        running.store(false, std::sync::atomic::Ordering::Relaxed);
        handle.send(0, 2).unwrap();
        wait_for(&log, |l| l.len() == 3);
        running.store(true, std::sync::atomic::Ordering::Relaxed);
        // it gave its queue back, so this starts it again
        wait_for(&log, |_| pool.handle.mailboxes[0].idle.lock().unwrap().is_some());
        handle.send(0, 3).unwrap();
        wait_for(&log, |l| l.len() == 4);
        pool.stop().unwrap();
        assert_eq!(*log.lock().unwrap(), ["0 message 1", "0 exit message 2", "0 stop", "1 message 3", "1 stop"]);
    }

    /// A worker whose first generation's stop waits for the test.
    struct Blocker {
        generation: usize,
        release: Option<std::sync::mpsc::Receiver<()>>,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Worker for Blocker {
        type Msg = u32;
        fn running(&self) -> bool {
            false
        }
        fn event(&mut self, _cx: &mut Context<'_>, _event: &Event) {}
        fn message(&mut self, _cx: &mut Context<'_>, msg: u32) {
            self.log.lock().unwrap().push(format!("{} message {msg}", self.generation));
        }
        fn exit_message(&mut self, _cx: &mut Context<'_>, msg: u32) {
            self.log.lock().unwrap().push(format!("{} exit message {msg}", self.generation));
        }
        fn stop(&mut self, _cx: &mut Context<'_>) {
            self.log.lock().unwrap().push(format!("{} stop", self.generation));
            if let Some(release) = self.release.take() {
                let _ = release.recv();
            }
        }
    }

    /// A message queued while an ended lazy thread is still in its exit path makes it run again in place: nothing is
    /// left on a queue nobody runs.
    #[test]
    fn a_message_during_the_exit_path_runs_the_thread_again() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = std::sync::mpsc::channel();
        let release = Arc::new(Mutex::new(Some(rx)));
        let generations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (l, r, g) = (Arc::clone(&log), Arc::clone(&release), Arc::clone(&generations));
        let pool: Pool<u32> = Pool::spawn_lazy(1, TEST_STACK, |i| format!("B[{i}]"), move |_| Blocker {
            generation: g.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            release: r.lock().unwrap().take(),
            log: Arc::clone(&l),
        })
        .unwrap();
        let handle = pool.handle();
        handle.send(0, 1).unwrap();
        wait_for(&log, |l| l.len() == 2);
        // generation 0 is in its stop, its queue drained: this message is queued for it
        handle.send(0, 2).unwrap();
        tx.send(()).unwrap();
        wait_for(&log, |l| l.len() == 4);
        wait_for(&log, |_| pool.handle.mailboxes[0].idle.lock().unwrap().is_some());
        pool.stop().unwrap();
        assert_eq!(*log.lock().unwrap(), ["0 exit message 1", "0 stop", "1 exit message 2", "1 stop"]);
    }

    /// A join waits out its limit without waking threads that keep running, then asks them to stop.
    #[test]
    fn a_join_waits_without_waking() {
        let (pool, log) = recorder_pool(2);
        let started = Instant::now();
        pool.join_within(Duration::from_millis(100)).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        wait_for(&log, |l| l.iter().filter(|s| matches!(s, Seen::Stop(_))).count() == 2);
    }

    #[test]
    fn a_panicking_thread_is_reported_by_stop() {
        let pool: Pool<()> =
            Pool::spawn(2, TEST_STACK, |i| format!("P[{i}]"), |_| Panicker).unwrap();
        pool.handle().send(1, ()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let err = pool.stop().unwrap_err();
        assert_eq!(err.0, "P[1]");
    }
}
