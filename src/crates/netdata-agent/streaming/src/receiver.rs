//! The receiver, ported from `src/streaming/stream-receiver-connection.c` (admission and the first response) and
//! `src/streaming/stream-thread.c` (assigning children to stream threads).
//!
//! Admission runs on the web worker that read the `STREAM` request, as in C: rejections decided before the takeover
//! go back on the web connection; after the takeover the socket is written with blocking sends and a timeout, then
//! handed to the least loaded stream thread.

use std::io;
use std::net::Shutdown;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use netdata_agent_evloop::conn::Conn;
use netdata_agent_tls::Link;
use netdata_agent_evloop::{Context, Event, Interest, PoolHandle, Token};
use netdata_agent_ingest::{self as ingest, Parser};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_pluginsd_proto::{LINE_MAX, LineReader};
use netdata_agent_rrd::chart::flags;
use netdata_agent_rrd::collection;
use netdata_agent_rrd::contexts::Taker;
use netdata_agent_rrd::host::{Attach, Host, HostInfo, Hosts, ReceiverLink, ReceiverSlot, StreamSend, receiver_op};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};
use netdata_agent_rrd::upstream::Traffic;
use netdata_agent_sys::now_monotonic_usec;
use netdata_agent_text::duration::duration_to_string;
use netdata_agent_text::size::size_to_string;

use crate::caps;
use crate::conf::{Keepalive, ReceiverDefaults, StreamConf};
use crate::connector::Connector;
use crate::decompress::Decompressor;
use crate::handshake::{self, StreamRequest};
use crate::pins::Pins;
use crate::reason::Reason;
use crate::records::{self, Counters, Peer};
use crate::sender::Sender;
use crate::thread::{StreamMsg, StreamWorker};

/// `send_to_child()` as the wire of a receiver's parser: its lines and calls go into the connection's buffer, which
/// its stream thread writes; once the connection is gone, 0 (C's "no buffer"). The slot is held weakly, so the socket
/// still closes with the connection.
struct ChildWire {
    slot: Weak<ReceiverSlot>,
}

impl ingest::functions::Wire for ChildWire {
    fn send(&self, text: &[u8], traffic: Traffic) -> isize {
        self.slot.upgrade().map_or(0, |slot| slot.send_to_child(text, traffic))
    }
}

/// `CONNECTION_PROBE_INTERVAL_SECONDS` and `CONNECTION_PROBE_COUNT` of the receiver's TCP keepalive.
const KEEPALIVE_PROBE_INTERVAL_S: u32 = 10;
const KEEPALIVE_PROBES: u32 = 3;

/// `stream_receiver_automatic_keepalive_idle()`: half the update every, 30..=3600 s.
fn automatic_keepalive_idle(update_every: u64) -> u32 {
    let idle = if update_every > 0 {
        update_every.div_ceil(2)
    } else {
        30
    };
    idle.clamp(30, 3600) as u32
}

/// `stream_receiver_update_every()`: the smallest update every of the child's charts, else the handshake's (0 for none).
fn receiver_update_every(host: &Host, handshake_update_every: i64) -> u64 {
    match host.receiver_min_update_every() {
        u32::MAX => u64::try_from(handshake_update_every).unwrap_or(0),
        observed => u64::from(observed),
    }
}

/// `STREAM_RECEIVER_IDLE_TIMEOUT_MIN_SECONDS`: a child quiet this long (or twice its update every) is disconnected.
const IDLE_TIMEOUT_MIN_S: u64 = 600;
/// How long replication may make no progress before a child is checked for stalled charts.
pub(crate) const REPLICATION_STALL: Duration = Duration::from_secs(600);

/// `stream_receiver_reconcile_keepalive()`: the keepalive options go on the socket once, and again when the
/// automatic idle's update every changed; each failed option is logged.
fn reconcile_keepalive(
    fd: std::os::fd::BorrowedFd<'_>,
    host: &Host,
    peer: &Peer,
    keepalive: &Keepalive,
    handshake_update_every: i64,
    initialized: &mut bool,
) {
    use nix::sys::socket::{setsockopt, sockopt};
    let observed = host.receiver_min_update_every();
    if *initialized
        && (!keepalive.automatic || observed == host.receiver_min_update_every_applied())
    {
        return;
    }
    *initialized = true;
    host.set_receiver_min_update_every_applied(observed);
    let enabled = keepalive.enabled;
    let idle_s = if enabled && keepalive.automatic {
        automatic_keepalive_idle(receiver_update_every(host, handshake_update_every))
    } else {
        keepalive.idle_s
    };
    let raw = std::os::fd::AsRawFd::as_raw_fd(&fd);
    let warn = |what: &str| {
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "STREAM RCV '{}' [from [{}]:{}]: {what} on socket {raw}",
            host.hostname(),
            peer.ip,
            peer.port
        );
    };
    if setsockopt(&fd, sockopt::KeepAlive, &enabled).is_err() {
        warn(if enabled {
            "cannot enable SO_KEEPALIVE"
        } else {
            "cannot disable SO_KEEPALIVE"
        });
        return;
    }
    if !enabled {
        return;
    }
    if setsockopt(&fd, sockopt::TcpKeepIdle, &idle_s).is_err() {
        warn("cannot set TCP_KEEPIDLE");
    }
    if setsockopt(&fd, sockopt::TcpKeepInterval, &KEEPALIVE_PROBE_INTERVAL_S).is_err() {
        warn("cannot set TCP_KEEPINTVL");
    }
    if setsockopt(&fd, sockopt::TcpKeepCount, &KEEPALIVE_PROBES).is_err() {
        warn("cannot set TCP_KEEPCNT");
    }
}

/// A receiver older than this without traffic is stale and may be replaced (`stream_receiver_accept_connection()`).
const STALE_RECEIVER_S: u64 = 30;

/// The complete lines of `bytes` through the child's parser; false at the first line it refuses. A line that left
/// bytes in the child's empty buffer has them written before the next line, as C's inline POLLOUT writes inside the
/// add; a failed write is only logged there (the read's end removes the child).
fn parse(reader: &mut LineReader, parser: &mut Parser, attached: &mut Attached, bytes: &[u8]) -> bool {
    for line in reader.push(bytes) {
        if !parser.feed(&line) {
            return false;
        }
        if parser.take_sent() && crate::thread::take_inline_child(&attached.slot) {
            attached.write_wanted = true;
            let _ = write_buffer(attached, Some((parser, Some(&line))));
        }
    }
    true
}

/// `STREAM RCV[n] '<host>' [from [<ip>]:<port>]: ` of the stream thread's records.
fn prefix(a: &Attached) -> String {
    format!("STREAM RCV[{}] '{}' [from [{}]:{}]: ", a.thread, a.host.hostname(), a.peer.ip, a.peer.port)
}

/// Who writes a child's buffer: what a failed write does (C's `process_opcodes_and_enable_removal`) and under which
/// fields its record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Writer {
    /// The poller's writable event (and the tick that stands for it): a failure removes the child.
    Poll,
    /// After a read (`stream_receiver_dequeue_senders()`): under the parser's fields; the caller ends the child.
    Read,
    /// An opcode (POLLOUT, inline or posted): a failure is logged and nothing more, as C's handler must not remove
    /// a receiver whose parser a caller may be inside; the next event or the periodic check removes it.
    Opcode,
}

/// `stream_receiver_send_data()`'s loop: the child's buffer written one contiguous chunk at a time under its lock,
/// until it drains (a grown ring then shrinks back, at most every 5 minutes, and the write is no longer wanted) or the
/// socket is full. The failure's reason, after C's record, when the connection failed: under the fields of `parser`
/// given inside or after a read, with the line being parsed when the write is that line's.
fn write_buffer(attached: &mut Attached, parser: Option<(&Parser, Option<&[u8]>)>) -> Result<(), Reason> {
    let slot = Arc::clone(&attached.slot);
    let mut buffer = slot.buffer();
    let failure = loop {
        let Some(b) = buffer.as_mut() else {
            return Ok(());
        };
        let chunk = b.next();
        if chunk.is_empty() {
            return Ok(());
        }
        match attached.stream.write(chunk) {
            Ok(n) if n > 0 => {
                let now_ut = now_monotonic_usec();
                b.del(n, now_ut);
                attached.hosts.storage().pulse().network.stream_sent(n);
                attached.host.stream_bytes_sent(n);
                // a write is traffic too (C's last_traffic_ut), for the idle timeout and the stale check at accept
                slot.last_traffic_ut.store(now_ut, Ordering::Relaxed);
                if b.stats().bytes_outstanding == 0 {
                    attached.write_wanted = false;
                    b.recreate_timed(now_ut, false);
                    return Ok(());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // only a zero write or a reset is the remote end closing; EPIPE is a write failure
            Ok(_) => break (Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE, 0, 0),
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {
                break (Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE, -1, netdata_agent_log::errno_of(&e));
            }
            Err(e) => break (Reason::DISCONNECT_SOCKET_WRITE_FAILED, -1, netdata_agent_log::errno_of(&e)),
        }
    };
    let (sent, sends) = buffer.as_ref().map_or((0, 0), |b| (b.stats().bytes_sent, b.stats().sends));
    drop(buffer);
    let (reason, rc, errno) = failure;
    let _parser = parser.map(|(parser, line)| line.map_or_else(|| parser.log_frame(), |line| parser.log_frame_of(line)));
    nd_log!(
        Source::Daemon,
        Priority::Err,
        errno = errno;
        "{}{} ({rc}, on fd {}) - closing receiver connection - we have sent {sent} bytes in {sends} operations.",
        prefix(attached),
        reason.text(),
        raw_fd(&attached.stream)
    );
    Err(reason)
}

/// Values admission takes from the rest of the daemon.
#[derive(Debug, Clone)]
pub struct Defaults {
    /// `rrd_memory_mode_name(default_rrd_memory_mode)`.
    pub db_mode: String,
    /// `default_rrd_history_entries`.
    pub history: i64,
    /// `health_plugin_enabled()`.
    pub health_enabled: bool,
    /// `nd_profile.update_every`.
    pub update_every: i32,
    /// `sysconf(_SC_PAGESIZE)`.
    pub page_size: i64,
    /// `gap_when_lost_iterations_above`: `[db] gap when lost iterations above` + 2.
    pub gap_when_lost_iterations_above: i64,
}

/// What the web worker does with a `STREAM` request after `pre_admit()`.
#[derive(Debug)]
pub enum PreAdmission {
    /// Send these bytes on the web connection (no HTTP header) and close it; the code is for the access log.
    Reply(&'static str, u16),
    /// Take the connection over, log the status, send this with a 60 s timeout, and close it.
    Refuse(&'static str, Box<Refusal>),
    /// Take the connection over and call `admit()`.
    Proceed(Box<Pending>),
}

/// A refusal decided before the takeover but logged after it (C takes the socket over first).
#[derive(Debug)]
pub struct Refusal {
    peer: Peer,
    msg: &'static str,
    reason: Reason,
    priority: Priority,
}

/// A request that passed the checks made on the web connection.
#[derive(Debug)]
pub struct Pending {
    request: StreamRequest,
    peer: Peer,
    /// When admission started (wall clock), for the disconnect record's `connected=`.
    accepted_s: i64,
}

/// A connection handed to a stream thread.
#[derive(Debug)]
pub struct Attached {
    host: Arc<Host>,
    hosts: Arc<Hosts>,
    slot: Arc<ReceiverSlot>,
    stream: Link<Conn>,
    thread: usize,
    parser: ingest::Config,
    peer: Peer,
    accepted_s: i64,
    /// For the text of socket errors: the receiver's TCP keepalive policy and `rpt->handshake_update_every`.
    keepalive: Keepalive,
    handshake_update_every: i64,
    /// `rpt->thread.keepalive_initialized`.
    keepalive_initialized: bool,
    /// `rpt->thread.wanted & ND_POLL_WRITE`: a POLLOUT asked for the buffer to be written, and no send has drained it
    /// since; the periodic check re-arms it while the buffer holds any. Only then do a read's end and the poller
    /// write.
    write_wanted: bool,
    /// The stream threads, for the backfilled charts' requests to come back to this one.
    pool: PoolHandle<StreamMsg>,
    /// The receiver waits for replication once attached (replication is enabled), else runs.
    replication_wait: bool,
    /// The senders' connector, whose environment a NODE_ID to the child reads.
    connector: Arc<Connector>,
}

impl Attached {
    /// `stream_receiver_remove()`'s release of the host: offline in pulse, the receiver slot freed with `reason` (the
    /// one the host's sender stops with) and the connection's parser dropped before another receiver can attach
    /// (`pluginsd_process_cleanup()` at the end of `rrdhost_clear_receiver()`: its THREAD CLEANUP record is the
    /// removal's), the parent label updated. The caller gives back the host's stream thread pin.
    fn leave_host(&self, reason: Reason, parser: Option<Parser>) {
        self.host
            .pulse_status(netdata_agent_rrd::pulse::host_status::RCV_OFFLINE);
        self.host.clear_receiver_then(&self.slot, reason.0, || drop(parser));
        self.hosts.update_is_parent_label();
    }
}

/// A connection on its stream thread.
pub(crate) struct Child {
    attached: Attached,
    /// The negotiated compression's decompressor; `None` for an uncompressed stream.
    decompressor: Option<Decompressor>,
    reader: LineReader,
    parser: Parser,
    /// The fields every record of this child carries, shared by every event.
    frame: Arc<[(netdata_agent_log::Field, netdata_agent_log::Value)]>,
    bytes_in: u64,
    /// `rpt->replication`: the request count last seen, when it last moved, and the progress time last checked.
    replication_requests: u64,
    replication_progress: Option<Instant>,
    replication_checked: Option<Instant>,
}

/// The receiving side of this agent.
pub struct Receivers {
    pub conf: Mutex<StreamConf>,
    pub hosts: Arc<Hosts>,
    pub defaults: Defaults,
    pool: PoolHandle<StreamMsg>,
    /// The hosts' stream threads (`stream_thread_globals.assign`).
    pins: Arc<Mutex<Pins>>,
    /// The senders' connector, which a proxied child's sender joins.
    connector: Arc<Connector>,
    /// `[web] accept a streaming request every` (seconds, 0 for no limit).
    streaming_rate_s: AtomicI64,
    /// The wall-clock second of the last accepted request under the rate limit (`last_stream_accepted_t`).
    last_accepted_s: Mutex<i64>,
}

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The log text of a parameter value: the key is masked (D34).
fn logged_value<'a>(name: &str, value: &'a str) -> &'a str {
    if name == "key" && !value.is_empty() {
        netdata_agent_log::REDACTED
    } else {
        value
    }
}

/// The descriptor a record names, -1 for none.
fn raw_fd(link: &Link<Conn>) -> std::os::fd::RawFd {
    link.socket().map_or(-1, std::os::fd::AsRawFd::as_raw_fd)
}

/// `nd_sock_send_timeout()`: wait up to `timeout` for the socket to take data, then one `send()` or
/// `netdata_ssl_write()` in the socket's current mode. `Err` carries the errno C's next record reports: ETIMEDOUT when
/// the socket never took data, a failed `poll()`'s or send's, 0 for a partial send. A refusal runs on the web
/// server's non-blocking socket, so dropping its link afterwards never waits on the peer's `close_notify`.
fn send_timeout(link: &mut Link<Conn>, bytes: &[u8], timeout: Duration) -> Result<(), i32> {
    let Some(conn) = link.socket() else {
        return Err(0);
    };
    writable_within(std::os::fd::AsFd::as_fd(conn), timeout)?;
    match link.write(bytes) {
        Ok(n) if n == bytes.len() => Ok(()),
        Ok(_) => Err(0),
        Err(e) => Err(netdata_agent_log::errno_of(&e)),
    }
}

/// `wait_on_socket_or_cancel_with_timeout()` for `POLLOUT` ([`netdata_agent_sys::wait_fd`], never cancelled: a web
/// worker): `Err` with C's errno on a timeout (ETIMEDOUT), a failed `poll()` (its errno) or an event other than writable
/// (0).
fn writable_within(fd: std::os::fd::BorrowedFd<'_>, timeout: Duration) -> Result<(), i32> {
    let waited = netdata_agent_sys::wait_fd(fd, timeout.as_millis() as i64, nix::poll::PollFlags::POLLOUT, &|| false);
    if waited.rc == 0 { Ok(()) } else { Err(waited.errno) }
}

impl Receivers {
    /// `pins` is the table the stream threads of `pool` share (see `StreamWorker::new()`); `connector` is the one
    /// localhost's sender uses.
    pub fn new(
        conf: StreamConf,
        hosts: Arc<Hosts>,
        pins: Arc<Mutex<Pins>>,
        defaults: Defaults,
        pool: PoolHandle<StreamMsg>,
        connector: Arc<Connector>,
    ) -> Self {
        *pins.lock().unwrap_or_else(PoisonError::into_inner) = Pins::new(pool.threads());
        Receivers {
            conf: Mutex::new(conf),
            hosts,
            defaults,
            pool,
            pins,
            connector,
            streaming_rate_s: AtomicI64::new(0),
            last_accepted_s: Mutex::new(0),
        }
    }

    /// Sets `[web] accept a streaming request every`, which is read after the receivers exist.
    pub fn set_streaming_rate(&self, seconds: i64) {
        self.streaming_rate_s.store(seconds, Ordering::Relaxed);
    }

    /// The `web_client_streaming_rate_t` step: at most one request per period is accepted.
    fn rate_limited(&self) -> Option<i64> {
        let rate = self.streaming_rate_s.load(Ordering::Relaxed);
        if rate <= 0 {
            return None;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let mut last = self
            .last_accepted_s
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *last == 0 {
            *last = now;
        }
        if now - *last < rate {
            return Some(rate - (now - *last));
        }
        *last = now;
        None
    }

    /// The part of `stream_receiver_accept_connection()` before the connection is taken over. Every rejection is
    /// logged as C's status pair, inside the web request's frame.
    pub fn pre_admit(
        &self,
        decoded: &[u8],
        user_agent: Option<&[u8]>,
        client_ip: &str,
        client_port: &str,
    ) -> PreAdmission {
        // stream_receiver_response_too_busy_now() once the exit started (!service_running(ABILITY_STREAMING_CONNECTIONS)),
        // before anything is read or logged
        if netdata_agent_sys::exit::initiated() {
            return PreAdmission::Reply(handshake::ERROR_BUSY_TRY_LATER, 503);
        }
        let accepted_s = now_s();
        let mut request = StreamRequest::parse(decoded, self.defaults.update_every, user_agent);
        for (hostname, name, value) in &request.unused {
            nd_log!(
                Source::Daemon,
                Priority::Notice,
                "STREAM RCV '{}' [from [{client_ip}]:{client_port}]: request has parameter '{name}' = '{}', which is not \
                 used.",
                hostname.as_deref().filter(|h| !h.is_empty()).unwrap_or("-"),
                logged_value(name, value)
            );
        }
        let validated = {
            let mut conf = self.conf.lock().unwrap_or_else(PoisonError::into_inner);
            handshake::validate(&mut request, &mut conf, client_ip)
        };
        let peer = Peer {
            ip: client_ip.to_string(),
            port: client_port.to_string(),
            hostname: request.hostname.clone(),
            key: request.key.clone(),
            machine_guid: request.machine_guid.clone(),
        };
        if let Err(denied) = validated {
            peer.status(denied.message(), Reason::PARENT_DENIED_ACCESS, Priority::Warning);
            return PreAdmission::Reply(handshake::ERROR_NOT_PERMITTED, 401);
        }
        let guid = request.machine_guid.clone().unwrap_or_default();
        if guid == self.hosts.localhost().machine_guid() {
            return PreAdmission::Refuse(
                handshake::ERROR_SAME_LOCALHOST,
                Box::new(Refusal {
                    peer,
                    msg: "rejecting streaming connection; machine UUID is my own",
                    reason: Reason::PARENT_IS_LOCALHOST,
                    priority: Priority::Debug,
                }),
            );
        }
        // a vnode a plugin of this agent collects; the attach checks again under the receiver lock (a claim may come
        // while this connection is being set up)
        if self.hosts.find_by_guid(&guid).is_some_and(|host| host.is_virtual()) {
            return PreAdmission::Refuse(
                handshake::ERROR_LOCAL_VNODE,
                Box::new(Refusal {
                    peer,
                    msg: "rejecting streaming connection; this is a locally collected vnode",
                    reason: Reason::PARENT_VNODE_IS_LOCAL,
                    priority: Priority::Debug,
                }),
            );
        }
        if let Some(wait_s) = self.rate_limited() {
            peer.status(
                &format!(
                    "rejecting streaming connection; rate limit, will accept new connection in {wait_s} secs"
                ),
                Reason::PARENT_BUSY_TRY_LATER,
                Priority::Notice,
            );
            return PreAdmission::Reply(handshake::ERROR_BUSY_TRY_LATER, 503);
        }
        let existing = self.hosts.find_by_guid(&guid);
        let mut age_s = 0;
        let mut stale = None;
        let mut working = false;
        if let Some(host) = &existing {
            if let Some(slot) = host.receiver() {
                let last = slot.last_traffic_ut.load(Ordering::Relaxed);
                age_s = if last == 0 {
                    0
                } else {
                    now_monotonic_usec().saturating_sub(last) / 1_000_000
                };
                if age_s < STALE_RECEIVER_S {
                    working = true;
                } else {
                    stale = Some(slot);
                }
            }
        }
        if let (Some(_), Some(host)) = (&stale, &existing) {
            if host.hostname() != request.hostname.as_deref().unwrap_or_default() {
                peer.status(
                    "rejecting streaming connection; machine GUID is connected with a different hostname",
                    Reason::PARENT_DENIED_ACCESS,
                    Priority::Warning,
                );
                return PreAdmission::Reply(handshake::ERROR_NOT_PERMITTED, 401);
            }
        }
        if let (Some(slot), Some(host)) = (&stale, &existing) {
            if host.stop_receiver_and_wait(slot) {
                stale = None;
                nd_log!(
                    Source::Daemon,
                    Priority::Notice,
                    "STREAM RCV '{}' [from [{client_ip}]:{client_port}]: stopped previous stale receiver to accept this \
                     one.",
                    peer.hostname.as_deref().unwrap_or("")
                );
            }
        }
        if working || stale.is_some() {
            peer.status(
                &format!(
                    "rejecting streaming connection; multiple connections for the same host, old connection was last \
                     used {age_s} secs ago{}",
                    if stale.is_some() {
                        " (signaled old receiver to stop)"
                    } else {
                        " (new connection not accepted)"
                    }
                ),
                Reason::PARENT_NODE_ALREADY_CONNECTED,
                Priority::Warning,
            );
            return PreAdmission::Reply(handshake::ERROR_ALREADY_STREAMING, 409);
        }
        PreAdmission::Proceed(Box::new(Pending {
            request,
            peer,
            accepted_s,
        }))
    }

    /// `PreAdmission::Refuse`: the connection has been taken over; the status is logged, then the reply sent.
    pub fn refuse(&self, mut link: Link<Conn>, message: &str, refusal: &Refusal) {
        let peer = &refusal.peer;
        peer.status(refusal.msg, refusal.reason, refusal.priority);
        if let Err(errno) = send_timeout(&mut link, message.as_bytes(), Duration::from_secs(60)) {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                errno = errno;
                "STREAM RCV '{}' [from [{}]:{}]: failed to reply.",
                peer.hostname.as_deref().unwrap_or(""),
                peer.ip,
                peer.port
            );
        }
    }

    /// The rest of `stream_receiver_accept_connection()`: the receiver configuration, the host, the prompt, and the
    /// handover to a stream thread. False when the connection was refused or failed here, and closed (C's
    /// `stream_receiver_free()` in the web thread).
    pub fn admit(&self, pending: Pending, mut link: Link<Conn>) -> bool {
        let Pending {
            request,
            peer,
            accepted_s,
        } = pending;
        let key = request.key.clone().unwrap_or_default();
        let guid = request.machine_guid.clone().unwrap_or_default();
        // `stream_receive.replication.enabled` ([db] enable replication) decides the state a receiver starts in
        let (config, replication_wait) = {
            let mut conf = self.conf.lock().unwrap_or_else(PoisonError::into_inner);
            let defaults = ReceiverDefaults {
                db_mode: self.defaults.db_mode.clone(),
                history: self.defaults.history,
                health_enabled: self.defaults.health_enabled,
                update_every: i64::from(request.update_every),
            };
            (
                conf.receiver_config(&key, &guid, &defaults),
                conf.receive.enabled,
            )
        };
        let dbengine = self.hosts.storage().dbengine().is_some();
        let (mode, fallback) = receiver_mode(&config.db_mode, &self.defaults.db_mode, dbengine);
        if fallback {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM RCV '{}' [from [{}]:{}]: dbengine is not enabled, falling back to default.",
                peer.hostname.as_deref().unwrap_or(""),
                peer.ip,
                peer.port
            );
        }
        let update_every = if config.update_every <= 0 {
            1
        } else {
            config.update_every as i32
        };
        let health_enabled = host_health_enabled(config.health_enabled, mode);
        let text =
            |v: &Option<String>, default: &str| v.clone().unwrap_or_else(|| default.to_string());
        // set_host_properties() and rrdhost_init_timezone(): an empty value is a missing one
        let non_empty = |v: &Option<String>, default: &str| {
            v.as_deref()
                .filter(|v| !v.is_empty())
                .unwrap_or(default)
                .to_string()
        };
        let mut wanted = HostInfo {
            hostname: text(&request.hostname, ""),
            registry_hostname: text(&request.registry_hostname, ""),
            os: text(&request.os, "unknown"),
            timezone: non_empty(&request.timezone, "unknown"),
            abbrev_timezone: non_empty(&request.abbrev_timezone, "UTC"),
            utc_offset: request.utc_offset,
            program_name: non_empty(&request.program_name, "unknown"),
            program_version: non_empty(&request.program_version, "unknown"),
            update_every,
            db_mode: mode,
            history_entries: align_entries_to_pagesize(
                mode,
                config.history,
                self.defaults.page_size,
            ),
            health_enabled,
            system_info: request.system_info.clone(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: StreamSend::new(
                config.send_enabled,
                &config.send_parents,
                &config.send_api_key,
                &config.send_charts_matching,
            ),
            cache_dir: None,
        };
        wanted.set_replication(
            config.replication.enabled,
            config.replication.period,
            config.replication.step,
        );
        let host = self.hosts.find_or_create(
            &guid,
            mode,
            || wanted.clone(),
            |host| {
                host.update(
                    &wanted,
                    config.update_every,
                    config.history,
                    config.replication.enabled,
                    config.replication.period,
                    config.replication.step,
                )
            },
        );
        // rrdhost_find_or_create() returned NULL: an index collision or a host being stored (D126.7, D95.7)
        let Ok(host) = host else {
            peer.status(
                "rejecting streaming connection; host creation is busy, retry later",
                Reason::PARENT_BUSY_TRY_LATER,
                Priority::Notice,
            );
            let _ = send_timeout(
                &mut link,
                handshake::ERROR_BUSY_TRY_LATER.as_bytes(),
                Duration::from_secs(5),
            );
            return false;
        };
        if host.is_pending_context_load() {
            peer.status(
                "rejecting streaming connection; host is initializing, retry later",
                Reason::PARENT_IS_INITIALIZING,
                Priority::Notice,
            );
            let _ = send_timeout(
                &mut link,
                handshake::ERROR_INITIALIZATION.as_bytes(),
                Duration::from_secs(5),
            );
            return false;
        }
        // stream_sender_structures_init() of a created or revived host whose proxy settings stream it (D117.1)
        if host.upstream().is_none() {
            Sender::attach(&host, &self.connector);
        }
        let capabilities = caps::select_compression(
            request.capabilities,
            config.compression_enabled,
            &config.compression_priorities,
            caps::COMPRESSIONS_AVAILABLE,
        );
        // stream_receiver_signal_to_stop_and_wait()'s shutdown() of the socket, from another thread
        let shutdown_handle = link.socket().and_then(|c| socket2::SockRef::from(c).try_clone().ok());
        let slot = Arc::new(ReceiverSlot::new(
            now_monotonic_usec(),
            (peer.ip.clone(), peer.port.clone()),
            ReceiverLink {
                hops: request.hops,
                connected_since_s: accepted_s,
                capabilities,
            },
            Box::new(move || {
                if let Some(s) = &shutdown_handle {
                    let _ = s.shutdown(Shutdown::Both);
                }
            }),
        ));
        match host.set_receiver(Arc::clone(&slot)) {
            // rrdhost_set_receiver()'s stream_parents_host_reset(), outside the receiver lock here (D118.3)
            Attach::Attached => {
                if let Some(up) = host.upstream() {
                    up.parents_reset(Reason::SP_PREPARING.0);
                }
            }
            Attach::AlreadyServed => {
                peer.status(
                    "rejecting streaming connection; host is already served by another receiver",
                    Reason::PARENT_NODE_ALREADY_CONNECTED,
                    Priority::Info,
                );
                let _ = send_timeout(
                    &mut link,
                    handshake::ERROR_ALREADY_STREAMING.as_bytes(),
                    Duration::from_secs(5),
                );
                return false;
            }
            // the same rejection the accept's gate sends, so the child backs off the same way
            Attach::VnodeIsLocal => {
                peer.status(
                    "rejecting streaming connection; this host was claimed as a locally collected vnode",
                    Reason::PARENT_VNODE_IS_LOCAL,
                    Priority::Warning,
                );
                let _ = send_timeout(&mut link, handshake::ERROR_LOCAL_VNODE.as_bytes(), Duration::from_secs(5));
                return false;
            }
            Attach::CleanupBusy => {
                peer.status(
                    "rejecting streaming connection; internal cleanup is in progress for this node, please retry \
                     shortly",
                    Reason::PARENT_BUSY_TRY_LATER,
                    Priority::Info,
                );
                let _ = send_timeout(
                    &mut link,
                    handshake::ERROR_BUSY_TRY_LATER.as_bytes(),
                    Duration::from_secs(5),
                );
                return false;
            }
        }
        // rrdhost_set_receiver(): a child that was just connected gets its health postponed
        if config.health_enabled != 0 && config.health_delay > 0 {
            host.set_health_delay_up_to(now_s().saturating_add(config.health_delay));
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "STREAM RCV '{}' [from [{}]:{}]: Postponing health checks for {} seconds, because it was just \
                 connected.",
                host.hostname(),
                peer.ip,
                peer.port,
                config.health_delay
            );
        }
        let prompt = caps::prompt(capabilities);
        // the negotiated capabilities are logged while the prompt is built, before the socket is set up
        peer.established(&host.hostname(), capabilities);
        let mut keepalive_initialized = false;
        // web server sockets are non-blocking: C sends the prompt in blocking mode
        if let Some(conn) = link.socket() {
            let socket = socket2::SockRef::from(conn);
            let fd = std::os::fd::AsRawFd::as_raw_fd(conn);
            if let Err(e) = socket.set_nonblocking(false) {
                nd_log!(Source::Daemon, Priority::Err, errno = netdata_agent_log::errno_of(&e);
                    "STREAM RCV '{}' [from [{}]:{}]: cannot remove the non-blocking flag from socket {fd}",
                    host.hostname(), peer.ip, peer.port);
            }
            if let Err(e) = socket.set_read_timeout(Some(Duration::from_secs(600))) {
                nd_log!(Source::Daemon, Priority::Err, errno = netdata_agent_log::errno_of(&e);
                    "STREAM RCV '{}' [from [{}]:{}]: cannot set timeout for socket {fd}",
                    host.hostname(), peer.ip, peer.port);
            }
            reconcile_keepalive(
                std::os::fd::AsFd::as_fd(conn),
                &host,
                &peer,
                &config.keepalive,
                i64::from(request.update_every),
                &mut keepalive_initialized,
            );
        }
        if let Err(errno) = send_timeout(&mut link, prompt.as_bytes(), Duration::from_secs(60)) {
            peer.status_errno(
                "cannot reply back, dropping connection",
                Reason::CONNECT_SEND_TIMEOUT,
                Priority::Err,
                errno,
            );
            host.clear_receiver(&slot, Reason::DISCONNECT_SOCKET_WRITE_FAILED.0);
            return false;
        }
        // svc_rrdhost_obsolete_all_charts(): the charts the child does not define again stay obsolete
        host.obsolete_all_charts();
        peer.status(&connected_msg(&host), Reason::NEVER, Priority::Info);
        self.hosts.update_is_parent_label();
        // let it reconnect to parents asap
        if let Some(up) = host.upstream() {
            up.parents_reset(Reason::SP_PREPARING.0);
        }
        let nonblocking = link.socket().map(|c| socket2::SockRef::from(c).set_nonblocking(true));
        if !matches!(nonblocking, Some(Ok(()))) {
            host.clear_receiver(&slot, Reason::DISCONNECT_SOCKET_WRITE_FAILED.0);
            self.hosts.update_is_parent_label();
            return false;
        }
        // stream_receiver_add_to_queue(): the host's stream thread, which its sender shares
        let thread = self.pins.lock().unwrap_or_else(PoisonError::into_inner).queue(host.machine_guid());
        let attached = Attached {
            host,
            hosts: Arc::clone(&self.hosts),
            slot,
            stream: link,
            thread,
            parser: ingest::Config {
                capabilities,
                update_every: self.defaults.update_every,
                page_size: self.defaults.page_size,
                now: collection::now_realtime_timeval,
                gap_when_lost_iterations_above: self.defaults.gap_when_lost_iterations_above,
            },
            peer,
            accepted_s,
            keepalive: config.keepalive,
            handshake_update_every: i64::from(request.update_every),
            keepalive_initialized,
            write_wanted: false,
            pool: self.pool.clone(),
            replication_wait,
            connector: Arc::clone(&self.connector),
        };
        // stream_receiver_add_to_queue(); the host waits for its stream thread (set before the thread can attach it)
        attached
            .host
            .pulse_status(netdata_agent_rrd::pulse::host_status::RCV_WAITING);
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "STREAM RCV[{thread}] '{}': moving host to receiver queue...",
            attached.host.hostname()
        );
        if let Err(StreamMsg::Attach(attached)) = self
            .pool
            .send(thread, StreamMsg::Attach(Box::new(attached)))
        {
            self.pins.lock().unwrap_or_else(PoisonError::into_inner).remove(attached.host.machine_guid());
            // the stream threads ended: the exit started
            attached.leave_host(Reason::DISCONNECT_SHUTDOWN, None);
            return false;
        }
        true
    }
}

/// `stream_receiver_did_replication_progress()` and `stream_sender_did_replication_progress()`: the replication
/// commands counted moved, none yet, some work still waits, or less than ten minutes since they last moved; `seen`
/// is the count last seen and when it moved.
pub(crate) fn replication_progressed(
    seen: (&mut u64, &mut Option<Instant>),
    commands: u64,
    waiting: bool,
    now: Instant,
) -> bool {
    let (last_commands, progress) = seen;
    if *last_commands != commands {
        *last_commands = commands;
        *progress = Some(now);
        return true;
    }
    if commands == 0 || waiting {
        return true;
    }
    match *progress {
        None => {
            *progress = Some(now);
            true
        }
        Some(last) => now.saturating_duration_since(last) < REPLICATION_STALL,
    }
}

/// `stream_receiver_connected_msg()`: how old the host's last sample is.
fn connected_msg(host: &Host) -> String {
    connected_msg_at(host.contexts().retention().1, now_s())
}

/// The connected record for a last sample at `last`, one in the future read as now.
fn connected_msg_at(last: i64, now: i64) -> String {
    let last = last.min(now);
    if last == 0 {
        "connected and ready to receive data, new node".to_string()
    } else if last == now {
        "connected and ready to receive data, last sample in the db just now".to_string()
    } else {
        let ago = netdata_agent_text::duration::duration_to_string(now - last, "s", true)
            .unwrap_or_default();
        format!("connected and ready to receive data, last sample in the db {ago} ago")
    }
}

impl StreamWorker {
    /// `stream_receiver_move_to_running_unsafe()`: the connection joins this thread, its parser answering
    /// backfilled charts through this thread's messages.
    pub(crate) fn attach(&mut self, cx: &mut Context<'_>, mut attached: Attached) {
        let index = self
            .children
            .iter()
            .position(Option::is_none)
            .unwrap_or_else(|| {
                self.children.push(None);
                self.children.len() - 1
            });
        if cx
            .registry()
            .register(
                &mut attached.stream,
                Token(index),
                Interest::READABLE | Interest::WRITABLE,
            )
            .is_err()
        {
            self.pins.lock().unwrap_or_else(PoisonError::into_inner).remove(attached.host.machine_guid());
            attached.leave_host(Reason::DISCONNECT_SOCKET_ERROR, None);
            return;
        }
        let mut parser = Parser::new(
            Arc::clone(&attached.host),
            Arc::clone(attached.hosts.localhost()),
            attached.parser,
            Arc::new(ChildWire { slot: Arc::downgrade(&attached.slot) }),
        );
        // a backfilled chart's request comes back to this thread for this connection
        let (pool, thread, receiver) = (
            attached.pool.clone(),
            attached.thread,
            Arc::downgrade(&attached.slot),
        );
        parser.set_replay_sink(Arc::new(move |request| {
            // an ended thread is not started again for it
            pool.send_if_running(thread, StreamMsg::Replay(receiver.clone(), request))
                .is_ok()
        }));
        let decompressor = Decompressor::for_capabilities(attached.parser.capabilities);
        let frame = attached.peer.child_frame(attached.parser.capabilities, attached.stream.is_tls());
        {
            // stream_receiver_move_to_running_unsafe()
            let _frame = netdata_agent_log::push(vec![
                (
                    netdata_agent_log::Field::NidlNode,
                    netdata_agent_log::Value::Str(attached.host.hostname()),
                ),
                (
                    netdata_agent_log::Field::MessageId,
                    netdata_agent_log::Value::Uuid(netdata_agent_log::msgid::STREAMING_FROM_CHILD),
                ),
            ]);
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "STREAM RCV[{}] '{}' [from [{}]:{}]: moving host from receiver queue to receiver running...",
                attached.thread,
                attached.peer.hostname_or_dash(),
                attached.peer.ip,
                attached.peer.port
            );
        }
        let a = &mut attached;
        // the no-traffic timeout counts from the admission, not from the accept the waiting list held back
        a.slot
            .last_traffic_ut
            .store(now_monotonic_usec(), std::sync::atomic::Ordering::Relaxed);
        // sock_enlarge_rcv_buf() and sock_enlarge_snd_buf() at the move to running (the receive buffer came from the
        // listener already, D126.3)
        crate::sock::enlarge_buffers(&socket2::SockRef::from(
            a.stream.socket().expect("a taken-over link has its socket"),
        ));
        reconcile_keepalive(
            std::os::fd::AsFd::as_fd(a.stream.socket().expect("a taken-over link has its socket")),
            &a.host,
            &a.peer,
            &a.keepalive,
            a.handshake_update_every,
            &mut a.keepalive_initialized,
        );
        self.children[index] = Some(Child {
            attached,
            decompressor,
            reader: LineReader::default(),
            parser,
            frame,
            bytes_in: 0,
            replication_requests: 0,
            replication_progress: None,
            replication_checked: None,
        });
        // the parser exists: the host's retention changes now owe the child a stream path
        // (stream_path_send_to_child() finds no parser before)
        if let Some(child) = &self.children[index] {
            child
                .attached
                .host
                .contexts()
                .record_first_time_changes(Taker::Receiver, true);
            use netdata_agent_rrd::pulse::host_status::{RCV_REPLICATION_WAIT, RCV_RUNNING};
            child
                .attached
                .host
                .pulse_status(if child.attached.replication_wait {
                    RCV_REPLICATION_WAIT
                } else {
                    RCV_RUNNING
                });
        }
        // what the move to running sends, under the child's frame
        let frame = self.children[index].as_ref().map(|c| Arc::clone(&c.frame));
        let _frame = frame.as_ref().map(records::child_event);
        // the end of the move to running: the child's buffer exists from here (before it a send is 0 and nothing is
        // queued, D119.2), then the host's node id goes down (D106.9)
        if let Some(child) = &self.children[index] {
            let a = &child.attached;
            let (pool, thread, slot) = (a.pool.clone(), a.thread, Arc::downgrade(&a.slot));
            let (hostname, (ip, port)) = (a.host.hostname(), a.slot.remote.clone());
            // the opcodes posted and not yet taken: one message carries them all (C's message slot)
            let ops = Arc::new(AtomicU32::new(0));
            a.slot.open_buffer(Box::new(move |op| {
                // on its own thread a POLLOUT is C's inline one: the child's own parser writes it after the line that
                // added it, anything else at the step's end (the socket is the thread's, not this caller's); either
                // write only logs a failure, as C's (stream-thread.c:111-117), so no caller's parser is dropped
                if op == receiver_op::POLLOUT && crate::thread::current() == Some(thread) {
                    return crate::thread::child_pollout_inline(slot.clone());
                }
                if ops.fetch_or(op, Ordering::AcqRel) != 0 {
                    return;
                }
                if pool.send_if_running(thread, StreamMsg::ChildOps(slot.clone(), Arc::clone(&ops))).is_err() {
                    ops.store(0, Ordering::Release);
                    // the thread ended (the exit started): C's stream_thread_by_slot_id() finds no thread
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "STREAM RCV '{hostname}' [from [{ip}]:{port}]: the opcode ({op}) message cannot be verified. \
                         Ignoring it."
                    );
                }
            }));
            crate::sender::send_node_and_claim_id_to_child(&a.host, a.connector.env());
        }
        // the node id's inline POLLOUT, written at once as C's (and nothing to write, nor wanted, without it)
        let pollout = self.children[index].as_ref().is_some_and(|c| crate::thread::take_inline_child(&c.attached.slot));
        if pollout {
            self.send_data(cx, index, Writer::Opcode);
        }
        // C only adds the socket to the poll: the first read is the next turn's (D126.6)
        cx.report_again(Token(index));
    }

    /// A child whose last read returned data, served again in a later turn (D126.6): its next read, as a readable
    /// event's.
    pub(crate) fn child_again(&mut self, cx: &mut Context<'_>, index: usize) {
        let Some(child) = self.children.get(index).and_then(Option::as_ref) else {
            return;
        };
        let _frame = records::child_event(&child.frame);
        if child.attached.slot.stop_requested.load(Ordering::Acquire) {
            return self.disconnect(cx, index, Reason::DISCONNECT_SIGNALED_TO_STOP);
        }
        self.receive(cx, index);
    }

    /// `stream_thread_handle_op()` for a receiver: POLLOUT writes what is owed to the child of this connection, then
    /// BUFFER_OVERFLOW restarts it (`stream_receiver_handle_op()`). A connection no longer here gets C's "ignored"
    /// record when the opcodes were `posted` through the thread's queue; an inline POLLOUT runs in C while the
    /// receiver is in its thread, so it cannot miss.
    pub(crate) fn child_ops(&mut self, cx: &mut Context<'_>, slot: &Weak<ReceiverSlot>, ops: u32, posted: bool) {
        let index = slot.upgrade().and_then(|slot| {
            self.children.iter().position(|c| c.as_ref().is_some_and(|c| Arc::ptr_eq(&c.attached.slot, &slot)))
        });
        let Some(index) = index else {
            if posted {
                crate::sender::dispatch::opcode_ignored(cx.index(), ops);
            }
            return;
        };
        let frame = self.children[index].as_ref().map(|c| Arc::clone(&c.frame));
        let _frame = frame.as_ref().map(records::child_event);
        // a failed write leaves the rest to the next event (C's handler returns: the receiver is "removed")
        if ops & receiver_op::POLLOUT != 0 && !self.send_data(cx, index, Writer::Opcode) {
            return;
        }
        let ops = ops & !receiver_op::POLLOUT;
        if ops & receiver_op::BUFFER_OVERFLOW != 0 {
            let Some(child) = self.children[index].as_ref() else {
                return;
            };
            let stats = child.attached.slot.buffer().as_ref().map(|b| *b.stats()).unwrap_or_default();
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "{}send buffer is full (buffer size {}, max {}, used {}, available {}). Restarting connection.",
                Self::prefix(child),
                stats.bytes_size,
                stats.bytes_max_size,
                stats.bytes_outstanding,
                stats.bytes_available
            );
            return self.disconnect(cx, index, Reason::DISCONNECT_BUFFER_OVERFLOW);
        }
        if ops != 0 {
            nd_log!(Source::Daemon, Priority::Err, "STREAM RCV[{}]: invalid msg id {ops}", cx.index());
        }
    }

    /// A backfilled chart's replication request, for the connection it came from: sent and flushed under the
    /// child's frame; dropped when the connection is gone (C's bytes go with the buffer they were added to).
    pub(crate) fn replay(
        &mut self,
        receiver: &Weak<ReceiverSlot>,
        request: &ingest::ReplayRequest,
    ) {
        let Some(index) = self.children.iter().position(|c| {
            c.as_ref()
                .is_some_and(|c| std::ptr::eq(Arc::as_ptr(&c.attached.slot), receiver.as_ptr()))
        }) else {
            return;
        };
        let Some(child) = self.children[index].as_mut() else {
            return;
        };
        let frame = Arc::clone(&child.frame);
        let _frame = records::child_event(&frame);
        // its line wakes this thread inline, so the message's drain writes it; one the buffer refused is logged there,
        // and the overflow opcode restarts the connection
        child.parser.replay_backfilled(request);
    }

    /// `stream_receive_process_poll_events()`: under the child's frame, the stop flag, then socket errors (or a
    /// hangup with nothing left to read), then sending, then receiving.
    pub(crate) fn child_event(&mut self, cx: &mut Context<'_>, index: usize, event: &Event) {
        let Some(child) = self.children.get(index).and_then(Option::as_ref) else {
            return;
        };
        let _frame = records::child_event(&child.frame);
        // the shutdown that woke the socket is not a remote close
        if child.attached.slot.stop_requested.load(Ordering::Acquire) {
            return self.disconnect(cx, index, Reason::DISCONNECT_SIGNALED_TO_STOP);
        }
        let hangup = event.is_read_closed();
        if event.is_error() || (hangup && !event.is_readable()) {
            let reason = if hangup {
                Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE
            } else {
                Reason::DISCONNECT_SOCKET_ERROR
            };
            if event.is_error() {
                Self::log_poll_error(child, reason);
            } else {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "{}{} - closing connection",
                    Self::prefix(child),
                    reason.text()
                );
            }
            return self.disconnect(cx, index, reason);
        }
        if event.is_writable() && !self.send_data(cx, index, Writer::Poll) {
            return;
        }
        if event.is_readable() || hangup {
            self.receive(cx, index);
        }
    }

    /// The 100 ms tick's receiver part: stop requests, then the retention changes the RRDCONTEXT thread recorded.
    pub(crate) fn tick_children(&mut self, cx: &mut Context<'_>) {
        for index in 0..self.children.len() {
            let Some(child) = self.children[index].as_mut() else {
                continue;
            };
            if child.attached.slot.stop_requested.load(Ordering::Acquire) {
                let frame = Arc::clone(&child.frame);
                let _frame = records::child_event(&frame);
                self.disconnect(cx, index, Reason::DISCONNECT_SIGNALED_TO_STOP);
                continue;
            }
            // stream_path_retention_updated() from the RRDCONTEXT thread: its messages go out on this tick (D46
            // point 4), each with the retention start of its change
            let changes = child.attached.host.contexts().take_first_time_changes(Taker::Receiver);
            // (their lines wake this thread inline: the step's drain writes them)
            for first_time_s in changes {
                child.parser.retention_updated(first_time_s);
            }
        }
    }

    /// The thread's exit: every child disconnected.
    pub(crate) fn stop_children(&mut self, cx: &mut Context<'_>) {
        for index in 0..self.children.len() {
            self.disconnect(cx, index, Reason::DISCONNECT_SHUTDOWN);
        }
    }

    /// `stream_receiver_remove_internal()`: the disconnect record, then the host lets go of the receiver. The
    /// parser's fields are the caller's: C has them only while reading (`stream_receiver_receive_data()`).
    fn disconnect(&mut self, cx: &mut Context<'_>, index: usize, reason: Reason) {
        let Some(child) = self.children[index].take() else {
            return;
        };
        let Child { mut attached, parser, frame, bytes_in, .. } = child;
        {
            let attached = &mut attached;
            let _ = cx.registry().deregister(&mut attached.stream);
            let bytes_out = attached.slot.buffer().as_ref().map_or(0, |b| b.stats().bytes_sent);
            let counters = Counters {
                thread: attached.thread,
                msgs: parser.data_collections_count,
                bytes_in,
                bytes_out: bytes_out as u64,
                connected_s: (now_s() - attached.accepted_s).max(0),
                // C's idle time since the last read or write, 0 before any
                idle_s: match attached.slot.last_traffic_ut.load(Ordering::Relaxed) {
                    0 => 0,
                    last => (now_monotonic_usec().saturating_sub(last) / 1_000_000) as i64,
                },
                replication_percent: attached.host.replication_percent(),
            };
            let labels = attached.host.labels();
            let iface = labels
                .get(b"_net_default_iface")
                .map(|v| String::from_utf8_lossy(v).into_owned());
            let _removal = records::removal(&frame, &attached.host.hostname());
            records::disconnected(&attached.peer, iface.as_deref(), reason, &counters);
            // stream_thread_node_removed() first: a child that reconnects while its host is detached goes to the
            // least loaded thread (R55 M6)
            self.pins.lock().unwrap_or_else(PoisonError::into_inner).remove(attached.host.machine_guid());
            attached.leave_host(reason, Some(parser));
        }
        // stream_receiver_free(): the buffer goes (a late send gets 0), the socket closes after the records, and a TLS
        // close leaves what its shutdown set
        attached.slot.close_buffer();
        let tls = attached.stream.is_tls();
        drop(attached);
        self.exit_errno = if tls { nix::errno::Errno::last_raw() } else { 0 };
    }

    /// `STREAM RCV[n] '<host>' [from [<ip>]:<port>]: ` of the stream thread's records.
    /// `stream_receiver_check_all_nodes_from_poll()`: a probe finds a connection the child closed or that failed,
    /// and a child silent for longer than its timeout, while none of its charts replicates, is disconnected.
    pub(crate) fn check_all(&mut self, cx: &mut Context<'_>, now_ut: u64) {
        for index in 0..self.children.len() {
            let Some(child) = self.children[index].as_mut() else {
                continue;
            };
            let frame = Arc::clone(&child.frame);
            // nd_sock_peek_nowait(): recv(MSG_PEEK), or netdata_ssl_peek()
            let peeked = match &mut child.attached.stream {
                Link::Plain(conn) => {
                    socket2::SockRef::from(&*conn).peek(&mut [std::mem::MaybeUninit::uninit(); 1])
                }
                Link::Tls(t) => t.peek(&mut [0u8; 1]),
                Link::Gone => Ok(0),
            };
            let a = &child.attached;
            let at = format!(
                "STREAM RCV[{}] '{}' [from {}]: ",
                a.thread,
                a.host.hostname(),
                a.peer.ip
            );
            let reset =
                |e: &std::io::Error| e.raw_os_error() == Some(nix::errno::Errno::ECONNRESET as i32);
            match peeked {
                Ok(0) => {
                    let _frame = records::child_event(&frame);
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "{at}socket closed by remote - closing connection"
                    );
                    self.disconnect(cx, index, Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE);
                    continue;
                }
                Err(e) if reset(&e) => {
                    let _frame = records::child_event(&frame);
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "{at}socket closed by remote - closing connection"
                    );
                    self.disconnect(cx, index, Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE);
                    continue;
                }
                Err(e)
                    if !matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    let _frame = records::child_event(&frame);
                    let text = netdata_agent_log::strerror(netdata_agent_log::errno_of(&e));
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "{at}socket error detected: {text} - closing connection"
                    );
                    self.disconnect(cx, index, Reason::DISCONNECT_SOCKET_ERROR);
                    continue;
                }
                _ => {}
            }
            let timeout_s = IDLE_TIMEOUT_MIN_S
                .max(receiver_update_every(&a.host, a.handshake_update_every) * 2);
            let idle = Duration::from_micros(now_ut.saturating_sub(a.slot.last_traffic_ut.load(Ordering::Relaxed)));
            if idle > Duration::from_secs(timeout_s) && a.host.replicating_charts() == 0 {
                let _frame = records::child_event(&frame);
                let idle_us = i64::try_from(idle.as_micros()).unwrap_or(i64::MAX);
                let duration = duration_to_string(idle_us, "us", true).unwrap_or_default();
                // the buffer's: the first contiguous chunk pending, the fill of the grown maximum
                let stats = a.slot.buffer().as_ref().map(|b| *b.stats()).unwrap_or_default();
                let pending = if stats.bytes_outstanding == 0 {
                    "0".to_string()
                } else {
                    size_to_string(stats.bytes_outstanding as u64, "B", false).unwrap_or_default()
                };
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "{at}there was not traffic for {timeout_s} seconds - closing connection - we have sent {} bytes in \
                     {} operations, it is idle for {duration}, and we have {pending} pending to send (buffer is used \
                     {:.2}%).",
                    stats.bytes_sent,
                    stats.sends,
                    stats.buffer_ratio
                );
                self.disconnect(cx, index, Reason::DISCONNECT_TIMEOUT);
                continue;
            }
            // output re-armed while the buffer holds any, off when it holds none (SR:1224): the poller then writes,
            // with a failure removing the child
            let owed = a.slot.buffer().as_ref().is_some_and(|b| b.stats().bytes_outstanding != 0);
            if let Some(child) = self.children[index].as_mut() {
                child.attached.write_wanted = owed;
            }
            if owed {
                let _frame = records::child_event(&frame);
                self.send_data(cx, index, Writer::Poll);
            }
        }
    }

    /// `stream_receiver_did_replication_progress()` for a child.
    fn replication_progressed(child: &mut Child, now: Instant) -> bool {
        let host = &child.attached.host;
        replication_progressed(
            (
                &mut child.replication_requests,
                &mut child.replication_progress,
            ),
            u64::from(host.replication_requests()) + u64::from(host.replication_replies()),
            host.backfill_pending() != 0,
            now,
        )
    }

    /// `stream_receiver_replication_check_from_poll()`: a child whose replication made no progress for ten minutes
    /// while some of its charts never finished is disconnected, after its unfinished charts are listed.
    pub(crate) fn check_replication(&mut self, cx: &mut Context<'_>, now: Instant) {
        for index in 0..self.children.len() {
            let Some(child) = self.children[index].as_mut() else {
                continue;
            };
            if Self::replication_progressed(child, now) {
                child.replication_checked = None;
                continue;
            }
            if child.replication_checked == child.replication_progress {
                continue;
            }
            let a = &child.attached;
            let at = format!(
                "STREAM RCV[{}] '{}' [from {}]: ",
                a.thread,
                a.host.hostname(),
                a.peer.ip
            );
            let (mut stalled, mut finished) = (0usize, 0usize);
            for chart in a.host.charts().all() {
                let f = chart.flags();
                if f & flags::OBSOLETE != 0 {
                    continue;
                }
                if f & flags::RECEIVER_REPLICATION_FINISHED != 0 {
                    finished += 1;
                    continue;
                }
                let state = if f & flags::RECEIVER_REPLICATION_IN_PROGRESS != 0 {
                    "has not finished"
                } else {
                    "has not started"
                };
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "{at}REPLICATION EXCEPTIONS: instance '{}' {state} replication yet.",
                    chart.id()
                );
                stalled += 1;
            }
            if stalled > 0 && !Self::replication_progressed(child, now) {
                let host = &child.attached.host;
                let (requested, replies) = (host.replication_requests(), host.replication_replies());
                nd_log!(
                    Source::Daemon,
                    Priority::Warning,
                    "{at}REPLICATION EXCEPTIONS SUMMARY: node has {stalled} stalled replication requests ({finished} \
                     finished). We have requested {requested} and got replies for {replies} replication commands. \
                     Disconnecting node to restore streaming."
                );
                self.disconnect(cx, index, Reason::DISCONNECT_REPLICATION_STALLED);
                continue;
            }
            child.replication_checked = child.replication_progress;
        }
    }

    fn prefix(child: &Child) -> String {
        prefix(&child.attached)
    }

    /// `stream_receiver_send_data()`: what is owed to the child written from its buffer (`write_buffer()`), the
    /// failure handled as `writer` asks. False when the connection failed.
    pub(crate) fn send_data(&mut self, cx: &mut Context<'_>, index: usize, writer: Writer) -> bool {
        let Some(child) = self.children[index].as_mut() else {
            return false;
        };
        if writer == Writer::Opcode {
            child.attached.write_wanted = true;
        } else if !child.attached.write_wanted {
            return true;
        }
        let parser = (writer == Writer::Read).then_some((&child.parser, None));
        let Err(reason) = write_buffer(&mut child.attached, parser) else {
            return true;
        };
        if writer == Writer::Poll {
            self.disconnect(cx, index, reason);
        }
        false
    }

    /// `stream_receiver_log_poll_error()`: the socket's pending error and the keepalive policy.
    fn log_poll_error(child: &Child, reason: Reason) {
        let k = &child.attached.keepalive;
        let keepalive = if !k.enabled {
            "disabled".to_string()
        } else {
            let idle_s = if k.automatic {
                automatic_keepalive_idle(receiver_update_every(
                    &child.attached.host,
                    child.attached.handshake_update_every,
                ))
            } else {
                k.idle_s
            };
            format!(
                "enabled policy={} idle={idle_s}s interval={KEEPALIVE_PROBE_INTERVAL_S}s probes={KEEPALIVE_PROBES}",
                if k.automatic {
                    "automatic"
                } else {
                    "configured"
                }
            )
        };
        let prefix = Self::prefix(child);
        match child.attached.stream.socket().map_or(Ok(None), Conn::take_error) {
            Err(err) => {
                let errno = netdata_agent_log::errno_of(&err);
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    errno = errno;
                    "{prefix}{} - closing connection; SO_ERROR is unavailable: {} (errno={errno}); TCP keepalive: {keepalive}",
                    reason.text(),
                    netdata_agent_log::strerror(errno)
                );
            }
            Ok(Some(err)) => {
                let errno = netdata_agent_log::errno_of(&err);
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    errno = errno;
                    "{prefix}{} - closing connection; SO_ERROR={errno} ({}); TCP keepalive: {keepalive}",
                    reason.text(),
                    netdata_agent_log::strerror(errno)
                );
            }
            Ok(None) => nd_log!(
                Source::Daemon,
                Priority::Err,
                "{prefix}{} - closing connection; SO_ERROR=0 (no pending socket error); TCP keepalive: {keepalive}",
                reason.text()
            ),
        }
    }

    /// `stream_receiver_dequeue_senders()`'s second half, after each read (D117.3): the POLLOUTs this read's commits
    /// posted, then the host's own sender when it runs on this thread, so a forwarded burst does not wait whole in its
    /// ring. A failure there leaves the receiver alone.
    fn send_proxied(&mut self, cx: &mut Context<'_>, index: usize) {
        let Some(child) = self.children[index].as_ref() else {
            return;
        };
        if child.attached.host.upstream().is_none() {
            return;
        }
        let host = Arc::clone(&child.attached.host);
        self.drain_inline(cx);
        let mine = self.senders.iter().position(|d| d.as_ref().is_some_and(|d| Arc::ptr_eq(&d.host, &host)));
        if let Some(i) = mine {
            self.send_sender(cx, i, false);
        }
    }

    /// `stream_receiver_receive_data()`: reads what arrived and feeds every complete line to the parser; a refused
    /// line ends the connection. The caller has pushed the child's frame.
    fn receive(&mut self, cx: &mut Context<'_>, index: usize) {
        let mut buf = [0u8; crate::compression::MAX_CHUNK];
        // C's one read per host before the next (`count = 1`): a read that returned data asks for its turn again,
        // after the thread's other sources (D126.6)
        {
            let Some(child) = self.children[index].as_mut() else {
                return;
            };
            // C's parser frame of stream_receiver_receive_data() covers the read (a TLS read's records carry the
            // scope chart) and every record after it; the parser's fields are taken when a record is due, as C's
            // callbacks read them
            // receiver_read_uncompressed() reads PLUGINSD_LINE_MAX bytes (the partial line moved out of its buffer);
            // receiver_read_compressed() fills the chunk buffer behind the partial message it holds (D129.1), which
            // only a receiver leaving (the exit, a stop) leaves fuller than a partial message
            let size = child.decompressor.as_ref().map_or(LINE_MAX, |d| buf.len().saturating_sub(d.held()));
            // C's now_ut, the dispatch's time, before the read
            let read_ut = now_monotonic_usec();
            let read = {
                let _parser = child.attached.stream.is_tls().then(|| child.parser.log_frame());
                child.attached.stream.read(&mut buf[..size])
            };
            let failed = |child: &Child, reason: Reason, errno: i32| {
                let _parser = child.parser.log_frame();
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    errno = errno;
                    "{}{} (fd {}) - closing receiver connection.",
                    Self::prefix(child),
                    reason.text(),
                    raw_fd(&child.attached.stream)
                );
                reason
            };
            match read {
                Ok(0) => {
                    let reason = failed(child, Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE, 0);
                    let _parser = child.parser.log_frame();
                    self.disconnect(cx, index, reason);
                }
                Ok(n) => {
                    child.bytes_in += n as u64;
                    child
                        .attached
                        .hosts
                        .storage()
                        .pulse()
                        .network
                        .stream_received(n);
                    child.attached.host.stream_bytes_received(n);
                    match child.decompressor.as_mut() {
                        None => {
                            if !parse(&mut child.reader, &mut child.parser, &mut child.attached, &buf[..n]) {
                                let _parser = child.parser.log_frame();
                                return self.disconnect(cx, index, Reason::RCV_DISCONNECT_PARSER_FAILED);
                            }
                        }
                        Some(decompressor) => {
                            // stream_receive_and_process(): a message at a time, its lines parsed before the next is
                            // decompressed, while the streaming service runs and no stop is asked
                            decompressor.feed(&buf[..n]);
                            let mut out = Vec::new();
                            let slot = Arc::clone(&child.attached.slot);
                            let stop = || slot.stop_requested.load(Ordering::Acquire);
                            while !netdata_agent_sys::exit::initiated() && !stop() {
                                out.clear();
                                // C decompresses under the parser's fields
                                let next = {
                                    let _parser = child.parser.log_frame();
                                    decompressor.next_message(&mut out)
                                };
                                match next {
                                    Ok(false) => break,
                                    Ok(true) => {
                                        if !parse(&mut child.reader, &mut child.parser, &mut child.attached, &out) {
                                            let _parser = child.parser.log_frame();
                                            return self.disconnect(cx, index, Reason::RCV_DISCONNECT_PARSER_FAILED);
                                        }
                                    }
                                    Err(failure) => {
                                        let _parser = child.parser.log_frame();
                                        let a = &child.attached;
                                        nd_log!(
                                            Source::Daemon,
                                            Priority::Err,
                                            "STREAM RCV[x] '{}' [from [{}]:{}]: {failure}",
                                            a.host.hostname(),
                                            a.peer.ip,
                                            a.peer.port
                                        );
                                        return self.disconnect(cx, index, Reason::RCV_DECOMPRESSION_FAILED);
                                    }
                                }
                            }
                            if stop() {
                                // C's removal runs inside the read's parser stack
                                let _parser = child.parser.log_frame();
                                return self.disconnect(cx, index, Reason::DISCONNECT_SIGNALED_TO_STOP);
                            }
                        }
                    }
                    // a read processed without a removal is traffic; a removal inside it logs the idle time before it
                    child.attached.slot.last_traffic_ut.store(read_ut, Ordering::Relaxed);
                    // the charts just received may lower the update every the keepalive follows
                    let a = &mut child.attached;
                    reconcile_keepalive(
                        std::os::fd::AsFd::as_fd(a.stream.socket().expect("a taken-over link has its socket")),
                        &a.host,
                        &a.peer,
                        &a.keepalive,
                        a.handshake_update_every,
                        &mut a.keepalive_initialized,
                    );
                    // stream_receiver_dequeue_senders(): a failed write here ends the connection as a read failure
                    if !self.send_data(cx, index, Writer::Read) {
                        let Some(child) = self.children[index].as_ref() else {
                            return;
                        };
                        let reason = failed(child, Reason::DISCONNECT_SOCKET_READ_FAILED, 0);
                        let _parser = child.parser.log_frame();
                        return self.disconnect(cx, index, reason);
                    }
                    self.send_proxied(cx, index);
                    // once the exit started, the next turn's running() ends the loop before this is served (D110)
                    if self.children[index].is_some() {
                        cx.report_again(Token(index));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                // C's SOCKET_FULL: another turn
                Err(e) if e.kind() == io::ErrorKind::Interrupted => cx.report_again(Token(index)),
                Err(e) => {
                    let reason = if e.kind() == io::ErrorKind::ConnectionReset {
                        Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE
                    } else {
                        Reason::DISCONNECT_SOCKET_READ_FAILED
                    };
                    let reason = failed(child, reason, netdata_agent_log::errno_of(&e));
                    let _parser = child.parser.log_frame();
                    self.disconnect(cx, index, reason);
                }
            }
        }
    }
}

/// The host's health from `health enabled`: `rrdhost_find_or_create(..., health.enabled != CONFIG_BOOLEAN_NO, ...)`
/// (yes and auto alike), and `rrdhost_create()`/`rrdhost_update()`'s none without a database.
fn host_health_enabled(health_enabled: i32, mode: DbMode) -> bool {
    health_enabled != netdata_agent_inicfg::BOOLEAN_NO && mode != DbMode::None
}

/// `stream_conf_receiver_config()`'s memory mode: a child configured for dbengine gets the default when the dbengine
/// does not run, which C logs (N7); whether it fell back.
fn receiver_mode(configured: &str, default: &str, dbengine: bool) -> (DbMode, bool) {
    let mode = DbMode::from_name(configured);
    if mode == DbMode::Dbengine && !dbengine {
        return (DbMode::from_name(default), true);
    }
    (mode, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_nrpc::testing::inert;
    use netdata_agent_rrd::host::HostInfo;
    use netdata_agent_rrd::stream_buffer::INITIAL_MAX_SIZE;
    use std::os::fd::AsFd;

    /// Replication stalls after ten minutes without new requests, unless charts wait for their backfill (C's
    /// `backfill_pending` check); new requests restart the clock.
    #[test]
    fn replication_progress_waits_for_backfills() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(601);
        let (mut requests, mut since) = (0, None);
        assert!(replication_progressed((&mut requests, &mut since), 3, false, t0));
        assert!(replication_progressed((&mut requests, &mut since), 3, false, t0 + Duration::from_secs(599)));
        assert!(!replication_progressed((&mut requests, &mut since), 3, false, later));
        assert!(replication_progressed((&mut requests, &mut since), 3, true, later), "work waits");
        assert!(replication_progressed((&mut requests, &mut since), 4, false, later));
        assert!(replication_progressed((&mut requests, &mut since), 0, false, later), "not started");
    }

    /// A child configured for dbengine falls back to the default only when the dbengine does not run; other names
    /// are C's modes (an unknown one is ram).
    #[test]
    fn dbengine_children_fall_back_only_without_the_engine() {
        let cases = [
            ("dbengine", "ram", false, (DbMode::Ram, true)),
            ("dbengine", "alloc", false, (DbMode::Alloc, true)),
            ("dbengine", "dbengine", true, (DbMode::Dbengine, false)),
            ("ram", "dbengine", true, (DbMode::Ram, false)),
            ("bogus", "dbengine", true, (DbMode::Ram, false)),
        ];
        for (configured, default, dbengine, want) in cases {
            assert_eq!(
                receiver_mode(configured, default, dbengine),
                want,
                "{configured}"
            );
        }
    }

    fn host() -> Host {
        Host::new(
            "guid",
            false,
            HostInfo {
                hostname: "child".into(),
                registry_hostname: "child".into(),
                os: "linux".into(),
                timezone: "UTC".into(),
                abbrev_timezone: "UTC".into(),
                utc_offset: 0,
                program_name: "p".into(),
                program_version: "1".into(),
                update_every: 1,
                db_mode: DbMode::Ram,
                history_entries: 4096,
                health_enabled: false,
                system_info: Default::default(),
                replication_enabled: false,
                replication_period: 0,
                replication_step: 0,
                stream_send: None,
                cache_dir: None,
            },
        )
    }

    fn keepalive() -> Keepalive {
        Keepalive {
            enabled: true,
            automatic: true,
            idle_s: 0,
        }
    }

    #[test]
    fn the_update_every_follows_the_charts_then_the_handshake() {
        let h = host();
        assert_eq!(receiver_update_every(&h, 5), 5);
        assert_eq!(receiver_update_every(&h, -1), 0);
        h.observe_receiver_update_every(10);
        h.observe_receiver_update_every(2);
        h.observe_receiver_update_every(7);
        assert_eq!(receiver_update_every(&h, 5), 2);
        assert_eq!(automatic_keepalive_idle(0), 30);
        assert_eq!(automatic_keepalive_idle(121), 61);
        assert_eq!(automatic_keepalive_idle(10_000), 3600);
    }

    #[test]
    fn keepalive_goes_on_the_socket_and_follows_the_update_every() {
        use nix::sys::socket::{getsockopt, sockopt};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let (h, peer) = (host(), Peer::default());
        let mut initialized = false;
        reconcile_keepalive(server.as_fd(), &h, &peer, &keepalive(), 0, &mut initialized);
        assert!(initialized && getsockopt(&server, sockopt::KeepAlive).unwrap());
        assert_eq!(getsockopt(&server, sockopt::TcpKeepIdle).unwrap(), 30);
        assert_eq!(getsockopt(&server, sockopt::TcpKeepInterval).unwrap(), 10);
        assert_eq!(getsockopt(&server, sockopt::TcpKeepCount).unwrap(), 3);
        // a chart every 200 s makes the automatic idle 100 s
        h.observe_receiver_update_every(200);
        reconcile_keepalive(server.as_fd(), &h, &peer, &keepalive(), 0, &mut initialized);
        assert_eq!(getsockopt(&server, sockopt::TcpKeepIdle).unwrap(), 100);
        // a configured idle is applied once
        let fixed = Keepalive {
            automatic: false,
            idle_s: 45,
            ..keepalive()
        };
        let mut initialized = false;
        reconcile_keepalive(server.as_fd(), &h, &peer, &fixed, 0, &mut initialized);
        assert_eq!(getsockopt(&server, sockopt::TcpKeepIdle).unwrap(), 45);
    }

    /// A child taken over by `admit()` and queued for stream thread 0, its peer end kept open.
    fn queued(
        n: u8,
        pool: &netdata_agent_evloop::Pool<StreamMsg>,
        hosts: &Arc<Hosts>,
        connector: &Arc<Connector>,
    ) -> (Arc<Host>, Arc<ReceiverSlot>, mio::net::UnixStream) {
        let (attached, host, slot, theirs) = child(n, crate::caps::V2, pool, hosts, connector);
        pool.handle().send(0, StreamMsg::Attach(Box::new(attached))).unwrap();
        (host, slot, theirs)
    }

    /// A child taken over by `admit()` with `capabilities` negotiated, for stream thread 0, and its peer end.
    fn child(
        n: u8,
        capabilities: u32,
        pool: &netdata_agent_evloop::Pool<StreamMsg>,
        hosts: &Arc<Hosts>,
        connector: &Arc<Connector>,
    ) -> (Attached, Arc<Host>, Arc<ReceiverSlot>, mio::net::UnixStream) {
        let host = Arc::new(Host::new(
            &format!("5a1e0000-0000-4000-8000-0000000000{n:02x}"),
            false,
            crate::connector::tests::info("", ""),
        ));
        let slot = Arc::new(ReceiverSlot::new(
            1,
            ("127.0.0.1".into(), "1".into()),
            ReceiverLink::default(),
            Box::new(|| {}),
        ));
        assert_eq!(host.set_receiver(Arc::clone(&slot)), netdata_agent_rrd::host::Attach::Attached);
        let (ours, theirs) = mio::net::UnixStream::pair().unwrap();
        let attached = Attached {
            host: Arc::clone(&host),
            hosts: Arc::clone(hosts),
            slot: Arc::clone(&slot),
            stream: Link::Plain(Conn::Unix(ours)),
            thread: 0,
            parser: ingest::Config {
                capabilities,
                update_every: 1,
                page_size: 4096,
                now: || (1_700_000_000, 0),
                gap_when_lost_iterations_above: 3,
            },
            peer: Peer::default(),
            accepted_s: 0,
            keepalive: keepalive(),
            handshake_update_every: 1,
            keepalive_initialized: false,
            write_wanted: false,
            pool: pool.handle(),
            replication_wait: false,
            connector: Arc::clone(connector),
        };
        (attached, host, slot, theirs)
    }

    /// A stream thread driven one turn at a time, with the pool and connector its children's admission needs.
    fn stepper() -> (
        netdata_agent_evloop::testing::Stepper<StreamWorker>,
        netdata_agent_evloop::Pool<StreamMsg>,
        Arc<Hosts>,
        Arc<Connector>,
    ) {
        let (pool, connector) = crate::connector::tests::connector();
        let hosts = Arc::new(Hosts::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
            crate::connector::tests::info("", ""),
        )));
        let worker = StreamWorker::new(Arc::new(Mutex::new(Pins::new(1))), 1);
        (netdata_agent_evloop::testing::Stepper::new(0, worker).unwrap(), pool, hosts, connector)
    }

    /// What each attached child has read so far, in attach order.
    fn read_so_far(s: &mut netdata_agent_evloop::testing::Stepper<StreamWorker>) -> Vec<u64> {
        s.worker().children.iter().flatten().map(|c| c.bytes_in).collect()
    }

    /// C's one read of a host per turn (`count = 1`, D126.6) of `PLUGINSD_LINE_MAX` bytes (D129.1): two children with
    /// 3 x 16384 bytes each are read one such read a turn each, the first in the turn after the attach, until a read
    /// finds the socket empty.
    #[test]
    fn children_are_read_once_a_turn_each() {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let mut peers = Vec::new();
        for n in [0xd1, 0xd2] {
            let (attached, _, _, mut theirs) = child(n, crate::caps::V2, &pool, &hosts, &connector);
            theirs.write_all(&[b'\n'; 3 * 16384]).unwrap();
            s.with(|w, cx| w.attach(cx, attached));
            peers.push(theirs);
        }
        assert_eq!(read_so_far(&mut s), [0, 0]);
        let l = LINE_MAX as u64;
        for read in [l, 2 * l, 3 * l, 3 * 16384, 3 * 16384] {
            assert!(s.turn(Duration::from_millis(50)));
            assert_eq!(read_so_far(&mut s), [read, read]);
        }
        assert_eq!(s.owed(), []);
    }

    /// A child that sends two reads' worth and closes is read twice, then removed at the third read.
    #[test]
    fn a_closed_child_is_removed_at_the_read_that_finds_the_end() {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _, mut theirs) = child(0xd3, crate::caps::V2, &pool, &hosts, &connector);
        theirs.write_all(&[b'\n'; 2 * LINE_MAX]).unwrap();
        drop(theirs);
        s.with(|w, cx| w.attach(cx, attached));
        let l = LINE_MAX as u64;
        for read in [l, 2 * l] {
            assert!(s.turn(Duration::from_millis(50)));
            assert_eq!(read_so_far(&mut s), [read]);
        }
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(read_so_far(&mut s).is_empty());
        assert!(host.receiver().is_none());
        let texts: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
        assert!(texts.iter().any(|t| t.contains("CLOSED BY REMOTE END")), "{texts:?}");
    }

    /// A child's functions follow its receiver (`stream-receiver.c:1400`, `:1472`): what the child registers over its
    /// connection is available while the connection lasts and unavailable once it ends; a receiver that is not the
    /// attached one leaving changes nothing (R61-12); the next attach retires what the host registered before it.
    #[test]
    fn a_childs_functions_follow_its_receiver() {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _, mut theirs) = child(0xd5, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        theirs.write_all(b"FUNCTION GLOBAL \"f\" 10 \"help\" \"top\" \"0x0\" 100 0\n").unwrap();
        for _ in 0..20 {
            if host.functions().get(b"f").is_some() {
                break;
            }
            s.turn(Duration::from_millis(50));
        }
        let available = || host.functions().available(b"f");
        assert!(available(), "registered by the child");
        let slot = || Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        host.clear_receiver(&slot(), 0);
        assert!(available(), "a receiver that is not the attached one left");
        drop(theirs);
        for _ in 0..20 {
            if host.receiver().is_none() {
                break;
            }
            let _ = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        }
        assert!(host.receiver().is_none());
        assert!(!available(), "the child's connection ended");
        // registered while no receiver is attached
        let desc = netdata_agent_nrpc::MethodDesc {
            name: b"f",
            help: b"help",
            tags: b"",
            timeout_s: 10,
            priority: 0,
            version: 0,
            access: 0,
            sync: false,
            source: netdata_agent_nrpc::Source::Stream,
            handler: inert(),
        };
        host.register_function(&desc).unwrap();
        assert!(available());
        let again = slot();
        assert_eq!(host.set_receiver(Arc::clone(&again)), Attach::Attached);
        assert!(!available(), "the attach starts a new epoch");
        host.clear_receiver(&again, 0);
    }

    /// A sender that notes, when told its receiver left, whether the host is still pinned to its thread.
    #[derive(Debug)]
    struct PinProbe {
        pins: Arc<Mutex<Pins>>,
        guid: String,
        pinned_when_left: Mutex<Option<bool>>,
    }

    impl netdata_agent_rrd::upstream::Upstream for PinProbe {
        fn start(&self) {}
        fn disabled_capabilities(&self) -> u32 {
            0
        }
        fn capabilities(&self) -> u32 {
            0
        }
        fn commit(&self, _: &[u8], _: netdata_agent_rrd::upstream::Traffic) {}
        fn resync_iterations(&self) -> u16 {
            3
        }
        fn flush_ut(&self) -> u64 {
            0
        }
        fn commit_since(&self, _: &[u8], _: netdata_agent_rrd::upstream::Traffic, _: u64) -> bool {
            false
        }
        fn receiver_left(&self, _: i32) {
            let pinned = self.pins.lock().unwrap().is_pinned(&self.guid);
            *self.pinned_when_left.lock().unwrap() = Some(pinned);
        }
        fn parents_reset(&self, _: i32) {}
        fn free(&self) {}
        fn reinit(&self, _: &StreamSend) {}
    }

    /// A removed child's host leaves its thread before the host is detached, as C's `stream_thread_node_removed()`
    /// comes first (R55 M6): a child that reconnects meanwhile goes to the least loaded thread.
    #[test]
    fn a_removed_child_is_unpinned_before_its_host_detaches() {
        let pins = Arc::new(Mutex::new(Pins::new(1)));
        let mut s = netdata_agent_evloop::testing::Stepper::new(0, StreamWorker::new(Arc::clone(&pins), 1)).unwrap();
        let (pool, connector) = crate::connector::tests::connector();
        let hosts = Arc::new(Hosts::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
            crate::connector::tests::info("", ""),
        )));
        let (attached, host, _, theirs) = child(0xd4, crate::caps::V2, &pool, &hosts, &connector);
        pins.lock().unwrap().queue(host.machine_guid());
        let probe = Arc::new(PinProbe {
            pins: Arc::clone(&pins),
            guid: host.machine_guid().to_string(),
            pinned_when_left: Mutex::new(None),
        });
        host.set_upstream(Arc::clone(&probe) as Arc<dyn netdata_agent_rrd::upstream::Upstream>);
        drop(theirs);
        s.with(|w, cx| w.attach(cx, attached));
        let _ = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.receiver().is_none());
        assert_eq!(*probe.pinned_when_left.lock().unwrap(), Some(false));
    }

    const ADMIT_KEY: &str = "11111111-2222-3333-4444-555555555555";

    /// Receivers over a host index of their own (default stream.conf, ram), and the pool their connector runs.
    fn receivers() -> (Receivers, netdata_agent_evloop::Pool<StreamMsg>) {
        let (pool, connector) = crate::connector::tests::connector();
        let hosts = Arc::new(Hosts::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
            crate::connector::tests::info("", ""),
        )));
        let defaults = Defaults {
            db_mode: "ram".into(),
            history: 3600,
            health_enabled: false,
            update_every: 1,
            page_size: 4096,
            gap_when_lost_iterations_above: 3,
        };
        let pins = Arc::new(Mutex::new(Pins::new(1)));
        (Receivers::new(StreamConf::default(), hosts, pins, defaults, pool.handle(), connector), pool)
    }

    /// A request of `child` for `guid` that passed `pre_admit()`.
    fn pending(guid: &str) -> Pending {
        let q = format!("key={ADMIT_KEY}&hostname=child&machine_guid={guid}&ver=17088");
        Pending {
            request: StreamRequest::parse(q.as_bytes(), 1, None),
            peer: Peer {
                ip: "127.0.0.1".into(),
                port: "1".into(),
                hostname: Some("child".into()),
                key: Some(ADMIT_KEY.into()),
                machine_guid: Some(guid.into()),
            },
            accepted_s: 0,
        }
    }

    /// `stream_receiver_send_first_response()` logs the negotiated capabilities while it builds the prompt, before the
    /// socket is set up: a unix child's TCP option warnings follow it.
    #[test]
    fn the_capabilities_are_logged_before_the_socket_options() {
        let (r, _pool) = receivers();
        let guid = "5a1e0000-0000-4000-8000-0000000000e3";
        let (ours, _theirs) = mio::net::UnixStream::pair().unwrap();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&ours);
        let (admitted, records) =
            netdata_agent_log::capture(|| r.admit(pending(guid), Link::Plain(Conn::Unix(ours))));
        assert!(admitted);
        let order: Vec<String> = texts(records)
            .into_iter()
            .filter(|t| t.contains("established link") || t.contains("cannot set TCP_"))
            .map(|t| t.split_once("]: ").unwrap().1.split(": ").next().unwrap().to_string())
            .collect();
        assert_eq!(
            order,
            [
                "established link with negotiated capabilities".to_string(),
                format!("cannot set TCP_KEEPIDLE on socket {fd}"),
                format!("cannot set TCP_KEEPINTVL on socket {fd}"),
                format!("cannot set TCP_KEEPCNT on socket {fd}"),
            ]
        );
    }

    /// `rrdhost_set_receiver()`: a child that attaches with health on and a positive `postpone alerts on connect`
    /// has its health postponed until that many seconds from now; with health off, or without a delay, the host's
    /// delay is left alone and nothing is logged.
    #[test]
    fn an_attach_postpones_the_child_s_health() {
        let cases = [
            ("health enabled = yes\n  postpone alerts on connect = 90s", Some(90)),
            ("health enabled = auto", Some(60)),
            ("health enabled = no\n  postpone alerts on connect = 90s", None),
            ("health enabled = yes\n  postpone alerts on connect = 0", None),
        ];
        for (i, (settings, delay)) in cases.into_iter().enumerate() {
            let (r, _pool) = receivers();
            r.conf.lock().unwrap().config.load_bytes(
                format!("[{ADMIT_KEY}]\n  enabled = yes\n  {settings}\n").as_bytes(),
                "stream.conf",
                false,
                None,
            );
            let guid = format!("5a1e0000-0000-4000-8000-0000000000f{i}");
            let (ours, _theirs) = mio::net::UnixStream::pair().unwrap();
            let before = now_s();
            let (admitted, records) =
                netdata_agent_log::capture(|| r.admit(pending(&guid), Link::Plain(Conn::Unix(ours))));
            assert!(admitted, "{settings}");
            let host = r.hosts.find_by_guid(&guid).expect("the child's host");
            let postponed: Vec<String> =
                texts(records).into_iter().filter(|t| t.contains("Postponing health checks")).collect();
            match delay {
                Some(delay) => {
                    let up_to = host.health_delay_up_to();
                    assert!((before + delay..=now_s() + delay).contains(&up_to), "{settings}: {up_to}");
                    assert_eq!(postponed.len(), 1, "{settings}");
                    assert!(postponed[0].ends_with(&format!(
                        "Postponing health checks for {delay} seconds, because it was just connected."
                    )));
                }
                None => {
                    assert_eq!(host.health_delay_up_to(), 0, "{settings}");
                    assert!(postponed.is_empty(), "{settings}");
                }
            }
        }
    }

    /// `stream_receiver_send_first_response()`: a prompt the socket does not take whole (here EPIPE, the child gone)
    /// is C's ERR status pair (SEND TIMEOUT, the send's errno on the access record), then the receiver is cleared.
    #[test]
    fn a_failed_prompt_drops_the_connection_and_clears_the_receiver() {
        let (r, _pool) = receivers();
        let guid = "5a1e0000-0000-4000-8000-0000000000e2";
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(theirs);
        // a web server socket is non-blocking; a dup shares its status flags and options
        ours.set_nonblocking(true).unwrap();
        let probe = ours.try_clone().unwrap();
        let ours = mio::net::UnixStream::from_std(ours);
        let (admitted, records) =
            netdata_agent_log::capture(|| r.admit(pending(guid), Link::Plain(Conn::Unix(ours))));
        assert!(!admitted);
        // the prompt went out on a blocking socket with a 600 s receive timeout
        let fdinfo = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", std::os::fd::AsRawFd::as_raw_fd(&probe)))
            .unwrap();
        let flags = fdinfo.lines().find_map(|l| l.strip_prefix("flags:")).unwrap().trim();
        assert_eq!(u32::from_str_radix(flags, 8).unwrap() & 0o4000, 0, "O_NONBLOCK");
        assert_eq!(probe.read_timeout().unwrap(), Some(Duration::from_secs(600)));
        let host = r.hosts.find_by_guid(guid).unwrap();
        assert!(host.receiver().is_none() && host.is_orphan());
        assert!(host.receiver_last_disconnected_s() > 0);
        let failed: Vec<_> = records
            .into_iter()
            .filter(|r| r.priority == Priority::Err)
            .map(|r| (r.source, r.errno, r.message.unwrap_or_default()))
            .collect();
        assert_eq!(
            failed,
            [
                (
                    Source::Access,
                    nix::errno::Errno::EPIPE as i32,
                    format!(
                        "api_key:'[REDACTED]' machine_guid:'{guid}' node:'child' msg:'cannot reply back, dropping \
                         connection' reason:'SEND TIMEOUT'"
                    )
                ),
                (
                    Source::Daemon,
                    0,
                    "STREAM RCV 'child' [from [127.0.0.1]:1]: cannot reply back, dropping connection  (SEND TIMEOUT)"
                        .to_string()
                ),
            ]
        );
    }

    /// Yes and auto both run health on the host; no, or a host without a database, does not.
    #[test]
    fn health_runs_for_yes_and_auto_with_a_database() {
        use netdata_agent_inicfg::{BOOLEAN_AUTO, BOOLEAN_NO, BOOLEAN_YES};
        let cases = [
            (BOOLEAN_YES, DbMode::Ram, true),
            (BOOLEAN_AUTO, DbMode::Ram, true),
            (BOOLEAN_NO, DbMode::Ram, false),
            (BOOLEAN_YES, DbMode::Dbengine, true),
            (BOOLEAN_AUTO, DbMode::None, false),
            (BOOLEAN_YES, DbMode::None, false),
        ];
        for (health, mode, want) in cases {
            assert_eq!(host_health_enabled(health, mode), want, "{health} {mode:?}");
        }
    }

    /// `stream_receiver_send_first_response()`: a host still loading its contexts (an archived one) is refused with
    /// C's NOTICE status pair (REMOTE IS INITIALIZING) and the initialization reply, and is never attached.
    #[test]
    fn a_host_pending_its_context_load_is_refused_as_initializing() {
        use std::io::Read;
        let (r, _pool) = receivers();
        let guid = "5a1e0000-0000-4000-8000-0000000000e1";
        let host = r.hosts.add_archived(guid, crate::connector::tests::info("", ""), |_| {});
        let (ours, mut theirs) = mio::net::UnixStream::pair().unwrap();
        let (admitted, records) =
            netdata_agent_log::capture(|| r.admit(pending(guid), Link::Plain(Conn::Unix(ours))));
        assert!(!admitted);
        let mut reply = String::new();
        theirs.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, "The server is initializing. Try later.");
        assert!(host.receiver().is_none() && host.is_pending_context_load());
        let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
        assert_eq!(
            records,
            [
                (
                    Source::Access,
                    Priority::Notice,
                    format!(
                        "api_key:'[REDACTED]' machine_guid:'{guid}' node:'child' msg:'rejecting streaming connection; \
                         host is initializing, retry later' reason:'REMOTE IS INITIALIZING'"
                    )
                ),
                (
                    Source::Daemon,
                    Priority::Notice,
                    "STREAM RCV 'child' [from [127.0.0.1]:1]: rejecting streaming connection; host is initializing, \
                     retry later  (REMOTE IS INITIALIZING)"
                        .to_string()
                ),
            ]
        );
    }

    /// `stream_receiver_connected_msg()`'s forms: no sample, a sample now or in the future (read as now), and an age
    /// in C's duration text.
    #[test]
    fn the_connected_record_names_the_last_samples_age_as_c() {
        const NOW: i64 = 1_700_000_000;
        assert_eq!(
            [
                connected_msg_at(0, NOW),
                connected_msg_at(NOW, NOW),
                connected_msg_at(NOW + 5, NOW),
                connected_msg_at(NOW - 3725, NOW),
            ],
            [
                "connected and ready to receive data, new node",
                "connected and ready to receive data, last sample in the db just now",
                "connected and ready to receive data, last sample in the db just now",
                "connected and ready to receive data, last sample in the db 1h 2m 5s ago",
            ]
        );
    }

    /// An attach while the maintenance marks the host's charts obsolete is refused as busy
    /// (`stream_receiver_send_first_response()` on RRDHOST_SET_RECEIVER_CLEANUP_BUSY): the status records at INFO,
    /// C's busy text, the host left without a receiver.
    #[test]
    fn an_attach_during_the_obsolete_all_walk_is_answered_busy() {
        use std::io::Read;
        let (r, _pool) = receivers();
        let guid = "5a1e0000-0000-4000-8000-0000000000e2";
        let host = r.hosts.add_archived(guid, crate::connector::tests::info("", ""), |_| {});
        host.clear_pending_context_load();
        host.set_obsolete_all_busy(true);
        let (ours, mut theirs) = mio::net::UnixStream::pair().unwrap();
        let (admitted, records) =
            netdata_agent_log::capture(|| r.admit(pending(guid), Link::Plain(Conn::Unix(ours))));
        assert!(!admitted);
        let mut reply = String::new();
        theirs.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, "The server is too busy now to accept this request. Try later.");
        assert!(host.receiver().is_none());
        // the host's update records (the reconnect's switches, the registry, no longer archived) come first
        let records: Vec<_> = records
            .into_iter()
            .map(|r| (r.source, r.priority, r.message.unwrap_or_default()))
            .filter(|(_, _, m)| m.contains("rejecting"))
            .collect();
        assert_eq!(
            records,
            [
                (
                    Source::Access,
                    Priority::Info,
                    format!(
                        "api_key:'[REDACTED]' machine_guid:'{guid}' node:'child' msg:'rejecting streaming connection; \
                         internal cleanup is in progress for this node, please retry shortly' reason:'BUSY TRY LATER'"
                    )
                ),
                (
                    Source::Daemon,
                    Priority::Info,
                    "STREAM RCV 'child' [from [127.0.0.1]:1]: rejecting streaming connection; internal cleanup is in \
                     progress for this node, please retry shortly  (BUSY TRY LATER)"
                        .to_string()
                ),
            ]
        );
    }

    /// A vnode a plugin of this agent collects is refused before the takeover (`stream-receiver-connection.c:624-653`),
    /// at C's debug level; a host that is not one goes on.
    #[test]
    fn a_vnode_is_refused_before_the_takeover() {
        let (r, _pool) = receivers();
        r.conf.lock().unwrap().config.load_bytes(
            format!("[{ADMIT_KEY}]\n  enabled = yes\n").as_bytes(),
            "stream.conf",
            false,
            None,
        );
        let guid = "5a1e0000-0000-4000-8000-0000000000e3";
        let query = format!("key={ADMIT_KEY}&hostname=child&machine_guid={guid}&ver=17088");
        let host = r.hosts.add_archived(guid, crate::connector::tests::info("", ""), |_| {});
        let first = r.pre_admit(query.as_bytes(), None, "127.0.0.1", "1");
        assert!(matches!(first, PreAdmission::Proceed(_)), "{first:?}");
        host.set_virtual();
        let refused = r.pre_admit(query.as_bytes(), None, "127.0.0.1", "1");
        let PreAdmission::Refuse(reply, refusal) = refused else { panic!("{refused:?}") };
        assert_eq!(
            (reply, refusal.msg, refusal.reason, refusal.priority),
            (
                handshake::ERROR_LOCAL_VNODE,
                "rejecting streaming connection; this is a locally collected vnode",
                Reason::PARENT_VNODE_IS_LOCAL,
                Priority::Debug
            )
        );
    }

    /// A vnode claimed after the accept is refused at the attach, under the receiver lock, with C's warning and reply
    /// (`stream-receiver-connection.c:234-242`).
    #[test]
    fn a_vnode_claimed_before_the_attach_is_refused() {
        use std::io::Read;
        let (r, _pool) = receivers();
        let guid = "5a1e0000-0000-4000-8000-0000000000e4";
        let host = r.hosts.add_archived(guid, crate::connector::tests::info("", ""), |_| {});
        host.clear_pending_context_load();
        host.set_virtual();
        let (ours, mut theirs) = mio::net::UnixStream::pair().unwrap();
        let (admitted, records) =
            netdata_agent_log::capture(|| r.admit(pending(guid), Link::Plain(Conn::Unix(ours))));
        assert!(!admitted);
        let mut reply = String::new();
        theirs.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, handshake::ERROR_LOCAL_VNODE);
        assert!(host.receiver().is_none());
        let records: Vec<_> = records
            .into_iter()
            .map(|r| (r.source, r.priority, r.message.unwrap_or_default()))
            .filter(|(_, _, m)| m.contains("rejecting"))
            .collect();
        assert_eq!(
            records,
            [
                (
                    Source::Access,
                    Priority::Warning,
                    format!(
                        "api_key:'[REDACTED]' machine_guid:'{guid}' node:'child' msg:'rejecting streaming connection; \
                         this host was claimed as a locally collected vnode' reason:'LOCAL VNODE'"
                    )
                ),
                (
                    Source::Daemon,
                    Priority::Warning,
                    "STREAM RCV 'child' [from [127.0.0.1]:1]: rejecting streaming connection; this host was claimed as \
                     a locally collected vnode  (LOCAL VNODE)"
                        .to_string()
                ),
            ]
        );
    }

    /// The traffic time moves only once a read was processed without a removal (`stream_receiver_receive_data()`): a
    /// line refused after a second of quiet logs `idle=1s`, the time since the previous traffic, as C.
    #[test]
    fn a_removal_inside_a_read_logs_the_idle_time_before_it() {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, _host, slot, mut theirs) = child(0xd8, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        slot.last_traffic_ut.store(now_monotonic_usec(), Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(1100));
        theirs.write_all(b"BOGUS\n").unwrap();
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        let texts: Vec<_> = texts(records).into_iter().filter(|t| t.contains("receiver disconnected")).collect();
        assert_eq!(texts.len(), 1, "{texts:?}");
        assert!(texts[0].contains("reason=\"DISCONNECTED PARSE ERROR\"") && texts[0].contains(" idle=1s "), "{texts:?}");
    }

    /// The THREAD CLEANUP record of a child removed inside a BEGIN2 carries the parser's fields only when the removal
    /// is inside a read (`stream_receiver_receive_data()`'s stack): a refused line, not the shutdown.
    #[test]
    fn the_cleanup_record_has_the_parsers_fields_only_inside_a_read() {
        use std::io::Write;
        let cleanup = |refuse: bool| {
            let (mut s, pool, hosts, connector) = stepper();
            let (attached, _host, _, mut theirs) =
                child(if refuse { 0xd9 } else { 0xda }, crate::caps::V2, &pool, &hosts, &connector);
            s.with(|w, cx| w.attach(cx, attached));
            theirs
                .write_all(
                    b"CHART 'x.c' '' 't' 'u' 'f' 'x.ctx' line 1 1 '' p m\nDIMENSION 'd' '' absolute 1 1 ''\n\
                      BEGIN2 'x.c' 1 1700000000 #\nSET2 'd' 1 1 A\n",
                )
                .unwrap();
            s.turn(Duration::from_millis(50));
            let (_, records) = netdata_agent_log::capture(|| {
                if refuse {
                    theirs.write_all(b"BOGUS\n").unwrap();
                    s.turn(Duration::from_millis(50));
                } else {
                    s.with(|w, cx| w.stop_children(cx));
                }
            });
            let record = records
                .into_iter()
                .find(|r| r.message.as_deref().is_some_and(|m| m.contains("during THREAD CLEANUP")))
                .expect("a cleanup record");
            record.fields.into_iter().filter(|(f, _)| matches!(f, netdata_agent_log::Field::NidlInstance | netdata_agent_log::Field::NidlContext)).collect::<Vec<_>>()
        };
        assert_eq!(cleanup(false), []);
        assert_eq!(
            cleanup(true),
            [(netdata_agent_log::Field::NidlInstance, "x.c".to_string()), (netdata_agent_log::Field::NidlContext, "x.ctx".to_string())]
        );
    }

    /// A message decompressing past the chunk (zstd's 16385 bytes) ends the connection at its read: C's size record,
    /// "no bytes to decompress." and the disconnect with DECOMPRESSION FAILED.
    #[test]
    fn a_message_past_the_chunk_ends_the_connection_with_decompression_failed() {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _, mut theirs) =
            child(0xd6, crate::caps::V2 | crate::caps::ZSTD, &pool, &hosts, &connector);
        let big = zstd::bulk::compress(&vec![b'\n'; 16385], 1).unwrap();
        let mut f = crate::compression::encode_signature(big.len()).unwrap().to_vec();
        f.extend_from_slice(&big);
        theirs.write_all(&f).unwrap();
        s.with(|w, cx| w.attach(cx, attached));
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.receiver().is_none());
        let texts: Vec<_> = texts(records).into_iter().filter(|t| t.to_uppercase().contains("DECOMPRESS")).collect();
        assert_eq!(texts[0], "STREAM_DECOMPRESS: decompressed data is 16385 bytes, which is bigger than the max msg size 16384");
        assert_eq!(texts[1], "STREAM RCV[x] 'child' [from []:]: no bytes to decompress.");
        assert!(texts[2].contains("receiver disconnected: reason=\"DISCONNECTED DECOMPRESSION FAILED\""), "{texts:?}");
        assert_eq!(texts.len(), 3);
    }

    /// `rrdhost_set_receiver()` (at the attach) and `stream_receiver_accept_connection()` (after the prompt) each
    /// reset the host's parents with PREPARING; nothing else of the sender is called by an admission.
    #[test]
    fn an_admitted_child_resets_its_parents_at_the_attach_and_after_the_prompt() {
        let (pool, connector) = crate::connector::tests::connector();
        let hosts = Arc::new(Hosts::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
            crate::connector::tests::info("", ""),
        )));
        let guid = "5a1e0000-0000-4000-8000-0000000000cd";
        let host = hosts
            .find_or_create(guid, DbMode::Ram, || crate::connector::tests::info("", ""), |_| {})
            .unwrap();
        let r = Arc::new(netdata_agent_rrd::testing::Recorder::default());
        host.set_upstream(Arc::clone(&r) as Arc<dyn netdata_agent_rrd::upstream::Upstream>);
        let defaults = Defaults {
            db_mode: "ram".into(),
            history: 4096,
            health_enabled: false,
            update_every: 1,
            page_size: 4096,
            gap_when_lost_iterations_above: 3,
        };
        let receivers = Receivers::new(
            StreamConf::default(),
            Arc::clone(&hosts),
            Arc::new(Mutex::new(Pins::new(1))),
            defaults,
            pool.handle(),
            connector,
        );
        let query = format!("key=k&hostname=child&machine_guid={guid}&ver=8");
        let request = StreamRequest::parse(query.as_bytes(), 1, None);
        let (ours, theirs) = mio::net::UnixStream::pair().unwrap();
        let pending = Pending { request, peer: Peer::default(), accepted_s: 0 };
        assert!(receivers.admit(pending, Link::Plain(Conn::Unix(ours))));
        assert_eq!(*r.calls.lock().unwrap(), [("parents_reset", Reason::SP_PREPARING.0); 2]);
        drop(theirs);
    }

    /// The record texts a capture holds.
    fn texts(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
        records.into_iter().filter_map(|r| r.message).collect()
    }

    /// C's `last_traffic_ut` moves with every write to the child too: the idle timeout and the stale check at accept
    /// count it.
    #[test]
    fn a_write_to_the_child_is_traffic() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, _, slot, _theirs) = child(0xd6, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        slot.last_traffic_ut.store(1, Ordering::Relaxed);
        let before = now_monotonic_usec();
        s.with(|w, cx| {
            assert_eq!(slot.send_to_child(b"REPLAY_CHART x\n", Traffic::Replication), 15);
            w.drain_inline(cx);
        });
        assert!(slot.last_traffic_ut.load(Ordering::Relaxed) >= before);
    }

    /// `stream_receiver_check_all_nodes_from_poll()`: a child quiet for longer than max(600 s, twice its smallest
    /// update every) is disconnected (TIMEOUT), unless its charts replicate; a probe finding the end removes it.
    #[test]
    fn quiet_and_closed_children_are_disconnected_by_the_periodic_check() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, plain, plain_slot, _plain_peer) = child(0xd7, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        let (attached, slow, _, _slow_peer) = child(0xd8, crate::caps::V2, &pool, &hosts, &connector);
        slow.observe_receiver_update_every(400);
        s.with(|w, cx| w.attach(cx, attached));
        let (attached, replicating, _, _replicating_peer) = child(0xd9, crate::caps::V2, &pool, &hosts, &connector);
        replicating.replicating_charts_plus_one();
        s.with(|w, cx| w.attach(cx, attached));
        let last = plain_slot.last_traffic_ut.load(Ordering::Relaxed);
        let attached = |hosts: &[&Arc<Host>]| hosts.iter().map(|h| h.receiver().is_some()).collect::<Vec<_>>();
        let all = [&plain, &slow, &replicating];
        // every child was attached within a second of the first
        s.with(|w, cx| w.check_all(cx, last + 600_000_000));
        assert_eq!(attached(&all), [true, true, true]);
        let (_, records) = netdata_agent_log::capture(|| s.with(|w, cx| w.check_all(cx, last + 602_000_000)));
        assert_eq!(attached(&all), [false, true, true]);
        assert!(texts(records).iter().any(|t| t.contains("there was not traffic for 600 seconds - closing connection")));
        s.with(|w, cx| w.check_all(cx, last + 802_000_000));
        assert_eq!(attached(&all), [false, false, true]);
        let (attached_c, closed, _, closed_peer) = child(0xda, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached_c));
        drop(closed_peer);
        let (_, records) = netdata_agent_log::capture(|| s.with(|w, cx| w.check_all(cx, now_monotonic_usec())));
        assert!(closed.receiver().is_none());
        assert!(texts(records).iter().any(|t| t.contains("socket closed by remote - closing connection")));
    }

    /// A line the parser refuses removes the child at the read that brought it (PARSE ERROR).
    #[test]
    fn a_refused_line_removes_the_child_at_its_read() {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _, mut theirs) = child(0xdb, crate::caps::V2, &pool, &hosts, &connector);
        theirs.write_all(b"BOGUS\n").unwrap();
        s.with(|w, cx| w.attach(cx, attached));
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.receiver().is_none());
        let texts = texts(records);
        assert!(texts.iter().any(|t| t.contains("parser_action('BOGUS') failed on line 1")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("PARSE ERROR")), "{texts:?}");
    }

    /// A compressed stream is read into the 16384-byte chunk buffer behind the partial message it holds (D129.1): three
    /// messages of 12013 bytes are read 16384 bytes, then 16384 less the 4371 bytes of the second message held.
    #[test]
    fn a_compressed_read_takes_the_chunk_less_the_message_held() {
        use std::io::Write;
        // a zstd frame of one raw block of 12000 newlines: magic, a window of 16 KiB, the block header, the bytes
        let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x20];
        let n = 12000u32;
        frame.extend_from_slice(&((n << 3) | 1).to_le_bytes()[..3]);
        frame.extend(std::iter::repeat_n(b'\n', n as usize));
        let mut message = crate::compression::encode_signature(frame.len()).unwrap().to_vec();
        message.extend_from_slice(&frame);
        assert_eq!(message.len(), 12013);
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, _, _, mut theirs) =
            child(0xd5, crate::caps::V2 | crate::caps::ZSTD, &pool, &hosts, &connector);
        theirs.write_all(&message.repeat(3)).unwrap();
        s.with(|w, cx| w.attach(cx, attached));
        for read in [16384, 16384 + (16384 - 4371)] {
            assert!(s.turn(Duration::from_millis(50)));
            assert_eq!(read_so_far(&mut s), [read]);
        }
    }

    /// C decompresses and parses a read's messages one at a time: a message that fails after a good one in the same
    /// read leaves the good one's lines parsed, and the connection ends with DECOMPRESSION FAILED.
    #[test]
    fn a_read_s_messages_are_parsed_before_the_next_is_decompressed() {
        use std::io::Write;
        let framed = |payload: &[u8]| {
            let mut f = crate::compression::encode_signature(payload.len()).unwrap().to_vec();
            f.extend_from_slice(payload);
            f
        };
        let (mut s, pool, hosts, connector) = stepper();
        let capabilities = crate::caps::V2 | crate::caps::ZSTD;
        let (attached, host, _, mut theirs) = child(0xd4, capabilities, &pool, &hosts, &connector);
        let good = zstd::bulk::compress(b"CHART 'x.y' '' t u f c line 1 1\nDIMENSION 'd' '' absolute 1 1\n", 1).unwrap();
        theirs.write_all(&[framed(&good), framed(b"not zstd")].concat()).unwrap();
        s.with(|w, cx| w.attach(cx, attached));
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.charts().find("x.y", true).is_some());
        assert!(host.receiver().is_none());
        let texts: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
        assert!(texts.iter().any(|t| t.contains("no bytes to decompress.")), "{texts:?}");
    }

    /// The waiting list (D127.4): a stream thread admits its queued receivers in order, one per tick at most, each
    /// admission starting its no-traffic clock; the exit admits every one still queued, then removes them.
    #[test]
    fn queued_receivers_are_admitted_in_order_one_per_tick() {
        use netdata_agent_rrd::pulse::host_status::{RCV_RUNNING, RECEIVER};
        // the clock's epoch is its first reading
        let start = now_monotonic_usec();
        let (pool, connector) = crate::connector::tests::connector();
        let hosts = Arc::new(Hosts::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
            crate::connector::tests::info("", ""),
        )));
        let first = queued(0xc1, &pool, &hosts, &connector);
        let second = queued(0xc2, &pool, &hosts, &connector);
        let deadline = Instant::now() + Duration::from_secs(10);
        while [&first.0, &second.0].iter().any(|h| h.pulse_state() & RECEIVER != RCV_RUNNING) {
            assert!(Instant::now() < deadline, "not admitted");
            std::thread::sleep(Duration::from_millis(5));
        }
        let at = |slot: &Arc<ReceiverSlot>| slot.last_traffic_ut.load(std::sync::atomic::Ordering::Relaxed);
        let (one, two) = (at(&first.1), at(&second.1));
        assert!(one > start && two >= one + 100_000, "admitted at {one} and {two}, queued after {start}");
        // queued at the exit: admitted without the checks, then removed
        let third = queued(0xc3, &pool, &hosts, &connector);
        let fourth = queued(0xc4, &pool, &hosts, &connector);
        pool.stop().unwrap();
        for (host, slot, _) in [&first, &second, &third, &fourth] {
            assert!(host.receiver().is_none() && at(slot) > start, "{}", host.machine_guid());
        }
    }

    #[test]
    fn a_unix_child_gets_cs_warnings_for_the_tcp_options() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let (h, peer) = (host(), Peer::default());
        let ((), records) = netdata_agent_log::capture(|| {
            let mut initialized = false;
            reconcile_keepalive(a.as_fd(), &h, &peer, &keepalive(), 1, &mut initialized);
        });
        let messages: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&a);
        assert_eq!(
            messages,
            ["TCP_KEEPIDLE", "TCP_KEEPINTVL", "TCP_KEEPCNT"]
                .map(|o| format!("STREAM RCV 'child' [from []:]: cannot set {o} on socket {fd}"))
        );
    }

    /// `stream_receiver_reconcile_keepalive()` with the keepalive off: SO_KEEPALIVE, which the web server turned on,
    /// goes off and no TCP option is set; a configured policy is not reconciled again. Automatic: the handshake's
    /// update every until a chart reports one, applied again only when the observed minimum changes.
    #[test]
    fn keepalive_off_turns_it_off_and_only_a_new_minimum_reapplies() {
        use nix::sys::socket::{getsockopt, setsockopt, sockopt};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        // as the web server accepts it (static-threaded.c:48-50, daemon/src/server.rs:494)
        setsockopt(&server, sockopt::KeepAlive, &true).unwrap();
        let idle = || getsockopt(&server, sockopt::TcpKeepIdle).unwrap();
        let kernel_idle = idle();
        let on = || getsockopt(&server, sockopt::KeepAlive).unwrap();
        let (h, peer) = (host(), Peer::default());
        let off = Keepalive { enabled: false, automatic: false, idle_s: 0 };
        let mut initialized = false;
        reconcile_keepalive(server.as_fd(), &h, &peer, &off, 5, &mut initialized);
        assert_eq!((initialized, on(), idle()), (true, false, kernel_idle));
        setsockopt(&server, sockopt::KeepAlive, &true).unwrap();
        h.observe_receiver_update_every(200);
        reconcile_keepalive(server.as_fd(), &h, &peer, &off, 5, &mut initialized);
        assert!(on(), "a configured policy is applied once");
        let h = host();
        let mut initialized = false;
        reconcile_keepalive(server.as_fd(), &h, &peer, &keepalive(), 121, &mut initialized);
        assert_eq!((on(), idle()), (true, 61));
        setsockopt(&server, sockopt::TcpKeepIdle, &77).unwrap();
        reconcile_keepalive(server.as_fd(), &h, &peer, &keepalive(), 121, &mut initialized);
        assert_eq!(idle(), 77, "the same minimum: not applied again");
        h.observe_receiver_update_every(300);
        reconcile_keepalive(server.as_fd(), &h, &peer, &keepalive(), 121, &mut initialized);
        assert_eq!(idle(), 150);
    }

    /// A failed SO_KEEPALIVE is C's one warning, and the TCP options are not tried.
    #[test]
    fn a_failed_so_keepalive_ends_the_reconcile() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
        let (h, peer) = (host(), Peer::default());
        let off = Keepalive { enabled: false, automatic: false, idle_s: 0 };
        for (policy, word) in [(keepalive(), "enable"), (off, "disable")] {
            let mut initialized = false;
            let ((), records) = netdata_agent_log::capture(|| {
                reconcile_keepalive(file.as_fd(), &h, &peer, &policy, 1, &mut initialized);
            });
            assert!(initialized);
            assert_eq!(
                texts(records),
                [format!("STREAM RCV 'child' [from []:]: cannot {word} SO_KEEPALIVE on socket {fd}")]
            );
        }
    }

    /// `stream_receiver_move_to_running_unsafe()`: the admission enlarges both socket buffers (as a socket asked for
    /// `LARGE_SOCK_SIZE` directly), applies the keepalive, and starts the receiver waiting for replication when
    /// `[db] enable replication` is on, else running.
    #[test]
    fn the_move_to_running_sets_the_socket_and_the_state_up_as_cs() {
        use netdata_agent_rrd::pulse::host_status::{RCV_REPLICATION_WAIT, RCV_RUNNING, RECEIVER};
        use nix::sys::socket::{getsockopt, sockopt};
        let (control, _control_peer) = mio::net::UnixStream::pair().unwrap();
        let control = socket2::SockRef::from(&control);
        control.set_recv_buffer_size(crate::sock::LARGE_SOCK_SIZE).unwrap();
        control.set_send_buffer_size(crate::sock::LARGE_SOCK_SIZE).unwrap();
        let want = (control.recv_buffer_size().unwrap(), control.send_buffer_size().unwrap());
        let buffers = |c: &Conn| {
            let r = socket2::SockRef::from(c);
            (r.recv_buffer_size().unwrap(), r.send_buffer_size().unwrap())
        };
        let (mut s, pool, hosts, connector) = stepper();
        let mut peers = Vec::new();
        for (i, (n, wait, state)) in [(0xda, true, RCV_REPLICATION_WAIT), (0xdb, false, RCV_RUNNING)]
            .into_iter()
            .enumerate()
        {
            let (mut attached, host, slot, theirs) = child(n, crate::caps::V2, &pool, &hosts, &connector);
            peers.push(theirs);
            attached.replication_wait = wait;
            // owed before the child has a buffer: C's send_to_child() takes nothing
            assert_eq!(slot.send_to_child(b"FUNCTION_PAYLOAD x\n", Traffic::Functions), 0);
            assert_ne!(buffers(attached.stream.socket().unwrap()), want, "smaller by default");
            s.with(|w, cx| w.attach(cx, attached));
            let a = &s.worker().children[i].as_ref().unwrap().attached;
            let sock = a.stream.socket().unwrap();
            assert_eq!(
                (buffers(sock), getsockopt(sock, sockopt::KeepAlive).unwrap(), a.keepalive_initialized),
                (want, true, true)
            );
            assert_eq!(host.pulse_state() & RECEIVER, state);
        }
        for mut peer in peers {
            let mut buf = [0; 64];
            let read = std::io::Read::read(&mut peer, &mut buf);
            assert_eq!(read.map_err(|e| e.kind()), Err(std::io::ErrorKind::WouldBlock), "nothing reached the child");
        }
    }

    /// What a child's end can read now, without waiting.
    fn read_all(peer: &mut mio::net::UnixStream) -> Vec<u8> {
        use std::io::Read;
        let (mut out, mut buf) = (Vec::new(), [0; 4096]);
        while let Ok(n @ 1..) = peer.read(&mut buf) {
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    /// RECEIVER_POLLOUT (D164.B2): a line owed to an attached child by a step of its own thread goes out when the step
    /// ends (C's inline opcode), not at the tick, and not for another connection's POLLOUT. A POLLOUT for a connection
    /// no longer here writes nothing; posted, it is C's "ignored" record (`stream-thread.c:71-75`); inline, C runs it
    /// while the receiver is in its thread, so it cannot miss and logs nothing.
    #[test]
    fn an_owed_line_reaches_the_child_when_the_step_ends() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, mut peer) = child(0xd5, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        read_all(&mut peer);
        let gone = Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        s.with(|w, cx| {
            slot.send_to_child(b"FUNCTION_CANCEL abc\n", Traffic::Functions);
            assert_eq!(read_all(&mut peer), b"", "the step has not ended");
            let ((), records) = netdata_agent_log::capture(|| w.child_ops(cx, &Arc::downgrade(&gone), receiver_op::POLLOUT, false));
            assert_eq!(read_all(&mut peer), b"", "another connection's POLLOUT");
            assert_eq!(texts(records), Vec::<String>::new());
            w.drain_inline(cx);
        });
        assert_eq!(read_all(&mut peer), b"FUNCTION_CANCEL abc\n");
        let ((), records) =
            netdata_agent_log::capture(|| s.with(|w, cx| w.child_ops(cx, &Arc::downgrade(&gone), receiver_op::POLLOUT, true)));
        assert_eq!(texts(records), ["STREAM THREAD[0]: OPCODE 2 ignored."]);
        assert_eq!(read_all(&mut peer), b"");
        drop(host);
    }

    /// A line another thread owes a child posts RECEIVER_POLLOUT to the child's thread, which writes it when the
    /// message arrives (`stream_receiver_send_opcode()` off the owner thread).
    #[test]
    fn a_posted_pollout_writes_what_another_thread_owed() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, mut peer) = child(0xe2, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        read_all(&mut peer);
        std::thread::scope(|t| {
            t.spawn(|| slot.send_to_child(b"FUNCTION_CANCEL abc\n", Traffic::Functions));
        });
        assert_eq!(read_all(&mut peer), b"", "posted, not written by the caller");
        s.with(|w, cx| {
            let ops = Arc::new(AtomicU32::new(receiver_op::POLLOUT));
            netdata_agent_evloop::Worker::message(w, cx, StreamMsg::ChildOps(Arc::downgrade(&slot), ops));
        });
        assert_eq!(read_all(&mut peer), b"FUNCTION_CANCEL abc\n");
        drop(host);
    }

    /// A wake for a child whose thread is not running is C's ERR when `stream_thread_by_slot_id()` finds no thread
    /// (`stream-thread.c:103-107`), and the line stays owed.
    #[test]
    fn a_wake_for_a_thread_not_running_is_cs_err() {
        let (mut s, pool, hosts, connector) = stepper();
        let (mut attached, host, slot, _peer) = child(0xe4, crate::caps::V2, &pool, &hosts, &connector);
        attached.thread = 9;
        s.with(|w, cx| w.attach(cx, attached));
        let (sent, records) = netdata_agent_log::capture(|| slot.send_to_child(b"x\n", Traffic::Functions));
        assert_eq!(sent, 2, "queued, the wake lost");
        let lost = |op: u32| {
            format!(
                "STREAM RCV '{}' [from [127.0.0.1]:1]: the opcode ({op}) message cannot be verified. Ignoring it.",
                host.hostname()
            )
        };
        assert_eq!(texts(records), [lost(receiver_op::POLLOUT)]);
        // a refusal's opcode is lost the same way, with its own number
        let free = slot.buffer().as_ref().unwrap().stats().bytes_available;
        let (sent, records) = netdata_agent_log::capture(|| slot.send_to_child(&vec![b'x'; free], Traffic::Functions));
        assert_eq!((sent, texts(records)), (-1, vec![lost(receiver_op::BUFFER_OVERFLOW)]));
    }

    /// D166: an add of exactly the buffer's free space is refused, -1 through the child's wire (a call then answers
    /// 503), and the overflow opcode restarts the connection with C's record of the buffer's sizes and NOT
    /// SUFFICIENT SEND BUFFER (`stream-receiver.c:364-378`).
    #[test]
    fn an_exact_fit_restarts_the_connection_as_cs() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, _peer) = child(0xe5, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        let fill = INITIAL_MAX_SIZE - 4096;
        assert!(slot.buffer().as_mut().unwrap().add(&vec![b'x'; fill], fill, Traffic::Functions, true));
        let wire = ChildWire { slot: Arc::downgrade(&slot) };
        assert_eq!(ingest::functions::Wire::send(&wire, &[b'y'; 4096], Traffic::Functions), -1);
        let ops = Arc::new(AtomicU32::new(receiver_op::BUFFER_OVERFLOW));
        let ((), records) = netdata_agent_log::capture(|| {
            s.with(|w, cx| netdata_agent_evloop::Worker::message(w, cx, StreamMsg::ChildOps(Arc::downgrade(&slot), ops)))
        });
        let texts = texts(records);
        assert_eq!(
            texts[0],
            format!(
                "STREAM RCV[0] '{}' [from []:]: send buffer is full (buffer size {INITIAL_MAX_SIZE}, max \
                 {INITIAL_MAX_SIZE}, used {fill}, available 4096). Restarting connection.",
                host.hostname()
            )
        );
        assert!(texts[1].contains("reason=\"DISCONNECTED NOT SUFFICIENT SEND BUFFER\""), "{texts:?}");
        assert!(host.receiver().is_none());
        assert_eq!(slot.send_to_child(b"x\n", Traffic::Functions), 0, "the buffer went with the connection");
    }

    /// One message carries a receiver's opcodes (C's message slot): POLLOUT writes first, then the overflow restarts
    /// the connection, its record reading the buffer after the write (`stream-thread.c:51-68`).
    #[test]
    fn coalesced_opcodes_write_before_the_overflow_restarts() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, mut peer) = child(0xe6, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        assert!(slot.buffer().as_mut().unwrap().add(&[b'z'; 1000], 1000, Traffic::Functions, true));
        let ops = Arc::new(AtomicU32::new(receiver_op::POLLOUT | receiver_op::BUFFER_OVERFLOW));
        let ((), records) = netdata_agent_log::capture(|| {
            s.with(|w, cx| netdata_agent_evloop::Worker::message(w, cx, StreamMsg::ChildOps(Arc::downgrade(&slot), ops)))
        });
        assert_eq!(read_all(&mut peer), [b'z'; 1000]);
        assert!(
            texts(records)[0].ends_with(&format!(
                "send buffer is full (buffer size 16384, max {INITIAL_MAX_SIZE}, used 0, available \
                 {INITIAL_MAX_SIZE}). Restarting connection."
            )),
            "after the write"
        );
        assert!(host.receiver().is_none());
    }

    /// A backfilled chart's request the buffer refuses (`backfill_callback()` on the backfill thread,
    /// `pluginsd_replication.c:25-34`): the send's record and the backfill's, the connection kept until the overflow
    /// opcode restarts it; an opcode posted for it after that is C's "ignored" DEBUG with its number.
    #[test]
    fn a_refused_backfilled_request_restarts_the_connection() {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, mut peer) = child(0xe7, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        peer.write_all(b"CHART 'x.c' '' 't' 'u' 'f' 'x.ctx' line 1 1 '' p m\nDIMENSION 'd' '' absolute 1 1 ''\n").unwrap();
        s.turn(Duration::from_millis(50));
        let chart = host.charts().find("x.c", true).expect("defined");
        let line = "REPLAY_CHART \"x.c\" \"true\" 0 0\n";
        let fill = INITIAL_MAX_SIZE - line.len();
        assert!(slot.buffer().as_mut().unwrap().add(&vec![b'x'; fill], fill, Traffic::Functions, true));
        let request = ingest::ReplayRequest { chart, first_entry_child: 0, last_entry_child: 0, child_wall_clock_time: 0 };
        let ((), records) = netdata_agent_log::capture(|| {
            s.with(|w, cx| netdata_agent_evloop::Worker::message(w, cx, StreamMsg::Replay(Arc::downgrade(&slot), request)))
        });
        assert_eq!(
            texts(records),
            [
                format!(
                    "STREAM SND REPLAY ERROR: 'host:{}/chart:x.c' failed to send replication request to child (error -1)",
                    host.hostname()
                ),
                format!(
                    "PLUGINSD REPLAY ERROR: 'host:{}' failed to initiate replication for 'chart:x.c' - replication may \
                     not proceed for this instance.",
                    host.hostname()
                ),
            ]
        );
        assert!(host.receiver().is_some(), "until the overflow opcode");
        let mut overflow = || {
            let ops = Arc::new(AtomicU32::new(receiver_op::BUFFER_OVERFLOW));
            netdata_agent_log::capture(|| {
                s.with(|w, cx| {
                    netdata_agent_evloop::Worker::message(w, cx, StreamMsg::ChildOps(Arc::downgrade(&slot), ops))
                })
            })
            .1
        };
        let texts_now = texts(overflow());
        assert!(texts_now[0].contains("send buffer is full"), "{texts_now:?}");
        assert!(texts_now[1].contains("NOT SUFFICIENT SEND BUFFER"), "{texts_now:?}");
        assert_eq!(texts(overflow()), ["STREAM THREAD[0]: OPCODE 8 ignored."]);
    }

    /// C's writes are the ring's contiguous chunks: a queue that wraps goes out in two writes, in order, both counted
    /// (`stream_receiver_send_data()`'s loop).
    #[test]
    fn a_wrapped_buffer_is_written_in_two_chunks() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, _host, slot, mut peer) = child(0xe8, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        {
            let mut buffer = slot.buffer();
            let b = buffer.as_mut().unwrap();
            assert!(b.add(&[0; 10_000], 10_000, Traffic::Functions, true));
            b.del(10_000, 1);
        }
        let data: Vec<u8> = (0..10_000).map(|i| (i % 251) as u8).collect();
        assert_eq!(slot.send_to_child(&data, Traffic::Functions), 10_000);
        assert_eq!(slot.buffer().as_ref().unwrap().stats().bytes_outstanding, 16_384 - 10_000, "wrapped");
        // the posted POLLOUT's write
        s.with(|w, cx| assert!(w.send_data(cx, 0, Writer::Opcode)));
        assert_eq!(read_all(&mut peer), data);
        let stats = *slot.buffer().as_ref().unwrap().stats();
        assert_eq!((stats.sends, stats.bytes_sent, stats.bytes_outstanding), (3, 20_000, 0));
    }

    /// A drained buffer shrinks its grown ring back to 16 KiB, at most every 5 minutes, and keeps its maximum
    /// (`stream_circular_buffer_recreate_timed_unsafe()` after a send that drained it).
    #[test]
    fn a_drained_buffer_shrinks_its_ring() {
        // the first recreate is due once the monotonic clock passed 5 minutes
        if now_monotonic_usec() < 300_000_000 {
            eprintln!("skipped: the monotonic clock is under 5 minutes, so no recreate is due yet");
            return;
        }
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, _host, slot, mut peer) = child(0xe9, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        assert_eq!(slot.send_to_child(&[b'a'; 20_000], Traffic::Functions), 20_000);
        assert_eq!(slot.buffer().as_ref().unwrap().stats().bytes_size, 32_768);
        // the posted POLLOUT's write
        s.with(|w, cx| assert!(w.send_data(cx, 0, Writer::Opcode)));
        assert_eq!(read_all(&mut peer).len(), 20_000);
        assert_eq!(slot.send_to_child(b"b\n", Traffic::Functions), 2);
        let stats = *slot.buffer().as_ref().unwrap().stats();
        assert_eq!((stats.bytes_size, stats.bytes_max_size, stats.recreates), (16_384, INITIAL_MAX_SIZE, 1));
    }

    /// C's inline POLLOUT writes each add that finds the buffer empty inside the add (D168.1): a read with two REPLAY_END
    /// lines is two writes, each line's request out before the next line is parsed.
    #[test]
    fn each_request_of_a_read_is_its_own_write() {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, _host, slot, mut peer) = child(0xeb, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        peer.write_all(b"CHART 'x.c' '' 't' 'u' 'f' 'x.ctx' line 1 1 '' p m\nDIMENSION 'd' '' absolute 1 1 ''\n").unwrap();
        s.turn(Duration::from_millis(50));
        let rend = "RBEGIN 'x.c'\nREND 1 0 0 false 0 0 0x6553f100\n";
        peer.write_all(format!("{rend}{rend}").as_bytes()).unwrap();
        let _ = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        let line = "REPLAY_CHART \"x.c\" \"true\" 0 0\n";
        assert_eq!(String::from_utf8(read_all(&mut peer)).unwrap(), format!("{line}{line}"));
        let stats = *slot.buffer().as_ref().unwrap().stats();
        assert_eq!((stats.sends, stats.bytes_sent), (2, 2 * line.len()));
    }

    /// An opcode's failed write is C's record and nothing more (`process_opcodes_and_enable_removal` false, D168.3):
    /// the child stays until the next event removes it.
    #[test]
    fn an_opcodes_failed_write_leaves_the_removal_to_the_next_event() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, peer) = child(0xec, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        drop(peer);
        std::thread::scope(|t| {
            t.spawn(|| slot.send_to_child(b"FUNCTION_CANCEL abc\n", Traffic::Functions));
        });
        let ops = Arc::new(AtomicU32::new(receiver_op::POLLOUT));
        let ((), records) = netdata_agent_log::capture(|| {
            s.with(|w, cx| netdata_agent_evloop::Worker::message(w, cx, StreamMsg::ChildOps(Arc::downgrade(&slot), ops)))
        });
        assert_eq!(
            texts(records),
            [format!(
                "STREAM RCV[0] '{}' [from []:]: DISCONNECTED SOCKET WRITE FAILED (-1, on fd {}) - closing receiver \
                 connection - we have sent 0 bytes in 0 operations.",
                host.hostname(),
                s.worker().children[0].as_ref().map(|c| raw_fd(&c.attached.stream)).unwrap()
            )]
        );
        assert!(host.receiver().is_some(), "not removed by the opcode");
        let _ = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.receiver().is_none(), "removed by the next event");
    }

    /// A stream thread's mailbox that keeps what it is sent, so a unit sees what a waker posts.
    struct Mailbox(Arc<Mutex<Vec<StreamMsg>>>);

    impl netdata_agent_evloop::Worker for Mailbox {
        type Msg = StreamMsg;
        fn event(&mut self, _: &mut Context<'_>, _: &Event) {}
        fn message(&mut self, _: &mut Context<'_>, msg: StreamMsg) {
            self.0.lock().unwrap().push(msg);
        }
    }

    /// A pool whose one thread is a mailbox: a child attached with it posts its opcodes there.
    fn mailbox() -> (netdata_agent_evloop::Pool<StreamMsg>, Arc<Mutex<Vec<StreamMsg>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let pool = netdata_agent_evloop::Pool::spawn(1, 256 * 1024, |i| format!("MAILBOX[{i}]"), move |_| {
            Mailbox(Arc::clone(&kept))
        })
        .unwrap();
        (pool, seen)
    }

    /// What the mailbox holds once it has `n` messages (and a little after, for any extra), taken.
    fn posted(seen: &Arc<Mutex<Vec<StreamMsg>>>, n: usize) -> Vec<StreamMsg> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while seen.lock().unwrap().len() < n && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        std::mem::take(&mut *seen.lock().unwrap())
    }

    /// Hands `messages` to the stream thread, returning what it logged.
    fn deliver(
        s: &mut netdata_agent_evloop::testing::Stepper<StreamWorker>,
        messages: Vec<StreamMsg>,
    ) -> Vec<String> {
        let ((), records) = netdata_agent_log::capture(|| {
            s.with(|w, cx| messages.into_iter().for_each(|m| netdata_agent_evloop::Worker::message(w, cx, m)))
        });
        texts(records)
    }

    /// The waker (C's message slot, `stream-thread.c:123-170`) posts one message per wake from another thread,
    /// gathers a refusal into a pending one, and is re-armed once the thread takes the bits; a pure POLLOUT writes and
    /// logs nothing (R70).
    #[test]
    fn the_waker_posts_one_message_per_wake_and_rearms() {
        let (mut s, _pool, hosts, connector) = stepper();
        let (pool, seen) = mailbox();
        let (attached, host, slot, mut peer) = child(0xf1, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        read_all(&mut peer);
        let away = |bytes: &[u8]| {
            std::thread::scope(|t| t.spawn(|| slot.send_to_child(bytes, Traffic::Functions)).join().unwrap())
        };
        assert_eq!((away(b"a\n"), away(b"b\n")), (2, 2));
        let messages = posted(&seen, 1);
        assert_eq!(messages.len(), 1, "one message for the POLLOUT");
        assert_eq!(deliver(&mut s, messages), Vec::<String>::new());
        assert_eq!(read_all(&mut peer), b"a\nb\n");
        assert_eq!(away(b"c\n"), 2);
        let free = slot.buffer().as_ref().unwrap().stats().bytes_available;
        assert_eq!(away(&vec![b'x'; free]), -1);
        let messages = posted(&seen, 1);
        assert_eq!(messages.len(), 1, "POLLOUT and BUFFER_OVERFLOW in one message");
        let StreamMsg::ChildOps(_, ops) = &messages[0] else { panic!("{messages:?}") };
        assert_eq!(ops.load(Ordering::Acquire), receiver_op::POLLOUT | receiver_op::BUFFER_OVERFLOW);
        let texts = deliver(&mut s, messages);
        assert_eq!(read_all(&mut peer), b"c\n");
        assert!(texts[0].contains("send buffer is full"), "{texts:?}");
        assert!(host.receiver().is_none());
    }

    /// On the child's own thread (a parser's or a backfill's send) a refusal is still posted, never inline
    /// (`stream-thread.c:111` runs only an exact POLLOUT inline), and the posted message restarts the connection
    /// (R70).
    #[test]
    fn a_refusal_on_the_childs_own_thread_is_posted() {
        let (mut s, _pool, hosts, connector) = stepper();
        let (pool, seen) = mailbox();
        let (attached, host, slot, _peer) = child(0xf2, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        let free = slot.buffer().as_ref().unwrap().stats().bytes_available;
        s.with(|w, cx| {
            assert_eq!(slot.send_to_child(&vec![b'x'; free], Traffic::Replication), -1);
            w.drain_inline(cx);
        });
        assert!(host.receiver().is_some());
        let messages = posted(&seen, 1);
        assert_eq!(messages.len(), 1, "BUFFER_OVERFLOW is posted");
        let texts = deliver(&mut s, messages);
        assert!(texts[0].contains("send buffer is full"), "{texts:?}");
        assert!(host.receiver().is_none());
    }

    /// A POLLOUT whose write fails ends the handling there (`stream-thread.c:58-60` returns), so the overflow gathered
    /// with it is not handled and the child stays until the next event (R70).
    #[test]
    fn a_failed_write_skips_the_overflow_it_was_gathered_with() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, peer) = child(0xf3, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        drop(peer);
        assert!(slot.buffer().as_mut().unwrap().add(b"x\n", 2, Traffic::Functions, true));
        let ops = Arc::new(AtomicU32::new(receiver_op::POLLOUT | receiver_op::BUFFER_OVERFLOW));
        let texts = deliver(&mut s, vec![StreamMsg::ChildOps(Arc::downgrade(&slot), ops)]);
        assert_eq!(texts.len(), 1, "{texts:?}");
        assert!(texts[0].contains("DISCONNECTED SOCKET WRITE FAILED"), "{texts:?}");
        assert!(host.receiver().is_some());
    }

    /// The periodic check re-arms the output while the buffer holds anything (`stream-receiver.c:1224`), so what no
    /// POLLOUT asked to write goes out then, and not before (R70-5).
    #[test]
    fn the_periodic_check_writes_what_is_left_in_the_buffer() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, _host, slot, mut peer) = child(0xf4, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        assert!(slot.buffer().as_mut().unwrap().add(b"left\n", 5, Traffic::Functions, true));
        s.with(|w, cx| assert!(w.send_data(cx, 0, Writer::Poll)));
        assert_eq!(read_all(&mut peer), b"", "no POLLOUT asked for it");
        let now = slot.last_traffic_ut.load(Ordering::Relaxed);
        s.with(|w, cx| w.check_all(cx, now));
        assert_eq!(read_all(&mut peer), b"left\n");
    }

    /// A read's end writes only what a POLLOUT asked for (C's `rpt->thread.wanted & ND_POLL_WRITE`,
    /// `stream-receiver.c:779-787`): a line another thread queued waits for its own posted POLLOUT (R70-5).
    #[test]
    fn a_read_leaves_what_another_thread_queued_to_its_pollout() {
        use std::io::Write;
        let (mut s, _pool, hosts, connector) = stepper();
        let (pool, seen) = mailbox();
        let (attached, _host, slot, mut peer) = child(0xf8, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        std::thread::scope(|t| {
            t.spawn(|| slot.send_to_child(b"FUNCTION_CANCEL abc\n", Traffic::Functions));
        });
        peer.write_all(b"CHART 'q.r' '' t u f c line 1 1\n").unwrap();
        s.turn(Duration::from_millis(50));
        assert_eq!(read_all(&mut peer), b"", "the read's end leaves it");
        deliver(&mut s, posted(&seen, 1));
        assert_eq!(read_all(&mut peer), b"FUNCTION_CANCEL abc\n");
    }

    /// The thread's exit drops a receiver's opcodes with their bits, so a later wake posts again (and, the thread gone,
    /// logs C's "cannot be verified") instead of finding its message pending (R70-6).
    #[test]
    fn the_exit_drops_a_receivers_opcodes_with_their_bits() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, _host, slot, _peer) = child(0xf9, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        let ops = Arc::new(AtomicU32::new(receiver_op::POLLOUT));
        s.with(|w, cx| {
            let msg = StreamMsg::ChildOps(Arc::downgrade(&slot), Arc::clone(&ops));
            netdata_agent_evloop::Worker::exit_message(w, cx, msg);
        });
        assert_eq!(ops.load(Ordering::Acquire), 0);
    }

    /// A child that sends a REND whose REPLAY_CHART cannot be written (its peer stopped reading): the error records of
    /// the turn that reads it.
    fn an_inline_write_failure() -> (Vec<netdata_agent_log::Captured>, Arc<Host>) {
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _slot, mut peer) = child(0xf5, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        peer.write_all(b"CHART 'x.c' '' 't' 'u' 'f' 'x.ctx' line 1 1 '' p m\nDIMENSION 'd' '' absolute 1 1 ''\n").unwrap();
        s.turn(Duration::from_millis(50));
        peer.write_all(b"RBEGIN 'x.c'\nREND 1 0 0 false 0 0 0x6553f100\n").unwrap();
        peer.shutdown(Shutdown::Read).unwrap();
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        (records.into_iter().filter(|r| r.priority == Priority::Err).collect(), host)
    }

    /// C's sequence when a line's write fails: logged at the line (under its line, as C's inline write runs inside the
    /// add while the parser's line is set, `pluginsd_parser.h:251-272`), logged again by the write after the read
    /// (`stream_receiver_dequeue_senders()`), then READ FAILED removes the child once (R70, R70-1).
    #[test]
    fn an_inline_write_failure_is_logged_at_the_line_and_after_the_read() {
        let (errs, host) = an_inline_write_failure();
        let msgs: Vec<String> = errs.iter().map(|r| r.message.clone().unwrap_or_default()).collect();
        assert_eq!(msgs.len(), 4, "{msgs:?}");
        assert!(msgs[0].contains("DISCONNECTED SOCKET WRITE FAILED (-1, on fd "), "{msgs:?}");
        assert!(msgs[0].ends_with("we have sent 0 bytes in 0 operations."), "{msgs:?}");
        assert!(msgs[1].contains("DISCONNECTED SOCKET WRITE FAILED (-1, on fd "), "{msgs:?}");
        assert!(msgs[2].contains("DISCONNECTED SOCKET READ FAILED (fd "), "{msgs:?}");
        assert!(msgs[3].contains("reason=\"DISCONNECTED SOCKET READ FAILED\""), "{msgs:?}");
        assert!(host.receiver().is_none());
        let request = |r: &netdata_agent_log::Captured| {
            r.fields.iter().find(|(f, _)| *f == netdata_agent_log::Field::Request).map(|(_, v)| v.clone())
        };
        assert_eq!(request(&errs[0]).as_deref(), Some("'REND' '1' '0' '0' 'false' '0' '0' '0x6553f100'"));
    }

    /// The write-failure record and the disconnect record read the buffer's counts (`stats->bytes_sent`,
    /// `stats->sends`, `stream-receiver.c:705-709,945-949`), not zeros (R70).
    #[test]
    fn the_records_count_what_the_buffer_sent() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, mut peer) = child(0xf6, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        s.with(|w, cx| {
            assert_eq!(slot.send_to_child(b"abc\n", Traffic::Functions), 4);
            w.drain_inline(cx);
        });
        assert_eq!(read_all(&mut peer), b"abc\n");
        drop(peer);
        let ((), records) = netdata_agent_log::capture(|| {
            s.with(|w, cx| {
                assert_eq!(slot.send_to_child(b"d\n", Traffic::Functions), 2);
                w.drain_inline(cx);
                w.check_all(cx, now_monotonic_usec());
            })
        });
        let texts = texts(records);
        assert!(texts[0].ends_with(" - closing receiver connection - we have sent 4 bytes in 1 operations."), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains(" bytes_out=4 ")), "{texts:?}");
        assert!(host.receiver().is_none());
    }

    /// A REPLAY_CHART the parser cannot send on a live child fails the line (PARSE ERROR,
    /// `pluginsd_replication.c:603`) without C's "send buffer is full" record; the overflow it posted then finds no
    /// receiver ("OPCODE 8 ignored.", `stream-thread.c:71-75`; plan U9, R70).
    #[test]
    fn a_refused_request_in_the_parser_is_a_parse_error() {
        use std::io::Write;
        let (mut s, _pool, hosts, connector) = stepper();
        let (pool, seen) = mailbox();
        let (attached, host, slot, mut peer) = child(0xf7, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        peer.write_all(b"CHART 'x.c' '' 't' 'u' 'f' 'x.ctx' line 1 1 '' p m\nDIMENSION 'd' '' absolute 1 1 ''\n").unwrap();
        s.turn(Duration::from_millis(50));
        // the socket full, so nothing leaves the ring; the ring then one REPLAY_CHART short of full
        s.with(|w, _| {
            let stream = &mut w.children[0].as_mut().unwrap().attached.stream;
            while matches!(stream.write(&[0u8; 65536]), Ok(1..)) {}
        });
        let line = "REPLAY_CHART \"x.c\" \"true\" 0 0\n";
        let fill = INITIAL_MAX_SIZE - line.len();
        assert!(slot.buffer().as_mut().unwrap().add(&vec![b'x'; fill], fill, Traffic::Functions, true));
        peer.write_all(b"RBEGIN 'x.c'\nREND 1 0 0 false 0 0 0x6553f100\n").unwrap();
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        let texts_now = texts(records);
        assert!(host.receiver().is_none(), "{texts_now:?}");
        assert!(
            texts_now.iter().any(|t| t.contains("failed to send replication request to child (error -1)")),
            "{texts_now:?}"
        );
        assert!(texts_now.iter().any(|t| t.contains("reason=\"DISCONNECTED PARSE ERROR\"")), "{texts_now:?}");
        assert!(!texts_now.iter().any(|t| t.contains("send buffer is full")), "{texts_now:?}");
        let messages = posted(&seen, 1);
        assert_eq!(messages.len(), 1, "the overflow is posted");
        assert_eq!(deliver(&mut s, messages), ["STREAM THREAD[0]: OPCODE 8 ignored."]);
    }

    /// The idle-timeout record reads the buffer (`stream-receiver.c:1179-1218`): the bytes and writes it counted, the
    /// first contiguous chunk pending, and the fill of the grown maximum (D166; Rust printed the whole queue over 10
    /// MiB).
    #[test]
    fn the_idle_record_reads_the_buffer() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, _peer) = child(0xea, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        {
            let mut buffer = slot.buffer();
            let b = buffer.as_mut().unwrap();
            let mut add = |n: usize| b.add(&vec![0; n], n, Traffic::Replication, true);
            assert!(add(8_388_608));
            b.del(8_388_608, 1);
            let mut add = |n: usize| b.add(&vec![0; n], n, Traffic::Replication, true);
            assert!(add(4_194_304) && add(6_291_457));
            b.del(10_485_761, 2);
            assert!(b.add(&vec![0; 12_000_000], 12_000_000, Traffic::Replication, true));
        }
        let last = slot.last_traffic_ut.load(Ordering::Relaxed);
        let (_, records) = netdata_agent_log::capture(|| s.with(|w, cx| w.check_all(cx, last + 602_000_000)));
        assert!(host.receiver().is_none());
        let record = texts(records).into_iter().find(|t| t.contains("there was not traffic")).expect("the record");
        let pending = size_to_string(10_485_759, "B", false).unwrap();
        assert!(record.contains(" - we have sent 18874369 bytes in 2 operations, it is idle for "), "{record}");
        assert!(record.ends_with(&format!("and we have {pending} pending to send (buffer is used 57.22%).")), "{record}");
    }

    /// A child's method is called through its own socket: the FUNCTION line and, the child having PROGRESS, a
    /// progress request go down with the full length sent (no "failed to send" record); the child's end answers the
    /// call 503 (`pluginsd_inflight_functions_cleanup()`).
    #[test]
    fn a_childs_method_is_called_through_its_socket() {
        use netdata_agent_nrpc::call::{CallSpec, Calls};
        use std::io::Write;
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _slot, mut peer) =
            child(0xe3, crate::caps::V2 | crate::caps::PROGRESS, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        read_all(&mut peer);
        peer.write_all(b"FUNCTION GLOBAL \"r69\" 10 \"h\" \"top\" \"0x0\" 100 0\n").unwrap();
        assert!(s.turn(Duration::from_millis(50)));
        let (send, answers) = std::sync::mpsc::channel();
        let tx = "5a1e000000004000800000000000e369";
        let called = Calls::process().call(CallSpec {
            owner: Some((host.functions(), "child")),
            cmd: b"r69",
            source: b"src",
            user_access: 0,
            timeout_s: 0,
            wait: false,
            allow_restricted: true,
            call_id: Some(tx.as_bytes()),
            payload: None,
            reply: netdata_agent_nrpc::reply::Reply::new(netdata_agent_nrpc::reply::ContentType::TextPlain),
            done: Some(Box::new(move |reply, code| {
                let _ = send.send((reply, code));
            })),
            progress: None,
            is_cancelled: None,
            tag: None,
        });
        assert_eq!(called.code, 200);
        let ((), records) = netdata_agent_log::capture(|| Calls::process().progress(tx));
        assert_eq!(texts(records), ["Extending function timeout due to PROGRESS update..."]);
        s.with(|w, cx| w.drain_inline(cx));
        assert_eq!(
            String::from_utf8(read_all(&mut peer)).unwrap(),
            format!("FUNCTION {tx} 10 \"r69\" \"0x0\" \"src\"\nFUNCTION_PROGRESS {tx}\n")
        );
        drop(peer);
        let _ = netdata_agent_log::capture(|| {
            for _ in 0..3 {
                s.turn(Duration::from_millis(50));
            }
        });
        let (reply, code) = answers.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(code, 503);
        assert!(String::from_utf8_lossy(&reply.body).contains("exited before responding"));
    }

    /// Once its connection is gone, a child's wire sends nothing and says so with 0 (C's `send_to_child()` without a
    /// buffer).
    #[test]
    fn a_wire_without_its_connection_sends_nothing() {
        let wire = ChildWire { slot: Weak::new() };
        assert_eq!(ingest::functions::Wire::send(&wire, b"x\n", Traffic::Functions), 0);
    }

    /// Keeps the connections handed to a stream thread, untouched: the queue a child waits in until its admission.
    struct Hold(Arc<Mutex<Vec<Attached>>>);

    impl netdata_agent_evloop::Worker for Hold {
        type Msg = StreamMsg;
        fn event(&mut self, _cx: &mut Context<'_>, _event: &Event) {}
        fn message(&mut self, _cx: &mut Context<'_>, msg: StreamMsg) {
            if let StreamMsg::Attach(attached) = msg {
                self.0.lock().unwrap().push(*attached);
            }
        }
    }

    /// `stream_receiver_accept_connection()`: the web worker writes the prompt, then queues the child with its host
    /// waiting (RCV_WAITING) and its socket non-blocking; what the child sends next stays unread until the admission,
    /// which starts it as `[db] enable replication` says (not the key's). The created host's replication period is
    /// capped by its ring: ram rounds 3600 entries up to 4096, every 2 s.
    #[test]
    fn admission_prompts_then_queues_the_child_waiting() {
        use netdata_agent_rrd::pulse::host_status::{RCV_REPLICATION_WAIT, RCV_WAITING, RECEIVER};
        use std::io::{Read, Write};
        const KEY: &str = "11111111-2222-3333-4444-555555555555";
        const GUID: &str = "5a1e0000-0000-4000-8000-0000000000e1";
        let held = Arc::new(Mutex::new(Vec::new()));
        let hold = {
            let held = Arc::clone(&held);
            netdata_agent_evloop::Pool::spawn(1, 256 * 1024, |i| format!("HOLD[{i}]"), move |_| {
                Hold(Arc::clone(&held))
            })
            .unwrap()
        };
        let (_pool, connector) = crate::connector::tests::connector();
        let hosts = Arc::new(Hosts::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
            crate::connector::tests::info("", ""),
        )));
        let mut conf = StreamConf::default();
        conf.config.load_bytes(
            format!("[{KEY}]\n  enabled = yes\n  enable replication = no\n").as_bytes(),
            "stream.conf",
            false,
            None,
        );
        let defaults = Defaults {
            db_mode: "ram".into(),
            history: 3600,
            health_enabled: false,
            update_every: 1,
            page_size: 4096,
            gap_when_lost_iterations_above: 3,
        };
        let receivers = Receivers::new(
            conf,
            Arc::clone(&hosts),
            Arc::new(Mutex::new(Pins::new(1))),
            defaults,
            hold.handle(),
            connector,
        );
        let query = format!("key={KEY}&hostname=c1&machine_guid={GUID}&ver=2&update_every=2");
        let PreAdmission::Proceed(pending) = receivers.pre_admit(query.as_bytes(), None, "127.0.0.1", "1") else {
            panic!("refused");
        };
        let (ours, mut theirs) = mio::net::UnixStream::pair().unwrap();
        assert!(receivers.admit(*pending, Link::Plain(Conn::Unix(ours))));
        let deadline = Instant::now() + Duration::from_secs(5);
        while held.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "not queued");
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut prompt = [0; 128];
        let n = theirs.read(&mut prompt).unwrap();
        assert_eq!(&prompt[..n], crate::caps::PROMPT_V2.as_bytes());
        let host = hosts.find_by_guid(GUID).unwrap();
        let attached = held.lock().unwrap().pop().unwrap();
        let flags = nix::fcntl::fcntl(attached.stream.socket().unwrap(), nix::fcntl::FcntlArg::F_GETFL).unwrap();
        let info = host.info();
        assert_eq!(
            (
                host.pulse_state() & RECEIVER,
                nix::fcntl::OFlag::from_bits_truncate(flags).contains(nix::fcntl::OFlag::O_NONBLOCK),
                attached.replication_wait,
                info.replication_enabled,
                info.replication_period,
            ),
            (RCV_WAITING, true, true, false, 8192)
        );
        theirs.write_all(b"CHART 'q.r' '' t u f c line 1 1\n").unwrap();
        assert!(host.charts().find("q.r", true).is_none());
        let worker = StreamWorker::new(Arc::new(Mutex::new(Pins::new(1))), 1);
        let mut s = netdata_agent_evloop::testing::Stepper::new(0, worker).unwrap();
        s.with(|w, cx| w.attach(cx, attached));
        assert_eq!(host.pulse_state() & RECEIVER, RCV_REPLICATION_WAIT);
        assert!(s.turn(Duration::from_millis(50)));
        assert!(host.charts().find("q.r", true).is_some());
        hold.stop().unwrap();
    }

    // ---- 10d probe / poll-error / cadence units (scratch) ----

    fn with_stream(mut attached: Attached, stream: Conn) -> Attached {
        attached.stream = Link::Plain(stream);
        attached
    }

    fn tcp_pair() -> (std::net::TcpStream, mio::net::TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        server.set_nonblocking(true).unwrap();
        (client, mio::net::TcpStream::from_std(server))
    }

    fn unconnected_tcp() -> mio::net::TcpStream {
        let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        s.set_nonblocking(true).unwrap();
        mio::net::TcpStream::from_std(s.into())
    }

    #[test]
    fn the_probe_takes_a_reset_for_the_remote_closing() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _, _unix) = child(0xe1, crate::caps::V2, &pool, &hosts, &connector);
        let (client, server) = tcp_pair();
        s.with(|w, cx| w.attach(cx, with_stream(attached, Conn::Tcp(server))));
        socket2::SockRef::from(&client).set_linger(Some(Duration::ZERO)).unwrap();
        drop(client);
        let (_, records) = netdata_agent_log::capture(|| s.with(|w, cx| w.check_all(cx, now_monotonic_usec())));
        assert!(host.receiver().is_none());
        let texts = texts(records);
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert_eq!(texts[0], "STREAM RCV[0] 'child' [from ]: socket closed by remote - closing connection");
        assert!(texts[1].contains("reason=\"DISCONNECTED SOCKET CLOSED BY REMOTE END\""), "{texts:?}");
    }

    #[test]
    fn the_probe_takes_other_errors_for_a_socket_error() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _, _unix) = child(0xe2, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, with_stream(attached, Conn::Tcp(unconnected_tcp()))));
        let (_, records) = netdata_agent_log::capture(|| s.with(|w, cx| w.check_all(cx, now_monotonic_usec())));
        assert!(host.receiver().is_none());
        let texts = texts(records);
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert_eq!(
            texts[0],
            "STREAM RCV[0] 'child' [from ]: socket error detected: Transport endpoint is not connected - closing connection"
        );
        assert!(texts[1].contains("reason=\"DISCONNECT SOCKET ERROR\""), "{texts:?}");
    }

    #[test]
    fn a_poll_error_logs_so_error_and_the_keepalive() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, slot, theirs) = child(0xe3, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| {
            w.attach(cx, attached);
            assert_eq!(slot.send_to_child(b"x\n", Traffic::Functions), 2);
            w.drain_inline(cx);
        });
        drop(theirs);
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.receiver().is_none());
        let all: Vec<_> = records.iter().map(|r| (r.errno, r.message.clone().unwrap_or_default())).collect();
        assert_eq!(
            all[0],
            (
                104,
                "STREAM RCV[0] 'child' [from []:]: DISCONNECTED SOCKET CLOSED BY REMOTE END - closing connection; \
                 SO_ERROR=104 (Connection reset by peer); TCP keepalive: enabled policy=automatic idle=30s interval=10s \
                 probes=3"
                    .to_string()
            ),
            "{all:?}"
        );
        assert_eq!(all.len(), 2, "{all:?}");
    }

    #[test]
    fn a_poll_error_without_a_hangup_is_a_socket_error() {
        use nix::sys::socket::{TimestampingFlag, setsockopt, sockopt};
        let (mut s, pool, hosts, connector) = stepper();
        let (mut attached, host, slot, _unix) = child(0xe4, crate::caps::V2, &pool, &hosts, &connector);
        attached.keepalive = Keepalive { enabled: false, ..keepalive() };
        let (_client, server) = tcp_pair();
        setsockopt(
            &server,
            sockopt::Timestamping,
            &(TimestampingFlag::SOF_TIMESTAMPING_SOFTWARE | TimestampingFlag::SOF_TIMESTAMPING_TX_SOFTWARE),
        )
        .unwrap();
        s.with(|w, cx| {
            w.attach(cx, with_stream(attached, Conn::Tcp(server)));
            assert_eq!(slot.send_to_child(b"x\n", Traffic::Functions), 2);
            w.drain_inline(cx);
        });
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.receiver().is_none());
        let all: Vec<_> = records.iter().map(|r| (r.errno, r.message.clone().unwrap_or_default())).collect();
        assert_eq!(
            all[0],
            (
                0,
                "STREAM RCV[0] 'child' [from []:]: DISCONNECT SOCKET ERROR - closing connection; SO_ERROR=0 (no pending \
                 socket error); TCP keepalive: disabled"
                    .to_string()
            ),
            "{all:?}"
        );
    }

    #[test]
    fn a_poll_error_on_a_descriptor_without_so_error_says_so() {
        let (mut s, pool, hosts, connector) = stepper();
        let (mut attached, host, _, _unix) = child(0xe5, crate::caps::V2, &pool, &hosts, &connector);
        attached.keepalive = Keepalive { automatic: false, idle_s: 45, ..keepalive() };
        let (read_end, write_end) = std::io::pipe().unwrap();
        let fd: std::os::fd::OwnedFd = write_end.into();
        let stream = mio::net::UnixStream::from_std(std::os::unix::net::UnixStream::from(fd));
        s.with(|w, cx| w.attach(cx, with_stream(attached, Conn::Unix(stream))));
        drop(read_end);
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.receiver().is_none());
        let all: Vec<_> = records.iter().map(|r| (r.errno, r.message.clone().unwrap_or_default())).collect();
        assert_eq!(
            all[0],
            (
                88,
                "STREAM RCV[0] 'child' [from []:]: DISCONNECT SOCKET ERROR - closing connection; SO_ERROR is \
                 unavailable: Socket operation on non-socket (errno=88); TCP keepalive: enabled policy=configured \
                 idle=45s interval=10s probes=3"
                    .to_string()
            ),
            "{all:?}"
        );
    }

    #[test]
    fn a_hangup_with_nothing_to_read_disconnects_at_the_poll() {
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _, _unix) = child(0xe6, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, with_stream(attached, Conn::Tcp(unconnected_tcp()))));
        let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(50)));
        assert!(host.receiver().is_none());
        let texts = texts(records);
        assert_eq!(
            texts[0],
            "STREAM RCV[0] 'child' [from []:]: DISCONNECTED SOCKET CLOSED BY REMOTE END - closing connection",
            "{texts:?}"
        );
        assert_eq!(texts.len(), 2, "{texts:?}");
    }

    #[test]
    fn the_periodic_check_runs_every_update_every_not_every_tick() {
        let t0 = Instant::now();
        let (mut s, pool, hosts, connector) = stepper();
        let (attached, host, _, peer) = child(0xe7, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        // the attach's read, then the socket off the poll: only the periodic probe can find the end
        assert!(s.turn(Duration::from_millis(50)));
        s.with(|w, cx| {
            let c = w.children[0].as_mut().unwrap();
            cx.registry().deregister(&mut c.attached.stream).unwrap();
        });
        drop(peer);
        let mut seen = Vec::new();
        while host.receiver().is_some() {
            assert!(t0.elapsed() < Duration::from_secs(5), "never checked");
            let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(200)));
            seen.extend(texts(records));
        }
        let at = t0.elapsed();
        assert!(at >= Duration::from_secs(1) && at < Duration::from_millis(1500), "{at:?}");
        assert_eq!(seen[0], "STREAM RCV[0] 'child' [from ]: socket closed by remote - closing connection", "{seen:?}");
    }

    /// A sender whose parent closes the link is requeued to connect again without a reset of its parents
    /// (`stream_sender_remove()`): the current parent keeps its postponement and its session ban, and takes the
    /// disconnect's reason.
    #[test]
    fn a_dropped_link_requeues_without_resetting_the_parents() {
        let (mut s, _pool, _hosts, connector) = stepper();
        let sending = Arc::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000ea",
            false,
            crate::connector::tests::info("127.0.0.1:1", "key"),
        ));
        let sender = Sender::attach(&sending, &connector).expect("created");
        let until = {
            let mut parents = sender.parents();
            parents.current = Some(0);
            parents.list[0].banned_for_this_session = true;
            parents.list[0].postpone_until_ut = 4_000_000_000_000_000;
            parents.list[0].postpone_until_ut
        };
        let (ours, parent) = mio::net::UnixStream::pair().unwrap();
        s.with(|w, cx| {
            w.queued_senders.push(crate::sender::Connected {
                sender: Arc::clone(&sender),
                link: Link::Plain(Conn::Unix(ours)),
                capabilities: crate::caps::V2,
                compressor: None,
                remote_ip: "p".into(),
                thread: 0,
            });
            w.dequeue_senders(cx);
        });
        assert!(s.worker().senders.iter().any(Option::is_some), "the sender runs here");
        drop(parent);
        let deadline = Instant::now() + Duration::from_secs(3);
        while s.worker().senders.iter().any(Option::is_some) {
            assert!(Instant::now() < deadline, "the sender stayed");
            let _ = netdata_agent_log::capture(|| s.turn(Duration::from_millis(100)));
        }
        let parents = sender.parents();
        let d = &parents.list[0];
        assert_eq!(
            (d.postpone_until_ut, d.banned_for_this_session, d.reason),
            (until, true, Reason::DISCONNECT_SOCKET_ERROR)
        );
    }

    #[test]
    fn the_stall_checks_run_every_ten_minutes_senders_first() {
        use netdata_agent_rrd::chart::{ChartSpec, ChartType};
        let (mut s, pool, hosts, connector) = stepper();
        let chart = |host: &Host, id: &str, set: u32| {
            let (c, _) = host.charts().create(&ChartSpec {
                type_: "t",
                id,
                name: None,
                family: None,
                context: None,
                title: "t",
                units: "u",
                plugin: "p",
                module: None,
                priority: 1,
                update_every: 1,
                chart_type: ChartType::Line,
                mode: DbMode::Ram,
                history_entries: 60,
                page_size: 4096,
            });
            c.update_meta(|m| {
                m.flags &= !(flags::SENDER_REPLICATION_FINISHED
                    | flags::SENDER_REPLICATION_IN_PROGRESS
                    | flags::RECEIVER_REPLICATION_FINISHED
                    | flags::RECEIVER_REPLICATION_IN_PROGRESS);
                m.flags |= set;
            });
        };
        let sending = Arc::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000e8",
            false,
            crate::connector::tests::info("127.0.0.1:1", "key"),
        ));
        let sender = Sender::attach(&sending, &connector).expect("created");
        let (ours, _parent) = mio::net::UnixStream::pair().unwrap();
        s.with(|w, cx| {
            w.queued_senders.push(crate::sender::Connected {
                sender: Arc::clone(&sender),
                link: Link::Plain(Conn::Unix(ours)),
                capabilities: crate::caps::V2,
                compressor: None,
                remote_ip: "p".into(),
                thread: 0,
            });
            w.dequeue_senders(cx);
        });
        for (id, set) in [
            ("obs", flags::OBSOLETE),
            ("ign", flags::UPSTREAM_IGNORE),
            ("fin", flags::SENDER_REPLICATION_FINISHED),
            ("run", flags::SENDER_REPLICATION_IN_PROGRESS),
            ("new", 0),
        ] {
            chart(&sending, id, set);
        }
        sender.counter_in.store(1, Ordering::Relaxed);
        let (attached, child_host, _, _peer) = child(0xe9, crate::caps::V2, &pool, &hosts, &connector);
        s.with(|w, cx| w.attach(cx, attached));
        chart(&child_host, "r", 0);
        child_host.count_replication_request();
        let past = Instant::now().checked_sub(Duration::from_secs(601)).unwrap();
        s.with(|w, cx| {
            w.check_sender_replication(cx, past);
            w.check_replication(cx, past);
        });
        let stall = |t: &String| t.contains("REPLICATION EXCEPTIONS") || t.contains("REPLICATION STALLED: instance");
        let tick = |s: &mut netdata_agent_evloop::testing::Stepper<StreamWorker>| {
            let mut seen = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(3);
            while s.worker().last_check == past {
                assert!(Instant::now() < deadline, "no tick");
                let (_, records) = netdata_agent_log::capture(|| s.turn(Duration::from_millis(200)));
                seen.extend(texts(records).into_iter().filter(|t| stall(t)));
            }
            seen
        };
        // an update every since the last check, not ten minutes since the last stall check: none runs
        s.worker().last_check = past;
        s.worker().last_replication_check = Instant::now();
        assert_eq!(tick(&mut s), Vec::<String>::new());
        s.worker().last_check = past;
        s.worker().last_replication_check = Instant::now().checked_sub(Duration::from_secs(599)).unwrap();
        assert_eq!(tick(&mut s), Vec::<String>::new(), "599 s is not ten minutes");
        assert!(child_host.receiver().is_some());
        // both due: the senders' check, then the receivers'
        s.worker().last_check = past;
        s.worker().last_replication_check = past;
        assert_eq!(
            tick(&mut s),
            [
                "STREAM SND[0] 'child' [to p]: REPLICATION STALLED: instance 't.run' has not finished replication yet.",
                "STREAM SND[0] 'child' [to p]: REPLICATION STALLED: instance 't.new' has not started replication yet.",
                "STREAM SND[0] 'child' [to p]: REPLICATION EXCEPTIONS SUMMARY: node has 2 stalled replication requests (1 \
                 completed).We have received 1 and sent 0 replication commands. Disconnecting node to restore streaming.",
                "STREAM RCV[0] 'child' [from ]: REPLICATION EXCEPTIONS: instance 't.r' has not started replication yet.",
                "STREAM RCV[0] 'child' [from ]: REPLICATION EXCEPTIONS SUMMARY: node has 1 stalled replication requests \
                 (0 finished). We have requested 1 and got replies for 0 replication commands. Disconnecting node to \
                 restore streaming.",
            ]
        );
        assert!(child_host.receiver().is_none());
    }
}
