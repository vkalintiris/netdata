//! The receiver, ported from `src/streaming/stream-receiver-connection.c` (admission and the first response) and
//! `src/streaming/stream-thread.c` (assigning children to stream threads).
//!
//! Admission runs on the web worker that read the `STREAM` request, as in C: rejections decided before the takeover
//! go back on the web connection; after the takeover the socket is written with blocking sends and a timeout, then
//! handed to the least loaded stream thread.

use std::io;
use std::net::Shutdown;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use netdata_agent_evloop::conn::Conn;
use netdata_agent_tls::Link;
use netdata_agent_evloop::{Context, Event, Interest, PoolHandle, Token};
use netdata_agent_ingest::{self as ingest, Parser};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_pluginsd_proto::{LINE_MAX, LineReader};
use netdata_agent_rrd::chart::flags;
use netdata_agent_rrd::collection;
use netdata_agent_rrd::host::{Attach, Host, HostInfo, Hosts, ReceiverLink, ReceiverSlot, StreamSend};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};
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
/// `CBUFFER_INITIAL_MAX_SIZE`: the buffer C queues data for a child in; its fill is in the timeout record.
const SEND_BUFFER_MAX: usize = 10 * 1024 * 1024;
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

/// `now_monotonic_usec()`.
pub fn now_monotonic_ut() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    // Starts at 1 so that no real reading is the "never" value 0.
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64 + 1
}

/// The complete lines of `bytes` through the parser; false at the first line it refuses.
fn parse(reader: &mut LineReader, parser: &mut Parser, bytes: &[u8]) -> bool {
    reader.push(bytes).into_iter().all(|line| parser.feed(&line))
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
    /// The stream threads, for the backfilled charts' requests to come back to this one.
    pool: PoolHandle<StreamMsg>,
    /// The receiver waits for replication once attached (replication is enabled), else runs.
    replication_wait: bool,
    /// The senders' connector, whose environment a NODE_ID to the child reads.
    connector: Arc<Connector>,
}

impl Attached {
    /// `stream_receiver_remove()`'s release of the host: offline in pulse, the receiver slot freed with `reason` (the
    /// one the host's sender stops with), the parent label updated. The caller gives back the host's stream thread
    /// pin.
    fn leave_host(&self, reason: Reason) {
        self.host
            .pulse_status(netdata_agent_rrd::pulse::host_status::RCV_OFFLINE);
        self.host.clear_receiver(&self.slot, reason.0);
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
    /// Bytes for the child that did not fit in the socket yet.
    pending_out: Vec<u8>,
    /// The fields every record of this child carries, shared by every event.
    frame: Arc<[(netdata_agent_log::Field, netdata_agent_log::Value)]>,
    bytes_in: u64,
    bytes_out: u64,
    /// Successful writes (`stats->sends`).
    sends: u64,
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

/// `wait_on_socket_or_cancel_with_timeout()` for `POLLOUT`: `Err` with C's errno on a timeout (ETIMEDOUT), a failed
/// `poll()` (its errno) or an event other than writable (0, cleared before the poll).
fn writable_within(fd: std::os::fd::BorrowedFd<'_>, timeout: Duration) -> Result<(), i32> {
    use nix::errno::Errno;
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Errno::ETIMEDOUT as i32);
        }
        // errno_clear(): what the send and the close leave is the next records' errno
        Errno::clear();
        let mut fds = [PollFd::new(fd, PollFlags::POLLOUT)];
        // whole milliseconds, rounded up, so the last one does not spin
        let ms = PollTimeout::try_from(left.as_micros().div_ceil(1000)).unwrap_or(PollTimeout::MAX);
        match poll(&mut fds, ms) {
            Ok(0) | Err(Errno::EINTR | Errno::EAGAIN) => {}
            Ok(_) if fds[0].revents().is_some_and(|r| r.contains(PollFlags::POLLOUT)) => return Ok(()),
            Ok(_) => return Err(0),
            Err(e) => return Err(e as i32),
        }
    }
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
        // Virtual nodes (step 14) come with vnodes.
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
                    now_monotonic_ut().saturating_sub(last) / 1_000_000
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
        // rrdhost_create() and rrdhost_update(): no health without a database
        let health_enabled = config.health_enabled != 0 && mode != DbMode::None;
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
            now_monotonic_ut(),
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
        // rrdhost_set_receiver(); health itself is not ported, the delay is only logged
        if config.health_enabled != 0 && config.health_delay > 0 {
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
        // the negotiated capabilities are logged before the prompt goes out
        peer.established(&host.hostname(), capabilities);
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
            attached.leave_host(Reason::DISCONNECT_SHUTDOWN);
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
    let now = now_s();
    let last = host.contexts().retention().1.min(now);
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
            attached.leave_host(Reason::DISCONNECT_SOCKET_ERROR);
            self.pins.lock().unwrap_or_else(PoisonError::into_inner).remove(attached.host.machine_guid());
            return;
        }
        let mut parser = Parser::new(
            Arc::clone(&attached.host),
            Arc::clone(attached.hosts.localhost()),
            attached.parser,
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
            .store(now_monotonic_ut(), std::sync::atomic::Ordering::Relaxed);
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
            pending_out: Vec::new(),
            frame,
            bytes_in: 0,
            bytes_out: 0,
            sends: 0,
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
                .record_first_time_changes(true);
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
        // the end of the move to running: lines owed before it are dropped as C's (no buffer yet, D119.2), then the
        // host's node id goes down (D106.9)
        if let Some(child) = &self.children[index] {
            child.attached.slot.take_to_child();
            crate::sender::send_node_and_claim_id_to_child(&child.attached.host, child.attached.connector.env());
        }
        self.deliver_owed(cx, index);
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

    /// The lines other threads owed a host's child (`send_to_child`), written to it now: after the host's sender
    /// executed its parent's commands.
    pub(crate) fn deliver_to_child(&mut self, cx: &mut Context<'_>, host: &Arc<Host>) {
        let index = self.children.iter().position(|c| c.as_ref().is_some_and(|c| Arc::ptr_eq(&c.attached.host, host)));
        if let Some(index) = index {
            let frame = self.children[index].as_ref().map(|c| Arc::clone(&c.frame));
            let _frame = frame.as_ref().map(records::child_event);
            self.deliver_owed(cx, index);
        }
    }

    /// The child's owed lines, flushed with whatever else waits for it.
    fn deliver_owed(&mut self, cx: &mut Context<'_>, index: usize) {
        let Some(child) = self.children[index].as_mut() else {
            return;
        };
        let owed = child.attached.slot.take_to_child();
        if !owed.is_empty() {
            child.pending_out.extend_from_slice(&owed);
            self.flush(cx, index, true);
        }
    }

    /// A backfilled chart's replication request, for the connection it came from: sent and flushed under the
    /// child's frame; dropped when the connection is gone (C's bytes go with the buffer they were added to).
    pub(crate) fn replay(
        &mut self,
        cx: &mut Context<'_>,
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
        child.parser.replay_backfilled(request);
        self.flush(cx, index, true);
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
        if event.is_writable() && !self.flush(cx, index, true) {
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
            let changes = child.attached.host.contexts().take_first_time_changes();
            // and what the connector thread owed the child (D106.5)
            let owed = child.attached.slot.take_to_child();
            if !changes.is_empty() || !owed.is_empty() {
                child.pending_out.extend_from_slice(&owed);
                for first_time_s in changes {
                    child.parser.retention_updated(first_time_s);
                }
                let frame = Arc::clone(&child.frame);
                let _frame = records::child_event(&frame);
                self.flush(cx, index, true);
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
        let Child { mut attached, parser, frame, bytes_in, bytes_out, .. } = child;
        {
            let attached = &mut attached;
            let _ = cx.registry().deregister(&mut attached.stream);
            let counters = Counters {
                thread: attached.thread,
                msgs: parser.data_collections_count,
                bytes_in,
                bytes_out,
                connected_s: (now_s() - attached.accepted_s).max(0),
                // C's idle time since the last read or write, 0 before any
                idle_s: match attached.slot.last_traffic_ut.load(Ordering::Relaxed) {
                    0 => 0,
                    last => (now_monotonic_ut().saturating_sub(last) / 1_000_000) as i64,
                },
                replication_percent: attached.host.replication_percent(),
            };
            let labels = attached.host.labels();
            let iface = labels
                .get(b"_net_default_iface")
                .map(|v| String::from_utf8_lossy(v).into_owned());
            let _removal = records::removal(&frame, &attached.host.hostname());
            records::disconnected(&attached.peer, iface.as_deref(), reason, &counters);
            attached.leave_host(reason);
            self.pins.lock().unwrap_or_else(PoisonError::into_inner).remove(attached.host.machine_guid());
            // pluginsd_process_cleanup() at the end of rrdhost_clear_receiver(): its THREAD CLEANUP record is the
            // removal's
            drop(parser);
        }
        // stream_receiver_free(): the socket closes after the records, and a TLS close leaves what its shutdown set
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
                let outstanding = child.pending_out.len();
                let pending = if outstanding == 0 {
                    "0".to_string()
                } else {
                    size_to_string(outstanding as u64, "B", false).unwrap_or_default()
                };
                let ratio = outstanding as f64 * 100.0 / SEND_BUFFER_MAX as f64;
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "{at}there was not traffic for {timeout_s} seconds - closing connection - we have sent {} bytes in \
                     {} operations, it is idle for {duration}, and we have {pending} pending to send (buffer is used \
                     {ratio:.2}%).",
                    child.bytes_out,
                    child.sends
                );
                self.disconnect(cx, index, Reason::DISCONNECT_TIMEOUT);
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
        let a = &child.attached;
        format!(
            "STREAM RCV[{}] '{}' [from [{}]:{}]: ",
            a.thread,
            a.host.hostname(),
            a.peer.ip,
            a.peer.port
        )
    }

    /// `stream_receiver_send_data()`: writes what the parser produced; a full socket keeps the rest for the next
    /// writable event. False when the connection failed: from the poller (`remove`) it is disconnected here; after a
    /// read (`stream_receiver_dequeue_senders()`) the caller ends it, as a read failure, as C does.
    fn flush(&mut self, cx: &mut Context<'_>, index: usize, remove: bool) -> bool {
        let Some(child) = self.children[index].as_mut() else {
            return false;
        };
        let out = child.parser.take_output();
        child.pending_out.extend_from_slice(&out);
        while !child.pending_out.is_empty() {
            let failure = match child.attached.stream.write(&child.pending_out) {
                Ok(n) if n > 0 => {
                    child.pending_out.drain(..n);
                    child.bytes_out += n as u64;
                    child
                        .attached
                        .hosts
                        .storage()
                        .pulse()
                        .network
                        .stream_sent(n);
                    child.attached.host.stream_bytes_sent(n);
                    child.sends += 1;
                    // a write is traffic too (C's last_traffic_ut), for the idle timeout and the stale check at accept
                    child
                        .attached
                        .slot
                        .last_traffic_ut
                        .store(now_monotonic_ut(), Ordering::Relaxed);
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return true,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // only a zero write or a reset is the remote end closing; EPIPE is a write failure
                Ok(_) => (Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE, 0, 0),
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {
                    (Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE, -1, netdata_agent_log::errno_of(&e))
                }
                Err(e) => (Reason::DISCONNECT_SOCKET_WRITE_FAILED, -1, netdata_agent_log::errno_of(&e)),
            };
            let (reason, rc, errno) = failure;
            let _parser = (!remove).then(|| child.parser.log_frame());
            nd_log!(
                Source::Daemon,
                Priority::Err,
                errno = errno;
                "{}{} ({rc}, on fd {}) - closing receiver connection - we have sent {} bytes in {} operations.",
                Self::prefix(child),
                reason.text(),
                raw_fd(&child.attached.stream),
                child.bytes_out,
                child.sends
            );
            if remove {
                self.disconnect(cx, index, reason);
            }
            return false;
        }
        true
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
                    child
                        .attached
                        .slot
                        .last_traffic_ut
                        .store(now_monotonic_ut(), Ordering::Relaxed);
                    match child.decompressor.as_mut() {
                        None => {
                            if !parse(&mut child.reader, &mut child.parser, &buf[..n]) {
                                let _parser = child.parser.log_frame();
                                return self.disconnect(cx, index, Reason::RCV_DISCONNECT_PARSER_FAILED);
                            }
                        }
                        Some(decompressor) => {
                            // stream_receive_and_process(): a message at a time, its lines parsed before the next is
                            // decompressed, while the streaming service runs and no stop is asked
                            decompressor.feed(&buf[..n]);
                            let mut out = Vec::new();
                            let stop_requested = &child.attached.slot.stop_requested;
                            let stop = || stop_requested.load(Ordering::Acquire);
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
                                        if !parse(&mut child.reader, &mut child.parser, &out) {
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
                                return self.disconnect(cx, index, Reason::DISCONNECT_SIGNALED_TO_STOP);
                            }
                        }
                    }
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
                    if !self.flush(cx, index, false) {
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
    use netdata_agent_rrd::host::HostInfo;
    use std::os::fd::AsFd;

    /// A child configured for dbengine falls back to the default only when the dbengine does not run; other names
    /// are C's modes (an unknown one is ram).
    /// Replication stalls after ten minutes without new requests, unless charts wait for their backfill (C's
    /// `backfill_pending` check); new requests restart the clock.
    #[test]
    fn replication_progress_waits_for_backfills() {
        let t0 = Instant::now();
        let later = t0 + REPLICATION_STALL + Duration::from_secs(1);
        let (mut requests, mut since) = (0, None);
        assert!(replication_progressed((&mut requests, &mut since), 3, false, t0));
        assert!(!replication_progressed((&mut requests, &mut since), 3, false, later));
        assert!(replication_progressed((&mut requests, &mut since), 3, true, later), "work waits");
        assert!(replication_progressed((&mut requests, &mut since), 4, false, later));
        assert!(replication_progressed((&mut requests, &mut since), 0, false, later), "not started");
    }

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
        let before = now_monotonic_ut();
        s.with(|w, cx| {
            w.children[0].as_mut().unwrap().pending_out.extend_from_slice(b"REPLAY_CHART x\n");
            assert!(w.flush(cx, 0, false));
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
        let (_, records) = netdata_agent_log::capture(|| s.with(|w, cx| w.check_all(cx, now_monotonic_ut())));
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
        let start = now_monotonic_ut();
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
}
