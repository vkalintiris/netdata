//! The sender's connector (`src/streaming/stream-connector.c`): one thread, `SNDR-CN[0]`, started with the first
//! queued host, that connects every queued host to one of its parents (blocking, one host at a time, D100.8), runs
//! the child's side of the handshake and hands the connection to the host's stream thread. Map:
//! `knowledge/map-m7-commit3-connector.md` §2, §6-§9.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use netdata_agent_evloop::PoolHandle;
use netdata_agent_log::{Field, Priority, Source, Value, nd_log, push};
use netdata_agent_rrd::host::{Host, sender_flags};
use netdata_agent_rrd::pulse::host_status;
use netdata_agent_tls::SslContext;

use crate::caps;
use crate::compress::Compressor;
use crate::compression::Algorithm;
use crate::connect_to::{NdSock, SockError, Thread, log_errno};
use crate::handshake;
use crate::parents::Local;
use crate::pins::Pins;
use crate::reason::Reason;
use crate::thread::StreamMsg;
use crate::sender::{Connected, Sender, Settings};

/// `CONNECTED_TO_SIZE`: what `s->remote_ip` keeps of the destination.
const CONNECTED_TO_SIZE: usize = 100;
/// The handshake's answer buffer, less its NUL.
const RESPONSE_SIZE: usize = 4095;

/// `STRCNT_CMD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cmd {
    Connect,
    Remove,
}

/// A row of `stream_responses[]`: the parent's answer, the version it means and, for a refusal, how the child logs
/// it and how long it waits.
struct Row {
    response: &'static str,
    version: i32,
    /// The VN prompt: the version follows it.
    dynamic: bool,
    error: Option<&'static str>,
    secs: i64,
    priority: Priority,
}

const fn refusal(response: &'static str, reason: Reason, error: &'static str, secs: i64, priority: Priority) -> Row {
    Row { response, version: reason.0, dynamic: false, error: Some(error), secs, priority }
}

/// `stream_responses[]`, the terminator last.
const RESPONSES: [Row; 11] = [
    Row { response: caps::PROMPT_VN, version: 3, dynamic: true, error: None, secs: 0, priority: Priority::Info },
    Row { response: caps::PROMPT_V2, version: 2, dynamic: false, error: None, secs: 0, priority: Priority::Info },
    Row { response: caps::PROMPT_V1, version: 1, dynamic: false, error: None, secs: 0, priority: Priority::Info },
    refusal(
        handshake::ERROR_SAME_LOCALHOST,
        Reason::PARENT_IS_LOCALHOST,
        "remote server rejected this stream, the host we are trying to stream is its localhost",
        3600,
        Priority::Debug,
    ),
    refusal(
        handshake::ERROR_LOCAL_VNODE,
        Reason::PARENT_VNODE_IS_LOCAL,
        "remote server rejected this stream, the vnode is collected locally on that server",
        3600,
        Priority::Debug,
    ),
    refusal(
        handshake::ERROR_ALREADY_STREAMING,
        Reason::PARENT_NODE_ALREADY_CONNECTED,
        "remote server rejected this stream, the host we are trying to stream is already streamed to it",
        120,
        Priority::Debug,
    ),
    refusal(
        handshake::ERROR_NOT_PERMITTED,
        Reason::PARENT_DENIED_ACCESS,
        "remote server denied access, probably we don't have the right API key?",
        60,
        Priority::Err,
    ),
    refusal(
        handshake::ERROR_BUSY_TRY_LATER,
        Reason::PARENT_BUSY_TRY_LATER,
        "remote server is currently busy, we should try later",
        120,
        Priority::Notice,
    ),
    refusal(
        handshake::ERROR_INTERNAL_ERROR,
        Reason::PARENT_INTERNAL_ERROR,
        "remote server is encountered an internal error, we should try later",
        300,
        Priority::Crit,
    ),
    refusal(
        handshake::ERROR_INITIALIZATION,
        Reason::PARENT_IS_INITIALIZING,
        "remote server is initializing, we should try later",
        30,
        Priority::Notice,
    ),
    refusal(
        "",
        Reason::CONNECT_HANDSHAKE_FAILED,
        "remote node response is not understood, is it Netdata?",
        60,
        Priority::Err,
    ),
];

/// The walk of `stream_connect_validate_first_response()`: the version the answer means and its row. The VN prompt
/// carries any version after it (a number of 0 or less stops at that row, whose texts are empty).
fn response_version(http: &[u8]) -> (i32, &'static Row) {
    let text = netdata_agent_text::c::c_str(http);
    for row in &RESPONSES[..RESPONSES.len() - 1] {
        let len = row.response.len();
        if row.dynamic && http.len() > len && http.len() < len + 30 && text.starts_with(row.response.as_bytes()) {
            return (netdata_agent_text::parse::str2i(&text[len..]), row);
        }
        if http.len() == len && text == row.response.as_bytes() {
            return (row.version, row);
        }
    }
    let last = &RESPONSES[RESPONSES.len() - 1];
    (last.version, last)
}

/// What the host's receiver negotiated with its child, 0 without one (a proxied host's ML_MODELS).
fn receiver_capabilities(host: &Host) -> u32 {
    host.receiver().map_or(0, |slot| slot.link.capabilities)
}

/// `buffer_key_value_urlencode()`: `key=` and the url-encoded value.
fn key_value(wb: &mut Vec<u8>, key: &str, value: &str) {
    wb.extend_from_slice(key.as_bytes());
    wb.push(b'=');
    netdata_agent_text::url::url_encode(wb, value.as_bytes());
}

/// What the sender needs of the daemon: its claim and Cloud state, and the Cloud URL a parent may hand down.
pub struct Env {
    /// `is_agent_claimed()`.
    pub claimed: Box<dyn Fn() -> bool + Send + Sync>,
    /// `aclk_online()`.
    pub aclk_online: Box<dyn Fn() -> bool + Send + Sync>,
    /// `cloud_config_url_set()`.
    pub set_cloud_url: Box<dyn Fn(&str) + Send + Sync>,
    /// `cloud_config_url_get()`, which a NODE_ID to a child carries.
    pub cloud_url: Box<dyn Fn() -> String + Send + Sync>,
}

impl Default for Env {
    /// An agent that is not claimed and whose Cloud URL does not change from C's default.
    fn default() -> Self {
        Env {
            claimed: Box::new(|| false),
            aclk_online: Box::new(|| false),
            set_cloud_url: Box::new(|_| {}),
            cloud_url: Box::new(|| "https://app.netdata.cloud".to_string()),
        }
    }
}

/// `struct connector` (`MAX_CONNECTORS` is 1).
pub struct Connector {
    pub(crate) settings: Settings,
    local: Local,
    localhost: std::sync::Weak<Host>,
    env: std::sync::OnceLock<Env>,
    pool: PoolHandle<StreamMsg>,
    pins: Arc<Mutex<Pins>>,
    stack_size: usize,
    /// `sc->queue.senders`, in the order hosts were queued.
    queue: Mutex<BTreeMap<u64, (Arc<Sender>, Cmd)>>,
    idx: AtomicU64,
    completion: netdata_agent_evloop::completion::Completion,
    /// `nd_thread_signal_cancel()` of the thread.
    cancel: AtomicBool,
    /// `service_signal_exit(SERVICE_STREAMING_CONNECTOR)`.
    exit: AtomicBool,
    started: Mutex<bool>,
    /// The thread, for the shutdown's wait of every remaining thread (`[13/22]`).
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The replication requests of every sender, which the REPLAY threads answer (D111.1).
    replication: Arc<crate::replication::Queue>,
    /// `netdata_ssl_streaming_sender_ctx`: built for the first host with an `:SSL` parent, again while it fails.
    tls: Mutex<Option<SslContext>>,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector").field("settings", &self.settings).finish_non_exhaustive()
    }
}

impl Connector {
    /// The connector of the stream threads of `pool` (whose hosts `pins` places); its thread starts with the first
    /// host.
    pub fn new(
        settings: Settings,
        local: Local,
        localhost: &Arc<Host>,
        pool: PoolHandle<StreamMsg>,
        pins: Arc<Mutex<Pins>>,
        stack_size: usize,
    ) -> Arc<Connector> {
        Arc::new(Connector {
            settings,
            local,
            localhost: Arc::downgrade(localhost),
            env: std::sync::OnceLock::new(),
            pool,
            pins,
            stack_size,
            queue: Mutex::default(),
            idx: AtomicU64::new(0),
            completion: Default::default(),
            cancel: AtomicBool::new(false),
            exit: AtomicBool::new(false),
            started: Mutex::new(false),
            thread: Mutex::new(None),
            replication: Arc::default(),
            tls: Mutex::new(None),
        })
    }

    /// `rrdhost_stream_parent_ssl_init()`: the sender's TLS context, built once some host's parents include an
    /// `:SSL` one, with the configured CA locations (C's records carry the host's frame, which the caller pushed).
    /// A context OpenSSL could not make is tried again at the next host's start, as C's.
    pub(crate) fn ssl_init(&self, ssl_parent: bool) {
        let mut tls = self.tls.lock().unwrap_or_else(PoisonError::into_inner);
        if tls.is_some() || !ssl_parent {
            return;
        }
        let s = &self.settings;
        *tls = netdata_agent_tls::streaming_sender_context(&netdata_agent_tls::ClientConfig {
            skip_verification: !s.ssl_validate_certificate,
            ca_file: &s.ssl_ca_file,
            ca_path: &s.ssl_ca_path,
        });
    }

    /// The context an attempt connects with (`s->sock.ctx`, re-read at every attempt): a clone shares it.
    fn tls(&self) -> Option<SslContext> {
        self.tls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The senders' replication requests, for the REPLAY threads.
    pub fn replication(&self) -> &Arc<crate::replication::Queue> {
        &self.replication
    }

    fn queue(&self) -> MutexGuard<'_, BTreeMap<u64, (Arc<Sender>, Cmd)>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `stream_threads_cancel()`'s part: attempts in progress stop at their next check.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// `service_signal_exit(SERVICE_STREAMING_CONNECTOR)`: a flag the thread sees at its next wake, as C's (no
    /// request-quit callback wakes it); it then removes every queued host over five more passes, 250 ms apart.
    pub fn signal_exit(&self) {
        self.exit.store(true, Ordering::Relaxed);
    }

    pub(crate) fn pool(&self) -> &PoolHandle<StreamMsg> {
        &self.pool
    }

    pub(crate) fn local(&self) -> &Local {
        &self.local
    }

    pub(crate) fn localhost(&self) -> Option<Arc<Host>> {
        self.localhost.upgrade()
    }

    /// Sets the daemon's side (once, when the daemon has it).
    pub fn set_env(&self, env: Env) {
        let _ = self.env.set(env);
    }

    pub(crate) fn env(&self) -> &Env {
        static DEFAULT: std::sync::OnceLock<Env> = std::sync::OnceLock::new();
        self.env.get().unwrap_or_else(|| DEFAULT.get_or_init(Env::default))
    }

    /// `stream_connector_init()`: starts the thread once; false when it could not start.
    pub(crate) fn init(self: &Arc<Self>, hostname: &str) -> bool {
        let mut started = self.started.lock().unwrap_or_else(PoisonError::into_inner);
        if *started {
            return true;
        }
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("SNDR-CN[0]".into())
            .stack_size(self.stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                me.run();
                netdata_agent_log::thread_finished();
            });
        match spawned {
            Ok(join) => {
                *started = true;
                *self.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(join);
            }
            Err(_) => nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM CONNECT '{hostname}': failed to create new thread for client."
            ),
        }
        *started
    }

    /// `service_wait_exit(~0, limit)`'s wait for the thread, which ends six passes after the exit started.
    pub fn join_within(&self, limit: Duration) {
        let Some(join) = self.thread.lock().unwrap_or_else(PoisonError::into_inner).take() else {
            return;
        };
        // an exit on this very thread does not wait for itself, as C's waits skip their caller
        if join.thread().id() == std::thread::current().id() {
            return;
        }
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline && !join.is_finished() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if join.is_finished() {
            let _ = join.join();
        }
    }

    /// `stream_connector_add()`: once per session, the host waits for its parents' reset delay and is queued.
    pub(crate) fn add(&self, s: &Arc<Sender>, host: &Arc<Host>) {
        {
            let mut state = s.lock();
            // a host freed meanwhile (the sender's ENABLED bit)
            if host.sender_flags() & sender_flags::ENABLED == 0 {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "STREAM CONNECT '{}' [disabled]: host has streaming disabled - not sending data to a parent.",
                    host.hostname()
                );
                return;
            }
            if host.sender_flags() & sender_flags::ADDED != 0 {
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "STREAM CONNECT '{}' [duplicate]: host has already added to sender - ignoring request.",
                    host.hostname()
                );
                return;
            }
            host.sender_flags_set(sender_flags::ADDED);
            host.sender_flags_clear(sender_flags::CONNECTED | sender_flags::READY_4_METRICS);
            state.parent_using_h2o = self.settings.h2o;
        }
        s.parents().reset(Reason::NEVER, self.settings.reconnect_delay_s);
        self.requeue(s, host, Cmd::Connect);
    }

    /// `stream_connector_remove_host()`: a sender still in the queue leaves it, its disconnect hooks run and it is
    /// removed with its exit reason, without the connector's record.
    pub(crate) fn remove_host(&self, s: &Arc<Sender>, host: &Host) {
        let found = {
            let mut queue = self.queue();
            let key = queue.iter().find(|(_, (q, _))| Arc::ptr_eq(q, s)).map(|(&k, _)| k);
            key.and_then(|k| queue.remove(&k))
        };
        if found.is_some() {
            let _frame = s.frame();
            s.on_disconnect(host);
            let reason = s.lock().exit_reason;
            s.remove(host, reason);
        }
    }

    /// `stream_connector_requeue()`.
    pub(crate) fn requeue(&self, s: &Arc<Sender>, host: &Host, cmd: Cmd) {
        self.requeue_after_close(s, host, cmd, 0);
    }

    /// [`Connector::requeue`], its record carrying `errno` (what a TLS close left, D107.5); the `errno` after it, which
    /// the record, when written, clears.
    pub(crate) fn requeue_after_close(&self, s: &Arc<Sender>, host: &Host, cmd: Cmd, mut errno: i32) -> i32 {
        if cmd == Cmd::Connect {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                errno = errno;
                "STREAM CONNECT '{}' [to parent]: adding host in connector queue...",
                host.hostname()
            );
            if !netdata_agent_log::filtered(Source::Daemon, Priority::Debug) {
                errno = 0;
            }
            host.pulse_status(host_status::SND_PENDING);
        }
        {
            // the index is taken under the queue's lock, so the queue keeps the order of the requeues
            let mut queue = self.queue();
            let idx = self.idx.fetch_add(1, Ordering::Relaxed) + 1;
            queue.insert(idx, (Arc::clone(s), cmd));
        }
        self.completion.mark();
        errno
    }

    /// `stream_connector_thread()`: a pass over the queue at every new host or every second (250 ms while exiting).
    /// From its first wake after the exit started (or its signal) every host is removed, and it ends six passes later.
    fn run(&self) {
        let th = Thread::new(&self.cancel);
        let (mut job_id, mut exiting) = (0, 0);
        while exiting <= 5 {
            job_id = self.completion.wait(job_id, Duration::from_millis(if exiting > 0 { 250 } else { 1000 }));
            // service_running(SERVICE_STREAMING_CONNECTOR): false once signalled or once the exit started (D110)
            if self.exit.load(Ordering::Relaxed) || netdata_agent_sys::exit::initiated() {
                exiting += 1;
            }
            let mut next = 0;
            loop {
                let entry = self.queue().range(next..).next().map(|(&k, (s, c))| (k, Arc::clone(s), *c));
                let Some((key, s, cmd)) = entry else {
                    break;
                };
                next = key + 1;
                let Some(host) = s.host() else {
                    self.queue().remove(&key);
                    // no hook runs for a freed host: its requests leave the replication queue here, parked ones too
                    self.replication.delete_pending(s.replication());
                    continue;
                };
                let _frame = s.frame();
                // each branch acts on the entry it takes out: a free's remove_host() may have taken it meanwhile
                if s.shutdown.load(Ordering::Relaxed) {
                    if self.queue().remove(&key).is_some() {
                        s.on_disconnect(&host);
                        s.connector_remove(&host);
                    }
                    continue;
                }
                match if exiting > 0 { Cmd::Remove } else { cmd } {
                    Cmd::Connect => {
                        if let Some(connected) = self.stream_connect(&s, &host, &th) {
                            if self.queue().remove(&key).is_none() {
                                // removed during the attempt: never dispatched, and a TLS link's close does not wait
                                // on the parent (R46 n5)
                                if let Some(conn) = connected.link.socket() {
                                    let _ = socket2::SockRef::from(conn).set_nonblocking(true);
                                }
                                continue;
                            }
                            s.on_connect(&host, &connected.link);
                            self.add_to_queue(connected, &host);
                        }
                    }
                    Cmd::Remove => {
                        if self.queue().remove(&key).is_some() {
                            s.on_disconnect(&host);
                            let reason = s.lock().exit_reason;
                            s.remove(&host, reason);
                        }
                    }
                }
            }
        }
    }

    /// `stream_sender_add_to_queue()`: the host's stream thread takes the connection.
    fn add_to_queue(&self, connected: Connected, host: &Host) {
        let thread = self.pins.lock().unwrap_or_else(PoisonError::into_inner).queue(host.machine_guid());
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "STREAM THREAD[{thread}] '{}': moving host to sender queue...",
            host.hostname()
        );
        host.pulse_status(host_status::SND_WAITING);
        let connected = Connected { thread, ..connected };
        if let Err(StreamMsg::AttachSender(connected)) = self.pool.send(thread, StreamMsg::AttachSender(Box::new(connected))) {
            self.pins.lock().unwrap_or_else(PoisonError::into_inner).remove(host.machine_guid());
            connected.sender.remove(host, Reason::DISCONNECT_SIGNALED_TO_STOP);
        }
    }

    /// The `STREAM` request of `stream_connect()`.
    fn request(&self, host: &Host, api_key: &str, hops: i16, capabilities: u32) -> Vec<u8> {
        let info = host.info();
        let mut wb = b"STREAM ".to_vec();
        key_value(&mut wb, "key", api_key);
        key_value(&mut wb, "&hostname", &info.hostname);
        key_value(&mut wb, "&registry_hostname", &info.registry_hostname);
        key_value(&mut wb, "&machine_guid", host.machine_guid());
        wb.extend_from_slice(format!("&update_every={}", self.local.update_every).as_bytes());
        key_value(&mut wb, "&os", &info.os);
        key_value(&mut wb, "&timezone", &info.timezone);
        key_value(&mut wb, "&abbrev_timezone", &info.abbrev_timezone);
        wb.extend_from_slice(format!("&utc_offset={}&hops={hops}&ver={capabilities}", info.utc_offset).as_bytes());
        info.system_info.to_url_encode_stream(&mut wb);
        key_value(&mut wb, "&NETDATA_PROTOCOL_VERSION", "1.1");
        wb.extend_from_slice(
            format!(
                " HTTP/1.1\r\nUser-Agent: {}/{}\r\nAccept: */*\r\n\r\n",
                info.program_name, info.program_version
            )
            .as_bytes(),
        );
        wb
    }

    /// `stream_connect()`: a parent, the request, its answer and the compressor; the connection on success.
    fn stream_connect(&self, s: &Arc<Sender>, host: &Arc<Host>, th: &Thread<'_>) -> Option<Connected> {
        // the sender's state is only copied in and out, so nothing waits on it across the attempt's I/O
        let mut attempt = {
            let st = s.lock();
            Attempt {
                hops: host.ingestion_hops().wrapping_add(1),
                status_reason: st.status_reason,
                capabilities: st.capabilities,
                remote_ip: st.remote_ip.clone(),
                parent_using_h2o: st.parent_using_h2o,
                api_key: st.api_key.clone(),
            }
        };
        let connected = self.attempt(s, host, &mut attempt, th);
        let mut st = s.lock();
        st.hops = attempt.hops;
        st.status_reason = attempt.status_reason;
        st.capabilities = attempt.capabilities;
        st.remote_ip = attempt.remote_ip;
        // C sets the compressor up only on a connect that succeeds
        if let Some(c) = &connected {
            st.compression = c.compressor.is_some();
        }
        connected
    }

    /// The parents are locked while `connect_to_one` picks and connects one, as C holds their read lock
    /// (`stream-parents.c:911-914`); the handshake's writes after it take the lock briefly, where C writes unlocked.
    fn attempt(&self, s: &Arc<Sender>, host: &Arc<Host>, st: &mut Attempt, th: &Thread<'_>) -> Option<Connected> {
        let settings = &self.settings;
        // the context and the verification as they are now (stream-connector.c:278-280)
        let mut sock = NdSock::new(self.tls(), settings.ssl_validate_certificate);
        // nd_sock_close() of the previous socket clears errno
        sock.close(th);
        host.pulse_status(host_status::SND_PENDING);
        let connected = s.parents().connect_to_one(
            &mut sock,
            host,
            &self.local,
            st.hops,
            &mut st.status_reason,
            settings.default_port,
            settings.timeout_s,
            th,
        );
        if !connected {
            if sock.error != SockError::NoDestinationAvailable {
                log_errno!(th, Priority::Warning, "can't connect to a parent, last error: {}", sock.error.text());
            }
            return None;
        }
        let destination = s.parents().current().map(|d| d.destination.clone()).unwrap_or_default();
        st.remote_ip = crate::records::cut(&destination, CONNECTED_TO_SIZE).to_string();
        st.capabilities = caps::sender_ours(s.disabled.load(Ordering::Relaxed), receiver_capabilities(host));
        let request = self.request(host, &st.api_key, st.hops, st.capabilities);
        let hostname = host.hostname();
        let remote = st.remote_ip.clone();
        if st.parent_using_h2o && !crate::h2o::upgrade_prelude(&mut sock, th) {
            sock.close(th);
            s.parents().set_connect_failure_reason(host, &mut st.status_reason, Reason::SND_DISCONNECT_HTTP_UPGRADE_FAILED, 60);
            return None;
        }
        if sock.send_timeout(&request, settings.timeout_s, th) <= 0 {
            let _frame = push(vec![(Field::ResponseCode, Value::I64(Reason::CONNECT_SEND_TIMEOUT.code()))]);
            sock.close(th);
            log_errno!(
                th,
                Priority::Err,
                "STREAM CONNECT '{hostname}' [to {remote}]: failed to send HTTP header to remote netdata."
            );
            s.parents().set_connect_failure_reason(host, &mut st.status_reason, Reason::CONNECT_SEND_TIMEOUT, 60);
            return None;
        }
        let mut response = [0u8; RESPONSE_SIZE];
        let bytes = sock.recv_timeout(&mut response, settings.timeout_s, th);
        if bytes <= 0 {
            sock.close(th);
            let _frame = push(vec![(Field::ResponseCode, Value::I64(Reason::CONNECT_RECEIVE_TIMEOUT.code()))]);
            log_errno!(th, Priority::Err, "STREAM CONNECT '{hostname}' [to {remote}]: remote netdata does not respond.");
            s.parents().set_connect_failure_reason(host, &mut st.status_reason, Reason::CONNECT_RECEIVE_TIMEOUT, 30);
            return None;
        }
        if !self.validate_first_response(s, host, st, &response[..bytes as usize], th) {
            sock.close(th);
            return None;
        }
        // stream_compression_initialize()
        let compressor = Algorithm::for_capabilities(st.capabilities)
            .and_then(|algorithm| Compressor::new(algorithm, &settings.compression_levels));
        // log_sender_capabilities()
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "STREAM SND '{hostname}' [to {remote}]: established link with negotiated capabilities: {}",
            caps::to_string(st.capabilities)
        );
        let _frame = push(vec![(Field::ResponseCode, Value::I64(200))]);
        log_errno!(th, Priority::Debug, "STREAM CONNECT '{hostname}' [to {remote}]: connected to parent...");
        Some(Connected {
            sender: Arc::clone(s),
            link: sock.take_link()?,
            capabilities: st.capabilities,
            compressor,
            remote_ip: remote,
            thread: 0,
        })
    }

    /// `stream_connect_validate_first_response()`: a prompt negotiates the capabilities; a refusal postpones the
    /// parent and says when it is tried again.
    fn validate_first_response(&self, s: &Sender, host: &Host, st: &mut Attempt, http: &[u8], th: &Thread<'_>) -> bool {
        let (version, row) = response_version(http);
        if version >= 1 {
            s.parents().set_reconnect_delay(Reason::SP_CONNECTED, self.settings.reconnect_delay_s);
            let ours = caps::sender_ours(s.disabled.load(Ordering::Relaxed), receiver_capabilities(host));
            st.capabilities = caps::negotiate(version, ours);
            st.status_reason = Reason(st.capabilities as i32);
            return true;
        }
        let reason = Reason(version);
        s.parents().set_connect_failure_reason(host, &mut st.status_reason, reason, row.secs);
        let _frame = push(vec![(Field::ResponseCode, Value::I64(reason.code()))]);
        let postponed_ut = s.parents().current().map_or(0, |d| d.postpone_until_ut);
        let at = netdata_agent_log::rfc3339_local(postponed_ut, 0);
        log_errno!(
            th,
            row.priority,
            "STREAM CONNECT '{}' [to {}]: {} - will retry in {} secs, at {at}",
            host.hostname(),
            st.remote_ip,
            row.error.unwrap_or("(null)"),
            row.secs
        );
        false
    }
}

/// What an attempt reads and writes of the sender's state.
struct Attempt {
    hops: i16,
    status_reason: Reason,
    capabilities: u32,
    remote_ip: String,
    parent_using_h2o: bool,
    api_key: String,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Instant;

    use netdata_agent_evloop::Pool;
    use netdata_agent_rrd::contexts::Taker;
    use netdata_agent_rrd::host::{Attach, HostInfo, ReceiverSlot, StreamSend};
    use netdata_agent_rrd::mode::DbMode;
    use netdata_agent_rrd::testing::{open_buffer, take_queued};

    use super::*;
    use crate::conf::Send;

    pub(crate) fn info(destination: &str, key: &str) -> HostInfo {
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
            stream_send: StreamSend::new(!destination.is_empty(), destination, key, "*"),
            cache_dir: None,
        }
    }

    /// A connector with one stream thread, its own thread not started.
    pub(crate) fn connector() -> (Pool<StreamMsg>, Arc<Connector>) {
        let pins = Arc::new(Mutex::new(Pins::new(1)));
        let pool = Pool::spawn(1, 256 * 1024, |i| format!("TEST[{i}]"), |_| {
            crate::thread::StreamWorker::new(Arc::clone(&pins), 1)
        })
        .unwrap();
        let localhost = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000aa", true, info("", "")));
        let local = Local { host_id: [0xaa; 16], user_agent: "t/1".into(), update_every: 1 };
        let c = Connector::new(Settings::of(&Send::default()), local, &localhost, pool.handle(), pins, 256 * 1024);
        (pool, c)
    }

    /// `stream_sender_charts_and_replication_reset()`'s last two stores (`stream-sender.c:116-117`): a removal's
    /// disconnect hooks zero the replication commands received and answered, which the stall check sums.
    #[test]
    fn a_removal_zeroes_the_replication_counters() {
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c6", false, info("127.0.0.1:1", "key")));
        let s = Sender::attach(&host, &c).expect("created");
        host.sender_flags_set(sender_flags::ADDED);
        c.requeue(&s, &host, Cmd::Connect);
        s.counter_in.store(3, Ordering::Relaxed);
        s.counter_out.store(4, Ordering::Relaxed);
        c.remove_host(&s, &host);
        assert_eq!((s.counter_in.load(Ordering::Relaxed), s.counter_out.load(Ordering::Relaxed)), (0, 0));
    }

    /// `stream_sender_on_disconnect()` (`stream-sender.c:202`): the connector's removal sends the host's node id down
    /// to its child; a second removal, the sender no longer queued, runs no hook.
    #[test]
    fn a_removal_sends_the_node_id_down_to_the_child() {
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000ca", false, info("127.0.0.1:1", "key")));
        let s = Sender::attach(&host, &c).expect("created");
        let link = netdata_agent_rrd::host::ReceiverLink { capabilities: caps::NODE_ID, ..Default::default() };
        let slot = Arc::new(ReceiverSlot::new(0, Default::default(), link, Box::new(|| {})));
        open_buffer(&slot);
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        host.set_node_id([0x33; 16]);
        host.update_claim_id_of_parent([0x22; 16]);
        host.sender_flags_set(sender_flags::ADDED);
        c.requeue(&s, &host, Cmd::Connect);
        c.remove_host(&s, &host);
        assert_eq!(
            String::from_utf8(take_queued(&slot)).unwrap(),
            "NODE_ID '22222222-2222-2222-2222-222222222222' '33333333-3333-3333-3333-333333333333' \
             'https://app.netdata.cloud'\n"
        );
        c.remove_host(&s, &host);
        assert!(take_queued(&slot).is_empty(), "not queued: no hook runs");
    }

    /// `stream_parents_host_reset()` at the connector's add (NEVER), at its removal (the exit reason) and at a
    /// receiver's end through the sender's `parents_reset()` (the receiver's reason): every parent postponed to one
    /// draw in [7, 20) s (the default 15 s delay), session bans lifted, the permanent ban kept.
    #[test]
    fn the_connectors_add_and_removal_and_a_receivers_end_reset_the_parents() {
        use netdata_agent_rrd::clock::now_realtime_ut;
        let (_pool, c) = connector();
        let host = Arc::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000c6",
            false,
            info("127.0.0.1:1 127.0.0.1:2", "key"),
        ));
        let s = Sender::attach(&host, &c).expect("created");
        let dirty = |s: &Sender| {
            let mut parents = s.parents();
            for d in &mut parents.list {
                d.reason = Reason::SP_CONNECTION_REFUSED;
                d.postpone_until_ut = 0;
            }
            parents.list[0].banned_for_this_session = true;
            parents.list[1].banned_permanently = true;
        };
        let check = |s: &Sender, before: u64, reason: Reason| {
            let after = now_realtime_ut();
            let parents = s.parents();
            let until = parents.list[0].postpone_until_ut;
            assert!(until >= before + 7_000_000 && until < after + 20_000_000, "{reason:?}");
            let states: Vec<_> = parents
                .list
                .iter()
                .map(|d| (d.postpone_until_ut, d.banned_for_this_session, d.banned_permanently, d.reason))
                .collect();
            assert_eq!(states, [(until, false, false, reason), (until, false, true, reason)]);
        };
        dirty(&s);
        let before = now_realtime_ut();
        c.add(&s, &host);
        check(&s, before, Reason::NEVER);
        assert_eq!(c.queue().len(), 1);
        dirty(&s);
        s.signal_stop(Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE, crate::sender::op::STOP_RECEIVER_LEFT);
        let before = now_realtime_ut();
        c.remove_host(&s, &host);
        check(&s, before, Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE);
        assert!(c.queue().is_empty());
        dirty(&s);
        let slot = Arc::new(ReceiverSlot::new(0, Default::default(), Default::default(), Box::new(|| {})));
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        let before = now_realtime_ut();
        host.clear_receiver(&slot, Reason::DISCONNECT_SOCKET_WRITE_FAILED.0);
        check(&s, before, Reason::DISCONNECT_SOCKET_WRITE_FAILED);
    }

    /// `stream_path_parent_disconnected()` at a sender's removal (not at a socket disconnect): the path is cut after
    /// this agent's entry and sent down to the child when something was cut; a second removal sends nothing.
    #[test]
    fn a_removal_cuts_the_path_after_this_agent_and_sends_it_down() {
        use netdata_agent_rrd::host::ReceiverLink;
        use netdata_agent_rrd::stream_path::PathEntry;
        // the connector holds localhost weakly: the test keeps it (the helper's would be gone)
        let pins = Arc::new(Mutex::new(Pins::new(1)));
        let pool = Pool::spawn(1, 256 * 1024, |i| format!("TEST[{i}]"), |_| {
            crate::thread::StreamWorker::new(Arc::clone(&pins), 1)
        })
        .unwrap();
        let localhost = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000aa", true, info("", "")));
        let local = Local { host_id: [0xaa; 16], user_agent: "t/1".into(), update_every: 1 };
        let c = Connector::new(Settings::of(&Send::default()), local, &localhost, pool.handle(), pins, 256 * 1024);
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c9", false, info("127.0.0.1:1", "key-a")));
        let s = Sender::attach(&host, &c).expect("created");
        let link = ReceiverLink { capabilities: caps::PATHS, ..ReceiverLink::default() };
        let slot = Arc::new(ReceiverSlot::new(1, Default::default(), link, Box::new(|| {})));
        open_buffer(&slot);
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        let us = c.local().host_id;
        let entry = |host_id: [u8; 16], hops| PathEntry { host_id, hops, ..PathEntry::default() };
        host.replace_stream_path(vec![entry([0xc9; 16], 0), entry(us, 1), entry([0xbb; 16], 2), entry([0xcc; 16], 3)]);
        let path = |host: &Host| host.stream_path().iter().map(|e| (e.host_id, e.hops)).collect::<Vec<_>>();
        host.sender_flags_set(sender_flags::ADDED);
        c.requeue(&s, &host, Cmd::Connect);
        c.remove_host(&s, &host);
        assert_eq!(path(&host), [([0xc9; 16], 0), (us, 1)]);
        let sent = take_queued(&slot);
        assert!(sent.starts_with(b"JSON STREAM_PATH\n"));
        assert_eq!(sent, netdata_agent_ingest::stream_path::message(&host, &localhost, None));
        c.requeue(&s, &host, Cmd::Connect);
        c.remove_host(&s, &host);
        assert_eq!(path(&host), [([0xc9; 16], 0), (us, 1)]);
        assert!(take_queued(&slot).is_empty());
    }

    /// The free at a host's cleanup (HOST CLEANUP) takes a queued sender off the connector at once, without the
    /// connector's record, and the host streams no more; the revival sets the same sender up with its new key and
    /// parents (D118), published at once, and since the agent's start as C's new sender is.
    #[test]
    fn a_free_takes_a_queued_sender_off_the_connector_and_a_revival_sets_it_up_again() {
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c1", false, info("127.0.0.1:1", "key-a")));
        let s = Sender::attach(&host, &c).expect("created");
        host.sender_flags_set(sender_flags::ADDED);
        c.requeue(&s, &host, Cmd::Connect);
        assert_eq!(c.queue().len(), 1);
        let ((), records) = netdata_agent_log::capture(|| host.cleanup_data_collection());
        assert!(c.queue().is_empty());
        assert_eq!(host.sender_flags() & (sender_flags::ADDED | sender_flags::ENABLED), 0);
        assert!(host.upstream().is_none());
        let texts: Vec<String> = records.iter().filter_map(|r| r.message.clone()).collect();
        assert!(!texts.iter().any(|t| t.contains("removed host") || t.contains("giving up")), "{texts:?}");
        assert!(s.parents().list.is_empty());
        assert_ne!(netdata_agent_rrd::upstream::Upstream::status(&*s).since_s, 0, "the removal's time");
        host.update(&info("127.0.0.2:2", "key-b"), 1, 3600, false, 0, 0);
        // a stop flag left from before cannot reach the revived sender
        s.shutdown.store(true, Ordering::Relaxed);
        assert!(Sender::attach(&host, &c).is_none(), "set up again, not created");
        // before any other hold of the parents
        let published: Vec<String> = netdata_agent_rrd::upstream::Upstream::published_parents(&*s)
            .into_iter()
            .map(|d| d.destination)
            .collect();
        assert_eq!(published, ["127.0.0.2:2"]);
        assert_eq!(netdata_agent_rrd::upstream::Upstream::status(&*s).since_s, 0);
        assert!(!s.shutdown.load(Ordering::Relaxed));
        assert!(host.upstream().is_some());
        assert_eq!(s.lock().api_key, "key-b");
        let destinations: Vec<String> = s.parents().list.iter().map(|d| d.destination.clone()).collect();
        assert_eq!(destinations, ["127.0.0.2:2"]);
    }

    /// `stream_receiver_send_node_and_claim_id_to_child()`: the host's node id with the parent's claim id and the
    /// Cloud URL, into the receiver's buffer; nothing for a zero node id or a child without NODE_ID.
    #[test]
    fn a_node_id_goes_down_to_a_child_that_takes_it() {
        let env = Env { cloud_url: Box::new(|| "https://nodeid.invalid".to_string()), ..Env::default() };
        let host = Host::new("5a1e0000-0000-4000-8000-0000000000c3", false, info("", ""));
        let attach = |caps: u32| {
            let link = netdata_agent_rrd::host::ReceiverLink { capabilities: caps, ..Default::default() };
            let slot = Arc::new(ReceiverSlot::new(0, Default::default(), link, Box::new(|| {})));
            open_buffer(&slot);
            assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
            slot
        };
        let slot = attach(caps::NODE_ID);
        crate::sender::send_node_and_claim_id_to_child(&host, &env);
        assert!(take_queued(&slot).is_empty(), "no node id yet");
        host.set_node_id([0x33; 16]);
        host.update_claim_id_of_parent([0x22; 16]);
        crate::sender::send_node_and_claim_id_to_child(&host, &env);
        assert_eq!(
            String::from_utf8(take_queued(&slot)).unwrap(),
            "NODE_ID '22222222-2222-2222-2222-222222222222' '33333333-3333-3333-3333-333333333333' \
             'https://nodeid.invalid'\n"
        );
        host.clear_receiver(&slot, 0);
        let slot = attach(0);
        crate::sender::send_node_and_claim_id_to_child(&host, &env);
        assert!(take_queued(&slot).is_empty(), "a child without NODE_ID");
        host.clear_receiver(&slot, 0);
        // a plugin claims its GUID as a local vnode: its receiver, still attached, gets none (`rrdhost_is_local()`)
        let slot = attach(caps::NODE_ID);
        host.set_virtual();
        crate::sender::send_node_and_claim_id_to_child(&host, &env);
        assert!(take_queued(&slot).is_empty(), "a local host");
    }

    /// `stream_sender_signal_to_stop_and_wait()`: a sender not ADDED is left as it is (nothing to stop); a queued one
    /// is marked with the receiver's reason, which the connector's record then names, and its removal clears the mark.
    #[test]
    fn a_stop_marks_a_queued_sender_and_not_one_that_is_not_added() {
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c4", false, info("127.0.0.1:1", "key")));
        let s = Sender::attach(&host, &c).expect("created");
        let reason = Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE;
        s.signal_stop(reason, crate::sender::op::STOP_RECEIVER_LEFT);
        assert!(!s.shutdown.load(Ordering::Relaxed));
        assert_eq!(s.lock().exit_reason, Reason::NEVER);
        host.sender_flags_set(sender_flags::ADDED);
        c.requeue(&s, &host, Cmd::Connect);
        s.signal_stop(reason, crate::sender::op::STOP_RECEIVER_LEFT);
        assert!(s.shutdown.load(Ordering::Relaxed));
        assert_eq!(s.lock().exit_reason, reason);
        // the connector's removal of a stopped entry: its record names the receiver's reason
        let ((), records) = netdata_agent_log::capture(|| s.connector_remove(&host));
        let texts: Vec<String> = records.iter().filter_map(|r| r.message.clone()).collect();
        let want = format!(
            "STREAM CNT 'child' [to ]: streaming connector removed host: {} (signaled to stop)",
            reason.text()
        );
        assert_eq!(texts, [want]);
        assert_eq!(host.sender_flags() & sender_flags::ADDED, 0);
        assert!(!s.shutdown.load(Ordering::Relaxed) && s.lock().exit_reason == Reason::NEVER);
    }

    /// A chart of `host` whose first point ends at `t`, through the contexts' queue: the host's first time becomes
    /// that point's start.
    pub(crate) fn collect_first_at(host: &Host, id: &str, t: i64) {
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
        let (chart, _) = host.charts().create(&ChartSpec {
            type_: "t",
            id,
            name: None,
            family: Some("fam"),
            context: Some(id),
            title: "Title",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 3600,
            page_size: 4096,
        });
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        for dim in chart.dims() {
            dim.store_metric(t as u64 * 1_000_000, 1.0, 0);
        }
        netdata_agent_rrd::contexts::collected_rrdset(&chart);
        host.contexts().process_queued();
        assert_eq!(host.contexts().retention().0, t - 1);
    }

    /// `stream_sender_on_ready_to_dispatch()` opens C's gate for a host's retention paths: its first-time changes
    /// are taken from then on (D120, D146.3), localhost's or a vnode's even when it was defined after the READY; a
    /// proxied host's stay its receiver's, its pending changes kept.
    #[test]
    fn the_ready_hook_records_a_hosts_first_time_changes() {
        let (_pool, c) = connector();
        let local = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c7", true, info("127.0.0.1:1", "key")));
        let s = Sender::attach(&local, &c).expect("created");
        collect_first_at(&local, "t.a", 1_790_000_000);
        assert!(local.contexts().take_first_time_changes(Taker::Sender).is_empty(), "not ready");
        s.on_ready_to_dispatch(&local, 0);
        collect_first_at(&local, "t.b", 1_789_999_990);
        assert_eq!(local.contexts().take_first_time_changes(Taker::Sender), [1_789_999_989]);

        // a vnode its plugin defines once the sender is ready
        let vnode = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c9", false, info("127.0.0.1:1", "key")));
        let s = Sender::attach(&vnode, &c).expect("created");
        s.on_ready_to_dispatch(&vnode, 0);
        vnode.set_virtual();
        collect_first_at(&vnode, "t.a", 1_790_000_000);
        assert_eq!(vnode.contexts().take_first_time_changes(Taker::Sender), [1_789_999_999]);

        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c8", false, info("127.0.0.1:1", "key")));
        let s = Sender::attach(&host, &c).expect("created");
        host.contexts().record_first_time_changes(Taker::Receiver, true);
        collect_first_at(&host, "t.a", 1_790_000_000);
        s.on_ready_to_dispatch(&host, 0);
        assert!(host.contexts().take_first_time_changes(Taker::Sender).is_empty(), "the receiver's");
        assert_eq!(host.contexts().take_first_time_changes(Taker::Receiver), [1_789_999_999], "kept for the receiver");
    }

    /// A host whose sender was freed is refused at the connector with C's record, and nothing is queued.
    #[test]
    fn a_disabled_host_is_not_queued() {
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c5", false, info("127.0.0.1:1", "key")));
        let s = Sender::attach(&host, &c).expect("created");
        host.cleanup_data_collection();
        let ((), records) = netdata_agent_log::capture(|| c.add(&s, &host));
        let texts: Vec<(netdata_agent_log::Priority, String)> =
            records.iter().map(|r| (r.priority, r.message.clone().unwrap_or_default())).collect();
        assert_eq!(
            texts,
            [(
                netdata_agent_log::Priority::Err,
                "STREAM CONNECT 'child' [disabled]: host has streaming disabled - not sending data to a parent."
                    .to_string()
            )]
        );
        assert!(c.queue().is_empty());
        assert_eq!(host.sender_flags() & sender_flags::ADDED, 0);
    }

    /// A sender that stays queued elsewhere is given up after the free's 2 s (D118.2), with the record, and left as
    /// it is; a session it gets meanwhile is given the stop.
    #[test]
    fn a_free_gives_up_on_a_sender_that_does_not_leave() {
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c2", false, info("127.0.0.1:1", "key")));
        let s = Sender::attach(&host, &c).expect("created");
        host.sender_flags_set(sender_flags::ADDED);
        // handed over to a stream thread while the free waits (one the pool lacks, so the post fails with a record)
        let late = {
            let s = Arc::clone(&s);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                s.out().session = Some(crate::sender::Session { thread: 9, id: 1 });
            })
        };
        let started = Instant::now();
        let ((), records) = netdata_agent_log::capture(|| host.cleanup_data_collection());
        late.join().unwrap();
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert!(host.upstream().is_none());
        let texts: Vec<String> = records.iter().filter_map(|r| r.message.clone()).collect();
        let giving_up = "STREAM SND 'child': sender takes too long to stop, giving up...".to_string();
        assert!(texts.contains(&giving_up), "{texts:?}");
        // the late session got the stop, once
        let stop = format!(
            "STREAM SND[x] 'child' [to ] the opcode ({}) message cannot be verified. Ignoring it.",
            crate::sender::op::STOP_HOST_CLEANUP
        );
        assert_eq!(texts.iter().filter(|t| **t == stop).count(), 1, "{texts:?}");
        // a sender still live is left as it is
        assert!(!s.parents().list.is_empty());
    }

    #[test]
    fn answers_map_to_c_s_versions() {
        let version = |answer: &str| response_version(answer.as_bytes()).0;
        assert_eq!(version(&format!("{}469565432", caps::PROMPT_VN)), 469_565_432);
        assert_eq!(version(caps::PROMPT_VN), 3);
        assert_eq!(version(&format!("{}4", caps::PROMPT_VN)), 4);
        assert_eq!(version(caps::PROMPT_V2), 2);
        assert_eq!(version(caps::PROMPT_V1), 1);
        assert_eq!(version(handshake::ERROR_NOT_PERMITTED), -4);
        assert_eq!(version(handshake::ERROR_INTERNAL_ERROR), -11);
        assert_eq!(version("HTTP/1.1 200 OK\r\n\r\n"), -1);
        // a prompt with trailing bytes is not understood
        assert_eq!(version(&format!("{}\n", caps::PROMPT_V1)), -1);
        // VN with no positive number stops at its row: no error text, no delay
        let (v, row) = response_version(format!("{}0", caps::PROMPT_VN).as_bytes());
        assert_eq!((v, row.error, row.secs, row.priority), (0, None, 0, Priority::Info));
        // 30 bytes past the prompt is too long for a version
        assert_eq!(version(&format!("{}{}", caps::PROMPT_VN, "1".repeat(30))), -1);
        // under 75 bytes, what follows the prompt's number is ignored: the number ends at its first non-digit
        assert_eq!(version(&format!("{}32\nREPLAY_CHART", caps::PROMPT_VN)), 32);
    }

    /// The collectors' gate starts a sender without the parents lock, which an attempt holds across its whole connect
    /// (R55 M2): C's gate takes a read lock the connector's does not exclude.
    #[test]
    fn a_start_does_not_wait_for_an_attempt() {
        use netdata_agent_rrd::upstream::Upstream;
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c6", false, info("127.0.0.1:1:SSL", "key")));
        let s = Sender::attach(&host, &c).expect("created");
        host.sender_flags_set(sender_flags::ADDED);
        let attempt = s.parents();
        let (tx, rx) = std::sync::mpsc::channel();
        let s2 = Arc::clone(&s);
        std::thread::spawn(move || {
            Upstream::start(&*s2);
            tx.send(()).unwrap();
        });
        let returned = rx.recv_timeout(std::time::Duration::from_millis(500));
        drop(attempt);
        assert!(returned.is_ok(), "start() waited for the parents lock an attempt holds");
    }

    /// The host's status reads the parents as the last hold of them left them (D241 F1 C): as made with the sender,
    /// unchanged while a pass holds them (and read without waiting for it), changed once the hold ends.
    #[test]
    fn the_parents_are_published_when_a_hold_of_them_ends() {
        use netdata_agent_rrd::upstream::Upstream;
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c3", false, info("127.0.0.1:1 127.0.0.1:2:SSL", "key")));
        let s = Sender::attach(&host, &c).expect("created");
        let seen = |s: &Sender| -> Vec<(String, bool, i32, u32)> {
            s.published_parents().into_iter().map(|d| (d.destination, d.ssl, d.reason, d.attempts)).collect()
        };
        let first = ("127.0.0.1:1".to_string(), false, Reason::NEVER.0, 0);
        assert_eq!(seen(&s), [first.clone(), ("127.0.0.1:2".to_string(), true, Reason::NEVER.0, 0)]);
        let mut pass = s.parents();
        pass.list[0].reason = Reason::SP_CONNECTION_REFUSED;
        pass.list[0].attempts = 3;
        assert_eq!(seen(&s)[0], first, "the list before the pass, at once");
        drop(pass);
        assert_eq!(seen(&s)[0], ("127.0.0.1:1".to_string(), false, Reason::SP_CONNECTION_REFUSED.0, 3));
    }
}
