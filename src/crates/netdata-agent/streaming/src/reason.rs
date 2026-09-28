//! `STREAM_HANDSHAKE` (`src/streaming/stream-handshake.h`): why a connection is or is not established, in both
//! directions. A number rather than an enum: a child's parse of the parent's VN answer yields any version, and the
//! host's status keeps the negotiated capabilities in the same field (D102.1).

/// A handshake result: 1 or more is a negotiated version or capabilities, 0 is "never connected", negative values
/// are the reasons of C's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reason(pub i32);

impl Reason {
    pub const OK_V3: Reason = Reason(3);
    pub const OK_V2: Reason = Reason(2);
    pub const OK_V1: Reason = Reason(1);
    pub const NEVER: Reason = Reason(0);
    pub const CONNECT_HANDSHAKE_FAILED: Reason = Reason(-1);
    pub const PARENT_IS_LOCALHOST: Reason = Reason(-2);
    pub const PARENT_NODE_ALREADY_CONNECTED: Reason = Reason(-3);
    pub const PARENT_DENIED_ACCESS: Reason = Reason(-4);
    pub const CONNECT_SEND_TIMEOUT: Reason = Reason(-5);
    pub const CONNECT_RECEIVE_TIMEOUT: Reason = Reason(-6);
    pub const CONNECT_INVALID_CERTIFICATE: Reason = Reason(-7);
    pub const CONNECT_SSL_ERROR: Reason = Reason(-8);
    pub const CONNECTION_FAILED: Reason = Reason(-9);
    pub const PARENT_BUSY_TRY_LATER: Reason = Reason(-10);
    pub const PARENT_INTERNAL_ERROR: Reason = Reason(-11);
    pub const PARENT_IS_INITIALIZING: Reason = Reason(-12);
    pub const RCV_DISCONNECT_PARSER_FAILED: Reason = Reason(-13);
    pub const RCV_DISCONNECT_STALE_RECEIVER: Reason = Reason(-14);
    pub const RCV_DECOMPRESSION_FAILED: Reason = Reason(-15);
    pub const SND_DISCONNECT_HOST_CLEANUP: Reason = Reason(-16);
    pub const SND_DISCONNECT_COMPRESSION_FAILED: Reason = Reason(-17);
    pub const SND_DISCONNECT_HTTP_UPGRADE_FAILED: Reason = Reason(-18);
    pub const SND_DISCONNECT_RECEIVER_LEFT: Reason = Reason(-19);
    pub const SND_VNODE_IS_STALE: Reason = Reason(-20);
    pub const DISCONNECT_SIGNALED_TO_STOP: Reason = Reason(-21);
    pub const DISCONNECT_SHUTDOWN: Reason = Reason(-22);
    pub const DISCONNECT_SOCKET_READ_FAILED: Reason = Reason(-23);
    pub const DISCONNECT_SOCKET_WRITE_FAILED: Reason = Reason(-24);
    pub const DISCONNECT_SOCKET_ERROR: Reason = Reason(-25);
    pub const DISCONNECT_TIMEOUT: Reason = Reason(-26);
    pub const DISCONNECT_SOCKET_CLOSED_BY_REMOTE: Reason = Reason(-27);
    pub const DISCONNECT_BUFFER_OVERFLOW: Reason = Reason(-28);
    pub const DISCONNECT_REPLICATION_STALLED: Reason = Reason(-29);
    pub const SP_PREPARING: Reason = Reason(-30);
    pub const SP_NO_HOST_IN_DESTINATION: Reason = Reason(-31);
    pub const SP_CONNECT_TIMEOUT: Reason = Reason(-32);
    pub const SP_CONNECTION_REFUSED: Reason = Reason(-33);
    pub const SP_CANT_RESOLVE_HOSTNAME: Reason = Reason(-34);
    pub const SP_CONNECTING: Reason = Reason(-35);
    pub const SP_CONNECTED: Reason = Reason(-36);
    pub const SP_NO_STREAM_INFO: Reason = Reason(-37);
    pub const SP_NO_DESTINATION: Reason = Reason(-38);
    pub const PARENT_VNODE_IS_LOCAL: Reason = Reason(-39);
    pub const RCV_DISCONNECT_LOCAL_VNODE_CLAIMED: Reason = Reason(-40);

    /// `stream_handshake_error_to_string()`.
    pub fn text(self) -> &'static str {
        if self.0 >= 1 {
            return "CONNECTED";
        }
        TABLE
            .iter()
            .find(|&&(r, ..)| r == self.0)
            .map_or("UNKNOWN", |&(_, text, _)| text)
    }

    /// `stream_handshake_error_to_response_code()`.
    pub fn code(self) -> i64 {
        if self.0 >= 1 {
            return 200;
        }
        TABLE
            .iter()
            .find(|&&(r, ..)| r == self.0)
            .map_or(404, |&(.., code)| code)
    }
}

/// `handshake_errors[]` below "CONNECTED".
const TABLE: [(i32, &str, i64); 41] = [
    (0, "NEVER CONNECTED", 204),
    (-1, "BAD HANDSHAKE", 400),
    (-2, "LOCALHOST", 101),
    (-3, "ALREADY CONNECTED", 409),
    (-4, "DENIED", 403),
    (-5, "SEND TIMEOUT", 408),
    (-6, "RECEIVE TIMEOUT", 504),
    (-7, "INVALID CERTIFICATE", 495),
    (-8, "SSL ERROR", 525),
    (-9, "CANT CONNECT", 502),
    (-10, "BUSY TRY LATER", 503),
    (-11, "INTERNAL ERROR", 500),
    (-12, "REMOTE IS INITIALIZING", 102),
    (-13, "DISCONNECTED PARSE ERROR", 400),
    (-14, "DISCONNECTED STALE RECEIVER", 410),
    (-15, "DISCONNECTED DECOMPRESSION FAILED", 415),
    (-40, "DISCONNECTED LOCAL VNODE CLAIMED", 409),
    (-16, "DISCONNECTED HOST CLEANUP", 202),
    (-17, "DISCONNECTED SND COMPRESSION FAILED", 415),
    (-18, "HTTP UPGRADE ERROR", 426),
    (-19, "RECEIVER LEFT", 498),
    (-20, "VNODE STALE", 498),
    (-21, "DISCONNECTED SIGNALED TO STOP", 499),
    (-22, "DISCONNECTED SHUTDOWN REQUESTED", 503),
    (-23, "DISCONNECTED SOCKET READ FAILED", 502),
    (-24, "DISCONNECTED SOCKET WRITE FAILED", 502),
    (-25, "DISCONNECT SOCKET ERROR", 500),
    (-26, "DISCONNECTED TIMEOUT", 504),
    (-27, "DISCONNECTED SOCKET CLOSED BY REMOTE END", 499),
    (-28, "DISCONNECTED NOT SUFFICIENT SEND BUFFER", 413),
    (-29, "REPLICATION STALLED", 507),
    (-30, "PREPARING", 102),
    (-31, "NO HOST IN DESTINATION - CONFIG ERROR", 404),
    (-32, "CONNECT TIMEOUT", 408),
    (-33, "CONNECTION REFUSED", 403),
    (-34, "CANT RESOLVE HOSTNAME", 400),
    (-35, "CONNECTING", 102),
    (-36, "SOCKET CONNECTED", 200),
    (-37, "NO STREAM INFO", 404),
    (-38, "NO PARENT TO SEND TO", 502),
    (-39, "LOCAL VNODE", 101),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_and_codes_as_c() {
        assert_eq!((Reason(469_565_432).text(), Reason(469_565_432).code()), ("CONNECTED", 200));
        assert_eq!((Reason::NEVER.text(), Reason::NEVER.code()), ("NEVER CONNECTED", 204));
        assert_eq!(
            (Reason::SP_NO_DESTINATION.text(), Reason::SP_NO_DESTINATION.code()),
            ("NO PARENT TO SEND TO", 502)
        );
        assert_eq!(
            (Reason::RCV_DISCONNECT_LOCAL_VNODE_CLAIMED.text(), Reason::RCV_DISCONNECT_LOCAL_VNODE_CLAIMED.code()),
            ("DISCONNECTED LOCAL VNODE CLAIMED", 409)
        );
        assert_eq!((Reason(-41).text(), Reason(-41).code()), ("UNKNOWN", 404));
        // every value from -40 to 0 is in the table once
        for r in -40..=0 {
            assert_eq!(TABLE.iter().filter(|&&(v, ..)| v == r).count(), 1, "{r}");
        }
    }
}
