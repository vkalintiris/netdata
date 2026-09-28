//! The child's HTTP upgrade before `STREAM` when `[stream] parent using h2o = yes`
//! (`stream_connect_upgrade_prelude()`), and the response parser it borrows from the ACLK
//! (`parse_http_response()` in `src/aclk/https_client.c`) with C's error records. Map:
//! `knowledge/map-m7-commit3-connector.md` §8.

use std::collections::HashMap;

use netdata_agent_log::Priority;

use crate::connect_to::{NdSock, Thread, log_errno};
use crate::parents::HTTP_HEADER_SIZE;

/// `HTTP_HDR_BUFFER_SIZE`.
const HDR_BUFFER_SIZE: usize = 1024;
/// The prelude's send and receive timeouts: `nd_sock_*_timeout()` take seconds, so C's 1000 is 1000 s.
const TIMEOUT_S: i64 = 1000;
/// `TRANSFER_ENCODING_CHUNKED`.
const CHUNKED: i64 = -2;
const CRLF: &[u8] = b"\r\n";

/// `http_parse_rc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parse {
    Error = -1,
    NeedMoreData = 0,
    Success = 1,
}

/// `http_parse_ctx`, over the received bytes as the ring buffer `rbuf_create(bytes, bytes)` holds them.
#[derive(Debug)]
struct Response<'a> {
    data: &'a [u8],
    tail: usize,
    code: i32,
    content_length: i64,
    headers: HashMap<Vec<u8>, Vec<u8>>,
}

/// `netdata_log_error()` of the parser.
fn aclk_error(th: &Thread<'_>, text: std::fmt::Arguments<'_>) {
    log_errno!(th, Priority::Err, "ACLK: {text}");
}

impl<'a> Response<'a> {
    fn new(data: &'a [u8]) -> Self {
        Response { data, tail: 0, code: 0, content_length: -1, headers: HashMap::new() }
    }

    fn available(&self) -> &'a [u8] {
        &self.data[self.tail..]
    }

    /// `rbuf_find_bytes()`: the offset from the tail of the first `needle`.
    fn find(&self, needle: &[u8]) -> Option<usize> {
        netdata_agent_text::c::find(self.available(), needle)
    }

    /// `rbuf_pop()`.
    fn pop(&mut self, n: usize) -> &'a [u8] {
        let n = n.min(self.available().len());
        let out = &self.data[self.tail..self.tail + n];
        self.tail += n;
        out
    }

    fn bump(&mut self, n: usize) {
        self.tail = (self.tail + n).min(self.data.len());
    }

    /// `process_http_hdr()`: content length and chunked encoding are checked, every other header kept (the last of
    /// duplicates wins).
    fn process_header(&mut self, key: Vec<u8>, value: &[u8], th: &Thread<'_>) -> bool {
        if self.content_length < 0 && key == b"content-length" {
            if self.content_length == CHUNKED {
                aclk_error(th, format_args!("Content-length and transfer-encoding: chunked headers are mutually exclusive"));
                return false;
            }
            // str2u() into an int
            self.content_length = i64::from(netdata_agent_text::parse::str2u(value) as i32);
            if self.content_length < 0 {
                aclk_error(th, format_args!("Invalid content-length {}", self.content_length));
                return false;
            }
            return true;
        }
        if key == b"transfer-encoding" {
            if value == b"chunked" {
                if self.content_length != -1 {
                    aclk_error(
                        th,
                        format_args!("Content-length and transfer-encoding: chunked headers are mutually exclusive"),
                    );
                    return false;
                }
                self.content_length = CHUNKED;
            }
            return true;
        }
        self.headers.insert(key, value.to_vec());
        true
    }

    /// `parse_http_hdr()`.
    fn parse_header(&mut self, th: &Thread<'_>) -> bool {
        let Some(mut end) = self.find(CRLF) else {
            aclk_error(th, format_args!("CRLF expected"));
            return false;
        };
        let separator = self.find(b": ");
        let Some(at) = separator.filter(|&at| at < end) else {
            aclk_error(th, format_args!("Missing Key/Value separator"));
            return false;
        };
        if at >= HDR_BUFFER_SIZE {
            aclk_error(th, format_args!("Key name is too long"));
            return false;
        }
        // C's copies end at their first NUL; the key is lowercased after the value's check
        let key = netdata_agent_text::c::c_str(self.pop(at));
        self.bump(2);
        end -= 2 + at;
        if end >= HDR_BUFFER_SIZE {
            aclk_error(th, format_args!("Value of key \"{}\" too long", String::from_utf8_lossy(key)));
            return false;
        }
        let value = netdata_agent_text::c::c_str(self.pop(end));
        self.process_header(key.to_ascii_lowercase(), value, th)
    }

    /// `process_chunked_content()`: the chunks up to the final one, or the error that stops them.
    fn chunked(&mut self, th: &Thread<'_>) -> Parse {
        loop {
            let Some(at) = self.find(CRLF) else {
                // C's ring is exactly as large as what was received, but the headers were taken from it, so it is
                // never full here
                return Parse::NeedMoreData;
            };
            if at == 0 {
                return self.final_crlf(th);
            }
            if at >= HDR_BUFFER_SIZE {
                aclk_error(th, format_args!("Chunk size is too long"));
                return Parse::Error;
            }
            let size = netdata_agent_text::parse::strtoll16(netdata_agent_text::c::c_str(self.pop(at))).0;
            if size < 0 || size == i64::MAX {
                aclk_error(th, format_args!("Chunk size out of range"));
                return Parse::Error;
            }
            if size == 0 {
                // the end CRLF of a zero chunk, then the final one
                match self.crlf(th) {
                    Parse::Success => return self.final_crlf(th),
                    other => return other,
                }
            }
            self.bump(CRLF.len());
            if self.available().len() < size as usize {
                return Parse::NeedMoreData;
            }
            self.bump(size as usize);
            match self.crlf(th) {
                Parse::Success => {}
                other => return other,
            }
        }
    }

    /// A chunk's closing CRLF.
    fn crlf(&mut self, th: &Thread<'_>) -> Parse {
        if self.available().len() < CRLF.len() {
            return Parse::NeedMoreData;
        }
        if self.pop(CRLF.len()) != CRLF {
            aclk_error(th, format_args!("CRLF expected"));
            return Parse::Error;
        }
        Parse::Success
    }

    fn final_crlf(&mut self, th: &Thread<'_>) -> Parse {
        self.crlf(th)
    }

    /// `parse_http_response()` from `HTTP_PARSE_INITIAL` with `HTTP_PARSE_FLAG_DONT_WAIT_FOR_CONTENT`.
    fn parse(&mut self, th: &Thread<'_>) -> Parse {
        let Some(line_end) = self.find(CRLF) else {
            return Parse::NeedMoreData;
        };
        let _ = line_end;
        if !self.available().starts_with(b"HTTP/1.1 ") {
            aclk_error(th, format_args!("Expected response to start with \"HTTP/1.1 \""));
            return Parse::Error;
        }
        self.bump(b"HTTP/1.1 ".len());
        let rc = self.pop(4);
        if rc.len() != 4 {
            aclk_error(th, format_args!("Expected HTTP status code"));
            return Parse::Error;
        }
        if rc[3] != b' ' {
            aclk_error(th, format_args!("Expected space after HTTP return code"));
            return Parse::Error;
        }
        // atoi() of the three characters
        self.code = netdata_agent_text::parse::str2i(&rc[..3]);
        if !(100..600).contains(&self.code) {
            aclk_error(th, format_args!("HTTP code not in range 100 to 599"));
            return Parse::Error;
        }
        let rest = self.find(CRLF).unwrap_or(0);
        self.bump(rest + CRLF.len());
        loop {
            let Some(at) = self.find(CRLF) else {
                return Parse::NeedMoreData;
            };
            if at == 0 {
                self.bump(CRLF.len());
                break;
            }
            if !self.parse_header(th) {
                return Parse::Error;
            }
            let rest = self.find(CRLF).unwrap_or(0);
            self.bump(rest + CRLF.len());
        }
        if self.content_length == CHUNKED {
            return self.chunked(th);
        }
        Parse::Success
    }

    fn header(&self, name: &str) -> Option<&[u8]> {
        self.headers.get(name.as_bytes()).map(Vec::as_slice)
    }
}

/// `error_report()`: an error record with `errno` cleared first.
fn report(th: &Thread<'_>, text: std::fmt::Arguments<'_>) {
    th.errno.set(0);
    log_errno!(th, Priority::Err, "{text}");
}

/// `stream_connect_upgrade_prelude()`: `GET /stream` asking to switch to `netdata_stream/2.0`, which must be answered
/// 101 with `connection: upgrade` and `upgrade: netdata_stream/2.0`. The first received bytes are parsed once.
pub fn upgrade_prelude(sock: &mut NdSock, th: &Thread<'_>) -> bool {
    let request = b"GET /stream HTTP/1.1\r\nUpgrade: netdata_stream/2.0\r\nConnection: Upgrade\r\n\r\n";
    if sock.send_timeout(request, TIMEOUT_S, th) <= 0 {
        report(th, format_args!("Error writing to remote"));
        return false;
    }
    let mut buf = vec![0u8; HTTP_HEADER_SIZE];
    let received = sock.recv_timeout(&mut buf, TIMEOUT_S, th);
    if received <= 0 {
        report(th, format_args!("Error reading from remote"));
        return false;
    }
    let mut response = Response::new(&buf[..received as usize]);
    let rc = response.parse(th);
    if rc != Parse::Success {
        report(th, format_args!("Failed to parse HTTP response sent. ({})", rc as i32));
        return false;
    }
    match response.code {
        301 => {
            match response.header("location") {
                Some(location) => report(
                    th,
                    format_args!(
                        "HTTP response is 301 Moved Permanently (location: \"{}\") instead of expected 101 Switching \
                         Protocols.",
                        String::from_utf8_lossy(location)
                    ),
                ),
                None => report(th, format_args!("HTTP response is 301 instead of expected 101 Switching Protocols.")),
            }
            return false;
        }
        404 => {
            report(
                th,
                format_args!("HTTP response is 404 instead of expected 101 Switching Protocols. Parent version too old."),
            );
            return false;
        }
        101 => {}
        code => {
            report(th, format_args!("HTTP response is {code} instead of expected 101 Switching Protocols"));
            return false;
        }
    }
    match response.header("connection") {
        None => {
            report(th, format_args!("Missing \"connection\" header in reply"));
            return false;
        }
        Some(v) if !v.starts_with(b"upgrade") => {
            report(th, format_args!("Expected \"connection: upgrade\""));
            return false;
        }
        Some(_) => {}
    }
    match response.header("upgrade") {
        None => {
            report(th, format_args!("Missing \"upgrade\" header in reply"));
            false
        }
        Some(v) if !v.starts_with(b"netdata_stream/2.0") => {
            report(th, format_args!("Expected \"upgrade: netdata_stream/2.0\""));
            false
        }
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    fn parse(bytes: &[u8]) -> (Parse, i32, Vec<String>, Option<Vec<u8>>) {
        let cancel = AtomicBool::new(false);
        let th = Thread::new(&cancel);
        let mut r = Response::new(bytes);
        let (rc, records) = netdata_agent_log::capture(|| r.parse(&th));
        let texts = records.into_iter().map(|r| r.message.unwrap_or_default()).collect();
        (rc, r.code, texts, r.header("connection").map(<[u8]>::to_vec))
    }

    #[test]
    fn responses_parse_as_the_aclk_parser() {
        let ok = parse(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: netdata_stream/2.0\r\n\r\n");
        assert_eq!(ok, (Parse::Success, 101, vec![], Some(b"Upgrade".to_vec())));
        assert_eq!(parse(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").0, Parse::Success);
        // the headers must end in the bytes received
        assert_eq!(parse(b"HTTP/1.1 101 OK\r\nConnection: upgrade\r\n").0, Parse::NeedMoreData);
        assert_eq!(parse(b"garbage").0, Parse::NeedMoreData);
        let e = parse(b"HTTP/1.0 200 OK\r\n\r\n");
        assert_eq!((e.0, e.2), (Parse::Error, vec!["ACLK: Expected response to start with \"HTTP/1.1 \"".into()]));
        assert_eq!(parse(b"HTTP/1.1 099 x\r\n\r\n").2, ["ACLK: HTTP code not in range 100 to 599"]);
        assert_eq!(parse(b"HTTP/1.1 2000 x\r\n\r\n").2, ["ACLK: Expected space after HTTP return code"]);
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\nbroken\r\n\r\n").2, ["ACLK: Missing Key/Value separator"]);
        // either order of the two headers is refused
        assert_eq!(
            parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n").0,
            Parse::Error
        );
        assert_eq!(
            parse(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n").2,
            ["ACLK: Content-length and transfer-encoding: chunked headers are mutually exclusive"]
        );
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n").0, Parse::Success);
        // a reply cut before its chunk size's CRLF waits for more (R40 m1)
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3").0, Parse::NeedMoreData);
        // the too-long value's record names the key as received (R40 m2)
        let long = format!("HTTP/1.1 200 OK\r\nContent-Security-Policy: {}\r\n\r\n", "x".repeat(1024));
        assert_eq!(parse(long.as_bytes()).2, ["ACLK: Value of key \"Content-Security-Policy\" too long"]);
    }
}
