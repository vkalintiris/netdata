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
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use mio::{Events, Poll, Waker};

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

    /// A registered source became ready.
    fn event(&mut self, cx: &mut Context<'_>, event: &Event);

    /// A message sent to this thread through the pool's handle.
    fn message(&mut self, cx: &mut Context<'_>, msg: Self::Msg);

    /// A timer armed with [`Context::add_timer`] expired.
    fn timer(&mut self, _cx: &mut Context<'_>, _timer: TimerId) {}

    /// Called once on the worker's thread when the pool stops, before the thread exits.
    fn stop(&mut self, _cx: &mut Context<'_>) {}
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
}

/// Sends messages to the threads of a running pool. Cheap to clone and usable from any thread.
pub struct PoolHandle<M> {
    mailboxes: Arc<[Mailbox<M>]>,
}

impl<M> Clone for PoolHandle<M> {
    fn clone(&self) -> Self {
        Self {
            mailboxes: Arc::clone(&self.mailboxes),
        }
    }
}

impl<M: Send + 'static> PoolHandle<M> {
    /// The number of threads in the pool.
    pub fn threads(&self) -> usize {
        self.mailboxes.len()
    }

    /// Queues `msg` for thread `index` and wakes it. Fails with the message when the index is out of range or the
    /// thread has already exited.
    pub fn send(&self, index: usize, msg: M) -> Result<(), M> {
        let Some(mailbox) = self.mailboxes.get(index) else {
            return Err(msg);
        };
        mailbox.tx.send(Envelope::Msg(msg)).map_err(|e| match e.0 {
            Envelope::Msg(msg) => msg,
            Envelope::Stop => unreachable!("only Msg envelopes are sent here"),
        })?;
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
            mailboxes.push(Mailbox { tx, waker });
            loops.push((poll, rx, make(index)));
        }
        let handle = PoolHandle {
            mailboxes: mailboxes.into(),
        };

        let mut joins = Vec::with_capacity(threads);
        for (index, (poll, rx, worker)) in loops.into_iter().enumerate() {
            let spawned = std::thread::Builder::new()
                .name(name(index))
                .stack_size(stack_size)
                .spawn(move || run_loop(index, poll, rx, worker));
            match spawned {
                Ok(join) => joins.push(join),
                Err(err) => {
                    // Stop the threads already running before reporting the failure.
                    let partial = Pool {
                        handle: handle.clone(),
                        threads: joins,
                    };
                    let _ = partial.stop();
                    return Err(err);
                }
            }
        }
        Ok(Pool {
            handle,
            threads: joins,
        })
    }

    /// A handle for sending messages to this pool's threads.
    pub fn handle(&self) -> PoolHandle<M> {
        self.handle.clone()
    }

    /// Asks every thread to run its worker's `stop` and exit, and waits for all of them.
    pub fn stop(self) -> Result<(), PoolPanicked> {
        for mailbox in self.handle.mailboxes.iter() {
            if mailbox.tx.send(Envelope::Stop).is_ok() {
                let _ = mailbox.waker.wake();
            }
        }
        let mut result = Ok(());
        for join in self.threads {
            let name = join.thread().name().unwrap_or("unnamed").to_string();
            if join.join().is_err() && result.is_ok() {
                result = Err(PoolPanicked(name));
            }
        }
        result
    }
}

fn run_loop<W: Worker>(
    index: usize,
    mut poll: Poll,
    rx: Receiver<Envelope<W::Msg>>,
    mut worker: W,
) {
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
            return;
        }
    }

    loop {
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
                return;
            }
        }

        let mut cx = Context {
            registry: poll.registry(),
            timers: &mut timers,
            index,
        };

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
            return;
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

    fn wait_for(log: &Arc<Mutex<Vec<Seen>>>, pred: impl Fn(&[Seen]) -> bool) {
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
