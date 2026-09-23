//! URL helpers ported from `src/libnetdata/url/url.c`.
//!
//! Inputs are byte slices; where C walks a NUL-terminated string, the end of the slice or a NUL byte ends it.

use netdata_agent_text::c::{at, c_str, find, find_ignore_case, is_print, is_space};
use netdata_agent_text::parse::strtoull10;

use crate::content_type::ContentType;

/// `from_hex()`: the C arithmetic, including the value it yields for bytes that are not hex digits (computed in
/// `int` and truncated to `char`), because `url_percent_escape_decode()` never validates its digits.
fn from_hex(ch: u8) -> u8 {
    let v = if ch.is_ascii_digit() {
        i32::from(ch) - i32::from(b'0')
    } else {
        i32::from(ch.to_ascii_lowercase()) - i32::from(b'a') + 10
    };
    v as u8
}

/// `url_percent_escape_decode()`: `s` points at the byte before two hex digits (normally `%`); 0 when fewer than
/// three bytes remain.
fn percent_escape_decode(s: &[u8]) -> u8 {
    if at(s, 0) != 0 && at(s, 1) != 0 && at(s, 2) != 0 {
        ((u32::from(from_hex(s[1])) << 4) | u32::from(from_hex(s[2]))) as u8
    } else {
        0
    }
}

fn is_utf8_byte(b: u8) -> bool {
    b & 0x80 != 0
}

fn is_utf8_startbyte(b: u8) -> bool {
    is_utf8_byte(b) && b & 0x40 != 0
}

/// `url_utf8_get_byte_length()`.
fn utf8_byte_length(first: u8) -> i32 {
    if !is_utf8_byte(first) {
        return 1;
    }
    let length = first.leading_ones() as i32;
    if length > 4 || length == 1 {
        -1
    } else {
        length
    }
}

/// `url_decode_multibyte_utf8()`: decodes a `%XX%XX..` UTF-8 sequence starting at `s[0]` into `out`, where `room`
/// is `d_end - d` in C. Continuation escapes are read three bytes apart without checking for `%`, as in C.
fn decode_multibyte_utf8(s: &[u8], out: &mut Vec<u8>, room: usize) -> usize {
    let first = percent_escape_decode(s);
    if first == 0 || !is_utf8_startbyte(first) {
        return 0;
    }
    let length = utf8_byte_length(first);
    if length <= 0 || length as usize >= room {
        return 0;
    }
    let length = length as usize;
    let mut decoded = Vec::with_capacity(length);
    for n in 0..length {
        let c = percent_escape_decode(s.get(n * 3..).unwrap_or(&[]));
        if !is_utf8_byte(c) || (n != 0 && is_utf8_startbyte(c)) {
            return 0;
        }
        decoded.push(c);
    }
    out.extend_from_slice(&decoded);
    length
}

/// `utf8_check()`: offset of the first malformed, overlong, surrogate or non-character sequence, if any.
pub(crate) fn utf8_check(s: &[u8]) -> Option<usize> {
    let s = c_str(s);
    let mut i = 0;
    while i < s.len() {
        let b0 = s[i];
        let (b1, b2, b3) = (at(s, i + 1), at(s, i + 2), at(s, i + 3));
        if b0 < 0x80 {
            i += 1;
        } else if b0 & 0xe0 == 0xc0 {
            if b1 & 0xc0 != 0x80 || b0 & 0xfe == 0xc0 {
                return Some(i);
            }
            i += 2;
        } else if b0 & 0xf0 == 0xe0 {
            if b1 & 0xc0 != 0x80
                || b2 & 0xc0 != 0x80
                || (b0 == 0xe0 && b1 & 0xe0 == 0x80)
                || (b0 == 0xed && b1 & 0xe0 == 0xa0)
                || (b0 == 0xef && b1 == 0xbf && b2 & 0xfe == 0xbe)
            {
                return Some(i);
            }
            i += 3;
        } else if b0 & 0xf8 == 0xf0 {
            if b1 & 0xc0 != 0x80
                || b2 & 0xc0 != 0x80
                || b3 & 0xc0 != 0x80
                || (b0 == 0xf0 && b1 & 0xf0 == 0x80)
                || (b0 == 0xf4 && b1 > 0x8f)
                || b0 > 0xf4
            {
                return Some(i);
            }
            i += 4;
        } else {
            return Some(i);
        }
    }
    None
}

/// What `url_decode_r()` leaves behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The destination buffer's string after the call. On failure C still leaves the text decoded before the
    /// failing escape, and callers such as the web server use it regardless of the result.
    pub text: Vec<u8>,
    /// `url_decode_r()` returned non-NULL.
    pub ok: bool,
}

/// `url_decode_r(to, url, size)`: `+` becomes a space; `%XX` must decode to a printable byte or a valid UTF-8
/// sequence, otherwise decoding stops there. `size` is the destination capacity (terminator included).
pub fn url_decode(url: &[u8], size: usize) -> Decoded {
    if size == 0 {
        return Decoded {
            text: Vec::new(),
            ok: false,
        };
    }
    let url = c_str(url);
    let end = size - 1;
    let mut out = Vec::with_capacity(url.len().min(end));
    let mut i = 0;
    while i < url.len() && out.len() < end {
        match url[i] {
            b'%' => {
                let t = percent_escape_decode(&url[i..]);
                if is_utf8_byte(t) {
                    let room = end - out.len();
                    let written = decode_multibyte_utf8(&url[i..], &mut out, room);
                    if written == 0 {
                        return Decoded {
                            text: out,
                            ok: false,
                        };
                    }
                    i += written * 3 - 1;
                } else if t != 0 && is_print(t) {
                    // Only printable bytes, to avoid header injection.
                    out.push(t);
                    i += 2;
                } else {
                    return Decoded {
                        text: out,
                        ok: false,
                    };
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    let ok = utf8_check(&out).is_none();
    Decoded { text: out, ok }
}

/// `url_find_protocol(s, end)`: the offset of the first ` HTTP/` in `buf[s..end]`, or where the scan stopped (at
/// `end` or a NUL byte).
pub fn find_protocol(buf: &[u8], mut s: usize, end: usize) -> usize {
    while s < end && at(buf, s) != 0 {
        while s < end && at(buf, s) != 0 && buf[s] != b' ' {
            s += 1;
        }
        if s >= end || at(buf, s) == 0 {
            break;
        }
        if end - s >= 6 && &buf[s..s + 6] == b" HTTP/" {
            break;
        }
        s += 1;
    }
    s
}

/// A request body received with POST or PUT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub body: Vec<u8>,
    pub content_type: ContentType,
}

/// `url_is_request_complete_and_extract_payload()` state kept per client between receives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExpectedSize {
    #[default]
    Unknown,
    Known(usize),
    /// `SIZE_MAX` in C: the `Content-Length` was invalid; the request never completes.
    Invalid,
}

/// Scans the C string after `Content-Length: ` for the value the way both C passes do.
fn content_length_at(buf: &[u8]) -> Option<(usize, usize)> {
    let cl = find_ignore_case(buf, b"Content-Length: ")? + 16;
    let mut p = cl;
    while matches!(at(buf, p), b' ' | b'\t') {
        p += 1;
    }
    if !at(buf, p).is_ascii_digit() {
        return None;
    }
    let (value, consumed, erange) = strtoull10(&buf[p..]);
    if erange {
        return None;
    }
    let mut end = p + consumed;
    while matches!(at(buf, end), b' ' | b'\t') {
        end += 1;
    }
    if at(buf, end) != b'\r' || at(buf, end + 1) != b'\n' {
        return None;
    }
    Some((usize::try_from(value).ok()?, p))
}

/// `url_is_request_complete_and_extract_payload(begin, end, length, ...)`: `buf` is the received request (C sees
/// it as a NUL-terminated string), `end` the rescan cursor and `length` the received length. On completion of a
/// POST/PUT the payload is stored in `payload`.
pub fn is_request_complete(
    buf: &[u8],
    end: usize,
    length: usize,
    payload: &mut Option<Payload>,
    expected: &mut ExpectedSize,
) -> bool {
    if length < 4 {
        return false;
    }
    let text = c_str(buf);
    // The caller rewinds its cursor by up to 4 bytes; clamp so searches start inside the buffer.
    let end = end.max(4);
    // strstr() from a pointer: the C string that starts there, even past an earlier embedded NUL.
    let search_from = |from: usize| {
        let from = from.min(length).min(buf.len());
        find(c_str(&buf[from..length.min(buf.len())]), b"\r\n\r\n").is_some()
    };

    let starts = |p: &[u8]| text.len() >= p.len() && &text[..p.len()] == p;
    if starts(b"GET ") {
        return search_from(end - 4);
    }
    if !(starts(b"POST ") || starts(b"PUT ")) {
        return search_from(end - 4);
    }

    if *expected == ExpectedSize::Invalid {
        return false;
    }
    if *expected == ExpectedSize::Unknown {
        // The first call may hold the whole request; later calls start before the newly received bytes.
        let headers_from = if end == length { 0 } else { end };
        if !search_from(headers_from) {
            return false;
        }
        let Some((content_length, digits_at)) = content_length_at(text) else {
            *expected = ExpectedSize::Invalid;
            return false;
        };
        let Some(header_end) = find(&text[digits_at..], b"\r\n\r\n") else {
            *expected = ExpectedSize::Invalid;
            return false;
        };
        let payload_offset = digits_at + header_end + 4;
        match payload_offset.checked_add(content_length) {
            Some(total) => *expected = ExpectedSize::Known(total),
            None => {
                *expected = ExpectedSize::Invalid;
                return false;
            }
        }
    }
    if *expected != ExpectedSize::Known(length) {
        return false;
    }

    // Second pass over the complete request, as in C.
    let Some((content_length, digits_at)) = content_length_at(text) else {
        return false;
    };
    let Some(header_end) = find(&text[digits_at..], b"\r\n\r\n") else {
        return false;
    };
    let payload_start = digits_at + header_end + 4;
    let payload_length = length - payload_start;
    if payload_length != content_length {
        return false;
    }
    let content_type = match find_ignore_case(text, b"Content-Type: ") {
        Some(ct) => {
            let mut p = ct + 14;
            while is_space(at(text, p)) {
                p += 1;
            }
            let mut q = p;
            while at(text, q) != 0 && !is_space(at(text, q)) && at(text, q) != b';' {
                q += 1;
            }
            ContentType::from_name(&text[p..q])
        }
        None => ContentType::TextPlain,
    };
    // The body is taken from the received bytes by length, not as a C string.
    let body = buf[payload_start..length].to_vec();
    *payload = Some(Payload { body, content_type });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_decode_cases() {
        let cases: [(&[u8], &[u8], bool); 12] = [
            (
                b"/api/v1/data?chart=system.cpu",
                b"/api/v1/data?chart=system.cpu",
                true,
            ),
            (b"/a+b%20c", b"/a b c", true),
            // %26/%3D/%3F decode to real separators.
            (b"/x?a=1%26b=2", b"/x?a=1&b=2", true),
            // Non-printable escapes stop decoding and keep what was decoded so far.
            (b"/abc%0Adef", b"/abc", false),
            (b"/abc%00def", b"/abc", false),
            // A UTF-8 sequence is decoded as a whole.
            (b"/%C3%A9t%C3%A9", "/été".as_bytes(), true),
            // Continuation escapes are read three bytes apart without checking for '%'.
            (b"/%C3xA9", "/é".as_bytes(), true),
            // A lone continuation byte as the first escape fails.
            (b"/%A9", b"/", false),
            // Truncated escapes decode to 0 and fail.
            (b"/ab%4", b"/ab", false),
            // Invalid hex digits yield C's garbage byte ('%zz' -> 0x33 '3').
            (b"/%zz", b"/3", true),
            // Raw invalid UTF-8 passes through but fails utf8_check (the text is still used).
            (b"/\xff", b"/\xff", false),
            // An overlong raw sequence is rejected by utf8_check.
            (b"/\xc0\xaf", b"/\xc0\xaf", false),
        ];
        for (input, text, ok) in cases {
            let got = url_decode(input, 4096);
            assert_eq!(
                got,
                Decoded {
                    text: text.to_vec(),
                    ok
                },
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn url_decode_respects_destination_size() {
        assert_eq!(
            url_decode(b"/abcdef", 4),
            Decoded {
                text: b"/ab".to_vec(),
                ok: true
            }
        );
        // A multibyte sequence that would reach the last byte fails.
        assert_eq!(
            url_decode(b"/%C3%A9", 4),
            Decoded {
                text: b"/".to_vec(),
                ok: false
            }
        );
        assert_eq!(
            url_decode(b"/x", 0),
            Decoded {
                text: Vec::new(),
                ok: false
            }
        );
    }

    #[test]
    fn find_protocol_cases() {
        let req = b"GET /a b HTTP/1.1\r\n";
        let end = req.iter().position(|&c| c == b'\r').unwrap();
        assert_eq!(find_protocol(req, 4, end), 8);
        let no_proto = b"GET /abc\r\n";
        assert_eq!(find_protocol(no_proto, 4, 8), 8);
        let short = b"GET / HTTP";
        assert_eq!(find_protocol(short, 4, short.len()), short.len());
    }

    #[test]
    fn get_completes_only_when_the_scan_window_holds_the_terminator() {
        let mut payload = None;
        let mut expected = ExpectedSize::Unknown;
        let req = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
        assert!(is_request_complete(
            req,
            req.len(),
            req.len(),
            &mut payload,
            &mut expected
        ));
        // First read with bytes after the header end: the last 4 bytes do not hold "\r\n\r\n" (C quirk).
        let piped = b"GET / HTTP/1.1\r\n\r\nGET";
        assert!(!is_request_complete(
            piped,
            piped.len(),
            piped.len(),
            &mut payload,
            &mut expected
        ));
        assert!(payload.is_none());
        // The search starts at the cursor even when an embedded NUL precedes it.
        let nul = b"GET /\0 HTTP/1.1\r\n\r\n";
        assert!(is_request_complete(
            nul,
            nul.len(),
            nul.len(),
            &mut payload,
            &mut expected
        ));
    }

    #[test]
    fn post_payload_is_extracted_by_content_length() {
        let mut payload = None;
        let mut expected = ExpectedSize::Unknown;
        let head = b"POST /api/v3/config HTTP/1.1\r\nContent-Type: application/json; charset=x\r\nContent-Length: 4\r\n\r\n";
        let mut req = head.to_vec();
        req.extend_from_slice(b"{}");
        assert!(!is_request_complete(
            &req,
            req.len(),
            req.len(),
            &mut payload,
            &mut expected
        ));
        assert_eq!(expected, ExpectedSize::Known(head.len() + 4));
        let cursor = req.len() - 4;
        req.extend_from_slice(b"[]");
        assert!(is_request_complete(
            &req,
            cursor,
            req.len(),
            &mut payload,
            &mut expected
        ));
        assert_eq!(
            payload,
            Some(Payload {
                body: b"{}[]".to_vec(),
                content_type: ContentType::ApplicationJson
            })
        );
    }

    #[test]
    fn post_with_invalid_content_length_never_completes() {
        for head in [
            &b"POST / HTTP/1.1\r\nContent-Length: x\r\n\r\n"[..],
            b"POST / HTTP/1.1\r\n\r\n",
            b"POST / HTTP/1.1\r\nContent-Length: 1 2\r\n\r\n",
            b"POST / HTTP/1.1\r\nContent-Length: 99999999999999999999999\r\n\r\n",
        ] {
            let mut payload = None;
            let mut expected = ExpectedSize::Unknown;
            assert!(!is_request_complete(
                head,
                head.len(),
                head.len(),
                &mut payload,
                &mut expected
            ));
            assert_eq!(
                expected,
                ExpectedSize::Invalid,
                "{:?}",
                String::from_utf8_lossy(head)
            );
        }
    }
}
