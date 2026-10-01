//! The receiver's log records: the status pair of `stream_receiver_log_status()` (one access and one daemon record),
//! the negotiated capabilities of `log_receiver_capabilities()`, and the disconnect record of
//! `stream_receiver_remove_internal()`, with the reasons of `stream-handshake.c`. Brief:
//! `knowledge/brief-logging-l3-l5.md` §2; the API key is masked (D34).

use std::sync::Arc;

use netdata_agent_log::{
    Field, FrameGuard, Priority, REDACTED, Source, Value, msgid, nd_log, push, push_shared,
};

use crate::caps;
use crate::reason::Reason;

/// What the receiver's records print of the connection: `rpt->remote_ip`, `rpt->remote_port` and the request's
/// hostname, key and machine GUID.
#[derive(Debug, Clone, Default)]
pub struct Peer {
    pub ip: String,
    pub port: String,
    pub hostname: Option<String>,
    pub key: Option<String>,
    pub machine_guid: Option<String>,
}

impl Peer {
    /// The hostname as the texts print it: `-` when there is none.
    pub fn hostname_or_dash(&self) -> &str {
        self.hostname
            .as_deref()
            .filter(|h| !h.is_empty())
            .unwrap_or("-")
    }

    /// `stream_receiver_log_status()`: an access record of the request's identity and a daemon record of the
    /// message, both at `priority`, under a frame of the peer, the host name, the reason's code and the
    /// "streaming from child" message id.
    pub fn status(&self, msg: &str, reason: Reason, priority: Priority) {
        self.status_errno(msg, reason, priority, 0);
    }

    /// [`Peer::status`] after a failed call: the access record carries its errno (C's logger clears errno after it,
    /// so the daemon record carries none).
    pub fn status_errno(&self, msg: &str, reason: Reason, priority: Priority, errno: i32) {
        let _frame = push(vec![
            (Field::SrcIp, Value::txt(self.ip.as_str())),
            (Field::SrcPort, Value::txt(self.port.as_str())),
            (
                Field::NidlNode,
                Value::txt(self.hostname.clone().unwrap_or_default()),
            ),
            (Field::ResponseCode, Value::I64(reason.code())),
            (Field::MessageId, Value::Uuid(msgid::STREAMING_FROM_CHILD)),
        ]);
        let key = match self.key.as_deref() {
            Some(key) if !key.is_empty() => REDACTED,
            _ => "",
        };
        nd_log!(
            Source::Access,
            priority,
            errno = errno;
            "api_key:'{key}' machine_guid:'{}' node:'{}' msg:'{msg}' reason:'{}'",
            self.machine_guid.as_deref().unwrap_or(""),
            self.hostname.as_deref().unwrap_or(""),
            reason.text()
        );
        // "<msg>  (<REASON>)" with two spaces, or "<msg> NEVER CONNECTED"
        let (open, close) = if reason == Reason::NEVER {
            ("", "")
        } else {
            (" (", ")")
        };
        nd_log!(
            Source::Daemon,
            priority,
            "STREAM RCV '{}' [from [{}]:{}]: {msg} {open}{}{close}",
            self.hostname.as_deref().unwrap_or(""),
            self.ip,
            self.port,
            reason.text()
        );
    }

    /// `log_receiver_capabilities()`.
    pub fn established(&self, host: &str, capabilities: u32) {
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "STREAM RCV '{host}' [from [{}]:{}]: established link with negotiated capabilities: {}",
            self.ip,
            self.port,
            caps::to_string(capabilities)
        );
    }

    /// The frame `stream_receive_process_poll_events()` pushes for every event of a child's socket: built once per
    /// child and shared.
    pub fn child_frame(&self, capabilities: u32, tls: bool) -> Arc<[(Field, Value)]> {
        Arc::from(vec![
            (Field::SrcIp, Value::txt(self.ip.as_str())),
            (Field::SrcPort, Value::txt(self.port.as_str())),
            (
                Field::NidlNode,
                Value::txt(self.hostname.clone().unwrap_or_default()),
            ),
            // stream_receiver_log_transport()
            (Field::SrcTransport, Value::txt(if tls { "https" } else { "http" })),
            // C's callback prints even no capabilities, as an empty string
            (
                Field::SrcCapabilities,
                Value::Str(caps::to_string(capabilities)),
            ),
        ])
    }
}

/// `receiver_state` counters the disconnect record prints.
#[derive(Debug, Clone, Copy)]
pub struct Counters {
    /// `sth->id`.
    pub thread: usize,
    /// `parser->user.data_collections_count`.
    pub msgs: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// Wall-clock seconds since the request was accepted.
    pub connected_s: i64,
    /// Seconds since the last read or write.
    pub idle_s: i64,
    /// `host->stream.rcv.status.replication.percent`.
    pub replication_percent: f64,
}

/// `stream_receiver_remove()`'s log stack: the child's fields, then the host's name and the "streaming from child"
/// message id. The removal's record and everything the removal logs after it (the parser's cleanup) are under it.
pub struct Removal {
    // dropped first: the frames pop in the reverse of their pushes
    _frame: FrameGuard,
    _child: FrameGuard,
}

pub fn removal(child: &Arc<[(Field, Value)]>, host: &str) -> Removal {
    let child = push_shared(Arc::clone(child));
    let frame = push(vec![
        (Field::NidlNode, Value::Str(host.to_string())),
        (Field::MessageId, Value::Uuid(msgid::STREAMING_FROM_CHILD)),
    ]);
    Removal { _frame: frame, _child: child }
}

/// `stream_receiver_remove_internal()`'s record: daemon, error, under [`removal`]'s fields.
pub fn disconnected(peer: &Peer, iface: Option<&str>, reason: Reason, c: &Counters) {
    nd_log!(
        Source::Daemon,
        Priority::Err,
        "STREAM RCV[{}] '{}' [from [{}]:{}]: receiver disconnected: reason=\"{}\" msgs={} bytes_in={} bytes_out={} \
         connected={}s idle={}s repl={:.0}% iface={}",
        c.thread,
        peer.hostname_or_dash(),
        dash(&peer.ip),
        dash(&peer.port),
        reason.text(),
        c.msgs,
        c.bytes_in,
        c.bytes_out,
        c.connected_s,
        c.idle_s,
        c.replication_percent,
        iface.filter(|i| !i.is_empty()).map_or("-", |i| cut(i, 63))
    );
}

/// `strncpyz()` into a C buffer of `max` bytes.
pub(crate) fn cut(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

/// Pushes a child's shared frame for the records of one event.
pub fn child_event(child: &Arc<[(Field, Value)]>) -> FrameGuard {
    push_shared(Arc::clone(child))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> Peer {
        Peer {
            ip: "localhost".into(),
            port: "50390".into(),
            hostname: Some("child".into()),
            key: Some("11111111-2222-3333-4444-555555555555".into()),
            machine_guid: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
        }
    }

    /// `stream_receiver_remove_internal()`'s record: the host's name in the frame, the peer's in the text, C's
    /// dashes and the interface cut to C's 64-byte buffer.
    #[test]
    fn the_disconnect_record_is_c_s() {
        let child: Arc<[(Field, Value)]> = Arc::from(vec![]);
        let counters = Counters {
            thread: 2,
            msgs: 604,
            bytes_in: 37139,
            bytes_out: 3126,
            connected_s: 36,
            idle_s: 0,
            replication_percent: 37.4,
        };
        let long = "i".repeat(70);
        let ((), records) = netdata_agent_log::capture(|| {
            let _removal = removal(&child, "host");
            disconnected(&peer(), Some("lo"), Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE, &counters);
            disconnected(&Peer::default(), Some(&long), Reason::DISCONNECT_SOCKET_ERROR, &counters);
        });
        let texts: Vec<_> = records
            .iter()
            .map(|r| (r.priority, r.message.clone().unwrap()))
            .collect();
        assert_eq!(
            texts,
            [
                (
                    Priority::Err,
                    "STREAM RCV[2] 'child' [from [localhost]:50390]: receiver disconnected: reason=\"DISCONNECTED \
                     SOCKET CLOSED BY REMOTE END\" msgs=604 bytes_in=37139 bytes_out=3126 connected=36s idle=0s \
                     repl=37% iface=lo"
                        .to_string()
                ),
                (
                    Priority::Err,
                    format!(
                        "STREAM RCV[2] '-' [from [-]:-]: receiver disconnected: reason=\"DISCONNECT SOCKET ERROR\" \
                         msgs=604 bytes_in=37139 bytes_out=3126 connected=36s idle=0s repl=37% iface={}",
                        "i".repeat(63)
                    )
                ),
            ]
        );
    }

    #[test]
    fn the_status_pair_masks_the_key_and_spaces_the_reason_like_c() {
        let ((), records) = netdata_agent_log::capture(|| {
            peer().status(
                "rejecting streaming connection; API key is not enabled in stream.conf",
                Reason::PARENT_DENIED_ACCESS,
                Priority::Warning,
            );
            peer().status(
                "connected and ready to receive data, new node",
                Reason::NEVER,
                Priority::Info,
            );
        });
        let texts: Vec<_> = records
            .iter()
            .map(|r| (r.source, r.priority, r.message.clone().unwrap()))
            .collect();
        assert_eq!(
            texts,
            [
                (
                    Source::Access,
                    Priority::Warning,
                    "api_key:'[REDACTED]' machine_guid:'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee' node:'child' msg:'rejecting \
                     streaming connection; API key is not enabled in stream.conf' reason:'DENIED'"
                        .to_string()
                ),
                (
                    Source::Daemon,
                    Priority::Warning,
                    "STREAM RCV 'child' [from [localhost]:50390]: rejecting streaming connection; API key is not \
                     enabled in stream.conf  (DENIED)"
                        .to_string()
                ),
                (
                    Source::Access,
                    Priority::Info,
                    "api_key:'[REDACTED]' machine_guid:'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee' node:'child' \
                     msg:'connected and ready to receive data, new node' reason:'NEVER CONNECTED'"
                        .to_string()
                ),
                (
                    Source::Daemon,
                    Priority::Info,
                    "STREAM RCV 'child' [from [localhost]:50390]: connected and ready to receive data, new node NEVER \
                     CONNECTED"
                        .to_string()
                ),
            ]
        );
    }
}
