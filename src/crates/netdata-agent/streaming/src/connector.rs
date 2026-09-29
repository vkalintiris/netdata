//! The sender's connector (`src/streaming/stream-connector.c`): one thread, `SNDR-CN[0]`, started with the first
//! queued host, that connects every queued host to one of its parents (blocking, one host at a time, D100.8), runs
//! the child's side of the handshake and hands the connection to the host's stream thread. Map:
//! `knowledge/map-m7-commit3-connector.md` §2, §6-§9.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
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
use crate::parents::{Local, Parents};
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

/// `buffer_key_value_urlencode()`: `key=` and the url-encoded value.
fn key_value(wb: &mut Vec<u8>, key: &str, value: &str) {
    wb.extend_from_slice(key.as_bytes());
    wb.push(b'=');
    netdata_agent_text::url::url_encode(wb, value.as_bytes());
}

/// `completion`: jobs counted under a lock, waited for with a timeout.
#[derive(Debug, Default)]
struct Completion {
    jobs: Mutex<u64>,
    cv: Condvar,
}

impl Completion {
    /// `completion_mark_complete_a_job()`.
    fn mark(&self) {
        *self.jobs.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.cv.notify_all();
    }

    /// `completion_wait_for_a_job_with_timeout()`: returns once a job completed after `seen`, or at the timeout.
    fn wait(&self, seen: u64, timeout: Duration) -> u64 {
        let jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        let (jobs, _) = self.cv.wait_timeout_while(jobs, timeout, |jobs| *jobs == seen).unwrap_or_else(PoisonError::into_inner);
        *jobs
    }
}

/// What the sender needs of the daemon: its claim and Cloud state, and the Cloud URL a parent may hand down.
pub struct Env {
    /// `is_agent_claimed()`.
    pub claimed: Box<dyn Fn() -> bool + Send + Sync>,
    /// `aclk_online()`.
    pub aclk_online: Box<dyn Fn() -> bool + Send + Sync>,
    /// `cloud_config_url_set()`.
    pub set_cloud_url: Box<dyn Fn(&str) + Send + Sync>,
}

impl Default for Env {
    /// An agent that is not claimed and whose Cloud URL does not change.
    fn default() -> Self {
        Env { claimed: Box::new(|| false), aclk_online: Box::new(|| false), set_cloud_url: Box::new(|_| {}) }
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
    completion: Completion,
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
            completion: Completion::default(),
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
    pub(crate) fn ssl_init(&self, parents: &Parents) {
        let mut tls = self.tls.lock().unwrap_or_else(PoisonError::into_inner);
        if tls.is_some() || !parents.list.iter().any(|d| d.ssl) {
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

    /// `stream_connector_requeue()`.
    pub(crate) fn requeue(&self, s: &Arc<Sender>, host: &Host, cmd: Cmd) {
        self.requeue_after_close(s, host, cmd, 0);
    }

    /// [`Connector::requeue`], its record carrying `errno` (what a TLS close left, D107.5).
    pub(crate) fn requeue_after_close(&self, s: &Arc<Sender>, host: &Host, cmd: Cmd, errno: i32) {
        if cmd == Cmd::Connect {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                errno = errno;
                "STREAM CONNECT '{}' [to parent]: adding host in connector queue...",
                host.hostname()
            );
            host.pulse_status(host_status::SND_PENDING);
        }
        {
            // the index is taken under the queue's lock, so the queue keeps the order of the requeues
            let mut queue = self.queue();
            let idx = self.idx.fetch_add(1, Ordering::Relaxed) + 1;
            queue.insert(idx, (Arc::clone(s), cmd));
        }
        self.completion.mark();
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
                if s.shutdown.load(Ordering::Relaxed) {
                    self.queue().remove(&key);
                    s.on_disconnect(&host);
                    s.connector_remove(&host);
                    continue;
                }
                match if exiting > 0 { Cmd::Remove } else { cmd } {
                    Cmd::Connect => {
                        if let Some(connected) = self.stream_connect(&s, &host, &th) {
                            self.queue().remove(&key);
                            s.on_connect(&host, &connected.link);
                            self.add_to_queue(connected, &host);
                        }
                    }
                    Cmd::Remove => {
                        self.queue().remove(&key);
                        s.on_disconnect(&host);
                        let reason = s.lock().exit_reason;
                        s.remove(&host, reason);
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
        // the parents stay locked for the attempt (C holds their read lock); the sender's state is only copied in
        // and out, so nothing else waits on it across the attempt's I/O
        let mut attempt = {
            let st = s.lock();
            Attempt {
                parents: s.parents(),
                hops: host.ingestion_hops().wrapping_add(1),
                status_reason: st.status_reason,
                capabilities: st.capabilities,
                remote_ip: st.remote_ip.clone(),
                parent_using_h2o: st.parent_using_h2o,
            }
        };
        let connected = self.attempt(s, host, &mut attempt, th);
        let mut st = s.lock();
        st.hops = attempt.hops;
        st.status_reason = attempt.status_reason;
        st.capabilities = attempt.capabilities;
        st.remote_ip = attempt.remote_ip;
        connected
    }

    fn attempt(&self, s: &Arc<Sender>, host: &Arc<Host>, st: &mut Attempt<'_>, th: &Thread<'_>) -> Option<Connected> {
        let settings = &self.settings;
        // the context and the verification as they are now (stream-connector.c:278-280)
        let mut sock = NdSock::new(self.tls(), settings.ssl_validate_certificate);
        // nd_sock_close() of the previous socket clears errno
        sock.close(th);
        host.pulse_status(host_status::SND_PENDING);
        let connected = st.parents.connect_to_one(
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
        let destination = st.parents.current().map(|d| d.destination.clone()).unwrap_or_default();
        st.remote_ip = crate::records::cut(&destination, CONNECTED_TO_SIZE).to_string();
        st.capabilities = caps::sender_ours(s.disabled.load(Ordering::Relaxed));
        let request = self.request(host, &s.api_key, st.hops, st.capabilities);
        let hostname = host.hostname();
        let remote = st.remote_ip.clone();
        if st.parent_using_h2o && !crate::h2o::upgrade_prelude(&mut sock, th) {
            sock.close(th);
            st.parents.set_connect_failure_reason(host, &mut st.status_reason, Reason::SND_DISCONNECT_HTTP_UPGRADE_FAILED, 60);
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
            st.parents.set_connect_failure_reason(host, &mut st.status_reason, Reason::CONNECT_SEND_TIMEOUT, 60);
            return None;
        }
        let mut response = [0u8; RESPONSE_SIZE];
        let bytes = sock.recv_timeout(&mut response, settings.timeout_s, th);
        if bytes <= 0 {
            sock.close(th);
            let _frame = push(vec![(Field::ResponseCode, Value::I64(Reason::CONNECT_RECEIVE_TIMEOUT.code()))]);
            log_errno!(th, Priority::Err, "STREAM CONNECT '{hostname}' [to {remote}]: remote netdata does not respond.");
            st.parents.set_connect_failure_reason(host, &mut st.status_reason, Reason::CONNECT_RECEIVE_TIMEOUT, 30);
            return None;
        }
        if !self.validate_first_response(host, st, s.disabled.load(Ordering::Relaxed), &response[..bytes as usize], th) {
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
    fn validate_first_response(&self, host: &Host, st: &mut Attempt<'_>, disabled: u32, http: &[u8], th: &Thread<'_>) -> bool {
        let (version, row) = response_version(http);
        if version >= 1 {
            st.parents.set_reconnect_delay(Reason::SP_CONNECTED, self.settings.reconnect_delay_s);
            st.capabilities = caps::negotiate(version, caps::sender_ours(disabled));
            st.status_reason = Reason(st.capabilities as i32);
            return true;
        }
        let reason = Reason(version);
        st.parents.set_connect_failure_reason(host, &mut st.status_reason, reason, row.secs);
        let _frame = push(vec![(Field::ResponseCode, Value::I64(reason.code()))]);
        let at = netdata_agent_log::rfc3339_local(st.parents.current().map_or(0, |d| d.postpone_until_ut), 0);
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

/// What an attempt reads and writes of the sender's state, with the parents it holds.
struct Attempt<'a> {
    parents: MutexGuard<'a, crate::parents::Parents>,
    hops: i16,
    status_reason: Reason,
    capabilities: u32,
    remote_ip: String,
    parent_using_h2o: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
