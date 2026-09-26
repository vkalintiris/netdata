//! The web workers: each pool thread polls every listener, accepts, and serves its clients inline, as the C
//! `static-threaded` web server does (`src/web/server/static/static-threaded.c`, `web_client.c`).

use netdata_agent_log::{ErrorLimit, Priority, Source, nd_log, nd_log_limit};
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use netdata_agent_evloop::conn::{Conn, Stream};
use netdata_agent_evloop::{Context, Event, Interest, TimerId, Token, Worker};
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::request::{
    self, Connection, Mode, Request, Settings, Transport, Validation,
};
use netdata_agent_web::response::{self, Head};
use netdata_agent_web::status;

use netdata_agent_text::print::html_escape;

use netdata_agent_rrd::host::Hosts;
use netdata_agent_streaming::receiver::{PreAdmission, Receivers};

use netdata_agent_inicfg::{Config, SECTION_WEB};

use crate::access_log::{Auth, ClientLog, Completed, RequestContext, logged_url};
use crate::acl::{self, WebAcl};
use crate::router;

/// What every worker needs to answer requests.
pub struct Shared {
    pub settings: Settings,
    pub version: &'static str,
    pub gzip_level: u32,
    /// `[web] x-frame-options response header`.
    pub x_frame_options: Option<String>,
    /// The `[web]` access lists.
    pub acl: WebAcl,
    /// `[web] timeout for first request` and `disconnect idle clients after`, in seconds (0 disables).
    pub first_request_timeout_s: u64,
    pub idle_timeout_s: u64,
    /// `netdata_configured_web_dir`.
    pub web_dir: String,
    pub hosts: Arc<Hosts>,
    /// The time-grouping SES/DES window limits.
    pub grouping_windows: netdata_agent_query::grouping::Windows,
    /// `get_release_channel()`, reported by `/api/v1/charts`.
    pub release_channel: &'static str,
    /// netdata.conf after startup (`netdata_config` and its lock): `/netdata.conf` and the reads C makes lazily.
    pub netdata_conf: Mutex<Config>,
    /// `[web] custom dashboard_info.js`, read at its first use.
    pub custom_dashboard_info: OnceLock<String>,
}

impl Shared {
    /// netdata.conf under its lock; a panic while it was held does not make it unreadable.
    pub fn conf(&self) -> MutexGuard<'_, Config> {
        self.netdata_conf
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// `[web] custom dashboard_info.js`: `charts2json()` reads it at its first call, so a fresh dump lacks it.
    pub fn custom_dashboard_info(&self) -> &str {
        self.custom_dashboard_info.get_or_init(|| {
            let value = self
                .conf()
                .get(SECTION_WEB, "custom dashboard_info.js", Some(""))
                .unwrap_or_default();
            String::from_utf8_lossy(&value).into_owned()
        })
    }
}

/// A handler's answer (`w->response`).
pub struct Reply {
    pub code: u16,
    pub content_type: ContentType,
    pub body: Vec<u8>,
    /// `WB_CONTENT_NO_CACHEABLE`: every response buffer starts no-cache (`buffer_create()`, `buffer_reset()`); static
    /// files and absolute data queries opt out (`buffer_cacheable()`).
    pub no_cacheable: bool,
    /// `response.data->date` and `->expires`; 0 lets the header builder derive them.
    pub date: i64,
    pub expires: i64,
    /// Extra header lines (`response.header`), each ending in CRLF.
    pub headers: Vec<u8>,
}

impl Default for Reply {
    fn default() -> Self {
        Reply {
            code: status::OK,
            content_type: ContentType::TextPlain,
            body: Vec::new(),
            no_cacheable: true,
            date: 0,
            expires: 0,
            headers: Vec::new(),
        }
    }
}

impl Reply {
    pub fn text(code: u16, body: &str) -> Self {
        Reply {
            code,
            body: body.as_bytes().to_vec(),
            ..Reply::default()
        }
    }

    /// An HTML reply of `prefix` followed by the HTML-escaped `name`, as the C error pages are built.
    pub fn html(code: u16, prefix: &str, name: &[u8]) -> Self {
        let mut body = prefix.as_bytes().to_vec();
        html_escape(&mut body, name);
        Reply {
            code,
            content_type: ContentType::TextHtml,
            body,
            ..Reply::default()
        }
    }
}

/// `now_realtime_sec()`.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The capacity of a web client's receive buffer (`w->response.data`), which decides how much each `recv()` asks
/// for. It grows by `buffer_increase()`'s rule and survives across the requests of a connection, including the
/// growth for response bodies. C also recycles clients between connections, so its first reads on a new
/// connection depend on history; a fresh client is modelled here.
#[derive(Debug, Clone, Copy)]
struct RecvBuffer {
    size: usize,
}

impl RecvBuffer {
    /// `NETDATA_WEB_RESPONSE_INITIAL_SIZE`.
    const INITIAL: usize = 8192;
    /// `NETDATA_WEB_REQUEST_INITIAL_SIZE`: the free space asked for before each read.
    const READ_ROOM: usize = 8192;

    /// `buffer_need_bytes()`: grow when `len + needed` reaches the size.
    fn need(&mut self, len: usize, needed: usize) {
        if len + needed < self.size {
            return;
        }
        let required = needed + 1;
        let remaining = self.size - len;
        if remaining >= required {
            return;
        }
        let optimal = if self.size > 5 * 1024 * 1024 {
            self.size / 2
        } else {
            self.size
        };
        self.size += (required - remaining).max(1024).max(optimal);
    }

    /// `web_client_receive()`: room for a request chunk, then `recv(left - 1)`.
    fn recv_len(&mut self, len: usize) -> usize {
        self.need(len, Self::READ_ROOM);
        self.size - len - 1
    }
}

struct Client {
    stream: Conn,
    /// `w->acl`.
    acl: u32,
    /// The `POLLINFO` activity the timeout checks read.
    activity: Activity,
    recv: RecvBuffer,
    received: Vec<u8>,
    request: Request,
    output: Vec<u8>,
    written: usize,
    close_after_write: bool,
    /// The access log's view of the connection.
    log: ClientLog,
    /// `w->user_auth`'s role and access for the current request, shared with its log frames.
    auth: Arc<Auth>,
    /// `w->transaction`: zero until a pass of the request gives it one.
    transaction: [u8; 16],
    /// The response waiting for its completed-request record.
    pending: Option<Completed>,
}

impl Client {
    /// `web_client_request_done()` after a keep-alive response went out: its record, then the reset.
    fn request_done(&mut self) {
        if let Some(done) = self.pending.take() {
            done.log(&self.log);
        }
        self.log.request_done();
        self.auth = Arc::default();
        self.transaction = [0; 16];
    }
}

/// `POLLINFO`: when the connection came and last moved data.
struct Activity {
    connected: Instant,
    last_received: Option<Instant>,
    last_sent: Option<Instant>,
    recv_count: u64,
    send_count: u64,
    /// `POLLINFO_FLAG_FIRST_REQUEST_RECEIVED`.
    first_request_received: bool,
}

/// A listen socket: one descriptor that every web worker registers in its own poller, as C shares its listening
/// sockets between its web threads (D53.3).
pub struct WebListener {
    socket: ListenSocket,
    /// `fds_acl_flags`.
    acl: u32,
    /// `fds_names`.
    name: String,
}

enum ListenSocket {
    Tcp(mio::net::TcpListener),
    Unix(mio::net::UnixListener),
    /// Kept open and listed, never polled: C crashes on the first datagram (D53.1).
    Udp(#[expect(dead_code, reason = "held open, never read")] std::net::UdpSocket),
}

impl WebListener {
    /// A listener as `listen::setup()` opened it (non-blocking).
    pub fn new(l: crate::listen::Listener) -> WebListener {
        let socket = match l.socket {
            crate::listen::Socket::Tcp(s) => ListenSocket::Tcp(mio::net::TcpListener::from_std(s)),
            crate::listen::Socket::Unix(s) => {
                ListenSocket::Unix(mio::net::UnixListener::from_std(s))
            }
            crate::listen::Socket::Udp(s) => ListenSocket::Udp(s),
        };
        WebListener {
            socket,
            acl: l.acl,
            name: l.name,
        }
    }

    /// The descriptor a worker polls, `None` for one it does not.
    fn polled_fd(&self) -> Option<std::os::fd::RawFd> {
        use std::os::fd::AsRawFd;
        match &self.socket {
            ListenSocket::Tcp(s) => Some(s.as_raw_fd()),
            ListenSocket::Unix(s) => Some(s.as_raw_fd()),
            ListenSocket::Udp(_) => None,
        }
    }

    /// `accept_socket()`: the connection and its peer as C names it; a unix peer is `localhost`, port `UNIX`.
    fn accept(&self) -> io::Result<(Conn, acl::Client, String)> {
        match &self.socket {
            ListenSocket::Tcp(l) => l.accept().map(|(stream, peer)| {
                let client = acl::Client {
                    ip: client_ip(&peer),
                    peer: Some(peer.ip()),
                    host: String::new(),
                    errno: 0,
                };
                (Conn::Tcp(stream), client, peer.port().to_string())
            }),
            ListenSocket::Unix(l) => l.accept().map(|(stream, _)| {
                let client = acl::Client {
                    ip: "localhost".to_string(),
                    peer: None,
                    host: String::new(),
                    errno: 0,
                };
                (Conn::Unix(stream), client, "UNIX".to_string())
            }),
            ListenSocket::Udp(_) => Err(io::ErrorKind::WouldBlock.into()),
        }
    }
}

pub struct WebWorker {
    listeners: Arc<[WebListener]>,
    clients: Vec<Option<Client>>,
    /// This worker's share of `[web] web server max sockets` (0: no limit).
    max_sockets: usize,
    /// Whether this worker polls its stream listeners (`listen_sockets_active`).
    listening: bool,
    shared: Arc<Shared>,
    receivers: Arc<Receivers>,
    stats: Stats,
}

/// The counters of C's `worker_private`, one per poller callback, logged when the thread stops.
#[derive(Debug, Default)]
struct Stats {
    connected: usize,
    disconnected: usize,
    max_concurrent: usize,
    receptions: usize,
    sends: usize,
}

impl WebWorker {
    /// Every worker polls every listener in `listeners`.
    pub fn new(
        listeners: Arc<[WebListener]>,
        max_sockets: usize,
        shared: Arc<Shared>,
        receivers: Arc<Receivers>,
    ) -> Self {
        WebWorker {
            listeners,
            clients: Vec::new(),
            max_sockets,
            listening: true,
            shared,
            receivers,
            stats: Stats::default(),
        }
    }

    /// The sockets this worker polls, listeners included (`p.used`).
    fn used(&self) -> usize {
        self.listeners.len() + self.clients.iter().flatten().count()
    }

    /// `poll_events()`'s listener switch, before every wait: a worker holding its share of sockets stops polling
    /// the stream listeners, and polls them again once below it.
    fn throttle(&mut self, cx: &mut Context<'_>) {
        let (used, limit) = (self.used(), self.max_sockets);
        let flip = if self.listening {
            limit != 0 && used >= limit
        } else {
            limit == 0 || used < limit
        };
        if !flip {
            return;
        }
        self.listening = !self.listening;
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "{} listening sockets (used TCP sockets {used}, max allowed for this worker {limit})",
            if self.listening {
                "ENABLING"
            } else {
                "DISABLING"
            }
        );
        for (i, listener) in self.listeners.iter().enumerate() {
            if let Some(fd) = listener.polled_fd() {
                let mut source = mio::unix::SourceFd(&fd);
                let _ = if self.listening {
                    cx.registry()
                        .register(&mut source, Token(i), Interest::READABLE)
                } else {
                    cx.registry().deregister(&mut source)
                };
            }
        }
    }

    /// `checks_every = idle / 3 + 1`, and a pass runs once more than that many seconds have passed.
    fn checks_every(&self) -> Duration {
        Duration::from_secs(self.shared.idle_timeout_s / 3 + 2)
    }

    fn client_token(&self, slot: usize) -> Token {
        Token(self.listeners.len() + slot)
    }

    fn accept(&mut self, cx: &mut Context<'_>, index: usize) {
        loop {
            // C accepts one connection per wake-up and re-checks its share before the next
            if self.max_sockets != 0 && self.used() >= self.max_sockets {
                break;
            }
            match self.listeners[index].accept() {
                Ok((mut stream, mut identity, port)) => {
                    // accept_socket(): the connection list, then web_client_update_acl_matches().
                    if !acl::connection_allowed(
                        &mut identity,
                        &self.shared.acl.connections,
                        "connection",
                    ) {
                        nd_log!(
                            Source::Daemon,
                            Priority::Warning,
                            "Permission denied for client '{}', port '{}'",
                            identity.ip,
                            port
                        );
                        // accept_socket() then fails with EPERM, which poll_events() logs
                        nd_log!(Source::Daemon, Priority::Err, errno = nix::errno::Errno::EPERM as i32;
                            "POLLFD: LISTENER: accept() failed.");
                        continue;
                    }
                    // poll_process_new_tcp_connection(): a client that is already gone is closed without a trace; a
                    // failed peek leaves its errno to the next record (a unix client may not have sent yet)
                    if is_socket_closed(&stream, &mut identity.errno) {
                        continue;
                    }
                    let client_acl = self
                        .shared
                        .acl
                        .matches(&mut identity, self.listeners[index].acl);
                    let mut log = ClientLog::new(
                        identity.ip.clone(),
                        port,
                        std::mem::take(&mut identity.host),
                    );
                    if stream.is_unix() {
                        // web_client_request_done()'s TCP_CORK fails on a unix socket before every request record
                        log.request_errno = nix::errno::Errno::EOPNOTSUPP as i32;
                    }
                    let slot = self
                        .clients
                        .iter()
                        .position(Option::is_none)
                        .unwrap_or_else(|| {
                            self.clients.push(None);
                            self.clients.len() - 1
                        });
                    let token = self.client_token(slot);
                    if cx
                        .registry()
                        .register(&mut stream, token, Interest::READABLE)
                        .is_err()
                    {
                        continue;
                    }
                    // web_client_create_on_fd()
                    if let Conn::Tcp(tcp) = &stream {
                        let _ = tcp.set_nodelay(true);
                    }
                    let _ = socket2::SockRef::from(&stream).set_keepalive(true);
                    self.clients[slot] = Some(Client {
                        stream,
                        acl: client_acl,
                        activity: Activity {
                            connected: Instant::now(),
                            last_received: None,
                            last_sent: None,
                            recv_count: 0,
                            send_count: 0,
                            first_request_received: false,
                        },
                        recv: RecvBuffer {
                            size: RecvBuffer::INITIAL,
                        },
                        received: Vec::new(),
                        request: Request::default(),
                        output: Vec::new(),
                        written: 0,
                        close_after_write: false,
                        log,
                        auth: Arc::default(),
                        transaction: [0; 16],
                        pending: None,
                    });
                    if let Some(client) = &self.clients[slot] {
                        // web_server_add_callback()
                        let s = &mut self.stats;
                        s.connected += 1;
                        s.max_concurrent = s.max_concurrent.max(s.connected - s.disconnected);
                        client.log.connection("CONNECTED", identity.errno);
                    }
                }
                // Another worker won the race.
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    let errno = netdata_agent_log::errno_of(&e);
                    if errno == nix::errno::Errno::EMFILE as i32 {
                        // poll_events(): the listeners count as used sockets too; at most one line every 10 s
                        static EMFILE: ErrorLimit = ErrorLimit::new(10, 1000);
                        let used = self.used();
                        nd_log_limit!(&EMFILE, Source::Daemon, Priority::Err, errno = errno;
                            "POLLFD: LISTENER: too many open files - used by this thread {used}, max for this \
                             thread {}", self.max_sockets);
                    } else {
                        nd_log!(Source::Daemon, Priority::Err, errno = errno; "POLLFD: LISTENER: accept() failed.");
                    }
                    break;
                }
            }
        }
    }

    /// `web_server_del_callback()`: DISCONNECTED, then the pending request's record, then the client goes back to
    /// the cache. After a hangup C's poller pushes its copies of the IP and port around it.
    fn close(&mut self, cx: &mut Context<'_>, slot: usize, hangup: bool) {
        if let Some(mut client) = self.clients[slot].take() {
            self.stats.disconnected += 1;
            let _ = cx.registry().deregister(&mut client.stream);
            let _frame = hangup.then(|| client.log.hangup_frame());
            client.log.connection("DISCONNECTED", 0);
            if let Some(mut done) = client.pending.take() {
                if client.written < client.output.len() {
                    done.sent_when(client.written);
                }
                done.log(&client.log);
            }
        }
    }

    /// `stream_receiver_takeover_web_connection()`: the socket leaves this worker for the streaming code; whatever
    /// arrived after the request is dropped, as C flushes it.
    /// A STREAM request past the web checks: the socket goes to the receiver, still inside the request's frame as
    /// in C, then the web client is deleted like any other.
    fn take_over(
        &mut self,
        cx: &mut Context<'_>,
        slot: usize,
        pre: PreAdmission,
        ctx: RequestContext,
    ) {
        let Some(mut client) = self.clients[slot].take() else {
            return;
        };
        let _ = cx.registry().deregister(&mut client.stream);
        let stream = Stream::from(client.stream);
        {
            let _frame = ctx.outer_frame();
            match pre {
                PreAdmission::Refuse(message, refusal) => {
                    self.receivers.refuse(stream, message, &refusal)
                }
                PreAdmission::Proceed(pending) => self.receivers.admit(*pending, stream),
                PreAdmission::Reply(..) => unreachable!("replies stay on the web connection"),
            }
        }
        self.stats.disconnected += 1;
        client.log.connection("DISCONNECTED", 0);
        if let Some(done) = client.pending.take() {
            done.log(&client.log);
        }
    }

    fn serve(&mut self, cx: &mut Context<'_>, slot: usize, event: &Event) {
        let shared = Arc::clone(&self.shared);
        let receivers = Arc::clone(&self.receivers);
        let token = self.client_token(slot);
        let Some(client) = self.clients[slot].as_mut() else {
            return;
        };
        // poll_process_error(): a hangup or a half-close (EPOLLRDHUP, even with the request in the same read) closes
        // the client before anything it sent is served.
        if event.is_read_closed() || event.is_error() {
            let flag = |set: bool, name: &'static str| if set { name } else { "" };
            let hangup = event.is_read_closed() || event.is_write_closed();
            // C polls for writing only while a response is pending
            let sending = !client.output.is_empty();
            {
                let _frame = client.log.hangup_frame();
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "POLLFD: LISTENER: received {} {} {} on socket {} client '{}' port '{}' expecting {} {}, having {} {}",
                    flag(event.is_error(), "ERROR"),
                    flag(hangup, "HUP"),
                    "",
                    std::os::fd::AsRawFd::as_raw_fd(&client.stream),
                    client.log.accept_ip,
                    client.log.port,
                    flag(!sending, "READ"),
                    flag(sending, "WRITE"),
                    flag(event.is_readable(), "READ"),
                    flag(event.is_writable(), "WRITE")
                );
            }
            self.close(cx, slot, true);
            return;
        }

        // One request at a time, as C: reading stops at the first complete request and resumes only after its
        // response is written, so later bytes wait in the kernel (backpressure) and are reported again when
        // reading is re-armed.
        if client.output.is_empty() && event.is_readable() {
            // web_server_rcv_callback()
            self.stats.receptions += 1;
            loop {
                let start = client.received.len();
                let want = client.recv.recv_len(start);
                client.received.resize(start + want, 0);
                match client.stream.read(&mut client.received[start..]) {
                    Ok(0) => {
                        self.close(cx, slot, true);
                        return;
                    }
                    Ok(n) => {
                        client.received.truncate(start + n);
                        client.activity.recv_count += 1;
                        client.activity.last_received = Some(Instant::now());
                        let outcome = respond(client, &shared, &receivers);
                        if outcome.is_some() {
                            client.activity.first_request_received = true;
                        }
                        match outcome {
                            Some(Outcome::Reply(bytes, sent)) => {
                                client.output = bytes;
                                client.written = sent;
                                break;
                            }
                            Some(Outcome::Stream(pre, ctx)) => {
                                self.take_over(cx, slot, pre, *ctx);
                                return;
                            }
                            Some(Outcome::Dead) => {
                                self.close(cx, slot, false);
                                return;
                            }
                            None => {}
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        client.received.truncate(start);
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        client.received.truncate(start);
                    }
                    Err(_) => {
                        self.close(cx, slot, true);
                        return;
                    }
                }
            }
        }

        let Some(client) = self.clients[slot].as_mut() else {
            return;
        };
        // web_server_snd_callback(): C's poller reports the socket writable once the response is queued
        if client.written < client.output.len() {
            self.stats.sends += 1;
        }
        while client.written < client.output.len() {
            match client.stream.write(&client.output[client.written..]) {
                Ok(n) => {
                    client.written += n;
                    client.activity.send_count += 1;
                    client.activity.last_sent = Some(Instant::now());
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.close(cx, slot, false);
                    return;
                }
            }
        }
        if client.written < client.output.len() {
            // While sending, C polls for writing only.
            let _ = cx
                .registry()
                .reregister(&mut client.stream, token, Interest::WRITABLE);
            return;
        }
        if !client.output.is_empty() {
            client.output.clear();
            client.written = 0;
            if client.close_after_write {
                self.close(cx, slot, false);
                return;
            }
            client.request_done();
            let _ = cx
                .registry()
                .reregister(&mut client.stream, token, Interest::READABLE);
        }
    }
}

/// `accept_socket()`: the numeric address, `localhost` for the loopback addresses, IPv4-mapped IPv6 unwrapped (after
/// the loopback check, so `::ffff:127.0.0.1` stays `127.0.0.1`).
fn client_ip(peer: &std::net::SocketAddr) -> String {
    let ip = peer.ip().to_string();
    if ip == "127.0.0.1" || ip == "::1" {
        return "localhost".to_string();
    }
    match ip.strip_prefix("::ffff:") {
        Some(v4) if peer.is_ipv6() => v4.to_string(),
        _ => ip,
    }
}

/// What a complete request turned into.
enum Outcome {
    /// Bytes to send (a whole HTTP response, or a raw streaming refusal that closes the connection), and how many of
    /// them already went out.
    Reply(Vec<u8>, usize),
    /// A `STREAM` request past the checks made on the web connection: the connection is taken over, under the
    /// request's frame.
    Stream(PreAdmission, Box<RequestContext>),
    /// The client went away while its response header was being sent (`WEB_CLIENT_IS_DEAD`).
    Dead,
}

/// Validates what was received and, when the request is complete, produces the whole response.
fn respond(client: &mut Client, shared: &Shared, receivers: &Receivers) -> Option<Outcome> {
    // web_client_process_request_from_web_server(): every pass restarts the request's clock (tv_in) and gives the
    // request a transaction id if it has none yet.
    let received = Instant::now();
    if client.transaction == [0; 16] {
        client.transaction = *uuid::Uuid::new_v4().as_bytes();
    }
    // The outer frame copies the mode before this pass; a partial STREAM or WEBSOCKET pass is reset to GET.
    let previous_mode = match client.request.mode {
        Some(Mode::Stream | Mode::Websocket) => None,
        mode => mode,
    };
    let conn = Connection {
        transport: Transport::Tcp,
        tls_configured: false,
        tls_active: false,
        tls_force: false,
        tls_default: false,
        acl_aclk: false,
    };
    let validation = client
        .request
        .validate(&client.received, &conn, &shared.settings);
    // X-Transaction-Id replaces the random id when it parses.
    if let Some(transaction) = client.request.headers.transaction {
        client.transaction = transaction;
    }
    client.log.forwarded_host = client
        .request
        .headers
        .forwarded_host
        .clone()
        .unwrap_or_default();
    client.log.forwarded_for = client.request.headers.forwarded_for.clone();
    let mode = client.request.mode;
    let ctx = RequestContext {
        conn: client.log.slot.id,
        ip: client.log.ip.clone(),
        port: client.log.port.clone(),
        host: client.log.host.clone(),
        forwarded_host: lossy(&client.log.forwarded_host),
        forwarded_for: lossy(&client.log.forwarded_for),
        mode,
        previous_mode,
        url: lossy(&logged_url(&client.request.url_as_received, mode)),
        transaction: client.transaction,
        auth: Arc::clone(&client.auth),
    };
    let _frame = ctx.outer_frame();
    let completed = |client: &Client, code: u16, sent: usize, size: usize| Completed {
        url: logged_url(&client.request.url_as_received, mode),
        mode,
        code,
        sent: sent as u64,
        gzip_blocks: Vec::new(),
        size: size as u64,
        tv_in: received,
        transaction: client.transaction,
        forwarded_for: client.log.forwarded_for.clone(),
        auth: Arc::clone(&client.auth),
    };

    // tv_ready: set once the response is ready, except for an incomplete request, a STREAM and a mode the ACL denies.
    let mut ready = true;
    let reply = match validation {
        Validation::Incomplete => {
            if client.received.len() <= request::MAX_REQUEST_SIZE {
                return None;
            }
            client.request.url_as_received = b"too big request".to_vec();
            Reply::text(
                status::BAD_REQUEST,
                &format!(
                    "Received request is too big  (received {} bytes, max is {} bytes).\r\n",
                    client.received.len(),
                    request::MAX_REQUEST_SIZE
                ),
            )
        }
        Validation::Ok
            if client.request.mode == Some(Mode::Stream)
                && !acl::can(client.acl, acl::bits::STREAMING) =>
        {
            // C sends the 451 body without an HTTP header and closes.
            let body = permission_denied_acl().body;
            client.pending = Some(completed(
                client,
                status::UNAVAILABLE_FOR_LEGAL_REASONS,
                body.len(),
                body.len(),
            ));
            client.close_after_write = true;
            client.request = Request::default();
            client.received.clear();
            return Some(Outcome::Reply(body, 0));
        }
        Validation::Ok if client.request.mode == Some(Mode::Stream) => {
            // stream_receiver_accept_connection(): rpt->remote_ip is w->user_auth.client_ip, which a previous
            // keep-alive request has wiped
            let pre = receivers.pre_admit(
                &client.request.query,
                client.request.headers.user_agent.as_deref(),
                &client.log.ip,
                &client.log.port,
            );
            let (code, len) = match &pre {
                PreAdmission::Reply(bytes, code) => (*code, bytes.len()),
                _ => (status::OK, 0),
            };
            client.pending = Some(completed(client, code, len, len));
            client.request = Request::default();
            client.received.clear();
            return Some(match pre {
                PreAdmission::Reply(bytes, _code) => {
                    client.close_after_write = true;
                    Outcome::Reply(bytes.as_bytes().to_vec(), 0)
                }
                other => Outcome::Stream(other, Box::new(ctx)),
            });
        }
        Validation::Ok => {
            let stream = &client.stream;
            let (reply, allowed) = dispatch(
                &client.request,
                client.acl,
                shared,
                received,
                &ctx,
                &|errno| is_socket_closed(stream, errno),
            );
            ready = allowed;
            reply
        }
        Validation::Redirect => Reply {
            code: status::HTTPS_UPGRADE,
            content_type: ContentType::TextHtml,
            body: REDIRECT_BODY.as_bytes().to_vec(),
            ..Reply::default()
        },
        Validation::UriTooLong => {
            client.request.url_as_received = b"too long request URI".to_vec();
            Reply::text(status::URI_TOO_LONG, "Request URI is too long.\r\n")
        }
        Validation::TooManyReadRetries(tries) => {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "Disabling slow client after {tries} attempts to read the request ({} bytes received)",
                client.received.len()
            );
            Reply::text(status::BAD_REQUEST, "Too many retries to read request.\r\n")
        }
        Validation::NotSupported => Reply::text(
            status::BAD_REQUEST,
            "HTTP method requested is not supported...\r\n",
        ),
    };
    if ready {
        client.log.tv_ready = Some(Instant::now());
    }

    let h = &client.request.headers;
    let is_options = client.request.mode == Some(Mode::Options);
    // Accept-Encoding: gzip turns compression on while the headers are parsed, so the gzip and chunked header lines
    // go out even for an empty body; C then closes the connection without sending any chunk.
    let (body, gzip, chunked, blocks) = if h.gzip {
        if reply.body.is_empty() {
            client.close_after_write = true;
            (Vec::new(), true, true, Vec::new())
        } else {
            let (framed, blocks) = gzip_chunked(&reply.body, shared.gzip_level);
            (framed, true, true, blocks)
        }
    } else {
        (reply.body.clone(), false, false, Vec::new())
    };
    let head = Head {
        code: reply.code,
        content_type: reply.content_type,
        date: reply.date,
        expires: reply.expires,
        no_cacheable: reply.no_cacheable,
        keepalive: h.keepalive,
        origin: h.origin.as_deref(),
        version: shared.version,
        path_is_mcp: client.request.path_is_mcp,
        is_options,
        x_frame_options: shared.x_frame_options.as_deref(),
        has_cookies: false,
        respect_do_not_track: shared.settings.respect_do_not_track,
        tracking_required: false,
        custom: &reply.headers,
        gzip,
        chunked,
        content_length: body.len(),
        server_host: h.server_host.as_deref(),
        url_as_received: &client.request.url_as_received,
        transaction: client.transaction,
    };
    let built = response::build(&head, now());
    let mut out = built.bytes;
    let header_len = out.len();
    out.extend_from_slice(&body);
    // What the access log reports: the compressed bytes under gzip (all of them once the response is out), else the
    // body length.
    let sent = blocks
        .last()
        .map_or(if gzip { 0 } else { reply.body.len() }, |b| b.1 as usize);
    let mut done = completed(client, built.code, sent, reply.body.len());
    done.gzip_blocks = blocks
        .into_iter()
        .map(|(at, total)| (header_len + at, total))
        .collect();
    client.pending = Some(done);

    client.close_after_write |= !built.keepalive;
    // The body was built in the receive buffer, which keeps its size for the next request.
    client.recv.need(0, reply.body.len() + 1);
    // Ready for the next request on this connection.
    client.request = Request::default();
    client.received.clear();
    // web_client_send_http_header(): the header goes out now, still inside the request's frame
    match send_header(&mut client.stream, &out[..header_len]) {
        Some(sent) => Some(Outcome::Reply(out, sent)),
        None => Some(Outcome::Dead),
    }
}

/// `web_client_send_http_header()`'s send: how much of the header went out, or `None` (after C's two records) when
/// the client is gone. A full socket sends nothing now; the rest goes out with the body.
fn send_header(stream: &mut Conn, header: &[u8]) -> Option<usize> {
    loop {
        match stream.write(header) {
            Ok(n) => return Some(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Some(0),
            Err(e) => {
                nd_log!(Source::Daemon, Priority::Err, errno = netdata_agent_log::errno_of(&e);
                    "Cannot send HTTP headers to web client.");
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "HTTP headers failed to be sent (I sent {} bytes but the system sent -1 bytes). Closing web client.",
                    header.len()
                );
                return None;
            }
        }
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

const REDIRECT_BODY: &str = "<!DOCTYPE html><!-- SPDX-License-Identifier: GPL-3.0-or-later --><html><body onload=\"window.location.href ='https://'+ window.location.hostname + ':' + window.location.port + window.location.pathname + window.location.search\">Redirecting to safety connection, case your browser does not support redirection, please click <a onclick=\"window.location.href ='https://'+ window.location.hostname + ':'  + window.location.port + window.location.pathname + window.location.search\">here</a>.</body></html>";

/// `NETDATA_WEB_RESPONSE_ZLIB_CHUNK_SIZE`: each zlib output buffer goes out as one chunk.
const ZLIB_CHUNK: usize = 16384;

/// `web_client_send_deflate()`: the body through zlib as C runs it (the level, a gzip window, `Z_FINISH`) into 16 KiB
/// buffers, each one chunk, framed as C frames them. Also where each chunk's framing starts and the compressed total
/// after it: C deflates a chunk only once the previous one is out, and its access record reports what it produced.
fn gzip_chunked(body: &[u8], level: u32) -> (Vec<u8>, Vec<(usize, u64)>) {
    use flate2::{Compress, Compression, FlushCompress};
    let mut z = Compress::new_gzip(Compression::new(level), 15);
    let (mut out, mut blocks, mut buf) = (Vec::new(), Vec::new(), vec![0u8; ZLIB_CHUNK]);
    let mut consumed = 0;
    loop {
        let start = out.len();
        if !blocks.is_empty() {
            out.extend_from_slice(b"\r\n");
        }
        let (taken, produced) = (z.total_in(), z.total_out());
        let _ = z.compress(&body[consumed..], &mut buf, FlushCompress::Finish);
        consumed += (z.total_in() - taken) as usize;
        let n = (z.total_out() - produced) as usize;
        out.extend_from_slice(format!("{n:X}\r\n").as_bytes());
        out.extend_from_slice(&buf[..n]);
        blocks.push((start, z.total_out()));
        // C finishes once the input is taken and a buffer had room left
        if consumed == body.len() && n < ZLIB_CHUNK {
            break;
        }
    }
    out.extend_from_slice(b"\r\n0\r\n\r\n");
    (out, blocks)
}

/// `web_client_permission_denied_acl()`.
pub fn permission_denied_acl() -> Reply {
    Reply::text(
        status::UNAVAILABLE_FOR_LEGAL_REASONS,
        "You need to be authorized to access this resource",
    )
}

/// `is_socket_closed()`: a peek that finds the end of the stream or an error other than "no data yet"; a failed peek
/// leaves its errno, as `recv()` does.
fn is_socket_closed(stream: &Conn, errno: &mut i32) -> bool {
    match socket2::SockRef::from(stream).peek(&mut [std::mem::MaybeUninit::uninit(); 1]) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => {
            *errno = netdata_agent_log::errno_of(&e);
            e.kind() != io::ErrorKind::WouldBlock
        }
    }
}

/// The request-mode switch of `web_client_process_request_from_web_server()`, after the STREAM case. False when the
/// response is never marked ready: C returns early for a denied WebSocket and in its `default:` case, while the
/// other denials break out of the switch to the checkpoint.
fn dispatch(
    req: &Request,
    client_acl: u32,
    shared: &Shared,
    received: Instant,
    ctx: &RequestContext,
    interrupted: &dyn Fn(&mut i32) -> bool,
) -> (Reply, bool) {
    match req.mode {
        Some(Mode::Options) if acl::can_access_web(client_acl, req.path_is_mcp) => {
            (Reply::text(status::OK, "OK"), true)
        }
        // The WebSocket handshake is not ported: past its ACL it is served as the GET it arrived as.
        Some(Mode::Websocket)
            if acl::can(client_acl, acl::bits::DASHBOARD)
                || acl::can(client_acl, acl::bits::MCP) =>
        {
            let reply =
                router::process_request(req, client_acl, shared, received, ctx, interrupted);
            (reply, true)
        }
        Some(Mode::Get | Mode::Post | Mode::Put | Mode::Delete)
            if acl::can_access_web(client_acl, req.path_is_mcp) =>
        {
            let reply =
                router::process_request(req, client_acl, shared, received, ctx, interrupted);
            (reply, true)
        }
        Some(Mode::Options | Mode::Get | Mode::Post | Mode::Put | Mode::Delete) => {
            (permission_denied_acl(), true)
        }
        _ => (permission_denied_acl(), false),
    }
}

impl Worker for WebWorker {
    type Msg = ();

    fn start(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        for (i, listener) in self.listeners.iter().enumerate() {
            if let Some(fd) = listener.polled_fd() {
                cx.registry().register(
                    &mut mio::unix::SourceFd(&fd),
                    Token(i),
                    Interest::READABLE,
                )?;
            }
            // poll_events()
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "POLLFD: LISTENER: listening on '{}'",
                listener.name
            );
        }
        self.throttle(cx);
        cx.add_timer(Instant::now() + self.checks_every());
        Ok(())
    }

    /// The cleanup pass of `poll_events()`: a client that has not completed its first request (and was sent
    /// nothing) within the first-request timeout, or has moved no data for the idle timeout, is closed.
    fn timer(&mut self, cx: &mut Context<'_>, _timer: TimerId) {
        let now = Instant::now();
        let (first, idle) = (
            self.shared.first_request_timeout_s,
            self.shared.idle_timeout_s,
        );
        let expired: Vec<(usize, bool)> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(slot, client)| {
                let a = &client.as_ref()?.activity;
                let secs = |t: Instant| now.saturating_duration_since(t).as_secs();
                let never_asked = !a.first_request_received
                    && a.send_count == 0
                    && first > 0
                    && secs(a.connected) >= first;
                let last = a.last_received.max(a.last_sent);
                let idle_expired =
                    a.recv_count > 0 && idle > 0 && last.is_some_and(|t| secs(t) >= idle);
                (never_asked || idle_expired).then_some((slot, never_asked))
            })
            .collect();
        for (slot, never_asked) in expired {
            if let Some(client) = &self.clients[slot] {
                // C prints the poller's loop index left at the number of listening sockets, and a trailing space
                let (listeners, fd) = (
                    self.listeners.len(),
                    std::os::fd::AsRawFd::as_raw_fd(&client.stream),
                );
                let (ip, port) = (&client.log.accept_ip, &client.log.port);
                if never_asked {
                    nd_log!(
                        Source::Daemon,
                        Priority::Debug,
                        "POLLFD: LISTENER: client slot {listeners} (fd {fd}) from {ip} port {port} has not completed its first request in {first} seconds - closing it. "
                    );
                } else {
                    nd_log!(
                        Source::Daemon,
                        Priority::Debug,
                        "POLLFD: LISTENER: client slot {listeners} (fd {fd}) from {ip} port {port} is idle for more than {idle} seconds - closing it. "
                    );
                }
            }
            self.close(cx, slot, false);
        }
        self.throttle(cx);
        cx.add_timer(now + self.checks_every());
    }

    fn event(&mut self, cx: &mut Context<'_>, event: &Event) {
        let token = event.token().0;
        if token < self.listeners.len() {
            self.accept(cx, token);
        } else {
            self.serve(cx, token - self.listeners.len(), event);
        }
        self.throttle(cx);
    }

    fn message(&mut self, _cx: &mut Context<'_>, _msg: ()) {}

    /// The end of `poll_events()` (every client closed), then the thread's cleanup; the first thread is also C's web
    /// server main thread, which closes the listening sockets.
    fn stop(&mut self, cx: &mut Context<'_>) {
        for slot in 0..self.clients.len() {
            self.close(cx, slot, false);
        }
        // off this worker's poller before the worker lets go of the shared descriptors (D43)
        for listener in self.listeners.iter() {
            if let Some(fd) = listener.polled_fd() {
                let _ = cx.registry().deregister(&mut mio::unix::SourceFd(&fd));
            }
        }
        let s = &self.stats;
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "stopped after {} connects, {} disconnects (max concurrent {}), {} receptions and {} sends",
            s.connected,
            s.disconnected,
            s.max_concurrent,
            s.receptions,
            s.sends
        );
        if cx.index() == 0 {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "closing all web server sockets..."
            );
            // the sockets close once the last worker lets go of them
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "all static web threads stopped."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_bodies_go_out_in_cs_chunks() {
        // incompressible enough to need two zlib buffers
        let mut x = 0x2545_f491_u32;
        let body: Vec<u8> = (0..40_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        let (out, blocks) = gzip_chunked(&body, 3);
        assert!(blocks.len() >= 3);
        // the first chunk opens the body, a full buffer; each later one is closed and reopened
        assert!(out.starts_with(b"4000\r\n"));
        assert_eq!(blocks[0], (0, ZLIB_CHUNK as u64));
        assert_eq!(&out[blocks[1].0..blocks[1].0 + 8], b"\r\n4000\r\n");
        assert!(out.ends_with(b"\r\n0\r\n\r\n"));
        let mut gz = flate2::read::GzDecoder::new(&out[6..6 + ZLIB_CHUNK]);
        let mut first = Vec::new();
        let _ = io::Read::read_to_end(&mut gz, &mut first);
        assert!(first.len() > 1000 && body.starts_with(&first));
        // a short body is one chunk
        let (out, blocks) = gzip_chunked(b"hello", 3);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            out.len(),
            format!("{:X}\r\n", blocks[0].1).len() + blocks[0].1 as usize + 7
        );
    }
}
