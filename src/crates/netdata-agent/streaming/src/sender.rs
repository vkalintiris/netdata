//! A host's sender (`struct sender_state`: `src/streaming/stream-sender-api.c` and the connector's side of
//! `stream-sender.c`) as far as milestone 7 commit 3 has it: created with the host's streaming settings, queued for
//! its parents at its first collection, connected by the connector and handed to its stream thread, which holds the
//! connection until the sender runtime (commit 4, D102.2). Map: `knowledge/map-m7-commit3-connector.md` §1-§2.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use netdata_agent_log::{Field, FrameGuard, Priority, Source, Value, msgid, nd_log, push};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::{Host, Upstream, sender_flags};
use netdata_agent_rrd::pulse::host_status;

use crate::caps;
use crate::compress::Compressor;
use crate::conf::{CompressionLevels, Send};
use crate::connector::Connector;
use crate::parents::Parents;
use crate::reason::Reason;

/// What the connector reads of `stream_send` (`[stream]` in `stream.conf`).
#[derive(Debug, Clone)]
pub struct Settings {
    pub default_port: u16,
    pub timeout_s: i64,
    pub reconnect_delay_s: i64,
    pub h2o: bool,
    pub compression_enabled: bool,
    pub compression_levels: CompressionLevels,
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
        }
    }
}

/// The sender's state under its lock (`stream_sender_lock()`).
#[derive(Debug)]
pub(crate) struct State {
    /// `s->capabilities`: offered, then negotiated.
    pub capabilities: u32,
    /// `s->disabled_capabilities`: every compression when `enable compression = no`.
    pub disabled_capabilities: u32,
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
    pub parents: Parents,
}

/// `struct sender_state`.
#[derive(Debug)]
pub struct Sender {
    me: Weak<Sender>,
    host: Weak<Host>,
    pub(crate) api_key: String,
    pub(crate) connector: Arc<Connector>,
    state: Mutex<State>,
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
                disabled_capabilities: disabled,
                hops: 0,
                remote_ip: String::new(),
                parent_using_h2o: false,
                exit_reason: Reason::NEVER,
                status_reason: Reason::NEVER,
                last_state_since_s: 0,
                parents: Parents::new(send.parents()),
            }),
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

    /// The frame of the connector's records for this host.
    pub(crate) fn frame(&self) -> FrameGuard {
        push(vec![
            (Field::NidlNode, Value::Str(self.hostname())),
            (Field::MessageId, Value::Uuid(msgid::STREAMING_TO_PARENT)),
        ])
    }

    /// `stream_sender_on_connect()`, on the connector thread; the chart reset, the default interface and the read
    /// buffer come with the sender runtime (commit 4).
    pub(crate) fn on_connect(&self, host: &Host) {
        nd_log!(Source::Daemon, Priority::Debug, "STREAM SND [{}]: running on-connect hooks...", host.hostname());
        host.sender_flags_set(sender_flags::CONNECTED);
    }

    /// `stream_sender_on_disconnect()`, as far as commit 3 has it (the chart reset and the child's path update come
    /// with commit 4).
    pub(crate) fn on_disconnect(&self, host: &Host) {
        nd_log!(Source::Daemon, Priority::Debug, "STREAM SND '{}': running on-disconnect hooks...", host.hostname());
        host.sender_flags_clear(sender_flags::READY_4_METRICS);
    }

    /// `stream_sender_remove()`: the host is off its connector and stream thread, ready to be queued again.
    pub(crate) fn remove(&self, host: &Host, reason: Reason) {
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
        // stream_parent_set_host_disconnect_reason()
        state.status_reason = reason;
        let since_ut = state.last_state_since_s as u64 * 1_000_000;
        if let Some(i) = state.parents.current {
            let d = &mut state.parents.list[i];
            d.since_ut = since_ut;
            d.reason = reason;
        }
        let delay = self.connector.settings.reconnect_delay_s;
        state.parents.reset(reason, delay);
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
