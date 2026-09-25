//! Request reception and validation, ported from `src/web/server/web_client.c` (`http_request_validate()`,
//! `web_client_valid_method()`, `web_client_decode_path_and_query_string()`) and `src/web/api/http_header.c`.
//!
//! The parser is sans-io: the connection appends received bytes to its buffer and calls [`Request::validate`]
//! after every receive, exactly where C calls `http_request_validate()`.

use netdata_agent_text::c::{at, c_str, eq_ignore_case, find, find_ignore_case, is_space};
use netdata_agent_text::parse::uuid_parse_flexi;

use crate::url::{self, ExpectedSize, Payload};

/// `NETDATA_WEB_REQUEST_MAX_SIZE`.
pub const MAX_REQUEST_SIZE: usize = 1024 * 1024;
/// `HTTP_REQ_MAX_HEADER_FETCH_TRIES`.
pub const MAX_HEADER_FETCH_TRIES: usize = MAX_REQUEST_SIZE.div_ceil(512);
/// `NETDATA_WEB_REQUEST_URL_DECODE_INITIAL_SIZE`.
const URL_DECODE_INITIAL_SIZE: usize = 4 * 1024;
/// `NI_MAXHOST - 1`: longest `Host`/`X-Forwarded-Host` value kept.
const MAX_HOST: usize = 1024;
/// `INET6_ADDRSTRLEN - 1`: longest `X-Forwarded-For` value kept.
const MAX_FORWARDED_FOR: usize = 45;
/// `UUID_STR_LEN * 2 - 1`: longest text handed to `uuid_parse_flexi()` from a header.
const MAX_UUID_TEXT: usize = 73;
/// `CLOUD_CLIENT_NAME_LENGTH - 1`.
const MAX_CLIENT_NAME: usize = 63;

/// `HTTP_REQUEST_MODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Get,
    Options,
    Post,
    Put,
    Delete,
    Stream,
    Websocket,
}

/// `HTTP_VALIDATION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validation {
    Ok,
    Incomplete,
    NotSupported,
    /// The attempts counted before the counters were reset, which C's record prints.
    TooManyReadRetries(usize),
    UriTooLong,
    Redirect,
}

/// How the client is connected (`WEB_CLIENT_FLAG_CONN_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Unix,
    Cloud,
    WebRtc,
}

/// The connection facts the parser consults.
#[derive(Debug, Clone, Copy)]
pub struct Connection {
    pub transport: Transport,
    /// `netdata_ssl_web_server_ctx` exists (TLS is configured on the server).
    pub tls_configured: bool,
    /// `SSL_connection(&w->ssl)`: this connection speaks TLS.
    pub tls_active: bool,
    /// `http_is_using_ssl_force(w)`.
    pub tls_force: bool,
    /// `http_is_using_ssl_default(w)`.
    pub tls_default: bool,
    /// `w->acl & HTTP_ACL_ACLK`: cloud identity headers are honoured only with it.
    pub acl_aclk: bool,
}

/// Server-wide settings the header callbacks read.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// `web_enable_gzip`.
    pub gzip: bool,
    /// `respect_web_browser_do_not_track_policy`.
    pub respect_do_not_track: bool,
}

/// `HTTP_USER_ROLE` values the `X-Netdata-Role` header can select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRole {
    Admin,
    Manager,
    Troubleshooter,
    Observer,
    Member,
    Billing,
}

/// What the recognized request headers set on the client.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    pub origin: Option<Vec<u8>>,
    pub keepalive: bool,
    /// `Connection: ...upgrade...`.
    pub websocket_handshake: bool,
    /// `Upgrade: websocket`, cleared by a `Sec-WebSocket-Version` other than 13.
    pub websocket: bool,
    pub websocket_key: Option<Vec<u8>>,
    pub websocket_protocol: Option<Vec<u8>>,
    pub websocket_extensions: Option<Vec<u8>>,
    /// Set by `DNT` when the server respects it.
    pub do_not_track: bool,
    /// Kept only for STREAM requests.
    pub user_agent: Option<Vec<u8>>,
    pub accept_json: bool,
    pub accept_sse: bool,
    pub accept_text: bool,
    /// `X-Auth-Token`.
    pub auth_token: Option<Vec<u8>>,
    pub server_host: Option<Vec<u8>>,
    pub forwarded_host: Option<Vec<u8>>,
    pub forwarded_for: Vec<u8>,
    /// The response is gzip-compressed (`zoutput`); set by `Accept-Encoding: ...gzip...`.
    pub gzip: bool,
    /// `WEB_CLIENT_CHUNKED_TRANSFER`.
    pub chunked: bool,
    pub transaction: Option<[u8; 16]>,
    /// Every `X-Netdata-Auth`/`Authorization: Bearer` token of the last header pass, in order; C authenticates each
    /// as it is parsed.
    pub bearer_tokens: Vec<Vec<u8>>,
    pub mcp_session_id: Option<[u8; 16]>,
    pub cloud_account_id: Option<[u8; 16]>,
    pub cloud_role: Option<UserRole>,
    /// `X-Netdata-Permissions` hex text; applying it sets the access and the cloud auth method.
    pub cloud_permissions: Option<Vec<u8>>,
    pub cloud_user_name: Option<Vec<u8>>,
}

/// The per-client request state `http_request_validate()` reads and writes.
#[derive(Debug, Clone, Default)]
pub struct Request {
    header_parse_tries: usize,
    header_parse_last_size: usize,
    expected: ExpectedSize,
    /// `url_decode_buffer_size`: grows and persists for the life of the client.
    decode_buffer_size: usize,
    pub mode: Option<Mode>,
    pub payload: Option<Payload>,
    pub url_as_received: Vec<u8>,
    pub path: Vec<u8>,
    pub query: Vec<u8>,
    /// `WEB_CLIENT_FLAG_PATH_IS_MCP`.
    pub path_is_mcp: bool,
    /// The poller should keep waiting for input (`web_client_enable_wait_receive`).
    pub wait_receive: bool,
    pub headers: Headers,
    /// A STREAM request refused because the listener requires TLS: the `hostname=` it named, for the log line.
    pub stream_tls_refused_hostname: Option<Vec<u8>>,
}

fn truncated(v: &[u8], max: usize) -> Vec<u8> {
    v[..v.len().min(max)].to_vec()
}

impl Request {
    fn reset_parse_counters(&mut self) {
        self.header_parse_tries = 0;
        self.header_parse_last_size = 0;
        self.expected = ExpectedSize::Unknown;
    }

    /// `web_client_valid_method()`: the offset after the method, or `None` when unsupported.
    fn valid_method(&mut self, text: &[u8], conn: &Connection) -> Option<usize> {
        let methods: [(&[u8], Mode); 6] = [
            (b"GET ", Mode::Get),
            (b"OPTIONS ", Mode::Options),
            (b"POST ", Mode::Post),
            (b"PUT ", Mode::Put),
            (b"DELETE ", Mode::Delete),
            (b"STREAM ", Mode::Stream),
        ];
        let (prefix, mode) = methods
            .into_iter()
            .find(|(prefix, _)| text.starts_with(prefix))?;
        self.mode = Some(mode);
        if mode == Mode::Stream && !conn.tls_active && conn.tls_force {
            self.reset_parse_counters();
            self.wait_receive = false;
            let rest = &text[prefix.len()..];
            self.stream_tls_refused_hostname = Some(match find(rest, b"hostname=") {
                Some(at_) => {
                    let value = &rest[at_ + 9..];
                    match value.iter().position(|&c| c == b'&') {
                        Some(end) => value[..end.min(255)].to_vec(),
                        None => b"not available".to_vec(),
                    }
                }
                None => b"not available".to_vec(),
            });
            return None;
        }
        Some(prefix.len())
    }

    /// `http_request_validate()` over the received bytes `buf`.
    pub fn validate(&mut self, buf: &[u8], conn: &Connection, settings: &Settings) -> Validation {
        let text = c_str(buf);
        let length = buf.len();
        let last_pos = self.header_parse_last_size;
        self.header_parse_last_size = length;
        self.header_parse_tries += 1;

        let is_valid = if last_pos != 0 {
            // Rescan from 4 bytes before the previous end, so a terminator split across receives is found.
            let mut from = last_pos.saturating_sub(4);
            if last_pos <= 4 || self.header_parse_last_size <= from {
                from = 0;
            }
            if !url::is_request_complete(buf, from, length, &mut self.payload, &mut self.expected) {
                if self.header_parse_tries > MAX_HEADER_FETCH_TRIES {
                    let tries = self.header_parse_tries;
                    self.reset_parse_counters();
                    self.wait_receive = false;
                    return Validation::TooManyReadRetries(tries);
                }
                return Validation::Incomplete;
            }
            true
        } else {
            url::is_request_complete(buf, length, length, &mut self.payload, &mut self.expected)
        };

        let Some(url_start) = self.valid_method(text, conn) else {
            self.reset_parse_counters();
            self.wait_receive = false;
            return Validation::NotSupported;
        };
        if !is_valid {
            self.wait_receive = true;
            return Validation::Incomplete;
        }

        let mut line_end = url_start;
        while line_end < length && at(text, line_end) != 0 && buf[line_end] != b'\r' {
            line_end += 1;
        }
        let mut s = url::find_protocol(buf, url_start, line_end);
        if s >= line_end || at(text, s) == 0 {
            self.wait_receive = true;
            return Validation::Incomplete;
        }
        let url_end = s;

        // A complete request ends with an empty line; headers are parsed along the way, on every attempt. C's
        // callbacks set state, so each pass starts over; only the tokens are collected, and only for this pass.
        self.headers.bearer_tokens.clear();
        while at(text, s) != 0 {
            loop {
                let c = at(text, s);
                if c == 0 {
                    break;
                }
                s += 1;
                if c == b'\r' {
                    break;
                }
            }
            if at(text, s) == 0 {
                break;
            }
            let c = at(text, s);
            s += 1;
            if c != b'\n' {
                continue;
            }
            if at(text, s) == b'\r' && at(text, s + 1) == b'\n' {
                if !self.decode_path_and_query(&text[url_start..url_end]) {
                    self.reset_parse_counters();
                    self.wait_receive = false;
                    return Validation::UriTooLong;
                }
                if conn.transport == Transport::Tcp
                    && conn.tls_configured
                    && !conn.tls_active
                    && (conn.tls_force || conn.tls_default)
                    && self.mode != Some(Mode::Stream)
                {
                    self.reset_parse_counters();
                    self.wait_receive = false;
                    return Validation::Redirect;
                }
                self.reset_parse_counters();
                self.wait_receive = false;
                return Validation::Ok;
            }
            s = self.parse_header_line(text, s, conn, settings);
        }

        self.wait_receive = true;
        Validation::Incomplete
    }

    /// `web_client_decode_path_and_query_string()`: false only when the URL reaches the maximum request size.
    fn decode_path_and_query(&mut self, encoded: &[u8]) -> bool {
        if encoded.len() >= MAX_REQUEST_SIZE {
            return false;
        }
        let required = encoded.len() + 1;
        if self.decode_buffer_size < required {
            let mut size = self.decode_buffer_size.max(URL_DECODE_INITIAL_SIZE);
            while size < required {
                size *= 2;
            }
            self.decode_buffer_size = size;
        }

        if self.url_as_received.is_empty() {
            self.url_as_received = encoded.to_vec();
        }
        self.path_is_mcp = false;

        // url_decode_r()'s result is ignored: whatever it decoded is used.
        let decoded = url::url_decode(encoded, self.decode_buffer_size).text;
        if self.mode == Some(Mode::Stream) {
            self.path.clear();
            self.query = decoded;
            return true;
        }
        match decoded.iter().position(|&c| c == b'?') {
            Some(q) => {
                self.query = decoded[q..].to_vec();
                self.path = decoded[..q].to_vec();
            }
            None => {
                self.query.clear();
                self.path = decoded;
            }
        }
        let p = &self.path;
        self.path_is_mcp = p.len() >= 4
            && (p.starts_with(b"/mcp") || p.starts_with(b"/sse"))
            && (p.len() == 4 || p[4] == b'/');
        true
    }

    /// `http_header_parse_line()`: `s` is the start of a header line in `text`; returns where the caller resumes.
    fn parse_header_line(
        &mut self,
        text: &[u8],
        s: usize,
        conn: &Connection,
        settings: &Settings,
    ) -> usize {
        // The name runs to the first ':' anywhere after s (a line without one swallows the next line).
        let mut e = s;
        while at(text, e) != 0 && text[e] != b':' {
            e += 1;
        }
        if at(text, e) == 0 {
            return e;
        }
        let mut v = e + 1;
        while at(text, v) == b' ' {
            v += 1;
        }
        let mut ve = v;
        while at(text, ve) != 0 && text[ve] != b'\r' {
            ve += 1;
        }
        if at(text, ve) == 0 || at(text, ve + 1) != b'\n' {
            return ve;
        }
        self.apply_header(&text[s..e], &text[v..ve], conn, settings);
        ve
    }

    /// The `supported_headers[]` callbacks: names match case-insensitively, the first entry wins.
    fn apply_header(&mut self, name: &[u8], v: &[u8], conn: &Connection, settings: &Settings) {
        let h = &mut self.headers;
        let cloud = conn.transport == Transport::Cloud && conn.acl_aclk;
        let is = |n: &str| eq_ignore_case(name, n.as_bytes());

        if is("Origin") {
            h.origin = Some(v.to_vec());
        } else if is("Connection") {
            if find_ignore_case(v, b"keep-alive").is_some() {
                h.keepalive = true;
            }
            if find_ignore_case(v, b"upgrade").is_some() {
                h.websocket_handshake = true;
            }
        } else if is("DNT") {
            if settings.respect_do_not_track {
                match v.first() {
                    Some(b'0') => h.do_not_track = false,
                    Some(b'1') => h.do_not_track = true,
                    _ => {}
                }
            }
        } else if is("User-Agent") {
            if self.mode == Some(Mode::Stream) {
                h.user_agent = Some(v.to_vec());
            }
        } else if is("Accept") {
            h.accept_json = false;
            h.accept_sse = false;
            h.accept_text = false;
            for item in accept_media_ranges(v) {
                let starts = |prefix: &[u8]| {
                    item.len() >= prefix.len() && item[..prefix.len()].eq_ignore_ascii_case(prefix)
                };
                if starts(b"application/json") {
                    h.accept_json = true;
                } else if starts(b"text/event-stream") {
                    h.accept_sse = true;
                } else if starts(b"text/plain") {
                    h.accept_text = true;
                }
            }
        } else if is("X-Auth-Token") {
            h.auth_token = Some(v.to_vec());
        } else if is("Host") {
            h.server_host = Some(truncated(v, MAX_HOST));
        } else if is("Accept-Encoding") {
            // web_client_enable_deflate(w, true): WebRTC clients keep plain output.
            if settings.gzip
                && find_ignore_case(v, b"gzip").is_some()
                && conn.transport != Transport::WebRtc
            {
                h.gzip = true;
                if conn.transport != Transport::Cloud {
                    // Cloud sends the whole response at once, not in chunks.
                    h.chunked = true;
                }
            }
        } else if is("X-Forwarded-Host") {
            h.forwarded_host = Some(truncated(v, MAX_HOST));
        } else if is("X-Forwarded-For") {
            if !v.is_empty() {
                h.forwarded_for = truncated(v, MAX_FORWARDED_FOR);
            }
        } else if is("X-Transaction-Id") {
            if let Some(uuid) = uuid_parse_flexi(&truncated(v, MAX_UUID_TEXT)) {
                h.transaction = Some(uuid);
            }
        } else if is("X-Netdata-Account-Id") {
            if cloud {
                if let Some(uuid) = uuid_parse_flexi(&truncated(v, MAX_UUID_TEXT)) {
                    h.cloud_account_id = Some(uuid);
                }
            }
        } else if is("X-Netdata-Role") {
            if cloud {
                let role = truncated(v, 99);
                let named = |n: &str| eq_ignore_case(&role, n.as_bytes());
                h.cloud_role = Some(if named("admin") {
                    UserRole::Admin
                } else if named("manager") {
                    UserRole::Manager
                } else if named("troubleshooter") {
                    UserRole::Troubleshooter
                } else if named("observer") {
                    UserRole::Observer
                } else if named("billing") {
                    UserRole::Billing
                } else {
                    UserRole::Member
                });
            }
        } else if is("X-Netdata-Permissions") {
            if cloud {
                h.cloud_permissions = Some(v.to_vec());
            }
        } else if is("X-Netdata-User-Name") {
            if cloud {
                h.cloud_user_name = Some(truncated(v, MAX_CLIENT_NAME));
            }
        } else if is("X-Netdata-Auth") || is("Authorization") {
            // Cloud requests need no bearer token.
            if !cloud && v.len() >= 7 && v[..7].eq_ignore_ascii_case(b"Bearer ") {
                let mut t = 7;
                while is_space(at(v, t)) {
                    t += 1;
                }
                h.bearer_tokens.push(v[t.min(v.len())..].to_vec());
            }
        } else if is("Upgrade") {
            if eq_ignore_case(v, b"websocket") {
                h.websocket = true;
            }
        } else if is("Sec-WebSocket-Key") {
            h.websocket_key = Some(v.to_vec());
        } else if is("Sec-WebSocket-Version") {
            if v != b"13" {
                h.websocket = false;
            }
        } else if is("Sec-WebSocket-Protocol") {
            h.websocket_protocol = Some(v.to_vec());
        } else if is("Sec-WebSocket-Extensions") {
            h.websocket_extensions = Some(v.to_vec());
        } else if is("Mcp-Session-Id") {
            h.mcp_session_id = uuid_parse_flexi(v);
        }
    }
}

/// The media ranges of an `Accept` value, each up to its first `,` or `;`, as `http_header_accept()` walks them.
fn accept_media_ranges(v: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < v.len() {
        while p < v.len() && matches!(v[p], b' ' | b'\t' | b',') {
            p += 1;
        }
        if p >= v.len() {
            break;
        }
        let start = p;
        while p < v.len() && v[p] != b',' && v[p] != b';' {
            p += 1;
        }
        let item = &v[start..p];
        while p < v.len() && v[p] != b',' {
            p += 1;
        }
        if !item.is_empty() {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: Connection = Connection {
        transport: Transport::Tcp,
        tls_configured: false,
        tls_active: false,
        tls_force: false,
        tls_default: false,
        acl_aclk: false,
    };
    const SETTINGS: Settings = Settings {
        gzip: true,
        respect_do_not_track: false,
    };

    fn feed(chunks: &[&[u8]]) -> (Request, Vec<Validation>) {
        let mut req = Request::default();
        let mut buf = Vec::new();
        let mut results = Vec::new();
        for chunk in chunks {
            buf.extend_from_slice(chunk);
            results.push(req.validate(&buf, &TCP, &SETTINGS));
        }
        (req, results)
    }

    #[test]
    fn a_simple_get_is_parsed() {
        let (req, results) = feed(&[
            b"GET /api/v1/info?x=1%20y HTTP/1.1\r\nHost: localhost:19999\r\nConnection: keep-alive\r\nAccept-Encoding: gzip, deflate\r\nOrigin: http://a\r\n\r\n",
        ]);
        assert_eq!(results, vec![Validation::Ok]);
        assert_eq!(req.mode, Some(Mode::Get));
        assert_eq!(req.url_as_received, b"/api/v1/info?x=1%20y");
        assert_eq!(req.path, b"/api/v1/info");
        assert_eq!(req.query, b"?x=1 y");
        let want = Headers {
            origin: Some(b"http://a".to_vec()),
            keepalive: true,
            server_host: Some(b"localhost:19999".to_vec()),
            gzip: true,
            chunked: true,
            ..Headers::default()
        };
        assert_eq!(req.headers, want);
        assert!(!req.wait_receive);
    }

    #[test]
    fn split_receives_complete_on_the_final_chunk() {
        let (req, results) = feed(&[b"GET /x HT", b"TP/1.1\r\nHo", b"st: h\r\n", b"\r\n"]);
        assert_eq!(
            results,
            vec![
                Validation::Incomplete,
                Validation::Incomplete,
                Validation::Incomplete,
                Validation::Ok
            ]
        );
        assert_eq!(req.path, b"/x");
        assert_eq!(req.headers.server_host, Some(b"h".to_vec()));
    }

    #[test]
    fn unsupported_methods_are_rejected_immediately() {
        let (_, results) = feed(&[b"PATCH / HTTP/1.1\r\n\r\n"]);
        assert_eq!(results, vec![Validation::NotSupported]);
        let (_, results) = feed(&[b"get / HTTP/1.1\r\n\r\n"]);
        assert_eq!(results, vec![Validation::NotSupported]);
    }

    #[test]
    fn a_header_without_colon_never_completes() {
        let (_, results) = feed(&[b"GET / HTTP/1.1\r\nFoo\r\n\r\n"]);
        assert_eq!(results, vec![Validation::Incomplete]);
    }

    #[test]
    fn a_header_without_colon_swallows_the_next_line() {
        let (req, results) = feed(&[b"GET / HTTP/1.1\r\nFoo\r\nHost: h\r\n\r\n"]);
        assert_eq!(results, vec![Validation::Ok]);
        assert_eq!(req.headers.server_host, None);
    }

    #[test]
    fn header_names_match_case_insensitively_but_are_not_trimmed() {
        let (req, _) = feed(&[b"GET / HTTP/1.1\r\nhOsT:  h1\r\n Origin: o\r\n\r\n"]);
        assert_eq!(req.headers.server_host, Some(b"h1".to_vec()));
        assert_eq!(req.headers.origin, None);
    }

    #[test]
    fn urls_keep_spaces_and_stop_at_the_first_http_marker() {
        let (req, results) = feed(&[b"GET /a b HTTP/1.1 HTTP/1.0\r\n\r\n"]);
        assert_eq!(results, vec![Validation::Ok]);
        assert_eq!(req.path, b"/a b");
    }

    #[test]
    fn an_undecodable_escape_truncates_the_path() {
        let (req, results) = feed(&[b"GET /abc%0Adef?x=1 HTTP/1.1\r\n\r\n"]);
        assert_eq!(results, vec![Validation::Ok]);
        assert_eq!(req.path, b"/abc");
        assert_eq!(req.query, b"");
    }

    #[test]
    fn mcp_paths_are_classified_on_segment_boundaries() {
        for (url, mcp) in [
            (&b"/mcp"[..], true),
            (b"/sse/x", true),
            (b"/mcpfoo", false),
            (b"/api", false),
        ] {
            let mut request = b"GET ".to_vec();
            request.extend_from_slice(url);
            request.extend_from_slice(b" HTTP/1.1\r\n\r\n");
            let (req, _) = feed(&[&request]);
            assert_eq!(req.path_is_mcp, mcp, "{:?}", String::from_utf8_lossy(url));
        }
    }

    #[test]
    fn stream_requests_keep_everything_as_query() {
        let (req, results) = feed(&[
            b"STREAM key=k&hostname=child&ver=9 HTTP/1.1\r\nUser-Agent: netdata/v2\r\n\r\n",
        ]);
        assert_eq!(results, vec![Validation::Ok]);
        assert_eq!(req.mode, Some(Mode::Stream));
        assert_eq!(req.path, b"");
        assert_eq!(req.query, b"key=k&hostname=child&ver=9");
        assert_eq!(req.headers.user_agent, Some(b"netdata/v2".to_vec()));
    }

    #[test]
    fn stream_without_tls_on_a_forced_tls_listener_is_refused() {
        let conn = Connection {
            tls_configured: true,
            tls_force: true,
            ..TCP
        };
        let mut req = Request::default();
        let buf = b"STREAM key=k&hostname=child&ver=9 HTTP/1.1\r\n\r\n";
        assert_eq!(
            req.validate(buf, &conn, &SETTINGS),
            Validation::NotSupported
        );
        assert_eq!(req.stream_tls_refused_hostname, Some(b"child".to_vec()));
    }

    #[test]
    fn plain_http_on_a_tls_default_listener_is_redirected() {
        let conn = Connection {
            tls_configured: true,
            tls_default: true,
            ..TCP
        };
        let mut req = Request::default();
        assert_eq!(
            req.validate(b"GET / HTTP/1.1\r\n\r\n", &conn, &SETTINGS),
            Validation::Redirect
        );
    }

    #[test]
    fn post_payload_is_attached() {
        let (req, results) = feed(&[
            b"POST /api/v3/config HTTP/1.1\r\nContent-Length: 2\r\n",
            b"Content-Type: application/json\r\n\r\n{}",
        ]);
        assert_eq!(results, vec![Validation::Incomplete, Validation::Ok]);
        let payload = req.payload.unwrap();
        assert_eq!(payload.body, b"{}");
    }

    #[test]
    fn too_many_partial_receives_give_up() {
        let mut req = Request::default();
        let mut buf = b"GET / HTTP/1.1\r\n".to_vec();
        let mut last = Validation::Incomplete;
        for _ in 0..=MAX_HEADER_FETCH_TRIES {
            buf.push(b'x');
            last = req.validate(&buf, &TCP, &SETTINGS);
        }
        assert_eq!(last, Validation::TooManyReadRetries(MAX_HEADER_FETCH_TRIES + 1));
    }

    #[test]
    fn tokens_do_not_accumulate_across_parse_attempts() {
        // A colon-less line keeps the request incomplete; every attempt re-parses the same headers.
        let mut chunks: Vec<&[u8]> =
            vec![b"GET / HTTP/1.1\r\nAuthorization: Bearer a\r\nFoo\r\n\r\n"];
        chunks.extend(std::iter::repeat_n(&b"\r\n\r\n"[..], 5));
        let (req, results) = feed(&chunks);
        assert!(results.iter().all(|r| *r == Validation::Incomplete));
        assert_eq!(req.headers.bearer_tokens, vec![b"a".to_vec()]);
    }

    #[test]
    fn identity_headers_and_uuids() {
        let (req, _) = feed(&[
            b"GET / HTTP/1.1\r\nX-Transaction-Id: 0123456789abcdef0123456789abcdef\r\nX-Forwarded-For: 10.0.0.1\r\nX-Netdata-Auth: Bearer   tok\r\nMcp-Session-Id: 01234567-89ab-cdef-0123-456789abcdef\r\nX-Netdata-Role: admin\r\n\r\n",
        ]);
        let h = &req.headers;
        assert_eq!(h.transaction.map(|u| u[0]), Some(0x01));
        assert_eq!(h.forwarded_for, b"10.0.0.1");
        assert_eq!(h.bearer_tokens, vec![b"tok".to_vec()]);
        assert_eq!(h.mcp_session_id.map(|u| u[15]), Some(0xef));
        // Cloud identity headers are ignored on plain TCP connections.
        assert_eq!(h.cloud_role, None);
    }

    #[test]
    fn accept_header_flags() {
        let (req, _) = feed(&[
            b"GET / HTTP/1.1\r\nAccept: text/html;q=1, Application/JSON, text/event-stream\r\n\r\n",
        ]);
        assert!(req.headers.accept_json && req.headers.accept_sse && !req.headers.accept_text);
    }
}
