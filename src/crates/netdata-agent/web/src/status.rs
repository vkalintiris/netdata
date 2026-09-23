//! HTTP status codes and reason phrases, ported from `src/libnetdata/http/http_defs.{h,c}`.

pub const OK: u16 = 200;
pub const MOVED_PERM: u16 = 301;
pub const BAD_REQUEST: u16 = 400;
pub const FORBIDDEN: u16 = 403;
pub const NOT_FOUND: u16 = 404;
pub const PRECOND_FAIL: u16 = 412;
pub const URI_TOO_LONG: u16 = 414;
pub const UNAVAILABLE_FOR_LEGAL_REASONS: u16 = 451;
pub const INTERNAL_SERVER_ERROR: u16 = 500;
pub const SERVICE_UNAVAILABLE: u16 = 503;
/// `HTTP_RESP_HTTPS_UPGRADE`: written on the status line as-is, then replaced by 301 internally.
pub const HTTPS_UPGRADE: u16 = 399;

/// `http_response_code2string()`.
// Specific codes come before the class ranges, as in the C switch and its default branch.
#[allow(clippy::match_overlapping_arm)]
pub fn reason(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        306 => "Switch Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        // nginx's extension to the standard
        499 => "Client Closed Request",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        100..=199 => "Informational",
        200..=299 => "Successful",
        300..=399 => "Redirection",
        400..=499 => "Client Error",
        500..=599 => "Server Error",
        _ => "Undefined Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons() {
        assert_eq!(reason(399), "Redirection");
        assert_eq!(reason(591), "Server Error");
        assert_eq!(reason(451), "Unavailable For Legal Reasons");
        assert_eq!(reason(700), "Undefined Error");
    }
}
