//! The receiver, ported from `src/streaming/stream-receiver-connection.c` (admission and the first response) and
//! `src/streaming/stream-thread.c` (assigning children to stream threads).
//!
//! Admission runs on the web worker that read the `STREAM` request, as in C: rejections decided before the takeover
//! go back on the web connection; after the takeover the socket is written with blocking sends and a timeout, then
//! handed to the least loaded stream thread.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use netdata_agent_evloop::conn::{Conn, Stream};
use netdata_agent_evloop::{Context, Event, Interest, PoolHandle, TimerId, Token, Worker};
use netdata_agent_ingest::{self as ingest, Parser};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_pluginsd_proto::LineReader;
use netdata_agent_rrd::chart::flags;
use netdata_agent_rrd::collection;
use netdata_agent_rrd::host::{Host, HostInfo, Hosts, ReceiverLink, ReceiverSlot, StreamSend};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};
use netdata_agent_text::duration::duration_to_string;
use netdata_agent_text::size::size_to_string;

use crate::caps;
use crate::conf::{Keepalive, ReceiverDefaults, StreamConf};
use crate::decompress::Decompressor;
use crate::handshake::{self, StreamRequest};
use crate::records::{self, Counters, Peer, Reason};

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
const REPLICATION_STALL: Duration = Duration::from_secs(600);

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
/// How often a stream thread checks its receivers for stop requests.
const TICK: Duration = Duration::from_millis(100);

/// `now_monotonic_usec()`.
pub fn now_monotonic_ut() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    // Starts at 1 so that no real reading is the "never" value 0.
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64 + 1
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
    stream: Conn,
    thread: usize,
    parser: ingest::Config,
    peer: Peer,
    accepted_s: i64,
    /// For the text of socket errors: the receiver's TCP keepalive policy and `rpt->handshake_update_every`.
    keepalive: Keepalive,
    handshake_update_every: i64,
    /// `rpt->thread.keepalive_initialized`.
    keepalive_initialized: bool,
}

/// A connection on its stream thread.
struct Child {
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
    /// The last read or write, for the disconnect record's `idle=` and the idle timeout.
    last_io: Instant,
    /// `rpt->replication`: the request count last seen, when it last moved, and the progress time last checked.
    replication_requests: u32,
    replication_progress: Option<Instant>,
    replication_checked: Option<Instant>,
}

/// The receiving side of this agent.
pub struct Receivers {
    pub conf: Mutex<StreamConf>,
    pub hosts: Arc<Hosts>,
    pub defaults: Defaults,
    pool: PoolHandle<Attached>,
    /// Children per stream thread (`nodes_count`).
    load: Arc<Mutex<Vec<usize>>>,
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

/// One blocking `send()` bounded by `timeout` (`nd_sock_send_timeout()`): true when everything went out.
fn send_timeout(stream: &Stream, bytes: &[u8], timeout: Duration) -> bool {
    let sent = stream
        .set_nonblocking(false)
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .and_then(|()| {
            let mut s = stream;
            s.write(bytes)
        });
    matches!(sent, Ok(n) if n == bytes.len())
}

impl Receivers {
    /// `load` is the table the stream threads of `pool` share (see `StreamWorker::new()`).
    pub fn new(
        conf: StreamConf,
        hosts: Arc<Hosts>,
        load: Arc<Mutex<Vec<usize>>>,
        defaults: Defaults,
        pool: PoolHandle<Attached>,
    ) -> Self {
        load.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .resize(pool.threads(), 0);
        Receivers {
            conf: Mutex::new(conf),
            hosts,
            defaults,
            pool,
            load,
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
            peer.status(denied.message(), Reason::Denied, Priority::Warning);
            return PreAdmission::Reply(handshake::ERROR_NOT_PERMITTED, 401);
        }
        let guid = request.machine_guid.clone().unwrap_or_default();
        if guid == self.hosts.localhost().machine_guid() {
            return PreAdmission::Refuse(
                handshake::ERROR_SAME_LOCALHOST,
                Box::new(Refusal {
                    peer,
                    msg: "rejecting streaming connection; machine UUID is my own",
                    reason: Reason::Localhost,
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
                Reason::BusyTryLater,
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
                    Reason::Denied,
                    Priority::Warning,
                );
                return PreAdmission::Reply(handshake::ERROR_NOT_PERMITTED, 401);
            }
        }
        if let (Some(slot), Some(host)) = (&stale, &existing) {
            if stop_and_wait(host, slot) {
                stale = None;
                nd_log!(
                    Source::Daemon,
                    Priority::Notice,
                    "STREAM RCV '{}' [from [{client_ip}]:{client_port}]: stopped previous stale receiver to accept this \
                     one.",
                    peer.hostname.as_deref().unwrap_or("")
                );
            } else {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "STREAM RCV[x] '{}' [from [{}]:{}]: streaming thread takes too long to stop, giving up...",
                    host.hostname(),
                    slot.remote.0,
                    slot.remote.1
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
                Reason::AlreadyConnected,
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
    pub fn refuse(&self, stream: Stream, message: &str, refusal: &Refusal) {
        let peer = &refusal.peer;
        peer.status(refusal.msg, refusal.reason, refusal.priority);
        if !send_timeout(&stream, message.as_bytes(), Duration::from_secs(60)) {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM RCV '{}' [from [{}]:{}]: failed to reply.",
                peer.hostname.as_deref().unwrap_or(""),
                peer.ip,
                peer.port
            );
        }
    }

    /// The rest of `stream_receiver_accept_connection()`: the receiver configuration, the host, the prompt, and the
    /// handover to a stream thread.
    pub fn admit(&self, pending: Pending, stream: Stream) {
        let Pending {
            request,
            peer,
            accepted_s,
        } = pending;
        let key = request.key.clone().unwrap_or_default();
        let guid = request.machine_guid.clone().unwrap_or_default();
        let config = {
            let mut conf = self.conf.lock().unwrap_or_else(PoisonError::into_inner);
            let defaults = ReceiverDefaults {
                db_mode: self.defaults.db_mode.clone(),
                history: self.defaults.history,
                health_enabled: self.defaults.health_enabled,
                update_every: i64::from(request.update_every),
            };
            conf.receiver_config(&key, &guid, &defaults)
        };
        // No dbengine yet: a child asking for it gets the default, as C does when dbengine is disabled.
        let mut mode = DbMode::from_name(&config.db_mode);
        if mode == DbMode::Dbengine {
            mode = DbMode::from_name(&self.defaults.db_mode);
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
            || wanted.clone(),
            |host| host.update(&wanted, config.update_every, config.history),
        );
        let capabilities = caps::select_compression(
            request.capabilities,
            config.compression_enabled,
            &config.compression_priorities,
            caps::COMPRESSIONS_AVAILABLE,
        );
        let shutdown_handle = stream.try_clone().ok();
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
        if !host.set_receiver(Arc::clone(&slot)) {
            peer.status(
                "rejecting streaming connection; host is already served by another receiver",
                Reason::AlreadyConnected,
                Priority::Info,
            );
            send_timeout(
                &stream,
                handshake::ERROR_ALREADY_STREAMING.as_bytes(),
                Duration::from_secs(5),
            );
            return;
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
        let _ = stream.set_read_timeout(Some(Duration::from_secs(600)));
        let mut keepalive_initialized = false;
        reconcile_keepalive(
            std::os::fd::AsFd::as_fd(&stream),
            &host,
            &peer,
            &config.keepalive,
            i64::from(request.update_every),
            &mut keepalive_initialized,
        );
        // the negotiated capabilities are logged before the prompt goes out
        peer.established(&host.hostname(), capabilities);
        if !send_timeout(&stream, prompt.as_bytes(), Duration::from_secs(60)) {
            peer.status(
                "cannot reply back, dropping connection",
                Reason::SendTimeout,
                Priority::Err,
            );
            host.clear_receiver(&slot);
            return;
        }
        peer.status(&connected_msg(&host), Reason::Never, Priority::Info);
        self.hosts.update_is_parent_label();
        if stream.set_nonblocking(true).is_err() {
            host.clear_receiver(&slot);
            self.hosts.update_is_parent_label();
            return;
        }
        let thread = {
            let mut load = self.load.lock().unwrap_or_else(PoisonError::into_inner);
            let thread = (0..load.len()).min_by_key(|&i| load[i]).unwrap_or(0);
            load[thread] += 1;
            thread
        };
        let attached = Attached {
            host,
            hosts: Arc::clone(&self.hosts),
            slot,
            stream: Conn::from_std(stream),
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
        };
        // stream_receiver_add_to_queue()
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "STREAM RCV[{thread}] '{}': moving host to receiver queue...",
            attached.host.hostname()
        );
        if let Err(attached) = self.pool.send(thread, attached) {
            attached.host.clear_receiver(&attached.slot);
            self.hosts.update_is_parent_label();
            self.load.lock().unwrap_or_else(PoisonError::into_inner)[thread] -= 1;
        }
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

/// `stream_receiver_signal_to_stop_and_wait()`: true when the old receiver let go within 2 s.
fn stop_and_wait(host: &Host, slot: &Arc<ReceiverSlot>) -> bool {
    slot.stop();
    for _ in 0..2000 {
        if !host.receiver().is_some_and(|r| Arc::ptr_eq(&r, slot)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    !host.receiver().is_some_and(|r| Arc::ptr_eq(&r, slot))
}

/// A stream thread: owns the connections of the children assigned to it, and parses what they send inline
/// (decisions D8).
pub struct StreamWorker {
    children: Vec<Option<Child>>,
    load: Arc<Mutex<Vec<usize>>>,
    tick: Option<TimerId>,
    /// `nd_profile.update_every`: how often every child is probed and checked for idleness.
    check_every: Duration,
    last_check: Instant,
    last_replication_check: Instant,
}

impl StreamWorker {
    pub fn new(load: Arc<Mutex<Vec<usize>>>, update_every: i32) -> Self {
        let now = Instant::now();
        StreamWorker {
            children: Vec::new(),
            load,
            tick: None,
            check_every: Duration::from_secs(u64::try_from(update_every).unwrap_or(1).max(1)),
            last_check: now,
            last_replication_check: now,
        }
    }

    /// `stream_receiver_remove_internal()`: the disconnect record, then the host lets go of the receiver. The
    /// parser's fields are the caller's: C has them only while reading (`stream_receiver_receive_data()`).
    fn disconnect(&mut self, cx: &mut Context<'_>, index: usize, reason: Reason) {
        if let Some(mut child) = self.children[index].take() {
            let attached = &mut child.attached;
            let _ = cx.registry().deregister(&mut attached.stream);
            let counters = Counters {
                thread: attached.thread,
                msgs: child.parser.data_collections_count,
                bytes_in: child.bytes_in,
                bytes_out: child.bytes_out,
                connected_s: (now_s() - attached.accepted_s).max(0),
                idle_s: child.last_io.elapsed().as_secs() as i64,
                replication_percent: attached.host.replication_percent(),
            };
            let labels = attached.host.labels();
            let iface = labels
                .get(b"_net_default_iface")
                .map(|v| String::from_utf8_lossy(v).into_owned());
            records::disconnected(
                &child.frame,
                &attached.peer,
                &attached.host.hostname(),
                iface.as_deref(),
                reason,
                &counters,
            );
            attached.host.clear_receiver(&attached.slot);
            attached.hosts.update_is_parent_label();
            self.load.lock().unwrap_or_else(PoisonError::into_inner)[attached.thread] -= 1;
        }
    }

    /// `STREAM RCV[n] '<host>' [from [<ip>]:<port>]: ` of the stream thread's records.
    /// `stream_receiver_check_all_nodes_from_poll()`: a probe finds a connection the child closed or that failed,
    /// and a child silent for longer than its timeout, while none of its charts replicates, is disconnected.
    fn check_all(&mut self, cx: &mut Context<'_>, now: Instant) {
        for index in 0..self.children.len() {
            let Some(child) = self.children[index].as_mut() else {
                continue;
            };
            let frame = Arc::clone(&child.frame);
            let a = &child.attached;
            let at = format!(
                "STREAM RCV[{}] '{}' [from {}]: ",
                a.thread,
                a.host.hostname(),
                a.peer.ip
            );
            let mut probe = [std::mem::MaybeUninit::uninit(); 1];
            let peeked = socket2::SockRef::from(&a.stream).peek(&mut probe);
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
                    self.disconnect(cx, index, Reason::ClosedByRemote);
                    continue;
                }
                Err(e) if reset(&e) => {
                    let _frame = records::child_event(&frame);
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "{at}socket closed by remote - closing connection"
                    );
                    self.disconnect(cx, index, Reason::ClosedByRemote);
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
                    self.disconnect(cx, index, Reason::SocketError);
                    continue;
                }
                _ => {}
            }
            let timeout_s = IDLE_TIMEOUT_MIN_S
                .max(receiver_update_every(&a.host, a.handshake_update_every) * 2);
            let idle = now.saturating_duration_since(child.last_io);
            if idle > Duration::from_secs(timeout_s) && !a.host.any_chart_replicating() {
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
                self.disconnect(cx, index, Reason::Timeout);
            }
        }
    }

    /// `stream_receiver_did_replication_progress()`: new replication requests, none yet, or less than ten minutes
    /// since the last one.
    fn replication_progressed(child: &mut Child, now: Instant) -> bool {
        let requests = child.attached.host.replication_requests();
        if child.replication_requests != requests {
            child.replication_requests = requests;
            child.replication_progress = Some(now);
            return true;
        }
        if requests == 0 {
            return true;
        }
        match child.replication_progress {
            None => {
                child.replication_progress = Some(now);
                true
            }
            Some(last) => now.saturating_duration_since(last) < REPLICATION_STALL,
        }
    }

    /// `stream_receiver_replication_check_from_poll()`: a child whose replication made no progress for ten minutes
    /// while some of its charts never finished is disconnected, after its unfinished charts are listed.
    fn check_replication(&mut self, cx: &mut Context<'_>, now: Instant) {
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
                let requested = child.attached.host.replication_requests();
                nd_log!(
                    Source::Daemon,
                    Priority::Warning,
                    "{at}REPLICATION EXCEPTIONS SUMMARY: node has {stalled} stalled replication requests ({finished} \
                     finished). We have requested {requested} and got replies for 0 replication commands. \
                     Disconnecting node to restore streaming."
                );
                self.disconnect(cx, index, Reason::ReplicationStalled);
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
                    child.sends += 1;
                    child.last_io = Instant::now();
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return true,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // only a zero write or a reset is the remote end closing; EPIPE is a write failure
                Ok(_) => (Reason::ClosedByRemote, 0, 0),
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {
                    (Reason::ClosedByRemote, -1, netdata_agent_log::errno_of(&e))
                }
                Err(e) => (Reason::WriteFailed, -1, netdata_agent_log::errno_of(&e)),
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
                std::os::fd::AsRawFd::as_raw_fd(&child.attached.stream),
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
        match child.attached.stream.take_error() {
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

    /// `stream_receiver_receive_data()`: reads what arrived and feeds every complete line to the parser; a refused
    /// line ends the connection. The caller has pushed the child's frame.
    fn receive(&mut self, cx: &mut Context<'_>, index: usize) {
        let mut buf = [0u8; 16384];
        loop {
            let Some(child) = self.children[index].as_mut() else {
                return;
            };
            let read = child.attached.stream.read(&mut buf);
            // C's parser frame of stream_receiver_receive_data() covers every record after the read; the parser's
            // fields are taken when a record is due, as C's callbacks read them
            let failed = |child: &Child, reason: Reason, errno: i32| {
                let _parser = child.parser.log_frame();
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    errno = errno;
                    "{}{} (fd {}) - closing receiver connection.",
                    Self::prefix(child),
                    reason.text(),
                    std::os::fd::AsRawFd::as_raw_fd(&child.attached.stream)
                );
                reason
            };
            match read {
                Ok(0) => {
                    let reason = failed(child, Reason::ClosedByRemote, 0);
                    let _parser = child.parser.log_frame();
                    return self.disconnect(cx, index, reason);
                }
                Ok(n) => {
                    child.bytes_in += n as u64;
                    child.last_io = Instant::now();
                    child
                        .attached
                        .slot
                        .last_traffic_ut
                        .store(now_monotonic_ut(), Ordering::Relaxed);
                    let plain;
                    let received = match child.decompressor.as_mut() {
                        None => &buf[..n],
                        Some(decompressor) => {
                            let mut out = Vec::new();
                            if let Err(failure) = decompressor.push(&buf[..n], &mut out) {
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
                                return self.disconnect(cx, index, Reason::DecompressionFailed);
                            }
                            plain = out;
                            &plain[..]
                        }
                    };
                    for line in child.reader.push(received) {
                        if !child.parser.feed(&line) {
                            let _parser = child.parser.log_frame();
                            return self.disconnect(cx, index, Reason::ParseError);
                        }
                    }
                    // the charts just received may lower the update every the keepalive follows
                    let a = &mut child.attached;
                    reconcile_keepalive(
                        std::os::fd::AsFd::as_fd(&a.stream),
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
                        let reason = failed(child, Reason::ReadFailed, 0);
                        let _parser = child.parser.log_frame();
                        return self.disconnect(cx, index, reason);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let reason = if e.kind() == io::ErrorKind::ConnectionReset {
                        Reason::ClosedByRemote
                    } else {
                        Reason::ReadFailed
                    };
                    let reason = failed(child, reason, netdata_agent_log::errno_of(&e));
                    let _parser = child.parser.log_frame();
                    return self.disconnect(cx, index, reason);
                }
            }
        }
    }
}

impl Worker for StreamWorker {
    type Msg = Attached;

    fn start(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        self.tick = Some(cx.add_timer(Instant::now() + TICK));
        Ok(())
    }

    /// `stream_receive_process_poll_events()`: under the child's frame, the stop flag, then socket errors (or a
    /// hangup with nothing left to read), then sending, then receiving.
    fn event(&mut self, cx: &mut Context<'_>, event: &Event) {
        let index = event.token().0;
        let Some(child) = self.children.get(index).and_then(Option::as_ref) else {
            return;
        };
        let _frame = records::child_event(&child.frame);
        // the shutdown that woke the socket is not a remote close
        if child.attached.slot.stop_requested.load(Ordering::Acquire) {
            return self.disconnect(cx, index, Reason::SignaledToStop);
        }
        let hangup = event.is_read_closed();
        if event.is_error() || (hangup && !event.is_readable()) {
            let reason = if hangup {
                Reason::ClosedByRemote
            } else {
                Reason::SocketError
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

    fn message(&mut self, cx: &mut Context<'_>, mut attached: Attached) {
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
            attached.host.clear_receiver(&attached.slot);
            attached.hosts.update_is_parent_label();
            self.load.lock().unwrap_or_else(PoisonError::into_inner)[attached.thread] -= 1;
            return;
        }
        let parser = Parser::new(
            Arc::clone(&attached.host),
            Arc::clone(attached.hosts.localhost()),
            attached.parser,
        );
        let decompressor = Decompressor::for_capabilities(attached.parser.capabilities);
        let frame = attached.peer.child_frame(attached.parser.capabilities);
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
        reconcile_keepalive(
            std::os::fd::AsFd::as_fd(&a.stream),
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
            last_io: Instant::now(),
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
        }
        // Bytes may have arrived before the registration.
        let frame = self.children[index].as_ref().map(|c| Arc::clone(&c.frame));
        let _frame = frame.as_ref().map(records::child_event);
        self.receive(cx, index);
    }

    fn timer(&mut self, cx: &mut Context<'_>, _timer: TimerId) {
        for index in 0..self.children.len() {
            let Some(child) = self.children[index].as_mut() else {
                continue;
            };
            if child.attached.slot.stop_requested.load(Ordering::Acquire) {
                let frame = Arc::clone(&child.frame);
                let _frame = records::child_event(&frame);
                self.disconnect(cx, index, Reason::SignaledToStop);
                continue;
            }
            // stream_path_retention_updated() from the RRDCONTEXT thread: its messages go out on this tick (D46
            // point 4), each with the retention start of its change
            let changes = child.attached.host.contexts().take_first_time_changes();
            if !changes.is_empty() {
                for first_time_s in changes {
                    child.parser.retention_updated(first_time_s);
                }
                let frame = Arc::clone(&child.frame);
                let _frame = records::child_event(&frame);
                self.flush(cx, index, true);
            }
        }
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
        for index in 0..self.children.len() {
            self.disconnect(cx, index, Reason::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_rrd::host::HostInfo;
    use std::os::fd::AsFd;

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
