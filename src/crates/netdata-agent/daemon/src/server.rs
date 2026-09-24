//! The web workers: each pool thread polls every listener, accepts, and serves its clients inline, as the C
//! `static-threaded` web server does (`src/web/server/static/static-threaded.c`, `web_client.c`).

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

use netdata_agent_inicfg::LogLevel;

use crate::acl::{self, WebAcl};
use crate::{api, router};

/// What every worker needs to answer requests.
pub struct Shared {
    pub settings: Settings,
    pub version: &'static str,
    pub gzip_level: u32,
    /// `[web] x-frame-options response header`.
    pub x_frame_options: Option<String>,
    /// The `[web]` access lists.
    pub acl: WebAcl,
    pub log: acl::Logger,
    /// `[web] timeout for first request` and `disconnect idle clients after`, in seconds (0 disables).
    pub first_request_timeout_s: u64,
    pub idle_timeout_s: u64,
    pub info: api::Info,
    /// `netdata_configured_web_dir`.
    pub web_dir: String,
    pub hosts: Arc<Hosts>,
}

/// A handler's answer (`w->response`).
pub struct Reply {
    pub code: u16,
    pub content_type: ContentType,
    pub body: Vec<u8>,
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
            no_cacheable: false,
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
    stream: mio::net::TcpStream,
    /// `w->user_auth.client_ip` as `accept_socket()` formats it.
    client_ip: String,
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

pub struct WebWorker {
    listeners: Vec<mio::net::TcpListener>,
    /// Each listener's ACL (`fds_acl_flags`).
    listener_acls: Vec<u32>,
    clients: Vec<Option<Client>>,
    shared: Arc<Shared>,
    receivers: Arc<Receivers>,
}

impl WebWorker {
    /// `listeners` (with their ACLs) must be non-blocking; they become this worker's own.
    pub fn new(
        listeners: Vec<(std::net::TcpListener, u32)>,
        shared: Arc<Shared>,
        receivers: Arc<Receivers>,
    ) -> Self {
        let (listeners, listener_acls): (Vec<_>, Vec<_>) = listeners.into_iter().unzip();
        WebWorker {
            listeners: listeners
                .into_iter()
                .map(mio::net::TcpListener::from_std)
                .collect(),
            listener_acls,
            clients: Vec::new(),
            shared,
            receivers,
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
            match self.listeners[index].accept() {
                Ok((mut stream, peer)) => {
                    // accept_socket(): the connection list, then web_client_update_acl_matches().
                    let mut identity = acl::Client {
                        ip: client_ip(&peer),
                        peer: peer.ip(),
                        host: String::new(),
                    };
                    let log = self.shared.log;
                    if !acl::connection_allowed(
                        &mut identity,
                        &self.shared.acl.connections,
                        "connection",
                        log,
                    ) {
                        log(
                            LogLevel::Warning,
                            &format!(
                                "Permission denied for client '{}', port '{}'",
                                identity.ip,
                                peer.port()
                            ),
                        );
                        continue;
                    }
                    let client_acl =
                        self.shared
                            .acl
                            .matches(&mut identity, self.listener_acls[index], log);
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
                    let _ = stream.set_nodelay(true);
                    let _ = socket2::SockRef::from(&stream).set_keepalive(true);
                    self.clients[slot] = Some(Client {
                        stream,
                        client_ip: identity.ip,
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
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // Another worker won the race, or the peer went away.
                Err(_) => break,
            }
        }
    }

    fn close(&mut self, cx: &mut Context<'_>, slot: usize) {
        if let Some(mut client) = self.clients[slot].take() {
            let _ = cx.registry().deregister(&mut client.stream);
        }
    }

    /// `stream_receiver_takeover_web_connection()`: the socket leaves this worker for the streaming code; whatever
    /// arrived after the request is dropped, as C flushes it.
    fn take_over(&mut self, cx: &mut Context<'_>, slot: usize, pre: PreAdmission) {
        let Some(mut client) = self.clients[slot].take() else {
            return;
        };
        let _ = cx.registry().deregister(&mut client.stream);
        let stream = std::net::TcpStream::from(client.stream);
        match pre {
            PreAdmission::Refuse(message) => self.receivers.refuse(stream, message),
            PreAdmission::Proceed(pending) => self.receivers.admit(*pending, stream),
            PreAdmission::Reply(..) => unreachable!("replies stay on the web connection"),
        }
    }

    fn serve(&mut self, cx: &mut Context<'_>, slot: usize, event: &Event) {
        let shared = Arc::clone(&self.shared);
        let receivers = Arc::clone(&self.receivers);
        let token = self.client_token(slot);
        let Some(client) = self.clients[slot].as_mut() else {
            return;
        };

        // One request at a time, as C: reading stops at the first complete request and resumes only after its
        // response is written, so later bytes wait in the kernel (backpressure) and are reported again when
        // reading is re-armed.
        if client.output.is_empty() && event.is_readable() {
            loop {
                let start = client.received.len();
                let want = client.recv.recv_len(start);
                client.received.resize(start + want, 0);
                match client.stream.read(&mut client.received[start..]) {
                    Ok(0) => {
                        self.close(cx, slot);
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
                            Some(Outcome::Reply(bytes)) => {
                                client.output = bytes;
                                client.written = 0;
                                break;
                            }
                            Some(Outcome::Stream(pre)) => {
                                self.take_over(cx, slot, pre);
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
                        self.close(cx, slot);
                        return;
                    }
                }
            }
        }

        let Some(client) = self.clients[slot].as_mut() else {
            return;
        };
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
                    self.close(cx, slot);
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
                self.close(cx, slot);
                return;
            }
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
    /// Bytes to send (a whole HTTP response, or a raw streaming refusal that closes the connection).
    Reply(Vec<u8>),
    /// A `STREAM` request past the checks made on the web connection: the connection is taken over.
    Stream(PreAdmission),
}

/// Validates what was received and, when the request is complete, produces the whole response.
fn respond(client: &mut Client, shared: &Shared, receivers: &Receivers) -> Option<Outcome> {
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
            permission_denied_acl()
        }
        Validation::Ok if client.request.mode == Some(Mode::Stream) => {
            // stream_receiver_accept_connection()
            let pre = receivers.pre_admit(
                &client.request.query,
                client.request.headers.user_agent.as_deref(),
                &client.client_ip,
            );
            client.request = Request::default();
            client.received.clear();
            return Some(match pre {
                PreAdmission::Reply(bytes, _code) => {
                    client.close_after_write = true;
                    Outcome::Reply(bytes.as_bytes().to_vec())
                }
                other => Outcome::Stream(other),
            });
        }
        Validation::Ok => dispatch(&client.request, client.acl, shared),
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
        Validation::TooManyReadRetries => {
            Reply::text(status::BAD_REQUEST, "Too many retries to read request.\r\n")
        }
        Validation::NotSupported => Reply::text(
            status::BAD_REQUEST,
            "HTTP method requested is not supported...\r\n",
        ),
    };

    let h = &client.request.headers;
    let is_options = client.request.mode == Some(Mode::Options);
    let transaction = h
        .transaction
        .unwrap_or_else(|| *uuid::Uuid::new_v4().as_bytes());
    // Accept-Encoding: gzip turns compression on while the headers are parsed, so the gzip and chunked header lines
    // go out even for an empty body; C then closes the connection without sending any chunk.
    let (body, gzip, chunked) = if h.gzip {
        if reply.body.is_empty() {
            client.close_after_write = true;
            (Vec::new(), true, true)
        } else {
            (gzip_chunked(&reply.body, shared.gzip_level), true, true)
        }
    } else {
        (reply.body.clone(), false, false)
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
        transaction,
    };
    let built = response::build(&head, now());
    let mut out = built.bytes;
    out.extend_from_slice(&body);

    client.close_after_write |= !built.keepalive;
    // The body was built in the receive buffer, which keeps its size for the next request.
    client.recv.need(0, reply.body.len() + 1);
    // Ready for the next request on this connection.
    client.request = Request::default();
    client.received.clear();
    Some(Outcome::Reply(out))
}

const REDIRECT_BODY: &str = "<!DOCTYPE html><!-- SPDX-License-Identifier: GPL-3.0-or-later --><html><body onload=\"window.location.href ='https://'+ window.location.hostname + ':' + window.location.port + window.location.pathname + window.location.search\">Redirecting to safety connection, case your browser does not support redirection, please click <a onclick=\"window.location.href ='https://'+ window.location.hostname + ':'  + window.location.port + window.location.pathname + window.location.search\">here</a>.</body></html>";

/// The body as one gzip member in chunked framing (C streams zlib output in chunks; clients see the same content).
fn gzip_chunked(body: &[u8], level: u32) -> Vec<u8> {
    use flate2::write::GzEncoder;
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::new(level));
    let _ = encoder.write_all(body);
    let compressed = encoder.finish().unwrap_or_default();
    let mut out = format!("{:X}\r\n", compressed.len()).into_bytes();
    out.extend_from_slice(&compressed);
    out.extend_from_slice(b"\r\n0\r\n\r\n");
    out
}

/// `web_client_permission_denied_acl()`.
pub fn permission_denied_acl() -> Reply {
    Reply::text(
        status::UNAVAILABLE_FOR_LEGAL_REASONS,
        "You need to be authorized to access this resource",
    )
}

/// The request-mode switch of `web_client_process_request_from_web_server()`, after the STREAM case.
fn dispatch(req: &Request, client_acl: u32, shared: &Shared) -> Reply {
    match req.mode {
        Some(Mode::Options) if acl::can_access_web(client_acl, req.path_is_mcp) => {
            Reply::text(status::OK, "OK")
        }
        // The WebSocket handshake is not ported: past its ACL it is served as the GET it arrived as.
        Some(Mode::Websocket)
            if acl::can(client_acl, acl::bits::DASHBOARD)
                || acl::can(client_acl, acl::bits::MCP) =>
        {
            router::process_request(req, client_acl, shared)
        }
        Some(Mode::Get | Mode::Post | Mode::Put | Mode::Delete)
            if acl::can_access_web(client_acl, req.path_is_mcp) =>
        {
            router::process_request(req, client_acl, shared)
        }
        _ => permission_denied_acl(),
    }
}

impl Worker for WebWorker {
    type Msg = ();

    fn start(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        for (i, listener) in self.listeners.iter_mut().enumerate() {
            cx.registry()
                .register(listener, Token(i), Interest::READABLE)?;
        }
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
        let expired: Vec<usize> = self
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
                (never_asked || idle_expired).then_some(slot)
            })
            .collect();
        for slot in expired {
            self.close(cx, slot);
        }
        cx.add_timer(now + self.checks_every());
    }

    fn event(&mut self, cx: &mut Context<'_>, event: &Event) {
        let token = event.token().0;
        if token < self.listeners.len() {
            self.accept(cx, token);
        } else {
            self.serve(cx, token - self.listeners.len(), event);
        }
    }

    fn message(&mut self, _cx: &mut Context<'_>, _msg: ()) {}
}
