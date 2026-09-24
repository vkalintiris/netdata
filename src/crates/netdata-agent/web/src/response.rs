//! The HTTP response header, ported from `web_client_build_http_header()` in `src/web/server/web_client.c`.

use netdata_agent_text::print::print_uuid_lower_compact;

use crate::content_type::ContentType;
use crate::status;

/// Everything `web_client_build_http_header()` reads from the client and the response buffer.
#[derive(Debug, Clone)]
pub struct Head<'a> {
    pub code: u16,
    pub content_type: ContentType,
    /// `response.data->date`; 0 means "now".
    pub date: i64,
    /// `response.data->expires`; 0 means "derive from the date".
    pub expires: i64,
    /// `WB_CONTENT_NO_CACHEABLE`.
    pub no_cacheable: bool,
    pub keepalive: bool,
    pub origin: Option<&'a [u8]>,
    /// `NETDATA_VERSION`.
    pub version: &'a str,
    pub path_is_mcp: bool,
    pub is_options: bool,
    /// `[web] x-frame-options response header`.
    pub x_frame_options: Option<&'a str>,
    pub has_cookies: bool,
    pub respect_do_not_track: bool,
    /// `WEB_CLIENT_FLAG_TRACKING_REQUIRED`.
    pub tracking_required: bool,
    /// Extra header lines a handler added (`response.header`), each ending in CRLF.
    pub custom: &'a [u8],
    pub gzip: bool,
    pub chunked: bool,
    /// `response.data->len`.
    pub content_length: usize,
    pub server_host: Option<&'a [u8]>,
    pub url_as_received: &'a [u8],
    pub transaction: [u8; 16],
}

/// What building the header changed on the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub bytes: Vec<u8>,
    /// Keep-alive after this response: disabled when the length is unknown.
    pub keepalive: bool,
    /// The response code after the call: 399 becomes 301.
    pub code: u16,
}

const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `rfc7231_datetime()`: `strftime("%a, %d %b %Y %H:%M:%S GMT")` of `gmtime_r()`.
pub fn rfc7231_date(t: i64) -> String {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // Civil date from days since the epoch (proleptic Gregorian, as gmtime).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let text = format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[days.rem_euclid(7) as usize],
        day,
        MONTHS[(month - 1) as usize],
        year,
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    );
    // strftime() into RFC7231_MAX_LENGTH (30) bytes fails when the text and its NUL do not fit: C sends it empty.
    if text.len() >= 30 {
        String::new()
    } else {
        text
    }
}

/// `web_client_build_http_header()`; `now` is the wall clock in seconds.
pub fn build(head: &Head<'_>, now: i64) -> Built {
    let no_cacheable = head.no_cacheable || head.code != status::OK;
    let date = if head.date == 0 { now } else { head.date };
    let expires = if head.expires == 0 {
        date + if no_cacheable { 0 } else { 86400 }
    } else {
        head.expires
    };
    let reason = status::reason(head.code);
    let mut out = Vec::with_capacity(512);
    let mut code = head.code;
    let mut keepalive = head.keepalive;

    if head.code == status::HTTPS_UPGRADE {
        out.extend_from_slice(
            format!("HTTP/1.1 {} {}\r\nLocation: https://", head.code, reason).as_bytes(),
        );
        out.extend_from_slice(head.server_host.unwrap_or(b""));
        out.extend_from_slice(head.url_as_received);
        out.extend_from_slice(b"\r\n");
        code = status::MOVED_PERM;
    } else {
        out.extend_from_slice(
            format!(
                "HTTP/1.1 {} {}\r\nConnection: {}\r\nServer: Netdata Embedded HTTP Server {}\r\nAccess-Control-Allow-Origin: ",
                head.code,
                reason,
                if keepalive { "keep-alive" } else { "close" },
                head.version
            )
            .as_bytes(),
        );
        out.extend_from_slice(head.origin.unwrap_or(b"*"));
        out.extend_from_slice(b"\r\nAccess-Control-Allow-Credentials: true\r\nDate: ");
        out.extend_from_slice(rfc7231_date(date).as_bytes());
        out.extend_from_slice(b"\r\n");
        head.content_type.write_header(&mut out);
    }

    if head.path_is_mcp && !head.is_options {
        out.extend_from_slice(b"Access-Control-Expose-Headers: Mcp-Session-Id\r\n");
    }
    if let Some(x_frame_options) = head.x_frame_options {
        out.extend_from_slice(format!("X-Frame-Options: {x_frame_options}\r\n").as_bytes());
    }
    if head.respect_do_not_track {
        if head.has_cookies || head.tracking_required {
            out.extend_from_slice(b"Tk: T;cookies\r\n");
        } else {
            out.extend_from_slice(b"Tk: N\r\n");
        }
    }

    if head.is_options {
        out.extend_from_slice(
            b"Access-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: accept, x-requested-with, origin, content-type, cookie, pragma, cache-control, x-auth-token, x-netdata-auth, x-transaction-id",
        );
        if head.path_is_mcp {
            out.extend_from_slice(
                b", authorization, mcp-protocol-version, mcp-session-id, last-event-id",
            );
        }
        out.extend_from_slice(b"\r\nAccess-Control-Max-Age: 1209600\r\n");
    } else {
        let cache = if no_cacheable {
            "no-cache, no-store, must-revalidate\r\nPragma: no-cache"
        } else {
            "public"
        };
        out.extend_from_slice(
            format!(
                "Cache-Control: {cache}\r\nExpires: {}\r\n",
                rfc7231_date(expires)
            )
            .as_bytes(),
        );
    }

    out.extend_from_slice(head.custom);

    if head.gzip {
        out.extend_from_slice(b"Content-Encoding: gzip\r\n");
    }
    if head.chunked {
        out.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
    } else if head.content_length > 0 {
        out.extend_from_slice(format!("Content-Length: {}\r\n", head.content_length).as_bytes());
    } else {
        // The length is unknown: the connection cannot be kept alive.
        keepalive = false;
    }

    out.extend_from_slice(b"X-Transaction-ID: ");
    print_uuid_lower_compact(&mut out, &head.transaction);
    out.extend_from_slice(b"\r\n\r\n");

    Built {
        bytes: out,
        keepalive,
        code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_that_do_not_fit_are_empty() {
        assert_eq!(rfc7231_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(
            rfc7231_date(253_402_300_799),
            "Fri, 31 Dec 9999 23:59:59 GMT"
        );
        assert_eq!(rfc7231_date(253_402_300_800), "");
    }

    fn head() -> Head<'static> {
        Head {
            code: 200,
            content_type: ContentType::ApplicationJson,
            date: 0,
            expires: 0,
            no_cacheable: false,
            keepalive: true,
            origin: None,
            version: "v2.11.0-458-g1e97a0fc9e",
            path_is_mcp: false,
            is_options: false,
            x_frame_options: None,
            has_cookies: false,
            respect_do_not_track: false,
            tracking_required: false,
            custom: b"",
            gzip: false,
            chunked: false,
            content_length: 5,
            server_host: None,
            url_as_received: b"/",
            transaction: [0xab; 16],
        }
    }

    #[test]
    fn rfc7231_dates() {
        assert_eq!(rfc7231_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(rfc7231_date(1_700_000_000), "Tue, 14 Nov 2023 22:13:20 GMT");
        assert_eq!(rfc7231_date(951_782_400), "Tue, 29 Feb 2000 00:00:00 GMT");
    }

    #[test]
    fn an_ok_response_header() {
        let built = build(&head(), 1_700_000_000);
        let want = "HTTP/1.1 200 OK\r\n\
            Connection: keep-alive\r\n\
            Server: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\n\
            Access-Control-Allow-Origin: *\r\n\
            Access-Control-Allow-Credentials: true\r\n\
            Date: Tue, 14 Nov 2023 22:13:20 GMT\r\n\
            Content-Type: application/json; charset=utf-8\r\n\
            Cache-Control: public\r\n\
            Expires: Wed, 15 Nov 2023 22:13:20 GMT\r\n\
            Content-Length: 5\r\n\
            X-Transaction-ID: abababababababababababababababab\r\n\r\n";
        assert_eq!(String::from_utf8(built.bytes).unwrap(), want);
        assert!(built.keepalive);
    }

    #[test]
    fn errors_are_not_cacheable_and_empty_bodies_close() {
        let built = build(
            &Head {
                code: 404,
                content_length: 0,
                ..head()
            },
            1_700_000_000,
        );
        let text = String::from_utf8(built.bytes).unwrap();
        assert!(text.contains(
            "Cache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\n"
        ));
        assert!(text.contains("Expires: Tue, 14 Nov 2023 22:13:20 GMT\r\n"));
        assert!(!text.contains("Content-Length"));
        assert!(!built.keepalive);
    }

    #[test]
    fn the_https_upgrade_header() {
        let built = build(
            &Head {
                code: 399,
                server_host: Some(b"h:19999"),
                url_as_received: b"/x?y",
                ..head()
            },
            1_700_000_000,
        );
        let text = String::from_utf8(built.bytes).unwrap();
        assert!(text.starts_with(
            "HTTP/1.1 399 Redirection\r\nLocation: https://h:19999/x?y\r\nCache-Control: no-cache"
        ));
        assert!(!text.contains("Server:") && !text.contains("Content-Type"));
        assert_eq!(built.code, 301);
    }

    #[test]
    fn options_and_gzip() {
        let built = build(
            &Head {
                is_options: true,
                path_is_mcp: true,
                ..head()
            },
            1_700_000_000,
        );
        let text = String::from_utf8(built.bytes).unwrap();
        assert!(text.contains("x-transaction-id, authorization, mcp-protocol-version, mcp-session-id, last-event-id\r\nAccess-Control-Max-Age: 1209600\r\n"));
        assert!(!text.contains("Cache-Control") && !text.contains("Expose-Headers"));

        let built = build(
            &Head {
                gzip: true,
                chunked: true,
                content_length: 0,
                ..head()
            },
            1_700_000_000,
        );
        let text = String::from_utf8(built.bytes).unwrap();
        assert!(text.contains("Content-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n"));
        assert!(built.keepalive);
    }
}
