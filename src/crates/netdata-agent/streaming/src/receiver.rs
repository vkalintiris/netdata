//! The receiver, ported from `src/streaming/stream-receiver-connection.c` (admission and the first response) and
//! `src/streaming/stream-thread.c` (assigning children to stream threads).
//!
//! Admission runs on the web worker that read the `STREAM` request, as in C: rejections decided before the takeover
//! go back on the web connection; after the takeover the socket is written with blocking sends and a timeout, then
//! handed to the least loaded stream thread.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use netdata_agent_evloop::{Context, Event, Interest, PoolHandle, TimerId, Token, Worker};
use netdata_agent_ingest::{self as ingest, Parser};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_pluginsd_proto::LineReader;
use netdata_agent_rrd::collection;
use netdata_agent_rrd::host::{Host, HostInfo, Hosts, ReceiverSlot};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};

use crate::caps;
use crate::conf::{ReceiverDefaults, StreamConf};
use crate::decompress::Decompressor;
use crate::handshake::{self, StreamRequest};

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
    /// Take the connection over, send this with a 60 s timeout, and close it.
    Refuse(&'static str),
    /// Take the connection over and call `admit()`.
    Proceed(Box<Pending>),
}

/// A request that passed the checks made on the web connection.
#[derive(Debug)]
pub struct Pending {
    request: StreamRequest,
}

/// A connection handed to a stream thread.
#[derive(Debug)]
pub struct Attached {
    host: Arc<Host>,
    slot: Arc<ReceiverSlot>,
    stream: mio::net::TcpStream,
    thread: usize,
    parser: ingest::Config,
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

/// One blocking `send()` bounded by `timeout` (`nd_sock_send_timeout()`): true when everything went out.
fn send_timeout(stream: &TcpStream, bytes: &[u8], timeout: Duration) -> bool {
    let sent = stream
        .set_nonblocking(false)
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .and_then(|()| (&*stream).write(bytes));
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

    /// The part of `stream_receiver_accept_connection()` before the connection is taken over.
    pub fn pre_admit(
        &self,
        decoded: &[u8],
        user_agent: Option<&[u8]>,
        client_ip: &str,
    ) -> PreAdmission {
        let mut request = StreamRequest::parse(decoded, self.defaults.update_every, user_agent);
        for (name, value) in &request.unused {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "STREAM RCV '{}' [from [{client_ip}]]: request has parameter '{name}' = '{value}', which is not used.",
                request
                    .hostname
                    .as_deref()
                    .filter(|h| !h.is_empty())
                    .unwrap_or("-")
            );
        }
        let validated = {
            let mut conf = self.conf.lock().unwrap_or_else(PoisonError::into_inner);
            handshake::validate(&mut request, &mut conf, client_ip)
        };
        if let Err(denied) = validated {
            nd_log!(Source::Daemon, Priority::Info, "{}", denied.message());
            return PreAdmission::Reply(handshake::ERROR_NOT_PERMITTED, 401);
        }
        let guid = request.machine_guid.clone().unwrap_or_default();
        if guid == self.hosts.localhost().machine_guid() {
            return PreAdmission::Refuse(handshake::ERROR_SAME_LOCALHOST);
        }
        // Virtual nodes (step 14) come with vnodes.
        if let Some(wait_s) = self.rate_limited() {
            nd_log!(
                Source::Daemon,
                Priority::Notice,
                "rejecting streaming connection; rate limit, will accept new connection in {wait_s} secs"
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
                nd_log!(
                    Source::Daemon,
                    Priority::Info,
                    "rejecting streaming connection; machine GUID is connected with a different hostname"
                );
                return PreAdmission::Reply(handshake::ERROR_NOT_PERMITTED, 401);
            }
        }
        if let (Some(slot), Some(host)) = (&stale, &existing) {
            if stop_and_wait(host, slot) {
                stale = None;
            }
        }
        if working || stale.is_some() {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "rejecting streaming connection; multiple connections for the same host, old connection was last used {age_s} secs ago{}",
                if stale.is_some() {
                    " (signaled old receiver to stop)"
                } else {
                    " (new connection not accepted)"
                }
            );
            return PreAdmission::Reply(handshake::ERROR_ALREADY_STREAMING, 409);
        }
        PreAdmission::Proceed(Box::new(Pending { request }))
    }

    /// `PreAdmission::Refuse`: the connection has been taken over.
    pub fn refuse(&self, stream: TcpStream, message: &str) {
        if !send_timeout(&stream, message.as_bytes(), Duration::from_secs(60)) {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM RCV: failed to reply."
            );
        }
    }

    /// The rest of `stream_receiver_accept_connection()`: the receiver configuration, the host, the prompt, and the
    /// handover to a stream thread.
    pub fn admit(&self, pending: Pending, stream: TcpStream) {
        let request = pending.request;
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
        let health_enabled = config.health_enabled != 0;
        let text =
            |v: &Option<String>, default: &str| v.clone().unwrap_or_else(|| default.to_string());
        let host = self.hosts.find_or_create(
            &guid,
            || {
                let mut info = HostInfo {
                    hostname: text(&request.hostname, ""),
                    registry_hostname: text(&request.registry_hostname, ""),
                    os: text(&request.os, "unknown"),
                    timezone: text(&request.timezone, "unknown"),
                    abbrev_timezone: text(&request.abbrev_timezone, "UTC"),
                    utc_offset: request.utc_offset,
                    program_name: text(&request.program_name, "unknown"),
                    program_version: text(&request.program_version, "unknown"),
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
                };
                info.set_replication(
                    config.replication.enabled,
                    config.replication.period,
                    config.replication.step,
                );
                info
            },
            |host| {
                host.update_info(|info| {
                    info.system_info = request.system_info.clone();
                    info.os = text(&request.os, "unknown");
                    info.timezone = text(&request.timezone, "unknown");
                    info.abbrev_timezone = text(&request.abbrev_timezone, "UTC");
                    info.utc_offset = request.utc_offset;
                    info.registry_hostname = text(&request.registry_hostname, "");
                    info.hostname = text(&request.hostname, "");
                    info.program_name = text(&request.program_name, "unknown");
                    info.program_version = text(&request.program_version, "unknown");
                    info.health_enabled = health_enabled;
                });
            },
        );
        let shutdown_handle = stream.try_clone().ok();
        let slot = Arc::new(ReceiverSlot::new(
            now_monotonic_ut(),
            Box::new(move || {
                if let Some(s) = &shutdown_handle {
                    let _ = s.shutdown(Shutdown::Both);
                }
            }),
        ));
        if !host.set_receiver(Arc::clone(&slot)) {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "rejecting streaming connection; host is already served by another receiver"
            );
            send_timeout(
                &stream,
                handshake::ERROR_ALREADY_STREAMING.as_bytes(),
                Duration::from_secs(5),
            );
            return;
        }
        let capabilities = caps::select_compression(
            request.capabilities,
            config.compression_enabled,
            &config.compression_priorities,
            caps::COMPRESSIONS_AVAILABLE,
        );
        let prompt = caps::prompt(capabilities);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(600)));
        if !send_timeout(&stream, prompt.as_bytes(), Duration::from_secs(60)) {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM RCV: cannot reply back, dropping connection"
            );
            host.clear_receiver(&slot);
            return;
        }
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "connected and ready to receive data"
        );
        if stream.set_nonblocking(true).is_err() {
            host.clear_receiver(&slot);
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
            slot,
            stream: mio::net::TcpStream::from_std(stream),
            thread,
            parser: ingest::Config {
                capabilities,
                update_every: self.defaults.update_every,
                page_size: self.defaults.page_size,
                now: collection::now_realtime_timeval,
                gap_when_lost_iterations_above: self.defaults.gap_when_lost_iterations_above,
            },
        };
        if let Err(attached) = self.pool.send(thread, attached) {
            attached.host.clear_receiver(&attached.slot);
            self.load.lock().unwrap_or_else(PoisonError::into_inner)[thread] -= 1;
        }
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
}

impl StreamWorker {
    pub fn new(load: Arc<Mutex<Vec<usize>>>) -> Self {
        StreamWorker {
            children: Vec::new(),
            load,
            tick: None,
        }
    }

    fn disconnect(&mut self, cx: &mut Context<'_>, index: usize) {
        if let Some(mut child) = self.children[index].take() {
            let attached = &mut child.attached;
            let _ = cx.registry().deregister(&mut attached.stream);
            attached.host.clear_receiver(&attached.slot);
            self.load.lock().unwrap_or_else(PoisonError::into_inner)[attached.thread] -= 1;
        }
    }

    /// Writes what the parser produced; a full socket keeps the rest for the next writable event.
    fn flush(&mut self, cx: &mut Context<'_>, index: usize) {
        let Some(child) = self.children[index].as_mut() else {
            return;
        };
        let out = child.parser.take_output();
        child.pending_out.extend_from_slice(&out);
        while !child.pending_out.is_empty() {
            match child.attached.stream.write(&child.pending_out) {
                Ok(n) => {
                    child.pending_out.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return self.disconnect(cx, index),
            }
        }
    }

    /// Reads what arrived and feeds every complete line to the parser; a refused line ends the connection.
    fn receive(&mut self, cx: &mut Context<'_>, index: usize) {
        let mut buf = [0u8; 16384];
        loop {
            let Some(child) = self.children[index].as_mut() else {
                return;
            };
            match child.attached.stream.read(&mut buf) {
                Ok(0) => return self.disconnect(cx, index),
                Ok(n) => {
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
                                nd_log!(Source::Daemon, Priority::Err, "STREAM RCV: {failure}");
                                return self.disconnect(cx, index);
                            }
                            plain = out;
                            &plain[..]
                        }
                    };
                    for line in child.reader.push(received) {
                        if !child.parser.feed(&line) {
                            return self.disconnect(cx, index);
                        }
                    }
                    self.flush(cx, index);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return self.disconnect(cx, index),
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

    fn event(&mut self, cx: &mut Context<'_>, event: &Event) {
        let index = event.token().0;
        if index >= self.children.len() {
            return;
        }
        if event.is_readable() || event.is_read_closed() {
            self.receive(cx, index);
        }
        if event.is_writable() {
            self.flush(cx, index);
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
            self.load.lock().unwrap_or_else(PoisonError::into_inner)[attached.thread] -= 1;
            return;
        }
        let parser = Parser::new(Arc::clone(&attached.host), attached.parser);
        let decompressor = Decompressor::for_capabilities(attached.parser.capabilities);
        self.children[index] = Some(Child {
            attached,
            decompressor,
            reader: LineReader::default(),
            parser,
            pending_out: Vec::new(),
        });
        // Bytes may have arrived before the registration.
        self.receive(cx, index);
    }

    fn timer(&mut self, cx: &mut Context<'_>, _timer: TimerId) {
        for index in 0..self.children.len() {
            if self.children[index]
                .as_ref()
                .is_some_and(|c| c.attached.slot.stop_requested.load(Ordering::Acquire))
            {
                self.disconnect(cx, index);
            }
        }
        self.tick = Some(cx.add_timer(Instant::now() + TICK));
    }

    fn stop(&mut self, cx: &mut Context<'_>) {
        for index in 0..self.children.len() {
            self.disconnect(cx, index);
        }
    }
}
