//! A host's sender (`struct sender_state`: `src/streaming/stream-sender-api.c` and `stream-sender.c`): created with
//! the host's streaming settings, queued for its parents at its first collection, connected by the connector and
//! handed to its stream thread, which sends what the host commits and executes what the parent sends down. Maps:
//! `knowledge/map-m7-commit3-connector.md` §1-§2, `knowledge/map-m7-commit4-runtime.md`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use netdata_agent_log::{Field, FrameGuard, Priority, Source, Value, msgid, nd_log, push};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::{Host, StreamSend, sender_flags};
use netdata_agent_rrd::upstream::Upstream;
use netdata_agent_rrd::pulse::host_status;
use netdata_agent_rrd::stream_buffer::CircularBuffer;

mod commit;
pub(crate) mod dispatch;
mod execute;
mod hooks;
pub(crate) use hooks::send_node_and_claim_id_to_child;

pub use netdata_agent_rrd::upstream::Traffic;

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
    /// `netdata_ssl_validate_certificate_sender`, `ssl_ca_file` and `ssl_ca_path` (empty when unset).
    pub ssl_validate_certificate: bool,
    pub ssl_ca_file: String,
    pub ssl_ca_path: String,
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
            ssl_validate_certificate: send.ssl_validate_certificate,
            ssl_ca_file: send.ssl_ca_file.clone().unwrap_or_default(),
            ssl_ca_path: send.ssl_ca_path.clone().unwrap_or_default(),
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
    /// `host->stream.snd.api_key`: the key of the settings the sender was set up with (a revival replaces it).
    pub api_key: String,
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
    pub buffer: CircularBuffer,
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
    /// The host's machine GUID, which keys its stream thread's pin after the host is gone.
    pub(crate) machine_guid: String,
    pub(crate) connector: Arc<Connector>,
    state: Mutex<State>,
    /// `host->stream.snd.parents`: held by the connector for an attempt, briefly by everyone else.
    parents: Mutex<Parents>,
    /// Whether one of the parents is reached over TLS: what the collectors' gate needs of them, without the lock an
    /// attempt holds for its whole connect (C's gate takes only a read lock, which the connector's does not exclude).
    ssl_parent: AtomicBool,
    out: Mutex<Out>,
    ops: Mutex<Option<Ops>>,
    /// `s->disabled_capabilities`: every compression when `enable compression = no`, and an algorithm that failed.
    pub(crate) disabled: std::sync::atomic::AtomicU32,
    /// `host->stream.snd.status.replication.counter_in` and `counter_out`: requests received and answered.
    pub(crate) counter_in: std::sync::atomic::AtomicU32,
    pub(crate) counter_out: std::sync::atomic::AtomicU32,
    /// `s->exit.shutdown`.
    pub(crate) shutdown: AtomicBool,
    /// `s->capabilities` as the collectors read it, without the commit lock (C reads it unlocked).
    pub(crate) negotiated: std::sync::atomic::AtomicU32,
    /// `s->replication`: its side of the connector's replication queue.
    replication: Arc<crate::replication::SenderQueue>,
}

/// The connection the connector hands to the host's stream thread (`stream_sender_add_to_queue()`): still blocking,
/// plain or over TLS.
pub struct Connected {
    pub sender: Arc<Sender>,
    pub link: netdata_agent_tls::Link<netdata_agent_evloop::conn::Conn>,
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
            .field("tls", &self.link.is_tls())
            .field("capabilities", &self.capabilities)
            .field("compressed", &self.compressor.is_some())
            .field("remote_ip", &self.remote_ip)
            .field("thread", &self.thread)
            .finish()
    }
}

impl Sender {
    /// `stream_sender_structures_init()`: a host whose settings stream it (`stream_send`) gets its sender, created
    /// the first time and set up again after a free ([`Upstream::reinit`]); the sender when this call created it.
    pub fn attach(host: &Arc<Host>, connector: &Arc<Connector>) -> Option<Arc<Sender>> {
        let mut created = None;
        host.init_upstream(|send| {
            let s = Sender::new(host, connector, send);
            created = Some(Arc::clone(&s));
            s as Arc<dyn Upstream>
        });
        created
    }

    fn new(host: &Arc<Host>, connector: &Arc<Connector>, send: &StreamSend) -> Arc<Sender> {
        let disabled = if connector.settings.compression_enabled { 0 } else { caps::COMPRESSIONS_AVAILABLE };
        let parents = Parents::new(send.parents());
        Arc::new_cyclic(|me| Sender {
            me: Weak::clone(me),
            host: Arc::downgrade(host),
            machine_guid: host.machine_guid().to_string(),
            connector: Arc::clone(connector),
            state: Mutex::new(State {
                // stream_our_capabilities() runs before the disabled capabilities are set
                capabilities: caps::sender_ours(0, 0),
                hops: 0,
                remote_ip: String::new(),
                parent_using_h2o: false,
                exit_reason: Reason::NEVER,
                status_reason: Reason::NEVER,
                last_state_since_s: 0,
                api_key: send.api_key.clone(),
            }),
            ssl_parent: AtomicBool::new(parents.any_ssl()),
            parents: Mutex::new(parents),
            out: Mutex::new(Out {
                buffer: CircularBuffer::default(),
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
            negotiated: std::sync::atomic::AtomicU32::new(0),
            replication: crate::replication::SenderQueue::new(),
        })
    }

    /// `stream_sender_structures_free()`: signalled to stop with HOST CLEANUP, then, while still queued or
    /// dispatched, taken off the connector every 10 ms (and given the stop again once it is dispatched), for 200 waits
    /// where C waits for good (D118.2; one `remove()` can still wait for the parents lock of an attempt in progress,
    /// as C); then emptied as a new sender. A sender that is still live after the waits is left as it is.
    fn free_now(&self) {
        let mut posted = self.signal_stop(Reason::SND_DISCONNECT_HOST_CLEANUP, op::STOP_HOST_CLEANUP);
        if let (Some(me), Some(host)) = (self.me.upgrade(), self.host()) {
            let mut waits = 0;
            while host.sender_flags() & sender_flags::ADDED != 0 {
                if waits == 200 {
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "STREAM SND '{}': sender takes too long to stop, giving up...",
                        host.hostname()
                    );
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
                self.connector.remove_host(&me, &host);
                // handed over to its stream thread after the signal: the stop goes to its session now (the out
                // lock released first: a failed post reads it)
                let now = self.out().session;
                if let Some(session) = now.filter(|s| Some(*s) != posted) {
                    self.post(session, op::STOP_HOST_CLEANUP, Reason::SND_DISCONNECT_HOST_CLEANUP);
                    posted = Some(session);
                }
                waits += 1;
            }
        }
        {
            let mut out = self.out();
            self.flush_buffer(&mut out);
            out.compressor = None;
            out.algorithm = None;
            out.remote_ip.clear();
            out.capabilities = 0;
        }
        self.negotiated.store(0, Ordering::Relaxed);
        self.connector.replication().delete_pending(&self.replication);
        self.replication.replicating_zero();
        self.counter_in.store(0, Ordering::Relaxed);
        self.counter_out.store(0, Ordering::Relaxed);
        self.set_parents(Parents::new(std::iter::empty()));
    }

    /// Replaces the parents, and the TLS flag beside them.
    fn set_parents(&self, parents: Parents) {
        self.ssl_parent.store(parents.any_ssl(), Ordering::Relaxed);
        *self.parents() = parents;
    }

    /// `stream_sender_structures_init()` of a freed sender: set up as a new one with the settings of now. A sender
    /// still queued or dispatched after a free that gave up keeps its capabilities, hops, remote address and its stop;
    /// its parents, key and disabled compressions are the new settings'.
    fn reinit_now(&self, send: &StreamSend) {
        let disabled = if self.connector.settings.compression_enabled { 0 } else { caps::COMPRESSIONS_AVAILABLE };
        self.disabled.store(disabled, Ordering::Relaxed);
        {
            let mut state = self.lock();
            if !self.host().is_some_and(|h| h.sender_flags() & sender_flags::ADDED != 0) {
                state.capabilities = caps::sender_ours(0, 0);
                state.hops = 0;
                state.remote_ip.clear();
                state.parent_using_h2o = false;
                // a stop flag left from before the free cannot reach the revived sender
                self.shutdown.store(false, Ordering::Relaxed);
            }
            state.api_key.clone_from(&send.api_key);
        }
        self.set_parents(Parents::new(send.parents()));
    }

    pub fn host(&self) -> Option<Arc<Host>> {
        self.host.upgrade()
    }

    pub(crate) fn replication(&self) -> &Arc<crate::replication::SenderQueue> {
        &self.replication
    }

    /// `stream_circular_buffer_flush_unsafe()` of the sender's buffer, its time mirrored for the replication queue.
    pub(crate) fn flush_buffer(&self, out: &mut Out) {
        out.buffer.flush(self.connector.settings.buffer_max_size, netdata_agent_sys::now_monotonic_usec());
        self.replication.last_flush_ut.store(out.buffer.last_flush_ut(), Ordering::Relaxed);
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
    /// session are dropped where they are handled (D103.5), and never displace the current session's waiting ones
    /// (R55 I4: a committer preempted across a whole reconnect).
    pub(crate) fn post(&self, session: Session, op: u32, reason: Reason) {
        let Some(me) = self.me.upgrade() else {
            return;
        };
        if op == op::POLLOUT && crate::thread::current() == Some(session.thread) {
            crate::thread::pollout_inline(&me, session);
            return;
        }
        let current = self.out().session;
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
                Some(ops) if Some(ops.session) == current && current != Some(session) => return,
                _ => {
                    *slot = Some(Ops { session, bits: op, reason });
                    true
                }
            }
        };
        if first
            && self
                .connector
                .pool()
                .send_if_running(session.thread, crate::thread::StreamMsg::SenderOps(Arc::downgrade(&me), session))
                .is_err()
        {
            // the thread ended (the exit started): C's stream_thread_by_slot_id() finds no thread
            *self.ops.lock().unwrap_or_else(PoisonError::into_inner) = None;
            let remote_ip = self.out().remote_ip.clone();
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND[x] '{}' [to {remote_ip}] the opcode ({op}) message cannot be verified. Ignoring it.",
                self.hostname()
            );
        }
    }

    /// The opcodes waiting for this sender's `session`, taken: a newer session's stay for its own thread's message.
    pub(crate) fn take_ops(&self, session: Session) -> Option<Ops> {
        let mut slot = self.ops.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|ops| ops.session == session) { slot.take() } else { None }
    }

    /// `stream_sender_signal_to_stop_and_wait()` without its wait: a sender queued for its parents or dispatched is
    /// marked to stop with `reason` (the connector's next pass removes a queued one), and a dispatched one gets `op`
    /// (a session gone meanwhile drops it where it is handled). Returns the session `op` was posted to.
    pub(crate) fn signal_stop(&self, reason: Reason, op: u32) -> Option<Session> {
        {
            // ADDED is read under the lock that add() and remove() set and clear it under, as C does
            let mut state = self.lock();
            if self.host().is_some_and(|h| h.sender_flags() & sender_flags::ADDED != 0) {
                self.shutdown.store(true, Ordering::Relaxed);
                state.exit_reason = reason;
            }
        }
        let session = self.out().session;
        if let Some(session) = session {
            self.post(session, op, reason);
        }
        session
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
        self.negotiated.load(Ordering::Relaxed)
    }

    fn commit(&self, bytes: &[u8], traffic: Traffic) {
        Sender::commit(self, bytes, traffic);
    }

    fn resync_iterations(&self) -> u16 {
        self.connector.settings.resync_iterations
    }

    fn flush_ut(&self) -> u64 {
        self.replication.last_flush_ut.load(Ordering::Relaxed)
    }

    fn commit_since(&self, bytes: &[u8], traffic: Traffic, flush_ut: u64) -> bool {
        self.commit_into(bytes, traffic, Some(flush_ut))
    }

    fn receiver_left(&self, reason: i32) {
        self.signal_stop(Reason(reason), op::STOP_RECEIVER_LEFT);
    }

    fn parents_reset(&self, reason: i32) {
        self.parents().reset(Reason(reason), self.connector.settings.reconnect_delay_s);
    }

    fn free(&self) {
        self.free_now();
    }

    fn reinit(&self, send: &StreamSend) {
        self.reinit_now(send);
    }

    /// `stream_sender_add_to_connector_queue()`.
    fn start(&self) {
        let (Some(me), Some(host)) = (self.me.upgrade(), self.host()) else {
            return;
        };
        let _frame = self.frame();
        // C queues the host even when its connector thread could not start
        self.connector.init(&host.hostname());
        self.connector.ssl_init(self.ssl_parent.load(Ordering::Relaxed));
        self.connector.add(&me, &host);
    }
}
