//! A stream thread (`src/streaming/stream-thread.c`): the event loop that runs the receivers of the children and the
//! senders of the hosts pinned to it (decisions D8, D103.2). The receivers' I/O is in `receiver.rs`.

use std::cell::{Cell, RefCell};
use std::io;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use netdata_agent_evloop::{Context, Event, TimerId, Token, Worker};
use netdata_agent_ingest as ingest;
use netdata_agent_rrd::host::ReceiverSlot;
use netdata_agent_rrd::stream_control;

use crate::pins::Pins;
use crate::receiver::{Attached, Child, REPLICATION_STALL};
use crate::sender::dispatch::{Dispatched, SENDER_TOKENS};
use crate::sender::{Connected, Sender, Session};
use crate::waiting_list::WaitingList;

/// The stream threads' tick: queued receivers and senders, stop requests and opcodes are looked at this often.
pub(crate) const TICK: Duration = Duration::from_millis(100);

thread_local! {
    /// The stream thread this thread is, if it is one (`sth->tid == gettid_cached()`).
    static CURRENT: Cell<Option<usize>> = const { Cell::new(None) };
    /// POLLOUTs posted on this stream thread, handled after the step that posted them.
    static INLINE: RefCell<Vec<(Weak<Sender>, Session)>> = const { RefCell::new(Vec::new()) };
}

/// The stream thread running here.
pub(crate) fn current() -> Option<usize> {
    CURRENT.with(Cell::get)
}

/// A POLLOUT for a sender of this thread: C handles it synchronously (`bypassed++`).
pub(crate) fn pollout_inline(sender: &Arc<Sender>, session: Session) {
    INLINE.with(|i| i.borrow_mut().push((Arc::downgrade(sender), session)));
}

pub(crate) fn take_inline() -> Vec<(Weak<Sender>, Session)> {
    INLINE.with(|i| std::mem::take(&mut *i.borrow_mut()))
}

/// What a stream thread is sent: a connection to take over, a backfilled chart's replication request for the
/// connection of `receiver`, or a sender's connection to its parent.
#[derive(Debug)]
pub enum StreamMsg {
    Attach(Box<Attached>),
    Replay(Weak<ReceiverSlot>, ingest::ReplayRequest),
    /// A sender's connection to its parent (`stream_sender_add_to_queue()`).
    AttachSender(Box<Connected>),
    /// A sender's opcodes for a session are waiting in its slot (`stream_sender_send_opcode()`).
    SenderOps(Weak<Sender>, Session),
}

/// A stream thread: owns the connections of the children assigned to it, and parses what they send inline, and the
/// connections of the senders pinned to it.
pub struct StreamWorker {
    pub(crate) children: Vec<Option<Child>>,
    /// The senders running here, by token.
    pub(crate) senders: Vec<Option<Dispatched>>,
    /// Senders queued by the connector, started at the next tick (`sth->queue.senders`, D103.6).
    pub(crate) queued_senders: Vec<Connected>,
    /// Receivers queued for this thread (`sth->queue.receivers`), admitted one per tick at most.
    waiting: WaitingList<Attached>,
    pub(crate) pins: Arc<Mutex<Pins>>,
    tick: Option<TimerId>,
    /// `nd_profile.update_every`: how often every connection is probed and checked for idleness.
    check_every: Duration,
    last_check: Instant,
    last_replication_check: Instant,
    /// The `errno` the last close left (a TLS close's EAGAIN); at the exit, what C's end record carries.
    pub(crate) exit_errno: i32,
}

impl StreamWorker {
    pub fn new(pins: Arc<Mutex<Pins>>, update_every: i32) -> Self {
        let now = Instant::now();
        StreamWorker {
            children: Vec::new(),
            senders: Vec::new(),
            queued_senders: Vec::new(),
            waiting: WaitingList::default(),
            pins,
            tick: None,
            check_every: Duration::from_secs(u64::try_from(update_every).unwrap_or(1).max(1)),
            last_check: now,
            last_replication_check: now,
            exit_errno: 0,
        }
    }
}

impl Worker for StreamWorker {
    type Msg = StreamMsg;

    /// `stream_thread_worker()`'s loop: it leaves at its first wake after the exit started (its tick bounds it as C's
    /// 100 ms poll does), through its exit path.
    fn running(&self) -> bool {
        !netdata_agent_sys::exit::initiated()
    }

    fn start(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        CURRENT.with(|c| c.set(Some(cx.index())));
        self.tick = Some(cx.add_timer(Instant::now() + TICK));
        Ok(())
    }

    fn event(&mut self, cx: &mut Context<'_>, event: &Event) {
        let token = event.token().0;
        if token >= SENDER_TOKENS {
            self.sender_event(cx, token - SENDER_TOKENS, event);
        } else {
            self.child_event(cx, token, event);
        }
        self.drain_inline(cx);
    }

    /// A child token due again (D126.6): its next read; senders drain to WouldBlock and never ask.
    fn reported_again(&mut self, cx: &mut Context<'_>, token: Token) {
        if token.0 < SENDER_TOKENS {
            self.child_again(cx, token.0);
        }
        self.drain_inline(cx);
    }

    fn message(&mut self, cx: &mut Context<'_>, msg: StreamMsg) {
        match msg {
            StreamMsg::Attach(attached) => self.waiting.push(*attached),
            StreamMsg::Replay(receiver, request) => self.replay(cx, &receiver, &request),
            StreamMsg::AttachSender(connected) => self.queued_senders.push(*connected),
            StreamMsg::SenderOps(sender, session) => {
                if let Some(sender) = sender.upgrade() {
                    self.sender_ops(cx, &sender, session);
                }
            }
        }
        self.drain_inline(cx);
    }

    /// What `stream_thread_worker()`'s exit path does with its queues: the connections queued for it are dequeued
    /// (and then disconnected by the cleanup), the opcodes and replication requests are dropped with the thread.
    fn exit_message(&mut self, _cx: &mut Context<'_>, msg: StreamMsg) {
        match msg {
            StreamMsg::Attach(attached) => self.waiting.push(*attached),
            StreamMsg::AttachSender(connected) => self.queued_senders.push(*connected),
            StreamMsg::Replay(..) | StreamMsg::SenderOps(..) => {}
        }
    }

    /// `stream_thread_worker()`'s periodic work: every tick the waiting list and the queued senders, then every
    /// update every the checks, and every ten minutes the replication check.
    fn timer(&mut self, cx: &mut Context<'_>, _timer: TimerId) {
        if let Some(attached) = self
            .waiting
            .next(stream_control::children_should_be_accepted(), ingest::throttle())
        {
            self.attach(cx, attached);
        }
        self.dequeue_senders(cx);
        self.tick_children(cx);
        self.tick_senders();
        let now = Instant::now();
        if now.duration_since(self.last_check) >= self.check_every {
            self.last_check = now;
            self.check_senders(cx);
            self.check_all(cx, crate::receiver::now_monotonic_ut());
            if now.duration_since(self.last_replication_check) >= REPLICATION_STALL {
                self.last_replication_check = now;
                // the senders before the receivers, as C's
                self.check_sender_replication(cx, now);
                self.check_replication(cx, now);
            }
        }
        // the POLLOUTs the tick's commits posted (a dequeued sender's hooks, retention changes) go out now, as C
        // handles them inline (review R41 n7)
        self.drain_inline(cx);
        self.tick = Some(cx.add_timer(Instant::now() + TICK));
    }

    /// The thread's exit: the queued senders, then every queued receiver moves to running, in order and without the
    /// waiting list's checks, then the senders are cleaned up before the receivers (`stream_sender_cleanup()` before
    /// `stream_receiver_cleanup()`).
    fn stop(&mut self, cx: &mut Context<'_>) {
        self.dequeue_senders(cx);
        for attached in self.waiting.take_all() {
            self.attach(cx, attached);
        }
        // records since the last close cleared C's errno: only the exit path's closes count
        self.exit_errno = 0;
        self.stop_senders(cx);
        self.stop_children(cx);
    }

    fn exit_errno(&self) -> i32 {
        self.exit_errno
    }
}
