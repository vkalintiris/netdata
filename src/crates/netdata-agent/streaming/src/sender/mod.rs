//! A host's sender (`struct sender_state`: `src/streaming/stream-sender-api.c` and `stream-sender.c`): created with
//! the host's streaming settings, queued for its parents at its first collection, connected by the connector and
//! handed to its stream thread, which sends what the host commits and executes what the parent sends down. Maps:
//! `knowledge/map-m7-commit3-connector.md` §1-§2, `knowledge/map-m7-commit4-runtime.md`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use netdata_agent_log::{Field, FrameGuard, Priority, Source, Value, msgid, nd_log, push};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::{Host, sender_flags};
use netdata_agent_rrd::upstream::Upstream;
use netdata_agent_rrd::pulse::host_status;

pub mod buffer;
mod commit;
pub(crate) mod dispatch;
mod execute;
mod hooks;

pub use buffer::Traffic;

use crate::caps;
use crate::compress::Compressor;
use crate::compression::Algorithm;
use crate::conf::{CompressionLevels, Send};
use crate::connector::Connector;
use crate::parents::Parents;
use crate::reason::Reason;

/// Bytes from the wire in a record.
pub(crate) fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A word in a record: `(unset)` when missing.
pub(crate) fn shown(w: Option<&[u8]>) -> String {
    w.map_or_else(|| "(unset)".to_string(), text)
}

/// What the connector reads of `stream_send` (`[stream]` in `stream.conf`).
#[derive(Debug, Clone)]
pub struct Settings {
    pub default_port: u16,
    pub timeout_s: i64,
    pub reconnect_delay_s: i64,
    pub h2o: bool,
    pub compression_enabled: bool,
    pub compression_levels: CompressionLevels,
    /// `stream_send.buffer_max_size`: the buffer's maximum at every connection.
    pub buffer_max_size: usize,
    /// `stream_send.initial_clock_resync_iterations`.
    pub resync_iterations: u16,
}

impl Settings {
    pub fn of(send: &Send) -> Self {
        Settings {
            default_port: send.default_port,
            timeout_s: send.timeout_s,
            reconnect_delay_s: send.reconnect_delay_s,
            h2o: send.h2o,
            compression_enabled: send.compression_enabled,
            compression_levels: send.compression_levels,
            buffer_max_size: send.buffer_max_size as usize,
            resync_iterations: send.initial_clock_resync_iterations,
        }
    }
}

/// The sender's state under its lock (`stream_sender_lock()`).
#[derive(Debug)]
pub(crate) struct State {
    /// `s->capabilities`: offered, then negotiated.
    pub capabilities: u32,
    pub hops: i16,
    /// `s->remote_ip`: the destination connected to, cut to `CONNECTED_TO_SIZE`.
    pub remote_ip: String,
    pub parent_using_h2o: bool,
    /// `s->exit.reason`.
    pub exit_reason: Reason,
    /// `host->stream.snd.status.reason`.
    pub status_reason: Reason,
    /// `s->last_state_since_t`.
    pub last_state_since_s: i64,
}

/// `s->thread.msg`: the stream thread and the random session of a dispatched connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub thread: usize,
    pub id: u32,
}

/// `STREAM_OPCODE_SENDER_*`.
pub mod op {
    pub const POLLOUT: u32 = 1 << 0;
    pub const BUFFER_OVERFLOW: u32 = 1 << 2;
    pub const RECONNECT_WITHOUT_COMPRESSION: u32 = 1 << 4;
    pub const STOP_RECEIVER_LEFT: u32 = 1 << 5;
    pub const STOP_HOST_CLEANUP: u32 = 1 << 6;
}

/// Opcodes waiting for the sender's stream thread (`s->thread.msg_slot`): ORed, the last nonzero reason kept, for the
/// session they were posted for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ops {
    pub session: Session,
    pub bits: u32,
    pub reason: Reason,
}

/// What the commit lock guards (`stream_sender_lock()` around `s->scb`, `s->thread.compressor` and `s->thread.msg`).
pub(crate) struct Out {
    pub buffer: buffer::CircularBuffer,
    pub compressor: Option<Compressor>,
    /// `s->thread.compressor.algorithm`, which stays when a set up fails.
    pub algorithm: Option<Algorithm>,
    pub levels: CompressionLevels,
    /// None while no connection is dispatched: commits are dropped.
    pub session: Option<Session>,
    /// `s->remote_ip` and `s->capabilities`, for the records and the pieces.
    pub remote_ip: String,
    pub capabilities: u32,
}

impl std::fmt::Debug for Out {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Out")
            .field("buffer", &self.buffer)
            .field("algorithm", &self.algorithm)
            .field("session", &self.session)
            .field("remote_ip", &self.remote_ip)
            .finish_non_exhaustive()
    }
}

/// `struct sender_state`.
#[derive(Debug)]
pub struct Sender {
    me: Weak<Sender>,
    host: Weak<Host>,
    pub(crate) api_key: String,
    pub(crate) connector: Arc<Connector>,
    state: Mutex<State>,
    /// `host->stream.snd.parents`: held by the connector for an attempt, briefly by everyone else.
    parents: Mutex<Parents>,
    out: Mutex<Out>,
    ops: Mutex<Option<Ops>>,
    /// `s->disabled_capabilities`: every compression when `enable compression = no`, and an algorithm that failed.
    pub(crate) disabled: std::sync::atomic::AtomicU32,
    /// `host->stream.snd.status.replication.counter_in` and `counter_out`: requests received and answered.
    pub(crate) counter_in: std::sync::atomic::AtomicU32,
    pub(crate) counter_out: std::sync::atomic::AtomicU32,
    /// `s->exit.shutdown`.
    pub(crate) shutdown: AtomicBool,
}

/// The connection the connector hands to the host's stream thread (`stream_sender_add_to_queue()`).
pub struct Connected {
    pub sender: Arc<Sender>,
    pub socket: socket2::Socket,
    pub capabilities: u32,
    /// `s->thread.compressor`, set up after the handshake; none on an uncompressed link.
    pub compressor: Option<Compressor>,
    pub remote_ip: String,
    pub thread: usize,
}

impl std::fmt::Debug for Connected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connected")
            .field("host", &self.sender.hostname())
            .field("socket", &self.socket)
            .field("capabilities", &self.capabilities)
            .field("compressed", &self.compressor.is_some())
            .field("remote_ip", &self.remote_ip)
            .field("thread", &self.thread)
            .finish()
    }
}

impl Sender {
    /// `stream_sender_structures_init()`: the sender of a host that streams (its `stream_send` is set), installed in
    /// the host.
    pub fn attach(host: &Arc<Host>, connector: &Arc<Connector>) -> Option<Arc<Sender>> {
        let send = host.info().stream_send?;
        let disabled = if connector.settings.compression_enabled { 0 } else { caps::COMPRESSIONS_AVAILABLE };
        let sender = Arc::new_cyclic(|me| Sender {
            me: Weak::clone(me),
            host: Arc::downgrade(host),
            api_key: send.api_key.clone(),
            connector: Arc::clone(connector),
            state: Mutex::new(State {
                // stream_our_capabilities() runs before the disabled capabilities are set
                capabilities: caps::sender_ours(0),
                hops: 0,
                remote_ip: String::new(),
                parent_using_h2o: false,
                exit_reason: Reason::NEVER,
                status_reason: Reason::NEVER,
                last_state_since_s: 0,
            }),
            parents: Mutex::new(Parents::new(send.parents())),
            out: Mutex::new(Out {
                buffer: buffer::CircularBuffer::default(),
                compressor: None,
                algorithm: None,
                levels: connector.settings.compression_levels,
                session: None,
                remote_ip: String::new(),
                capabilities: 0,
            }),
            ops: Mutex::new(None),
            disabled: std::sync::atomic::AtomicU32::new(disabled),
            counter_in: std::sync::atomic::AtomicU32::new(0),
            counter_out: std::sync::atomic::AtomicU32::new(0),
            shutdown: AtomicBool::new(false),
        });
        host.set_upstream(Arc::clone(&sender) as Arc<dyn Upstream>);
        Some(sender)
    }

    pub fn host(&self) -> Option<Arc<Host>> {
        self.host.upgrade()
    }

    pub fn hostname(&self) -> String {
        self.host().map(|h| h.hostname()).unwrap_or_default()
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn parents(&self) -> MutexGuard<'_, Parents> {
        self.parents.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The commit lock.
    pub(crate) fn out(&self) -> MutexGuard<'_, Out> {
        self.out.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `stream_sender_send_opcode()`: a POLLOUT posted on the session's own thread is handled there before it waits
    /// again; everything else waits in the sender's slot for one message to its thread. Opcodes of an earlier
    /// session are dropped where they are handled (D103.5).
    pub(crate) fn post(&self, session: Session, op: u32, reason: Reason) {
        let Some(me) = self.me.upgrade() else {
            return;
        };
        if op == op::POLLOUT && crate::thread::current() == Some(session.thread) {
            crate::thread::pollout_inline(&me, session);
            return;
        }
        let first = {
            let mut slot = self.ops.lock().unwrap_or_else(PoisonError::into_inner);
            match slot.as_mut() {
                Some(ops) if ops.session == session => {
                    ops.bits |= op;
                    if reason != Reason::NEVER {
                        ops.reason = reason;
                    }
                    false
                }
                _ => {
                    *slot = Some(Ops { session, bits: op, reason });
                    true
                }
            }
        };
        if first {
            let _ = self
                .connector
                .pool()
                .send(session.thread, crate::thread::StreamMsg::SenderOps(Arc::downgrade(&me), session));
        }
    }

    /// The opcodes waiting for this sender's `session`, taken: a newer session's stay for its own thread's message.
    pub(crate) fn take_ops(&self, session: Session) -> Option<Ops> {
        let mut slot = self.ops.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|ops| ops.session == session) { slot.take() } else { None }
    }

    /// The frame of the connector's records for this host.
    pub(crate) fn frame(&self) -> FrameGuard {
        push(vec![
            (Field::NidlNode, Value::Str(self.hostname())),
            (Field::MessageId, Value::Uuid(msgid::STREAMING_TO_PARENT)),
        ])
    }

    /// `stream_sender_remove()`: the host is off its connector and stream thread, ready to be queued again.
    pub(crate) fn remove(&self, host: &Host, reason: Reason) {
        let reason = {
            let mut state = self.lock();
            let reason = if reason == Reason::DISCONNECT_SIGNALED_TO_STOP && state.exit_reason != Reason::NEVER {
                state.exit_reason
            } else {
                reason
            };
            state.exit_reason = Reason::NEVER;
            self.shutdown.store(false, Ordering::Relaxed);
            host.sender_flags_clear(sender_flags::ADDED | sender_flags::CONNECTED | sender_flags::READY_4_METRICS);
            state.last_state_since_s = now_realtime_s();
            reason
        };
        let since_s = self.lock().last_state_since_s;
        self.set_disconnect_reason(reason, since_s);
        self.parents().reset(reason, self.connector.settings.reconnect_delay_s);
    }

    /// `stream_parent_set_host_disconnect_reason()`.
    pub(crate) fn set_disconnect_reason(&self, reason: Reason, since_s: i64) {
        self.lock().status_reason = reason;
        let mut parents = self.parents();
        if let Some(i) = parents.current {
            if let Some(d) = parents.list.get_mut(i) {
                d.since_ut = since_s as u64 * 1_000_000;
                d.reason = reason;
            }
        }
    }

    /// `stream_connector_remove()`: a host signalled to stop while waiting for its parents.
    pub(crate) fn connector_remove(&self, host: &Host) {
        let (remote_ip, exit_reason) = {
            let state = self.lock();
            (state.remote_ip.clone(), state.exit_reason)
        };
        nd_log!(
            Source::Daemon,
            Priority::Notice,
            "STREAM CNT '{}' [to {remote_ip}]: streaming connector removed host: {} (signaled to stop)",
            host.hostname(),
            exit_reason.text()
        );
        let reason = if exit_reason != Reason::NEVER { exit_reason } else { Reason::DISCONNECT_SIGNALED_TO_STOP };
        host.pulse_status(host_status::SND_OFFLINE);
        self.remove(host, reason);
    }
}

impl Upstream for Sender {
    fn disabled_capabilities(&self) -> u32 {
        self.disabled.load(Ordering::Relaxed)
    }

    fn capabilities(&self) -> u32 {
        self.out().capabilities
    }

    fn commit(&self, bytes: &[u8], traffic: Traffic) {
        Sender::commit(self, bytes, traffic);
    }

    fn resync_iterations(&self) -> u16 {
        self.connector.settings.resync_iterations
    }

    /// `stream_sender_add_to_connector_queue()`.
    fn start(&self) {
        let (Some(me), Some(host)) = (self.me.upgrade(), self.host()) else {
            return;
        };
        let _frame = self.frame();
        // C queues the host even when its connector thread could not start
        self.connector.init(&host.hostname());
        self.connector.add(&me, &host);
    }
}
