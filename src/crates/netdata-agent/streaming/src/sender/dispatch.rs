//! A sender's connection on its stream thread (`src/streaming/stream-sender.c` and the sender half of
//! `stream-thread.c`): taken from the queue at the next tick, then its poll events, sends, receives, opcodes, the
//! periodic idle check, the disconnect that hands it back to the connector, and the thread's exit. Map:
//! `knowledge/map-m7-commit4-runtime.md` §3, §4, §7.

use std::io;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use netdata_agent_evloop::conn::Conn;
use netdata_agent_evloop::{Context, Event, Interest, Token};
use netdata_agent_ingest::stream_path;
use netdata_agent_log::{
    ErrorLimit, Field, FrameGuard, Priority, Source, Value, errno_of, msgid, nd_log, nd_log_limit, push,
};
use netdata_agent_rrd::chart::flags;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::contexts::Taker;
use netdata_agent_rrd::host::{Host, receiver_op, sender_flags};
use netdata_agent_rrd::pulse::host_status;
use netdata_agent_rrd::status::SocketPeers;
use netdata_agent_text::duration::duration_to_string;
use netdata_agent_text::size::size_to_string;
use netdata_agent_tls::{Link, socket_peers};

use super::execute::Executor;
use super::{Connected, Sender, Session, op};
use crate::caps;
use crate::connector::Cmd;
use crate::random::os_random32;
use crate::reason::Reason;
use crate::receiver::replication_progressed;
use netdata_agent_sys::now_monotonic_usec;
use crate::thread::StreamWorker;

/// The tokens of senders' sockets: above every receiver's.
pub(crate) const SENDER_TOKENS: usize = 1 << (usize::BITS - 2);
/// `PLUGINSD_LINE_MAX + 1`: the receive buffer.
const READ_BUFFER: usize = netdata_agent_pluginsd_proto::LINE_MAX + 1;

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
    /// The socket's descriptor while the link is open, -1 once it closed (`state->sock.fd`): what the frames' DST_IP and
    /// DST_PORT read when a record is written.
    peer_fd: Arc<AtomicI32>,
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
        sender_frame(&self.host.hostname(), Some(&self.peer_fd), self.capabilities, self.tls)
    }
}

/// DST_IP and DST_PORT ask the socket when a record is written (`stream_sender_log_dst_ip()`, `_port()`); with no
/// socket (closed) they return false, and C's logfmt keeps their separators; DST_TRANSPORT is `https` while a TLS link
/// is open (`nd_sock_is_ssl()`).
fn sender_frame(hostname: &str, peer_fd: Option<&Arc<AtomicI32>>, capabilities: u32, tls: bool) -> FrameGuard {
    let mut fields = vec![(Field::NidlNode, Value::Str(hostname.to_string()))];
    let (ip_fd, port_fd) = (peer_fd.cloned(), peer_fd.cloned());
    fields.push((
        Field::DstIp,
        Value::lazy(move |out| {
            peer(ip_fd.as_deref()).is_some_and(|(ip, _)| {
                out.extend_from_slice(ip.as_bytes());
                true
            })
        }),
    ));
    fields.push((
        Field::DstPort,
        Value::lazy(move |out| {
            peer(port_fd.as_deref()).is_some_and(|(_, port)| {
                out.extend_from_slice(port.to_string().as_bytes());
                true
            })
        }),
    ));
    fields.push((Field::DstTransport, Value::txt(if tls { "https" } else { "http" })));
    fields.push((Field::DstCapabilities, Value::Str(caps::to_string(capabilities))));
    push(fields)
}

/// The parent's address when a record is written: nothing once the socket closed, else `socket_peers()`'s remote
/// (`unknown`:0 once the parent reset the connection, or for a unix socket, D107.10).
fn peer(peer_fd: Option<&AtomicI32>) -> Option<(String, u16)> {
    let fd = peer_fd?.load(Ordering::Acquire);
    (fd >= 0).then(|| {
        let [_, remote] = socket_peers(Some(fd));
        remote
    })
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
        let peer_fd = Arc::new(AtomicI32::new(link.socket().map_or(-1, AsRawFd::as_raw_fd)));
        let _frame = sender_frame(&hostname, Some(&peer_fd), capabilities, tls);
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
            // socket2 opened it close-on-exec
            crate::sock::enlarge_buffers(&socket);
        }
        let session = Session { thread, id: loop {
            // a zero session means "no dispatcher"
            let id = os_random32();
            if id != 0 {
                break id;
            }
        } };
        let peers = SocketPeers::from(socket_peers(link.socket().map(AsRawFd::as_raw_fd)));
        {
            // C's one hold of the sender lock (stream-sender.c:353-367): the state around the commit lock, so a status
            // read never pairs this connection's zeroed counters with the time and the ends of the one before
            let mut state = sender.lock();
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
            Sender::status_connected(&mut state, &host, peers, tls);
        }
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
            last_traffic_ut: now_monotonic_usec(),
            remote_ip,
            capabilities,
            peer_fd,
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
            let now_ut = now_monotonic_usec();
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
                        d.last_traffic_ut = now_monotonic_usec();
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
        // what the commands owe (a POLLOUT, the host's child's NODE_ID and path) goes out now, as C's inline opcodes
        self.drain_inline(cx);
        ok
    }

    /// `stream_thread_handle_op()` for a sender: the opcodes of its current session only (D103.5); POLLOUT sends
    /// without removing, the rest disconnect.
    pub(crate) fn sender_ops(&mut self, cx: &mut Context<'_>, sender: &Arc<Sender>, session: Session) {
        let Some(ops) = sender.take_ops(session) else {
            return;
        };
        self.handle_ops(cx, sender, ops.session, ops.bits, ops.reason, true);
    }

    /// `posted`: the opcode came through the thread's queue. One handled inline cannot miss its sender in C (it runs
    /// while the sender is in its thread); here it waits for the step's end, so a sender gone meanwhile is skipped
    /// without C's "ignored" record.
    fn handle_ops(
        &mut self,
        cx: &mut Context<'_>,
        sender: &Arc<Sender>,
        session: Session,
        bits: u32,
        reason: Reason,
        posted: bool,
    ) {
        let found = self.senders.iter().position(|d| {
            d.as_ref().is_some_and(|d| Arc::ptr_eq(&d.sender, sender) && d.session == session)
        });
        let Some(index) = found else {
            if posted {
                opcode_ignored(cx.index(), bits);
            }
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

    /// The POLLOUTs posted on this thread while it ran a step (C handles them inline): the senders', then the
    /// receivers' (lines owed to a child).
    pub(crate) fn drain_inline(&mut self, cx: &mut Context<'_>) {
        for (sender, session) in crate::thread::take_inline() {
            if let Some(sender) = sender.upgrade() {
                self.handle_ops(cx, &sender, session, op::POLLOUT, Reason::NEVER, false);
            }
        }
        for slot in crate::thread::take_inline_children() {
            self.child_ops(cx, &slot, receiver_op::POLLOUT, false);
        }
    }

    /// The 100 ms tick's sender part: `stream_path_retention_updated()` from the RRDCONTEXT thread, whose paths go up
    /// now, up to a tick after C (D120, as the receivers' `tick_children`); while a receiver serves the host its tick
    /// sends them (D146.3). The commits' POLLOUTs go out with the tick's last `drain_inline`.
    pub(crate) fn tick_senders(&self) {
        // every sender shares the agent's connector: localhost once a tick, not once a sender (R62-1)
        let mut senders = self.senders.iter().flatten().peekable();
        let Some(localhost) = senders.peek().and_then(|d| d.sender.connector.localhost()) else {
            return;
        };
        for d in senders {
            stream_path::send_retention_changes_to_parent(&d.host, &localhost);
        }
    }

    /// `stream_sender_check_all_nodes_from_poll()`: the idle timeout, and a send for anything outstanding (C's poll
    /// mask repair; with edge-triggered events it covers a missed edge).
    pub(crate) fn check_senders(&mut self, cx: &mut Context<'_>) {
        let now_ut = now_monotonic_usec();
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
        d.host.contexts().record_first_time_changes(Taker::Sender, false);
        let reason = {
            let mut state = d.sender.lock();
            let reason = if reason == Reason::DISCONNECT_SIGNALED_TO_STOP && state.exit_reason != Reason::NEVER {
                state.exit_reason
            } else {
                reason
            };
            state.exit_reason = reason;
            // the socket closes below
            state.peers = super::not_connected();
            state.tls = false;
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
        d.peer_fd.store(-1, Ordering::Release);
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

/// "STREAM THREAD[%zu]: OPCODE %u ignored.", once a second for senders and receivers together (one limiter, as C's).
pub(crate) fn opcode_ignored(thread: usize, bits: u32) {
    static LIMIT: ErrorLimit = ErrorLimit::new(1, 0);
    nd_log_limit!(&LIMIT, Source::Daemon, Priority::Debug, "STREAM THREAD[{thread}]: OPCODE {bits} ignored.");
}

impl Sender {
    /// The dequeue's bookkeeping under the sender's state (`stream-sender.c:364-365`): the connection counted on the
    /// host, the state's time, and the socket's two ends and TLS flag, which C asks the socket for at each status.
    fn status_connected(state: &mut super::State, host: &Host, peers: SocketPeers, tls: bool) {
        host.count_sender_connection();
        state.last_state_since_s = now_realtime_s();
        state.peers = peers;
        state.tls = tls;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;
    use std::time::Duration;

    use netdata_agent_evloop::Pool;
    use netdata_agent_evloop::conn::Conn;
    use netdata_agent_evloop::testing::Stepper;
    use netdata_agent_rrd::host::{Attach, ReceiverSlot};
    use netdata_agent_tls::Link;

    use super::*;
    use crate::conf::Send;
    use crate::connector::tests::{collect_first_at, info};
    use crate::connector::Connector;
    use crate::parents::Local;
    use crate::sender::Settings;
    use crate::pins::Pins;
    use crate::sender::Connected;

    /// What reached the parent's end of the link so far.
    fn received(theirs: &mut mio::net::UnixStream) -> String {
        let mut all = Vec::new();
        let mut buf = [0; 65536];
        loop {
            match theirs.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => all.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        String::from_utf8_lossy(&all).into_owned()
    }

    /// A builtin of this host for the calls below: the command, the caller's source and the payload, echoed.
    fn echo(
        reply: &mut netdata_agent_nrpc::reply::Reply,
        function: &[u8],
        payload: Option<&netdata_agent_nrpc::reply::Payload>,
        source: &[u8],
    ) -> u16 {
        reply.content_type = netdata_agent_nrpc::reply::ContentType::ApplicationJson;
        reply.body = [function, b" from ", source].concat();
        if let Some(p) = payload {
            reply.body.extend_from_slice(format!(" {} ", p.content_type.name()).as_bytes());
            reply.body.extend_from_slice(&p.body);
        }
        200
    }

    /// A sender of `host` dispatched on a stream thread driven one turn at a time, with `capabilities` negotiated, and
    /// its parent's end of the link; the connector, localhost (which the connector holds weakly) and the pool are kept.
    struct Linked {
        s: Stepper<StreamWorker>,
        theirs: mio::net::UnixStream,
        _kept: (Arc<Connector>, Arc<Host>, Pool<crate::thread::StreamMsg>),
    }

    fn linked(host: &Arc<Host>, capabilities: u32) -> Linked {
        let pins = Arc::new(Mutex::new(Pins::new(1)));
        let pool = Pool::spawn(1, 256 * 1024, |i| format!("TEST[{i}]"), |_| StreamWorker::new(Arc::clone(&pins), 1))
            .unwrap();
        let localhost = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000aa", true, info("", "")));
        let local = Local { host_id: [0xaa; 16], user_agent: "t/1".into(), update_every: 1 };
        let c = Connector::new(Settings::of(&Send::default()), local, &localhost, pool.handle(), Arc::clone(&pins), 256 * 1024);
        let mut s = Stepper::new(0, StreamWorker::new(Arc::clone(&pins), 1)).unwrap();
        let sender = Sender::attach(host, &c).expect("created");
        let (ours, theirs) = mio::net::UnixStream::pair().unwrap();
        let connected = Connected {
            sender,
            link: Link::Plain(Conn::Unix(ours)),
            capabilities,
            compressor: None,
            remote_ip: "127.0.0.1".into(),
            thread: 0,
        };
        s.with(|w, cx| {
            w.queued_senders.push(connected);
            w.dequeue_senders(cx);
        });
        Linked { s, theirs, _kept: (c, localhost, pool) }
    }

    impl Linked {
        /// What came up in up to 20 turns after the parent sent `down`, until an answer ends.
        fn exchange(&mut self, down: &str) -> String {
            use std::io::Write;
            self.theirs.write_all(down.as_bytes()).unwrap();
            let mut got = String::new();
            for _ in 0..20 {
                let _ = netdata_agent_log::capture(|| self.s.turn(Duration::from_millis(10)));
                self.s.with(|w, cx| w.drain_inline(cx));
                got.push_str(&received(&mut self.theirs));
                if got.contains("FUNCTION_RESULT_END") {
                    break;
                }
            }
            got
        }
    }

    /// A method of this host for the calls below.
    fn register(host: &Host, name: &'static [u8], timeout_s: i32, handler: netdata_agent_nrpc::Handler) {
        let sync = matches!(handler, netdata_agent_nrpc::Handler::Builtin(_));
        let desc = netdata_agent_nrpc::MethodDesc {
            name,
            help: b"help",
            tags: b"",
            timeout_s,
            priority: 0,
            version: 0,
            access: 0,
            sync,
            source: netdata_agent_nrpc::Source::Daemon,
            handler,
        };
        host.functions().register("child", &desc).unwrap();
    }

    /// A transport of this host that holds its calls for the test to answer.
    #[derive(Default)]
    struct Held(Mutex<Vec<netdata_agent_nrpc::call::Request>>);

    impl netdata_agent_nrpc::Transport for Held {
        fn dispatch(&self, req: netdata_agent_nrpc::call::Request) -> u16 {
            self.0.lock().unwrap().push(req);
            200
        }
    }

    impl Held {
        fn take(&self) -> netdata_agent_nrpc::call::Request {
            self.0.lock().unwrap().pop().expect("a call")
        }
    }

    /// `execute_commands_function()` (D147.1): a parent's FUNCTION and FUNCTION_PAYLOAD run on this host's methods and
    /// their answers go up as the parent's transactions; an unknown one gets C's 404.
    #[test]
    fn a_parents_call_is_run_and_answered() {
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000ca", false, info("127.0.0.1:1", "key")));
        host.set_collector_online();
        register(&host, b"echo", 10, netdata_agent_nrpc::Handler::Builtin(std::sync::Arc::new(echo)));
        let mut l = linked(&host, caps::FUNCTIONS);
        let _ = l.exchange("");
        let tx = "5a1e00000000400080000000000000f5";
        let got = l.exchange(&format!("FUNCTION {tx} 10 \"echo  now\" \"0x13\" \"method=api\"\n"));
        assert!(
            got.starts_with(&format!("FUNCTION_RESULT_BEGIN \"{tx}\" 200 \"application/json\" 0\necho now from method=api\nFUNCTION_RESULT_END\n")),
            "{got:?}"
        );
        let got = l.exchange(&format!(
            "FUNCTION_PAYLOAD {tx} 10 \"echo\" \"0x0\" \"src\" \"application/json\"\n{{\"a\":1}}\nFUNCTION_PAYLOAD_END\n"
        ));
        assert!(
            got.contains("\necho from src application/json {\"a\":1}\n\nFUNCTION_RESULT_END\n"),
            "{got:?}"
        );
        let got = l.exchange(&format!("FUNCTION {tx} 10 \"nothing\" \"0x0\" \"src\"\n"));
        assert!(
            got.contains(r#" 404 "application/json" "#)
                && got.contains(r#"{"status":404,"errorMessage":"This feature is not available on this host at this time."}"#),
            "{got:?}"
        );
    }

    /// `execute_commands_function()`'s call (`stream-sender-execute.c:68-91`): a parent may call a restricted method;
    /// a timeout that is not positive is 10 s, not the method's; the answer goes up under the transaction exactly as the
    /// parent sent it (`:20-22`), its call id parsed from it.
    #[test]
    fn a_parents_call_may_be_restricted_and_is_answered_as_sent() {
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000cc", false, info("127.0.0.1:1", "key")));
        host.set_collector_online();
        let held = Arc::new(Held::default());
        register(&host, b"__hidden", 10, netdata_agent_nrpc::Handler::Builtin(std::sync::Arc::new(echo)));
        register(&host, b"slow", 30, netdata_agent_nrpc::Handler::Transport(Arc::clone(&held) as _));
        let mut l = linked(&host, caps::FUNCTIONS);
        let _ = l.exchange("");
        let tx = "5A1E0000-0000-4000-8000-0000000000F6";
        let got = l.exchange(&format!("FUNCTION {tx} 10 \"__hidden\" \"0x0\" \"src\"\n"));
        assert_eq!(got, format!("FUNCTION_RESULT_BEGIN \"{tx}\" 200 \"application/json\" 0\n__hidden from src\nFUNCTION_RESULT_END\n"));
        let before = now_monotonic_usec();
        let _ = l.exchange("FUNCTION 5a1e00000000400080000000000000f9 0 \"slow\" \"0x0\" \"src\"\n");
        let after = now_monotonic_usec();
        let req = held.take();
        assert_eq!(req.call.key(), "5a1e00000000400080000000000000f9");
        let deadline = req.call.deadline_ut();
        assert!((before + 10_000_000..=after + 10_000_000).contains(&deadline), "{before} {deadline} {after}");
        (req.done)(req.reply, 200);
    }

    /// The plugin's progress goes up only to a parent that takes PROGRESS (`stream-sender-execute.c:93-97`), as
    /// `FUNCTION_PROGRESS '<call id, compact>' <done> <all>` (`:41-52`), ahead of the answer.
    #[test]
    fn a_calls_progress_goes_up_only_to_a_parent_that_takes_it() {
        let held = Arc::new(Held::default());
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000cd", false, info("127.0.0.1:1", "key")));
        host.set_collector_online();
        register(&host, b"slow", 10, netdata_agent_nrpc::Handler::Transport(Arc::clone(&held) as _));
        let mut l = linked(&host, caps::FUNCTIONS | caps::PROGRESS);
        let _ = l.exchange("");
        let tx = "5A1E0000-0000-4000-8000-0000000000F7";
        let _ = l.exchange(&format!("FUNCTION {tx} 10 \"slow\" \"0x0\" \"src\"\n"));
        let req = held.take();
        (req.progress.as_ref().expect("the parent takes PROGRESS"))(&req.call_id, 5, 10);
        let mut reply = req.reply;
        reply.body = b"rows".to_vec();
        (req.done)(reply, 200);
        assert_eq!(
            l.exchange(""),
            format!(
                "FUNCTION_PROGRESS '5a1e00000000400080000000000000f7' 5 10\nFUNCTION_RESULT_BEGIN \"{tx}\" 200 \"text/plain\" 0\nrows\nFUNCTION_RESULT_END\n"
            )
        );
        let other = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000ce", false, info("127.0.0.1:1", "key")));
        other.set_collector_online();
        register(&other, b"slow", 10, netdata_agent_nrpc::Handler::Transport(Arc::clone(&held) as _));
        let mut l = linked(&other, caps::FUNCTIONS);
        let _ = l.exchange("");
        let _ = l.exchange("FUNCTION 5a1e00000000400080000000000000f8 10 \"slow\" \"0x0\" \"src\"\n");
        let req = held.take();
        assert!(req.progress.is_none(), "a parent without PROGRESS");
        (req.done)(req.reply, 200);
    }

    /// An answer goes up only while the host's metadata may stream (`stream-sender-execute.c:17`): none while its
    /// collector is offline, the sender connected.
    #[test]
    fn a_parents_call_is_answered_only_while_the_hosts_metadata_may_stream() {
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000cf", false, info("127.0.0.1:1", "key")));
        register(&host, b"echo", 10, netdata_agent_nrpc::Handler::Builtin(std::sync::Arc::new(echo)));
        let mut l = linked(&host, caps::FUNCTIONS);
        let _ = l.exchange("");
        let got = l.exchange("FUNCTION 5a1e00000000400080000000000000fa 10 \"echo\" \"0x0\" \"src\"\n");
        assert!(!got.contains("FUNCTION_RESULT"), "{got:?}");
        host.set_collector_online();
        let got = l.exchange("FUNCTION 5a1e00000000400080000000000000fb 10 \"echo\" \"0x0\" \"src\"\n");
        assert!(
            got.starts_with(
                "FUNCTION_RESULT_BEGIN \"5a1e00000000400080000000000000fb\" 200 \"application/json\" 0\necho from src\nFUNCTION_RESULT_END\n"
            ),
            "{got:?}"
        );
    }

    /// D146.3 at the tick: a dispatched sender sends its host's first-time changes, each in a path of its own, while
    /// no receiver takes them (a vnode defined after the sender's READY included); none once a receiver does, which
    /// sends both halves itself.
    #[test]
    fn the_tick_sends_a_hosts_paths_while_no_receiver_takes_them() {
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c9", false, info("127.0.0.1:1", "key")));
        let mut l = linked(&host, caps::PATHS);
        let tick = |l: &mut Linked| {
            l.s.with(|w, cx| {
                w.tick_senders();
                w.drain_inline(cx);
            });
            let _ = netdata_agent_log::capture(|| l.s.turn(Duration::from_millis(20)));
            received(&mut l.theirs)
        };
        let _ = tick(&mut l);
        // a vnode its plugin defines once the sender is ready
        host.set_virtual();
        host.set_collector_online();
        collect_first_at(&host, "t.a", 1_790_000_000);
        let sent = tick(&mut l);
        assert!(sent.contains("JSON STREAM_PATH\n") && sent.contains(r#""first_time_t":1789999999,"#), "{sent:?}");
        assert!(tick(&mut l).is_empty(), "each change once");
        // the plugin's run ends: its changes owe nothing while it is offline
        host.virtual_offline();
        collect_first_at(&host, "t.b", 1_789_999_990);
        assert!(!tick(&mut l).contains("STREAM_PATH"), "offline");
        // a child streaming its GUID: attached and online before its stream thread takes the changes, when C sends
        // the parent's half (the child's has no parser yet)
        let slot = Arc::new(ReceiverSlot::new(0, Default::default(), Default::default(), Box::new(|| {})));
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        collect_first_at(&host, "t.c", 1_789_999_980);
        let sent = tick(&mut l);
        assert!(sent.contains(r#""first_time_t":1789999979,"#), "{sent:?}");
        // then its thread takes them, both halves
        host.contexts().record_first_time_changes(Taker::Receiver, true);
        collect_first_at(&host, "t.d", 1_789_999_970);
        assert!(!tick(&mut l).contains("STREAM_PATH"), "the receiver's to send");
        assert_eq!(host.contexts().take_first_time_changes(Taker::Receiver), [1_789_999_969]);
        host.clear_receiver(&slot, 0);
    }

    #[test]
    fn a_records_parent_address_is_the_sockets_when_written() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (parent, _) = listener.accept().unwrap();
        let fd = AtomicI32::new(client.as_raw_fd());
        assert_eq!(peer(Some(&fd)), Some(("127.0.0.1".to_string(), port)));

        // the parent resets the connection: the kernel no longer has its address
        socket2::SockRef::from(&parent).set_linger(Some(Duration::ZERO)).unwrap();
        drop(parent);
        let _ = (&client).read(&mut [0; 1]);
        assert_eq!(peer(Some(&fd)), Some(("unknown".to_string(), 0)));

        // the socket closed: no fields at all
        fd.store(-1, Ordering::Release);
        assert_eq!(peer(Some(&fd)), None);
        assert_eq!(peer(None), None);
    }

    /// The dispatch's bookkeeping (`stream-sender.c:364-365`): the connection counted once on the host and the state's
    /// time stamped, with the socket's two ends (a unix pair's are `unknown`); its disconnect clears the ends and the
    /// CONNECTED flag the status reads, and keeps the count and the time, which C stamps again only at a removal (a
    /// time set in the past shows a stamp in the same second).
    #[test]
    fn a_dispatch_is_counted_once_and_its_ends_leave_with_it() {
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000cb", false, info("127.0.0.1:1", "key")));
        let before_s = now_realtime_s();
        let mut l = linked(&host, caps::V2);
        let up = Arc::clone(host.upstream().expect("a sender"));
        let s = up.status();
        let unknown = SocketPeers {
            local_ip: "unknown".into(),
            local_port: 0,
            peer_ip: "unknown".into(),
            peer_port: 0,
        };
        assert_eq!((s.connections, host.sender_connections()), (1, 1));
        assert!((before_s..=now_realtime_s()).contains(&s.since_s), "{}", s.since_s);
        assert_eq!((&s.peers, s.tls, s.connected), (&unknown, false, false));
        // the connector sets the flag; the status reads it in its hold
        host.sender_flags_set(sender_flags::CONNECTED);
        assert!(up.status().connected);
        l.s.with(|w, _| w.senders[0].as_ref().expect("dispatched").sender.lock().last_state_since_s = 1_000);
        let _ = netdata_agent_log::capture(|| {
            l.s.with(|w, cx| w.disconnect_sender(cx, 0, Reason::DISCONNECT_SOCKET_ERROR, Reason::NEVER, true))
        });
        let after = up.status();
        assert_eq!((after.connections, after.since_s, after.connected), (1, 1_000, false));
        assert_eq!((&after.peers, after.tls), (&crate::sender::not_connected(), false));
    }

    /// The status's compression is the connect's until a dispatch hands the compressor to the commit lock, and the
    /// commit lock's while the connection is dispatched: C sets the compressor up in `stream_connect()`, before
    /// CONNECTED, where the dispatch here waits for the stream thread's tick.
    #[test]
    fn the_compression_is_the_connect_s_until_the_dispatch() {
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000cd", false, info("127.0.0.1:1", "key")));
        let mut l = linked(&host, caps::V2);
        let up = Arc::clone(host.upstream().expect("a sender"));
        let sender = l.s.with(|w, _| Arc::clone(&w.senders[0].as_ref().expect("dispatched").sender));
        // dispatched without a compressor: the connect's word no longer counts
        sender.lock().compression = true;
        assert!(!up.status().compression);
        let _ = netdata_agent_log::capture(|| {
            l.s.with(|w, cx| w.disconnect_sender(cx, 0, Reason::DISCONNECT_SOCKET_ERROR, Reason::NEVER, true))
        });
        // off its stream thread until the next dispatch: the connect's
        assert!(up.status().compression);
    }

    /// The status takes the sender's state and then its commit lock; commits take the commit lock alone. Both at once
    /// on two threads end (a commit path that took the state inside the commit lock would deadlock here).
    #[test]
    fn the_status_reads_beside_commits() {
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000cc", false, info("127.0.0.1:1", "key")));
        let _l = linked(&host, caps::V2);
        let up = Arc::clone(host.upstream().expect("a sender"));
        let committer = {
            let up = Arc::clone(&up);
            std::thread::spawn(move || {
                for _ in 0..5_000 {
                    up.commit(b"BEGIN x\n", crate::sender::Traffic::Data);
                }
            })
        };
        for _ in 0..5_000 {
            assert_eq!(up.status().connections, 1);
        }
        committer.join().unwrap();
    }
}
