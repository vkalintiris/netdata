//! The receiver, ported from `src/streaming/stream-receiver-connection.c` (admission and the first response) and
//! `src/streaming/stream-thread.c` (assigning children to stream threads).
//!
//! Admission runs on the web worker that read the `STREAM` request, as in C: rejections decided before the takeover
//! go back on the web connection; after the takeover the socket is written with blocking sends and a timeout, then
//! handed to the least loaded stream thread.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use netdata_agent_evloop::{Context, Event, Interest, PoolHandle, TimerId, Token, Worker};
use netdata_agent_inicfg::LogLevel;
use netdata_agent_rrd::host::{Host, HostInfo, Hosts, ReceiverSlot};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};

use crate::caps;
use crate::conf::{ReceiverDefaults, StreamConf};
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

/// Where the receiver writes its daemon log lines.
pub type Logger = Box<dyn Fn(LogLevel, &str) + Send + Sync>;

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
}

/// The receiving side of this agent.
pub struct Receivers {
    pub conf: Mutex<StreamConf>,
    pub hosts: Arc<Hosts>,
    pub defaults: Defaults,
    pool: PoolHandle<Attached>,
    /// Children per stream thread (`nodes_count`).
    load: Arc<Mutex<Vec<usize>>>,
    log: Logger,
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
        log: Logger,
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
            log,
        }
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
            (self.log)(
                LogLevel::Info,
                &format!(
                    "STREAM RCV '{}' [from [{client_ip}]]: request has parameter '{name}' = '{value}', which is not used.",
                    request
                        .hostname
                        .as_deref()
                        .filter(|h| !h.is_empty())
                        .unwrap_or("-"),
                ),
            );
        }
        let validated = {
            let mut conf = self.conf.lock().unwrap_or_else(PoisonError::into_inner);
            handshake::validate(&mut request, &mut conf, client_ip)
        };
        if let Err(denied) = validated {
            (self.log)(LogLevel::Info, denied.message());
            return PreAdmission::Reply(handshake::ERROR_NOT_PERMITTED, 401);
        }
        let guid = request.machine_guid.clone().unwrap_or_default();
        if guid == self.hosts.localhost().machine_guid() {
            return PreAdmission::Refuse(handshake::ERROR_SAME_LOCALHOST);
        }
        // Virtual nodes (step 14) and the streaming rate limit of `[web]` (step 15) come with vnodes and the [web]
        // section.
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
                (self.log)(
                    LogLevel::Info,
                    "rejecting streaming connection; machine GUID is connected with a different hostname",
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
            (self.log)(
                LogLevel::Info,
                &format!(
                    "rejecting streaming connection; multiple connections for the same host, old connection was last used {age_s} secs ago{}",
                    if stale.is_some() {
                        " (signaled old receiver to stop)"
                    } else {
                        " (new connection not accepted)"
                    }
                ),
            );
            return PreAdmission::Reply(handshake::ERROR_ALREADY_STREAMING, 409);
        }
        PreAdmission::Proceed(Box::new(Pending { request }))
    }

    /// `PreAdmission::Refuse`: the connection has been taken over.
    pub fn refuse(&self, stream: TcpStream, message: &str) {
        if !send_timeout(&stream, message.as_bytes(), Duration::from_secs(60)) {
            (self.log)(LogLevel::Error, "STREAM RCV: failed to reply.");
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
            || HostInfo {
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
            (self.log)(
                LogLevel::Info,
                "rejecting streaming connection; host is already served by another receiver",
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
            (self.log)(
                LogLevel::Error,
                "STREAM RCV: cannot reply back, dropping connection",
            );
            host.clear_receiver(&slot);
            return;
        }
        (self.log)(LogLevel::Info, "connected and ready to receive data");
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

/// A stream thread: owns the connections of the children assigned to it.
pub struct StreamWorker {
    children: Vec<Option<Attached>>,
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
            let _ = cx.registry().deregister(&mut child.stream);
            child.host.clear_receiver(&child.slot);
            self.load.lock().unwrap_or_else(PoisonError::into_inner)[child.thread] -= 1;
        }
    }

    /// Reads what arrived. The keyword parser is the next step of slice 1; until then the bytes only count as
    /// traffic.
    fn receive(&mut self, cx: &mut Context<'_>, index: usize) {
        let Some(child) = self.children[index].as_mut() else {
            return;
        };
        let mut buf = [0u8; 16384];
        loop {
            match child.stream.read(&mut buf) {
                Ok(0) => return self.disconnect(cx, index),
                Ok(_) => child
                    .slot
                    .last_traffic_ut
                    .store(now_monotonic_ut(), Ordering::Relaxed),
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
        if index < self.children.len() {
            self.receive(cx, index);
        }
    }

    fn message(&mut self, cx: &mut Context<'_>, mut child: Attached) {
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
            .register(&mut child.stream, Token(index), Interest::READABLE)
            .is_err()
        {
            child.host.clear_receiver(&child.slot);
            self.load.lock().unwrap_or_else(PoisonError::into_inner)[child.thread] -= 1;
            return;
        }
        self.children[index] = Some(child);
        // Bytes may have arrived before the registration.
        self.receive(cx, index);
    }

    fn timer(&mut self, cx: &mut Context<'_>, _timer: TimerId) {
        for index in 0..self.children.len() {
            if self.children[index]
                .as_ref()
                .is_some_and(|c| c.slot.stop_requested.load(Ordering::Acquire))
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
