//! A sender's connection on its stream thread (`src/streaming/stream-sender.c` and the sender half of
//! `stream-thread.c`): taken from the queue at the next tick, then its poll events, sends, receives, opcodes, the
//! periodic idle check, the disconnect that hands it back to the connector, and the thread's exit. Map:
//! `knowledge/map-m7-commit4-runtime.md` §3, §4, §7.

use std::io;
use std::os::fd::AsRawFd;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use netdata_agent_evloop::conn::Conn;
use netdata_agent_evloop::{Context, Event, Interest, Token};
use netdata_agent_log::{
    ErrorLimit, Field, FrameGuard, Priority, Source, Value, errno_of, msgid, nd_log, nd_log_limit, push,
};
use netdata_agent_rrd::chart::flags;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::{Host, sender_flags};
use netdata_agent_rrd::pulse::host_status;
use netdata_agent_text::duration::duration_to_string;
use netdata_agent_text::size::size_to_string;
use netdata_agent_tls::Link;

use super::execute::Executor;
use super::{Connected, Sender, Session, op};
use crate::caps;
use crate::connector::Cmd;
use crate::random::os_random32;
use crate::reason::Reason;
use crate::receiver::{now_monotonic_ut, replication_progressed};
use crate::thread::StreamWorker;

/// The tokens of senders' sockets: above every receiver's.
pub(crate) const SENDER_TOKENS: usize = 1 << (usize::BITS - 2);
/// `PLUGINSD_LINE_MAX + 1`: the receive buffer.
const READ_BUFFER: usize = netdata_agent_pluginsd_proto::LINE_MAX + 1;
/// `sock_enlarge_rcv_buf()` and `sock_enlarge_snd_buf()`.
const LARGE_SOCK_SIZE: usize = 32 * 1024 * 1024;

/// `EVLOOP_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Continue,
    NoMoreData,
    Full,
    Closed,
    Error,
}

/// A dispatched sender: `s->sock` and `s->thread` while it runs on this thread.
pub(crate) struct Dispatched {
    pub sender: Arc<Sender>,
    pub host: Arc<Host>,
    link: Link<Conn>,
    pub session: Session,
    pub(crate) rbuf: Vec<u8>,
    pub(crate) read_len: usize,
    last_traffic_ut: u64,
    pub remote_ip: String,
    pub capabilities: u32,
    /// The parent's address as `socket_peers()` gives it, for DST_IP and DST_PORT.
    peer: Option<(String, u16)>,
    /// The link is over TLS: DST_TRANSPORT `https` until its close.
    tls: bool,
    pub executor: Executor,
    /// `s->replication.last_counter_sum`, `last_progress_ut` and `last_checked_ut`: the stall check's state.
    replication_commands: u64,
    replication_progress: Option<Instant>,
    replication_checked: Option<Instant>,
}

impl Dispatched {
    fn fd(&self) -> i32 {
        self.link.socket().map_or(-1, AsRawFd::as_raw_fd)
    }

    /// The sender's frame (`stream_sender_log_*()` callbacks): the host, the parent while the socket is open, the
    /// transport and the capabilities.
    pub(crate) fn frame(&self) -> FrameGuard {
        sender_frame(&self.host.hostname(), self.peer.as_ref(), self.capabilities, self.tls)
    }
}

/// With no peer (the socket closed) the DST_IP and DST_PORT callbacks return false, and C's logfmt keeps their
/// separators; DST_TRANSPORT is `https` while a TLS link is open (`nd_sock_is_ssl()`).
fn sender_frame(hostname: &str, peer: Option<&(String, u16)>, capabilities: u32, tls: bool) -> FrameGuard {
    let mut fields = vec![(Field::NidlNode, Value::Str(hostname.to_string()))];
    match peer {
        Some((ip, port)) => {
            fields.push((Field::DstIp, Value::Str(ip.clone())));
            fields.push((Field::DstPort, Value::Str(port.to_string())));
        }
        None => {
            fields.push((Field::DstIp, Value::lazy(|_| false)));
            fields.push((Field::DstPort, Value::lazy(|_| false)));
        }
    }
    fields.push((Field::DstTransport, Value::txt(if tls { "https" } else { "http" })));
    fields.push((Field::DstCapabilities, Value::Str(caps::to_string(capabilities))));
    push(fields)
}

/// `socket_peers()` of an inet socket.
fn peer_of(socket: &socket2::SockRef<'_>) -> Option<(String, u16)> {
    let addr = socket.peer_addr().ok()?.as_socket()?;
    Some((addr.ip().to_string(), addr.port()))
}

impl StreamWorker {
    fn sender_token(index: usize) -> Token {
        Token(SENDER_TOKENS + index)
    }

    /// `stream_sender_move_queue_to_running_unsafe()`: every sender queued since the last tick starts running here.
    pub(crate) fn dequeue_senders(&mut self, cx: &mut Context<'_>) {
        for connected in std::mem::take(&mut self.queued_senders) {
            self.start_sender(cx, connected);
        }
    }

    fn start_sender(&mut self, cx: &mut Context<'_>, connected: Connected) {
        let Connected { sender, mut link, capabilities, compressor, remote_ip, thread } = connected;
        let Some(host) = sender.host() else {
            // freed between its connect and this dispatch: the pin goes back (review R43 N4), and the socket goes
            // non-blocking first so a TLS link's close does not wait on the parent (R46 n5)
            if let Some(conn) = link.socket() {
                let _ = socket2::SockRef::from(conn).set_nonblocking(true);
            }
            self.pins.lock().unwrap_or_else(PoisonError::into_inner).remove(&sender.machine_guid);
            return;
        };
        let hostname = host.hostname();
        let tls = link.is_tls();
        let peer = link.socket().and_then(|c| peer_of(&socket2::SockRef::from(c)));
        let _frame = sender_frame(&hostname, peer.as_ref(), capabilities, tls);
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "STREAM SND[{thread}] '{hostname}' [to {remote_ip}]: moving host from dispatcher queue to dispatcher running..."
        );
        // the connector's blocking socket, a TLS connection's included, goes non-blocking under it
        if let Some(conn) = link.socket() {
            let socket = socket2::SockRef::from(conn);
            let fd = conn.as_raw_fd();
            if socket.set_nonblocking(true).is_err() {
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "STREAM SND[{thread}] '{hostname}' [to {remote_ip}]: failed to set non-blocking mode on socket {fd}"
                );
            }
            // socket2 opened it close-on-exec; sock_enlarge_rcv_buf() and sock_enlarge_snd_buf(), errors ignored
            if socket.recv_buffer_size().is_ok_and(|size| size < LARGE_SOCK_SIZE) {
                let _ = socket.set_recv_buffer_size(LARGE_SOCK_SIZE);
            }
            if socket.send_buffer_size().is_ok_and(|size| size < LARGE_SOCK_SIZE) {
                let _ = socket.set_send_buffer_size(LARGE_SOCK_SIZE);
            }
        }
        let session = Session { thread, id: loop {
            // a zero session means "no dispatcher"
            let id = os_random32();
            if id != 0 {
                break id;
            }
        } };
        {
            let mut out = sender.out();
            out.session = Some(session);
            out.remote_ip = remote_ip.clone();
            out.capabilities = capabilities;
            sender.negotiated.store(capabilities, Ordering::Relaxed);
            out.algorithm = compressor.as_ref().map(|c| c.algorithm());
            out.compressor = compressor;
            sender.flush_buffer(&mut out);
            sender.connector.replication().recalculate(sender.replication(), out.buffer.used_percent());
        }
        sender.status_connected();
        let index = self.senders.iter().position(Option::is_none).unwrap_or_else(|| {
            self.senders.push(None);
            self.senders.len() - 1
        });
        if cx
            .registry()
            .register(&mut link, Self::sender_token(index), Interest::READABLE | Interest::WRITABLE)
            .is_err()
        {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND[{thread}] '{hostname}' [to {remote_ip}]: failed to add sender socket to nd_poll()"
            );
        }
        self.senders[index] = Some(Dispatched {
            sender: Arc::clone(&sender),
            host: Arc::clone(&host),
            link,
            session,
            rbuf: vec![0; READ_BUFFER],
            read_len: 0,
            last_traffic_ut: now_monotonic_ut(),
            remote_ip,
            capabilities,
            peer,
            tls,
            executor: Executor::default(),
            replication_commands: 0,
            // set at the dequeue, as C's
            replication_progress: Some(Instant::now()),
            replication_checked: None,
        });
        sender.on_ready_to_dispatch(&host, capabilities);
        self.drain_inline(cx);
        host.pulse_status(host_status::SND_RUNNING);
    }

    /// `stream_sender_process_poll_events()`: errors and hangups first (C checks EPOLLRDHUP before reading), then
    /// reading, then sending.
    pub(crate) fn sender_event(&mut self, cx: &mut Context<'_>, index: usize, event: &Event) {
        let Some(d) = self.senders.get(index).and_then(Option::as_ref) else {
            return;
        };
        let _frame = d.frame();
        if event.is_error() || event.is_read_closed() {
            let error = if event.is_error() {
                "socket reports errors"
            } else {
                "connection closed by remote end (HUP)"
            };
            let stats = *d.sender.out().buffer.stats();
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND[{}] '{}' [to {}]: {error} restarting connection - {} bytes transmitted in {} operations.",
                cx.index(),
                d.host.hostname(),
                d.remote_ip,
                stats.bytes_sent,
                stats.sends
            );
            return self.disconnect_sender(cx, index, Reason::DISCONNECT_SOCKET_ERROR, Reason::NEVER, true);
        }
        if event.is_readable() && !self.receive_sender(cx, index) {
            return;
        }
        if event.is_writable() {
            self.send_sender(cx, index, true);
        }
    }

    /// `stream_sender_send_data()`: sends until the buffer drains or the socket fills. A failure is removed only when
    /// `remove` (the poll's path); otherwise it is only logged and a later poll error removes the sender. False when
    /// the sender is gone or failed.
    pub(crate) fn send_sender(&mut self, cx: &mut Context<'_>, index: usize, remove: bool) -> bool {
        loop {
            let Some(d) = self.senders.get_mut(index).and_then(Option::as_mut) else {
                return false;
            };
            let now_ut = now_monotonic_ut();
            let mut out = d.sender.out();
            let chunk = out.buffer.next();
            if chunk.is_empty() {
                return true;
            }
            let (status, rc, errno) = match d.link.write(chunk) {
                Ok(0) => (Status::Closed, 0, 0),
                Ok(n) => {
                    d.host.storage().pulse().network.stream_sent(n);
                    out.buffer.del(n, now_ut);
                    d.sender.connector.replication().recalculate(d.sender.replication(), out.buffer.used_percent());
                    d.last_traffic_ut = now_ut;
                    if out.buffer.stats().bytes_outstanding == 0 {
                        out.buffer.recreate_timed(now_ut, false);
                        (Status::NoMoreData, n as isize, 0)
                    } else {
                        (Status::Continue, n as isize, 0)
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => (Status::Closed, -1, errno_of(&e)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (Status::Full, -1, errno_of(&e)),
                Err(e) => (Status::Error, -1, errno_of(&e)),
            };
            let stats = *out.buffer.stats();
            drop(out);
            match status {
                Status::Continue => continue,
                Status::NoMoreData | Status::Full => return true,
                Status::Closed | Status::Error => {
                    let (text, reason) = if status == Status::Error {
                        ("socket reports error while writing", Reason::DISCONNECT_SOCKET_WRITE_FAILED)
                    } else {
                        ("socket reports EOF (closed by parent)", Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE)
                    };
                    // under the caller's frame: the poll's, or none on the opcode path (C's)
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        errno = errno;
                        "STREAM SND[{}] '{}' [to {}]: {text} ({rc}, on fd {}) - restarting connection - we have sent {} \
                         bytes in {} operations.",
                        cx.index(),
                        d.host.hostname(),
                        d.remote_ip,
                        d.fd(),
                        stats.bytes_sent,
                        stats.sends
                    );
                    if remove {
                        self.disconnect_sender(cx, index, reason, Reason::NEVER, true);
                    }
                    return false;
                }
            }
        }
    }

    /// `stream_sender_receive_data()`: reads and executes until the socket is drained. False when the sender is gone.
    fn receive_sender(&mut self, cx: &mut Context<'_>, index: usize) -> bool {
        loop {
            let Some(d) = self.senders.get_mut(index).and_then(Option::as_mut) else {
                return false;
            };
            let (mut status, errno) = if d.read_len >= d.rbuf.len() - 1 {
                // the line to read is too big
                (Status::Error, 0)
            } else {
                let room = d.rbuf.len() - 1;
                match d.link.read(&mut d.rbuf[d.read_len..room]) {
                    Ok(0) => (Status::Closed, 0),
                    Ok(n) => {
                        d.read_len += n;
                        d.last_traffic_ut = now_monotonic_ut();
                        d.host.storage().pulse().network.stream_received(n);
                        (Status::Continue, 0)
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => (Status::Closed, errno_of(&e)),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => (Status::Full, 0),
                    Err(e) => (Status::Error, errno_of(&e)),
                }
            };
            if status == Status::Continue && !self.execute(cx, index) {
                status = Status::Error;
            }
            match status {
                Status::Continue => {
                    // the commands may have removed it
                    if self.senders.get(index).and_then(Option::as_ref).is_none() {
                        return false;
                    }
                }
                Status::Full | Status::NoMoreData => return true,
                Status::Closed | Status::Error => {
                    let Some(d) = self.senders.get(index).and_then(Option::as_ref) else {
                        return false;
                    };
                    let (text, reason) = if status == Status::Error {
                        ("error during receive", Reason::DISCONNECT_SOCKET_READ_FAILED)
                    } else {
                        ("socket reports EOF (closed by parent)", Reason::DISCONNECT_SOCKET_CLOSED_BY_REMOTE)
                    };
                    {
                        let _frame = d.frame();
                        nd_log!(
                            Source::Daemon,
                            Priority::Err,
                            errno = errno;
                            "STREAM SND[{}] '{}' [to {}]: {text} (fd {}) - restarting sender connection.",
                            cx.index(),
                            d.host.hostname(),
                            d.remote_ip,
                            d.fd()
                        );
                    }
                    self.disconnect_sender(cx, index, reason, Reason::NEVER, true);
                    return false;
                }
            }
        }
    }

    /// Runs the executor over the received bytes; false on a fatal command error.
    fn execute(&mut self, cx: &mut Context<'_>, index: usize) -> bool {
        let Some(d) = self.senders.get_mut(index).and_then(Option::as_mut) else {
            return false;
        };
        let ok = super::execute::execute(d);
        let host = Arc::clone(&d.host);
        self.drain_inline(cx);
        // what the commands owe the host's child (NODE_ID, the path) goes out now, as C's send_to_child
        self.deliver_to_child(cx, &host);
        ok
    }

    /// `stream_thread_handle_op()` for a sender: the opcodes of its current session only (D103.5); POLLOUT sends
    /// without removing, the rest disconnect.
    pub(crate) fn sender_ops(&mut self, cx: &mut Context<'_>, sender: &Arc<Sender>, session: Session) {
        let Some(ops) = sender.take_ops(session) else {
            return;
        };
        self.handle_ops(cx, sender, ops.session, ops.bits, ops.reason);
    }

    fn handle_ops(&mut self, cx: &mut Context<'_>, sender: &Arc<Sender>, session: Session, bits: u32, reason: Reason) {
        let found = self.senders.iter().position(|d| {
            d.as_ref().is_some_and(|d| Arc::ptr_eq(&d.sender, sender) && d.session == session)
        });
        let Some(index) = found else {
            opcode_ignored(cx.index(), bits);
            return;
        };
        let mut bits = bits;
        if bits & op::POLLOUT != 0 {
            if !self.send_sender(cx, index, false) {
                return;
            }
            bits &= !op::POLLOUT;
        }
        if bits != 0 {
            self.sender_handle_op(cx, index, bits, reason);
        }
    }

    /// `stream_sender_handle_op()`.
    fn sender_handle_op(&mut self, cx: &mut Context<'_>, index: usize, bits: u32, reason: Reason) {
        let Some(d) = self.senders.get(index).and_then(Option::as_ref) else {
            return;
        };
        let _frame = d.frame();
        let (thread, hostname, remote) = (cx.index(), d.host.hostname(), d.remote_ip.clone());
        if bits & op::BUFFER_OVERFLOW != 0 {
            let stats = *d.sender.out().buffer.stats();
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND[{thread}] '{hostname}' [to {remote}]: send buffer is full (buffer size {}, max {}, used {}, \
                 available {}). Restarting connection.",
                stats.bytes_size,
                stats.bytes_max_size,
                stats.bytes_outstanding,
                stats.bytes_available
            );
            return self.disconnect_sender(cx, index, Reason::DISCONNECT_BUFFER_OVERFLOW, Reason::NEVER, true);
        }
        if bits & op::STOP_RECEIVER_LEFT != 0 {
            return self.disconnect_sender(cx, index, Reason::SND_DISCONNECT_RECEIVER_LEFT, reason, false);
        }
        if bits & op::RECONNECT_WITHOUT_COMPRESSION != 0 {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND[{thread}] '{hostname}' [to {remote}]: restarting connection without compression."
            );
            return self.disconnect_sender(cx, index, Reason::SND_DISCONNECT_COMPRESSION_FAILED, Reason::NEVER, true);
        }
        if bits & op::STOP_HOST_CLEANUP != 0 {
            return self.disconnect_sender(cx, index, Reason::SND_DISCONNECT_HOST_CLEANUP, Reason::NEVER, false);
        }
        nd_log!(Source::Daemon, Priority::Err, "STREAM SND[{thread}]: invalid msg id {bits}");
    }

    /// The POLLOUTs posted on this thread while it ran a sender's hooks or commands (C handles them inline).
    pub(crate) fn drain_inline(&mut self, cx: &mut Context<'_>) {
        for (sender, session) in crate::thread::take_inline() {
            if let Some(sender) = sender.upgrade() {
                self.handle_ops(cx, &sender, session, op::POLLOUT, Reason::NEVER);
            }
        }
    }

    /// `stream_sender_check_all_nodes_from_poll()`: the idle timeout, and a send for anything outstanding (C's poll
    /// mask repair; with edge-triggered events it covers a missed edge).
    pub(crate) fn check_senders(&mut self, cx: &mut Context<'_>) {
        let now_ut = now_monotonic_ut();
        for index in 0..self.senders.len() {
            let Some(d) = self.senders[index].as_mut() else {
                continue;
            };
            let stats = *d.sender.out().buffer.stats();
            if d.last_traffic_ut == 0 || now_ut < d.last_traffic_ut {
                d.last_traffic_ut = now_ut;
            }
            let idle_ut = now_ut - d.last_traffic_ut;
            let timeout_s = d.sender.connector.settings.timeout_s;
            let idle = stats.bytes_outstanding != 0
                && idle_ut > (timeout_s as u64).wrapping_mul(1_000_000)
                && !d.sender.replication().busy();
            if idle {
                {
                    let _frame = d.frame();
                    let duration =
                        duration_to_string(idle_ut.min(i64::MAX as u64) as i64, "us", true).unwrap_or_default();
                    let pending = size_to_string(stats.bytes_outstanding as u64, "B", false).unwrap_or_default();
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "STREAM SND[{}] '{}' [to {}]: there was not traffic for {timeout_s} seconds - closing connection \
                         - we have sent {} bytes in {} operations, it is idle for {duration}, and we have {pending} \
                         pending to send (buffer is used {:.2}%).",
                        cx.index(),
                        d.host.hostname(),
                        d.remote_ip,
                        stats.bytes_sent,
                        stats.sends,
                        stats.buffer_ratio
                    );
                }
                self.disconnect_sender(cx, index, Reason::DISCONNECT_TIMEOUT, Reason::NEVER, true);
            } else if stats.bytes_outstanding != 0 {
                // an edge the send missed: C's level-triggered poll sends it under the poll's frame
                let _frame = d.frame();
                self.send_sender(cx, index, true);
            }
        }
    }

    /// `stream_sender_did_replication_progress()` for a dispatched sender: the commands received and answered.
    fn sender_replication_progressed(d: &mut Dispatched, now: Instant) -> bool {
        let commands = u64::from(d.sender.counter_in.load(Ordering::Relaxed))
            + u64::from(d.sender.counter_out.load(Ordering::Relaxed));
        let waiting = d.sender.replication().queued();
        replication_progressed((&mut d.replication_commands, &mut d.replication_progress), commands, waiting, now)
    }

    /// `stream_sender_replication_check_from_poll()`: a sender whose replication made no progress for ten minutes
    /// while some of its host's charts never finished is disconnected, after its unfinished charts are listed, and
    /// connects again.
    pub(crate) fn check_sender_replication(&mut self, cx: &mut Context<'_>, now: Instant) {
        for index in 0..self.senders.len() {
            let Some(d) = self.senders[index].as_mut() else {
                continue;
            };
            if Self::sender_replication_progressed(d, now) {
                d.replication_checked = None;
                continue;
            }
            if d.replication_checked == d.replication_progress {
                continue;
            }
            let frame = d.frame();
            let at = format!("STREAM SND[{}] '{}' [to {}]: ", cx.index(), d.host.hostname(), d.remote_ip);
            let (mut stalled, mut finished) = (0usize, 0usize);
            for chart in d.host.charts().all() {
                let f = chart.flags();
                if f & (flags::OBSOLETE | flags::UPSTREAM_IGNORE) != 0 {
                    continue;
                }
                if f & flags::SENDER_REPLICATION_FINISHED != 0 {
                    finished += 1;
                    continue;
                }
                let state = if f & flags::SENDER_REPLICATION_IN_PROGRESS != 0 {
                    "has not finished"
                } else {
                    "has not started"
                };
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "{at}REPLICATION STALLED: instance '{}' {state} replication yet.",
                    chart.id()
                );
                stalled += 1;
            }
            if stalled > 0 && !Self::sender_replication_progressed(d, now) {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "{at}REPLICATION EXCEPTIONS SUMMARY: node has {stalled} stalled replication requests ({finished} \
                     completed).We have received {} and sent {} replication commands. Disconnecting node to restore \
                     streaming.",
                    d.sender.counter_in.load(Ordering::Relaxed),
                    d.sender.counter_out.load(Ordering::Relaxed)
                );
                // the disconnect's record carries its own frame, as the idle check's
                drop(frame);
                self.disconnect_sender(cx, index, Reason::DISCONNECT_REPLICATION_STALLED, Reason::NEVER, true);
                continue;
            }
            d.replication_checked = d.replication_progress;
            drop(frame);
        }
    }

    /// `stream_sender_move_running_to_connector_or_remove()`: off the poll and the thread, the disconnect record
    /// (the socket still open for its fields), and back to the connector to connect again or to be removed.
    pub(crate) fn disconnect_sender(
        &mut self,
        cx: &mut Context<'_>,
        index: usize,
        reason: Reason,
        receiver_reason: Reason,
        reconnect: bool,
    ) {
        let Some(mut d) = self.senders.get_mut(index).and_then(Option::take) else {
            return;
        };
        let _frame = d.frame();
        let thread = cx.index();
        let hostname = d.host.hostname();
        if cx.registry().deregister(&mut d.link).is_err() {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND[{thread}] '{hostname}' [to {}]: failed to delete sender socket from nd_poll()",
                d.remote_ip
            );
        }
        d.host.sender_flags_clear(sender_flags::CONNECTED | sender_flags::READY_4_METRICS);
        let reason = {
            let mut state = d.sender.lock();
            let reason = if reason == Reason::DISCONNECT_SIGNALED_TO_STOP && state.exit_reason != Reason::NEVER {
                state.exit_reason
            } else {
                reason
            };
            state.exit_reason = reason;
            reason
        };
        d.sender.out().session = None;
        {
            let _id = push(vec![(Field::MessageId, Value::Uuid(msgid::STREAMING_TO_PARENT))]);
            if reason == Reason::SND_DISCONNECT_RECEIVER_LEFT && receiver_reason != Reason::NEVER {
                nd_log!(
                    Source::Daemon,
                    Priority::Notice,
                    "STREAM SND[{thread}] '{hostname}' [to {}]: sender disconnected from parent, reason: {} (receiver \
                     left due to: {})",
                    d.remote_ip,
                    reason.text(),
                    receiver_reason.text()
                );
            } else {
                nd_log!(
                    Source::Daemon,
                    Priority::Notice,
                    "STREAM SND[{thread}] '{hostname}' [to {}]: sender disconnected from parent, reason: {}",
                    d.remote_ip,
                    reason.text()
                );
            }
        }
        drop(d.link);
        // a TLS close leaves what its shutdown set (EAGAIN on the non-blocking socket), which C's next record carries
        let mut errno = if d.tls { nix::errno::Errno::last_raw() } else { 0 };
        // the socket is closed: the rest of the records have no parent address, and no TLS
        drop(_frame);
        let _frame = sender_frame(&hostname, None, d.capabilities, false);
        d.sender.set_disconnect_reason(reason, now_realtime_s());
        // stream_sender_clear_parent_claim_id()
        if d.host.update_claim_id_of_parent([0; 16]).is_some_and(|previous| previous != [0; 16]) {
            nd_log!(Source::Daemon, Priority::Info, errno = errno;
                "Host '{hostname}' [PCLAIMID] cleared parent's claim id");
            errno = 0;
        }
        d.host.pulse_status(host_status::SND_OFFLINE);
        self.pins.lock().unwrap_or_else(PoisonError::into_inner).remove(d.host.machine_guid());
        let cmd = if reconnect && !d.sender.shutdown.load(Ordering::Relaxed) { Cmd::Connect } else { Cmd::Remove };
        self.exit_errno = d.sender.connector.requeue_after_close(&d.sender, &d.host, cmd, errno);
    }

    /// `stream_sender_cleanup()` at the thread's exit, after the queued senders started (their hooks run): every
    /// sender goes back to the connector to be removed.
    pub(crate) fn stop_senders(&mut self, cx: &mut Context<'_>) {
        self.dequeue_senders(cx);
        for index in 0..self.senders.len() {
            let Some(d) = self.senders[index].as_ref() else {
                continue;
            };
            d.sender.lock().exit_reason = Reason::DISCONNECT_SHUTDOWN;
            d.sender.shutdown.store(true, Ordering::Relaxed);
            self.disconnect_sender(cx, index, Reason::DISCONNECT_SHUTDOWN, Reason::NEVER, false);
        }
    }
}

/// "STREAM THREAD[%zu]: OPCODE %u ignored.", once a second.
fn opcode_ignored(thread: usize, bits: u32) {
    static LIMIT: ErrorLimit = ErrorLimit::new(1, 0);
    nd_log_limit!(&LIMIT, Source::Daemon, Priority::Debug, "STREAM THREAD[{thread}]: OPCODE {bits} ignored.");
}

impl Sender {
    /// The dequeue's bookkeeping: connections counted, the state's time.
    fn status_connected(&self) {
        self.lock().last_state_since_s = now_realtime_s();
    }
}
