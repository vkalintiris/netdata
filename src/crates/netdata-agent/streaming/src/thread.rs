//! A stream thread (`src/streaming/stream-thread.c`): the event loop that runs the receivers of the children and the
//! senders of the hosts pinned to it (decisions D8, D103.2). The receivers' I/O is in `receiver.rs`.

use std::io;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use netdata_agent_evloop::{Context, Event, TimerId, Worker};
use netdata_agent_ingest as ingest;
use netdata_agent_rrd::host::ReceiverSlot;

use crate::pins::Pins;
use crate::receiver::{Attached, Child, REPLICATION_STALL};
use crate::sender::Connected;

/// The stream threads' tick: stop requests, queued senders and opcodes are looked at this often.
pub(crate) const TICK: Duration = Duration::from_millis(100);

/// What a stream thread is sent: a connection to take over, a backfilled chart's replication request for the
/// connection of `receiver`, or a sender's connection to its parent.
#[derive(Debug)]
pub enum StreamMsg {
    Attach(Box<Attached>),
    Replay(Weak<ReceiverSlot>, ingest::ReplayRequest),
    /// A sender's connection to its parent (`stream_sender_add_to_queue()`).
    AttachSender(Box<Connected>),
}

/// A stream thread: owns the connections of the children assigned to it, and parses what they send inline, and the
/// connections of the senders pinned to it.
pub struct StreamWorker {
    pub(crate) children: Vec<Option<Child>>,
    /// Senders connected to their parents, held without I/O until the sender runtime (D102.2).
    pub(crate) senders: Vec<Connected>,
    pub(crate) pins: Arc<Mutex<Pins>>,
    tick: Option<TimerId>,
    /// `nd_profile.update_every`: how often every connection is probed and checked for idleness.
    check_every: Duration,
    last_check: Instant,
    last_replication_check: Instant,
}

impl StreamWorker {
    pub fn new(pins: Arc<Mutex<Pins>>, update_every: i32) -> Self {
        let now = Instant::now();
        StreamWorker {
            children: Vec::new(),
            senders: Vec::new(),
            pins,
            tick: None,
            check_every: Duration::from_secs(u64::try_from(update_every).unwrap_or(1).max(1)),
            last_check: now,
            last_replication_check: now,
        }
    }
}

impl Worker for StreamWorker {
    type Msg = StreamMsg;

    fn start(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        self.tick = Some(cx.add_timer(Instant::now() + TICK));
        Ok(())
    }

    fn event(&mut self, cx: &mut Context<'_>, event: &Event) {
        self.child_event(cx, event.token().0, event);
    }

    fn message(&mut self, cx: &mut Context<'_>, msg: StreamMsg) {
        match msg {
            StreamMsg::Attach(attached) => self.attach(cx, *attached),
            StreamMsg::Replay(receiver, request) => self.replay(cx, &receiver, &request),
            StreamMsg::AttachSender(connected) => self.senders.push(*connected),
        }
    }

    /// `stream_thread_worker()`'s periodic work: every tick, then every update every the checks, and every ten
    /// minutes the replication check.
    fn timer(&mut self, cx: &mut Context<'_>, _timer: TimerId) {
        self.tick_children(cx);
        let now = Instant::now();
        if now.duration_since(self.last_check) >= self.check_every {
            self.last_check = now;
            self.check_all(cx, now);
            if now.duration_since(self.last_replication_check) >= REPLICATION_STALL {
                self.last_replication_check = now;
                self.check_replication(cx, now);
            }
        }
        self.tick = Some(cx.add_timer(Instant::now() + TICK));
    }

    fn stop(&mut self, cx: &mut Context<'_>) {
        self.stop_children(cx);
    }
}
