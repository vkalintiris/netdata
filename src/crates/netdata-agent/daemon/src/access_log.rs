//! The web server's access log: the completed-request record (`web_client_log_completed_request()`), the connection
//! records (`web_server_log_connection()`), the frames of `web_client_process_request_from_web_server()` and
//! `web_client_api_request()` that every record logged while serving a request inherits, and the model of C's web
//! client cache that numbers the connections (`web_client_cache.c`). Brief: `knowledge/brief-logging-l3-l5.md` §1.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use netdata_agent_log::{Field, FrameGuard, Priority, REDACTED, Source, Value, nd_log, push};
use netdata_agent_nrpc::access;
use netdata_agent_web::request::Mode;

/// The part of C's `web_clients_cache` that shows in `conn=`: fresh structs get the next id, reused ones are zeroed
/// and log 0. Global across the web threads, as in C.
struct Cache {
    next_id: u64,
    /// The use counts of the cached structs; the last one is the list head C reuses first.
    avail: Vec<u32>,
    used: usize,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    next_id: 0,
    avail: Vec::new(),
    used: 0,
});

/// A connection's place in the cache model.
#[derive(Debug)]
pub struct Slot {
    /// `w->id`: 0 for a reused struct.
    pub id: u64,
    use_count: u32,
}

impl Slot {
    /// `web_client_get_from_cache()`.
    pub fn acquire() -> Slot {
        let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        cache.used += 1;
        match cache.avail.pop() {
            Some(use_count) => Slot {
                id: 0,
                use_count: use_count + 1,
            },
            // web_client_create() counts 1 and the cache adds this use: a struct serves 100 connections
            None => {
                cache.next_id += 1;
                Slot {
                    id: cache.next_id,
                    use_count: 2,
                }
            }
        }
    }
}

impl Drop for Slot {
    /// `web_client_release_to_cache()`: freed when used too often, or when the cache holds too many for the clients
    /// still connected.
    fn drop(&mut self) {
        let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        cache.used = cache.used.saturating_sub(1);
        let (used, avail) = (cache.used, cache.avail.len());
        let free =
            self.use_count > 100 || (used > 0 && avail >= 2 * used) || (used <= 10 && avail >= 20);
        if !free {
            cache.avail.push(self.use_count);
        }
    }
}

/// `w->user_auth`'s role and access: `none`/0 until `web_client_ensure_proper_authorization()` runs at the start of
/// an API version handler. Shared with the request's log frames, which read the access when they write.
#[derive(Debug, Default)]
pub struct Auth {
    anonymous: AtomicBool,
    access: AtomicU32,
}

impl Auth {
    /// `web_client_ensure_proper_authorization()` without bearer protection: anonymous data access.
    pub fn authorize_anonymous(&self) {
        self.anonymous.store(true, Ordering::Relaxed);
        self.access.store(access::ANONYMOUS_DATA, Ordering::Relaxed);
    }

    /// `http_id2user_role()` of the two roles the web server gives without bearer tokens.
    fn role(&self) -> &'static str {
        if self.anonymous.load(Ordering::Relaxed) {
            "any"
        } else {
            "none"
        }
    }

    fn access(&self) -> u32 {
        self.access.load(Ordering::Relaxed)
    }
}

/// `log_cb_http_access_to_hex()`: always prints, `0x0` included.
fn access_value(auth: &Arc<Auth>) -> Value {
    let auth = Arc::clone(auth);
    Value::lazy(move |out| {
        out.extend_from_slice(format!("0x{:x}", auth.access()).as_bytes());
        true
    })
}

/// `HTTP_REQUEST_MODE_2str()`: modes it has no name for print `UNKNOWN`. A request whose method was not parsed is
/// still in the `GET` mode a client starts with.
pub fn mode_name(mode: Option<Mode>) -> &'static str {
    match mode {
        None | Some(Mode::Get) => "GET",
        Some(Mode::Options) => "OPTIONS",
        Some(Mode::Post) => "POST",
        Some(Mode::Put) => "PUT",
        Some(Mode::Delete) => "DELETE",
        Some(Mode::Stream) => "STREAM",
        Some(Mode::Websocket) => "UNKNOWN",
    }
}

/// The URL as the access log shows it: for a STREAM request, the value of every `key` parameter is masked (D34).
pub fn logged_url(url: &[u8], mode: Option<Mode>) -> Vec<u8> {
    if mode != Some(Mode::Stream) {
        return url.to_vec();
    }
    let (path, query) = match url.iter().position(|&c| c == b'?') {
        Some(at) => url.split_at(at + 1),
        None => (&url[..0], url),
    };
    let mut out = path.to_vec();
    for (i, segment) in query.split(|&c| c == b'&').enumerate() {
        if i > 0 {
            out.push(b'&');
        }
        let (name, value) = match segment.iter().position(|&c| c == b'=') {
            Some(at) => (&segment[..at], Some(&segment[at + 1..])),
            None => (segment, None),
        };
        let decoded = netdata_agent_web::url::url_decode(name, name.len() + 1).text;
        match value {
            Some(value) if decoded == b"key" && !value.is_empty() => {
                out.extend_from_slice(name);
                out.push(b'=');
                out.extend_from_slice(REDACTED.as_bytes());
            }
            _ => out.extend_from_slice(segment),
        }
    }
    out
}

/// `strip_control_characters()`: `iscntrl()` bytes become spaces.
fn stripped(url: &[u8]) -> String {
    let bytes: Vec<u8> = url
        .iter()
        .map(|&c| if c < 0x20 || c == 0x7f { b' ' } else { c })
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// What a connection keeps for its records.
#[derive(Debug)]
pub struct ClientLog {
    pub slot: Slot,
    /// `w->user_auth.client_ip`: wiped after every completed keep-alive request (C's `memset(&w->user_auth)`).
    pub ip: String,
    /// The poller's copy of the IP, which survives the wipe (`pi->client_ip`).
    pub accept_ip: String,
    pub port: String,
    /// `w->client_host`: the name the access lists resolved, if any.
    pub host: String,
    /// `w->timings.tv_ready`: never reset between the requests of a connection.
    pub tv_ready: Option<Instant>,
    /// The current request's `X-Forwarded-Host` and `X-Forwarded-For`, until the request is done.
    pub forwarded_host: Vec<u8>,
    pub forwarded_for: Vec<u8>,
    /// The errno C's request records carry: a unix connection's failed `TCP_CORK`.
    pub request_errno: i32,
}

impl ClientLog {
    pub fn new(ip: String, port: String, host: String) -> ClientLog {
        ClientLog {
            slot: Slot::acquire(),
            accept_ip: ip.clone(),
            ip,
            port,
            host,
            tv_ready: None,
            forwarded_host: Vec::new(),
            forwarded_for: Vec::new(),
            request_errno: 0,
        }
    }

    /// `web_server_log_connection()`: an access record at debug, `[<ip>]:<port> CONNECTED` / `DISCONNECTED`, with
    /// the errno an earlier call left.
    pub fn connection(&self, what: &str, errno: i32) {
        let _frame = push(vec![
            (Field::ConnectionId, Value::U64(self.slot.id)),
            (Field::SrcTransport, Value::txt("http")),
            (Field::SrcIp, Value::txt(self.ip.as_str())),
            (Field::SrcPort, Value::txt(self.port.as_str())),
            (
                Field::SrcForwardedHost,
                Value::txt(text(&self.forwarded_host)),
            ),
            (
                Field::SrcForwardedFor,
                Value::txt(text(&self.forwarded_for)),
            ),
        ]);
        nd_log!(
            Source::Access,
            Priority::Debug,
            errno = errno;
            "[{}]:{} {what}",
            self.ip,
            self.port
        );
    }

    /// `poll_process_error()`'s frame around a close after a hangup: the poller's copies of the IP and port.
    pub fn hangup_frame(&self) -> FrameGuard {
        push(vec![
            (Field::SrcIp, Value::txt(self.accept_ip.as_str())),
            (Field::SrcPort, Value::txt(self.port.as_str())),
        ])
    }

    /// `web_client_request_done()` after a keep-alive request: `user_auth` is zeroed, the IP and the forwarded
    /// headers with it.
    pub fn request_done(&mut self) {
        self.ip.clear();
        self.forwarded_host.clear();
        self.forwarded_for.clear();
    }
}

/// The request being served, as its frames and records see it.
#[derive(Debug, Default)]
pub struct RequestContext {
    pub conn: u64,
    pub ip: String,
    pub port: String,
    pub host: String,
    pub forwarded_host: String,
    pub forwarded_for: String,
    /// The request's own mode (the API frame's).
    pub mode: Option<Mode>,
    /// The mode before this pass (the outer frame's, copied when C pushes it).
    pub previous_mode: Option<Mode>,
    /// As received, the key masked for STREAM.
    pub url: String,
    pub transaction: [u8; 16],
    pub auth: Arc<Auth>,
}

impl RequestContext {
    /// The frame of `web_client_process_request_from_web_server()`. Its `X-Forwarded-Host` is the pointer of the
    /// previous pass, which C frees during this one; the port leaves it out.
    pub fn outer_frame(&self) -> FrameGuard {
        push(vec![
            (Field::SrcTransport, Value::txt("http")),
            (Field::SrcIp, Value::txt(self.ip.as_str())),
            (Field::SrcPort, Value::txt(self.port.as_str())),
            (
                Field::SrcForwardedFor,
                Value::txt(self.forwarded_for.as_str()),
            ),
            (Field::NidlNode, Value::txt(self.host.as_str())),
            (
                Field::RequestMethod,
                Value::txt(mode_name(self.previous_mode)),
            ),
            (Field::Request, Value::txt(self.url.as_str())),
            (Field::ConnectionId, Value::U64(self.conn)),
            (Field::TransactionId, Value::Uuid(self.transaction)),
            (Field::UserRole, Value::txt(self.auth.role())),
            (Field::UserAccess, access_value(&self.auth)),
        ])
    }

    /// The frame of `web_client_api_request()`: pushed after validation, so with the real mode and forwarded host.
    pub fn api_frame(&self) -> FrameGuard {
        push(vec![
            (Field::SrcIp, Value::txt(self.ip.as_str())),
            (Field::SrcPort, Value::txt(self.port.as_str())),
            (
                Field::SrcForwardedHost,
                Value::txt(self.forwarded_host.as_str()),
            ),
            (
                Field::SrcForwardedFor,
                Value::txt(self.forwarded_for.as_str()),
            ),
            (Field::NidlNode, Value::txt(self.host.as_str())),
            (Field::RequestMethod, Value::txt(mode_name(self.mode))),
            (Field::Request, Value::txt(self.url.as_str())),
            (Field::ConnectionId, Value::U64(self.conn)),
            (Field::TransactionId, Value::Uuid(self.transaction)),
            (Field::UserRole, Value::txt(self.auth.role())),
            (Field::UserAccess, access_value(&self.auth)),
        ])
    }
}

/// A response waiting for its completed-request record.
#[derive(Debug)]
pub struct Completed {
    pub url: Vec<u8>,
    pub mode: Option<Mode>,
    pub code: u16,
    /// Compressed bytes when gzip was used, else the body length.
    pub sent: u64,
    pub size: u64,
    pub tv_in: Instant,
    pub transaction: [u8; 16],
    pub forwarded_for: Vec<u8>,
    pub auth: Arc<Auth>,
}

/// `dt_usec()`: an absolute difference.
fn dt_usec(a: Instant, b: Instant) -> u64 {
    let d = if a > b { a - b } else { b - a };
    d.as_micros() as u64
}

impl Completed {
    /// `web_client_log_completed_request()`: written only when a URL was received, at a priority from the code,
    /// without a message, and outside any request frame.
    pub fn log(&self, client: &ClientLog) {
        if self.url.is_empty() {
            return;
        }
        let now = Instant::now();
        let (prep_ut, sent_ut) = match client.tv_ready {
            Some(ready) => (dt_usec(ready, self.tv_in), dt_usec(now, ready)),
            None => (0, 0),
        };
        let priority = match self.code {
            500.. => Priority::Emerg,
            400.. => Priority::Warning,
            300.. => Priority::Notice,
            _ => Priority::Info,
        };
        let _frame = push(vec![
            (Field::ConnectionId, Value::U64(client.slot.id)),
            (Field::TransactionId, Value::Uuid(self.transaction)),
            (Field::NidlNode, Value::txt(client.host.as_str())),
            (Field::RequestMethod, Value::txt(mode_name(self.mode))),
            (Field::Request, Value::Txt(stripped(&self.url))),
            (Field::ResponseCode, Value::U64(u64::from(self.code))),
            (Field::ResponseSentBytes, Value::U64(self.sent)),
            (Field::ResponseSizeBytes, Value::U64(self.size)),
            (Field::ResponsePreparationTimeUsec, Value::U64(prep_ut)),
            (Field::ResponseSentTimeUsec, Value::U64(sent_ut)),
            (
                Field::ResponseTotalTimeUsec,
                Value::U64(dt_usec(now, self.tv_in)),
            ),
            (Field::SrcIp, Value::txt(client.ip.as_str())),
            (Field::SrcPort, Value::txt(client.port.as_str())),
            (
                Field::SrcForwardedFor,
                Value::Txt(text(&self.forwarded_for)),
            ),
            (Field::UserRole, Value::txt(self.auth.role())),
            (Field::UserAccess, access_value(&self.auth)),
        ]);
        netdata_agent_log::logger(
            Source::Access,
            priority,
            client.request_errno,
            &netdata_agent_log::here!(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_key_is_masked_in_every_key_parameter() {
        let cases: [(&[u8], &[u8]); 5] = [
            (
                b"key=11111111-2222-3333-4444-555555555555&hostname=child&key=x",
                b"key=[REDACTED]&hostname=child&key=[REDACTED]",
            ),
            (b"/stream?%6Bey=secret&v=1", b"/stream?%6Bey=[REDACTED]&v=1"),
            (b"key=&hostname=child", b"key=&hostname=child"),
            (b"hostname=child&keys=a", b"hostname=child&keys=a"),
            (b"key", b"key"),
        ];
        for (url, want) in cases {
            assert_eq!(
                logged_url(url, Some(Mode::Stream)),
                want,
                "{}",
                String::from_utf8_lossy(url)
            );
        }
        assert_eq!(logged_url(b"key=secret", Some(Mode::Get)), b"key=secret");
    }

    #[test]
    fn control_characters_become_spaces() {
        assert_eq!(stripped(b"/a\tb\x7fc\nd"), "/a b c d");
    }
}
