//! The web workers: each pool thread polls every listener, accepts, and serves its clients inline, as the C
//! `static-threaded` web server does (`src/web/server/static/static-threaded.c`, `web_client.c`).

use std::io::{self, Read, Write};
use std::sync::Arc;

use netdata_agent_evloop::{Context, Event, Interest, Token, Worker};
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::request::{
    self, Connection, Mode, Request, Settings, Transport, Validation,
};
use netdata_agent_web::response::{self, Head};
use netdata_agent_web::status;

use netdata_agent_text::print::html_escape;

use crate::{api, router};

/// What every worker needs to answer requests.
pub struct Shared {
    pub settings: Settings,
    pub version: &'static str,
    pub gzip_level: u32,
    pub info: api::Info,
    /// `netdata_configured_web_dir`.
    pub web_dir: String,
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

struct Client {
    stream: mio::net::TcpStream,
    received: Vec<u8>,
    request: Request,
    output: Vec<u8>,
    written: usize,
    close_after_write: bool,
}

pub struct WebWorker {
    listeners: Vec<mio::net::TcpListener>,
    clients: Vec<Option<Client>>,
    shared: Arc<Shared>,
}

impl WebWorker {
    pub fn new(listeners: Vec<std::net::TcpListener>, shared: Arc<Shared>) -> io::Result<Self> {
        let listeners = listeners
            .into_iter()
            .map(|l| l.try_clone().map(mio::net::TcpListener::from_std))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(WebWorker {
            listeners,
            clients: Vec::new(),
            shared,
        })
    }

    fn client_token(&self, slot: usize) -> Token {
        Token(self.listeners.len() + slot)
    }

    fn accept(&mut self, cx: &mut Context<'_>, index: usize) {
        loop {
            match self.listeners[index].accept() {
                Ok((mut stream, _)) => {
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
                    self.clients[slot] = Some(Client {
                        stream,
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

    fn serve(&mut self, cx: &mut Context<'_>, slot: usize, event: &Event) {
        let shared = Arc::clone(&self.shared);
        let token = self.client_token(slot);
        let Some(client) = self.clients[slot].as_mut() else {
            return;
        };

        if event.is_readable() {
            let mut buf = [0u8; 8192];
            loop {
                match client.stream.read(&mut buf) {
                    Ok(0) => {
                        self.close(cx, slot);
                        return;
                    }
                    Ok(n) => {
                        client.received.extend_from_slice(&buf[..n]);
                        if !client.output.is_empty() {
                            continue;
                        }
                        if let Some(reply_bytes) = respond(client, &shared) {
                            client.output = reply_bytes;
                            client.written = 0;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
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
                Ok(n) => client.written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.close(cx, slot);
                    return;
                }
            }
        }
        if client.written < client.output.len() {
            let _ = cx.registry().reregister(
                &mut client.stream,
                token,
                Interest::READABLE | Interest::WRITABLE,
            );
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

/// Validates what was received and, when the request is complete, produces the whole response.
fn respond(client: &mut Client, shared: &Shared) -> Option<Vec<u8>> {
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
        Validation::Ok => dispatch(&client.request, shared),
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
    let (body, gzip, chunked) = if h.gzip && !reply.body.is_empty() {
        (gzip_chunked(&reply.body, shared.gzip_level), true, true)
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
        x_frame_options: None,
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

    client.close_after_write = !built.keepalive;
    // Ready for the next request on this connection.
    client.request = Request::default();
    client.received.clear();
    Some(out)
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

fn dispatch(req: &Request, shared: &Shared) -> Reply {
    if req.mode == Some(Mode::Options) {
        return Reply::text(status::OK, "OK");
    }
    router::process_request(req, shared)
}

impl Worker for WebWorker {
    type Msg = ();

    fn start(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        for (i, listener) in self.listeners.iter_mut().enumerate() {
            cx.registry()
                .register(listener, Token(i), Interest::READABLE)?;
        }
        Ok(())
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
