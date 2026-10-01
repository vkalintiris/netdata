//! The spawn server, the helper process (`spawn_server_nofork.c`): this binary re-executed by
//! [`crate::client::Server::create`] with [`MARKER`] in its environment (decisions D12, D136, D137 in the status
//! repository). It reads its bootstrap from its stdin, a socket that also passes it its listening socket and the status
//! pipe, then serves requests one at a time: it starts each child with the child contract of [`crate::exec`], answers
//! STARTED or FAILED at once and EXITED once it reaps the child, and stops its children when the daemon goes away.

use std::ffi::CString;
use std::io::{IoSliceMut, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_sys::{self as sys, CloseMode};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::signal::{SaFlags, SigSet, Signal, kill};
use nix::unistd::Pid;

use crate::{exec, wire};

/// The environment variable that makes this binary a spawn server: [`run_if_requested`] runs one and exits.
pub const MARKER: &str = "NETDATA_SPAWN_SERVER";

/// The bootstrap's first item: the creator and the server are one binary, this only rejects a stray stdin.
const BOOTSTRAP_TAG: &[u8] = b"netdata-spawn-server-1";

/// What the creator tells the server it starts, on the server's stdin: what C's forked server finds in its memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bootstrap {
    /// The key every request must carry (`server->magic`).
    pub(crate) magic: [u8; 16],
    /// `server->name`: the given name, `unnamed` without one.
    pub(crate) name: String,
    /// The listening socket's path, unlinked when the server ends.
    pub(crate) path: PathBuf,
    /// The creator's environment at the time, which C's server reads its log settings from.
    pub(crate) env: Vec<Vec<u8>>,
}

impl Bootstrap {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let magic = uuid::Uuid::from_bytes(self.magic).simple().to_string();
        let mut items: Vec<&[u8]> =
            vec![BOOTSTRAP_TAG, magic.as_bytes(), self.name.as_bytes(), self.path.as_os_str().as_bytes()];
        items.extend(self.env.iter().map(Vec::as_slice));
        wire::encode_list(&items)
    }

    pub(crate) fn decode(b: &[u8]) -> Option<Bootstrap> {
        let items = wire::decode_list(b)?;
        let [tag, magic, name, path, env @ ..] = items.as_slice() else {
            return None;
        };
        if *tag != BOOTSTRAP_TAG {
            return None;
        }
        Some(Bootstrap {
            magic: uuid::Uuid::try_parse_ascii(magic).ok()?.into_bytes(),
            name: String::from_utf8(name.to_vec()).ok()?,
            path: PathBuf::from(std::ffi::OsStr::from_bytes(path)),
            env: env.iter().map(|e| e.to_vec()).collect(),
        })
    }
}

/// `snprintfz(buf, 16, "spawn-%s", name)`: the server's `comm` and its log's program name.
pub(crate) fn title(name: &str) -> String {
    let mut title = format!("spawn-{name}");
    let mut end = title.len().min(15);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    title.truncate(end);
    title
}

/// Runs the spawn server when this process was started as one ([`MARKER`] set), then exits with its code; returns at
/// once otherwise. The first statement of `main()` of every binary a server is created from: a binary that is
/// re-executed without it would run whole while its creator waits for the server's handshake.
pub fn run_if_requested() {
    if std::env::var_os(MARKER).is_some() {
        sys::exit_now(serve());
    }
}

/// The server's life: C's child of `spawn_server_create()` (`spawn_server_nofork.c:1519-1538`), then its event loop.
fn serve() -> i32 {
    // only the bootstrap socket (0), stdout and stderr: C's server keeps its socket and pipe, received below
    if sys::close_fds_except(&[], CloseMode::Close).is_err() {
        return 1;
    }
    let Some((boot, listener, status)) = read_bootstrap(std::io::stdin().as_fd()) else {
        return 1;
    };
    let title = title(&boot.name);
    if let Ok(comm) = CString::new(title.as_str()) {
        let _ = nix::sys::prctl::set_name(&comm);
    }
    replace_stdio_with_dev_null();
    let env = boot.env;
    netdata_agent_log::initialize_for_external_plugins(Box::leak(title.into_boxed_str()), &|key: &str| {
        let prefix = [key.as_bytes(), b"="].concat();
        env.iter()
            .find(|e| e.starts_with(&prefix))
            .map(|e| String::from_utf8_lossy(&e[prefix.len()..]).into_owned())
    });
    event_loop(&listener, status.as_fd(), &boot.magic, &boot.path)
}

/// The bootstrap and the two descriptors that came with it (the listening socket, the status pipe's write end),
/// read to the creator's end of the stream.
fn read_bootstrap(sock: BorrowedFd<'_>) -> Option<(Bootstrap, UnixListener, OwnedFd)> {
    let mut payload = Vec::new();
    let mut fds = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let received = match sys::recv_with_fds(sock, &mut [IoSliceMut::new(&mut buf)], 2) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
            Ok(r) => r,
        };
        if received.truncated {
            return None;
        }
        fds.extend(received.fds);
        if received.bytes == 0 {
            break;
        }
        payload.extend_from_slice(&buf[..received.bytes]);
    }
    let boot = Bootstrap::decode(&payload)?;
    let [listen, status]: [OwnedFd; 2] = fds.try_into().ok()?;
    Some((boot, UnixListener::from(listen), status))
}

/// `replace_stdio_with_dev_null()` (`spawn_server_nofork.c:1406-1431`): stdin and stdout on `/dev/null`, stderr left
/// as the creator set it.
fn replace_stdio_with_dev_null() {
    let Ok(null) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/null") else {
        return;
    };
    if nix::unistd::dup2_stdin(&null).is_ok() {
        let _ = nix::unistd::dup2_stdout(&null);
    }
}

/// A started child the server answers for (`SPAWN_REQUEST` once kept).
struct Request {
    pid: Pid,
    request_id: usize,
    sock: UnixStream,
    cmdline: String,
}

/// `write()` of a report on a request's socket; when not all of it went, the errno C's record then carries (EPIPE once
/// the client is gone; 0 for a short write).
fn send_report(mut sock: &UnixStream, report: wire::Report) -> Result<(), i32> {
    match sock.write(&report.encode()) {
        Ok(wire::Report::LEN) => Ok(()),
        Ok(_) => Err(0),
        Err(e) => Err(netdata_agent_log::errno_of(&e)),
    }
}

/// `spawn_server_event_loop()` (`spawn_server_nofork.c:1236-1340`): the server's exit code.
fn event_loop(listener: &UnixListener, status: BorrowedFd<'_>, magic: &[u8; 16], path: &std::path::Path) -> i32 {
    // C blocks every signal first, then installs these; here the handlers come first, so a TERM queued since the exec
    // meets a handler, never its default action
    let flags = SaFlags::SA_RESTART | SaFlags::SA_NOCLDSTOP;
    let chld = match sys::signal_flag(Signal::SIGCHLD, flags) {
        Ok(flag) => flag,
        Err(e) => {
            nd_log!(Source::Collector, Priority::Err, errno = netdata_agent_log::errno_of(&e);
                "SPAWN SERVER: sigaction() failed for SIGCHLD");
            return 1;
        }
    };
    let term = match sys::signal_flag(Signal::SIGTERM, flags) {
        Ok(flag) => flag,
        Err(e) => {
            nd_log!(Source::Collector, Priority::Err, errno = netdata_agent_log::errno_of(&e);
                "SPAWN SERVER: sigaction() failed for SIGTERM");
            return 1;
        }
    };
    let mut mask = SigSet::all();
    mask.remove(Signal::SIGTERM);
    mask.remove(Signal::SIGCHLD);
    let _ = mask.thread_set_mask();

    let handshake = wire::Report::handshake(std::process::id() as i32).encode();
    let written = nix::unistd::write(status, &handshake);
    if !matches!(written, Ok(wire::Report::LEN)) {
        nd_log!(Source::Collector, Priority::Err, errno = written.err().map_or(0, |e| e as i32);
            "SPAWN SERVER: failed to write initial status report.");
        return 1;
    }

    let mut requests: Vec<Request> = Vec::new();
    while !term.load(Ordering::SeqCst) {
        let mut fds = [
            PollFd::new(listener.as_fd(), PollFlags::POLLIN),
            PollFd::new(status, PollFlags::POLLHUP | PollFlags::POLLERR),
        ];
        let ret = poll(&mut fds, PollTimeout::from(500u16));
        if chld.load(Ordering::SeqCst) || ret == Ok(0) {
            process_sigchld(&mut requests, chld);
            if matches!(ret, Ok(0) | Err(_)) {
                continue;
            }
        }
        if let Err(e) = ret {
            nd_log!(Source::Collector, Priority::Err, errno = e as i32; "SPAWN SERVER: poll() failed");
            break;
        }
        let revents = |i: usize| fds[i].revents().unwrap_or(PollFlags::empty());
        if revents(1).intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
            nd_log!(Source::Collector, Priority::Debug, "SPAWN SERVER: Parent process closed socket (exited?)");
            break;
        }
        if revents(0).contains(PollFlags::POLLIN) {
            match listener.accept() {
                Ok((sock, _)) => receive_request(sock, magic, &mut requests),
                Err(e) => {
                    nd_log!(Source::Collector, Priority::Err, errno = netdata_agent_log::errno_of(&e);
                        "SPAWN SERVER: accept() failed");
                }
            }
        }
    }

    let _ = std::fs::remove_file(path);
    if !requests.is_empty() {
        signal_all(&requests, Signal::SIGTERM);
        let grace = Duration::from_millis(wire::KILL_DEFAULT_GRACE_MS as u64);
        if !wait_for_children_to_exit(&mut requests, chld, grace) {
            nd_log!(Source::Collector, Priority::Warning,
                "SPAWN SERVER: children did not exit after SIGTERM; sending SIGKILL");
            signal_all(&requests, Signal::SIGKILL);
            if !wait_for_children_to_exit(&mut requests, chld, grace) {
                nd_log!(Source::Collector, Priority::Err,
                    "SPAWN SERVER: giving up waiting for children after SIGKILL");
            }
        }
    }
    0
}

/// `spawn_server_process_sigchld()` (`spawn_server_nofork.c:1146-1214`): reaps every child that ended, logs it, and
/// answers its request with EXITED.
fn process_sigchld(requests: &mut Vec<Request>, chld: &AtomicBool) {
    chld.store(false, Ordering::SeqCst);
    // the kernel's status as it is: a real-time signal's death included (R56 B1)
    while let Ok(Some((pid, raw))) = sys::waitpid_raw(-1, true) {
        let pid = Pid::from_raw(pid);
        let at = requests.iter().position(|rq| rq.pid == pid);
        let (request_id, cmdline) = at.map_or((0, None), |i| (requests[i].request_id, Some(requests[i].cmdline.as_str())));
        if log_reaped(pid.as_raw(), request_id, raw, cmdline)
            && let Some(i) = at
        {
            let rq = requests.remove(i);
            if let Err(errno) = send_report(&rq.sock, wire::Report::exited(raw)) {
                nd_log!(Source::Collector, Priority::Err, errno = errno;
                    "SPAWN SERVER: Cannot send exit status ({raw}) report for pid {}, request {}: {}",
                    rq.pid, rq.request_id, rq.cmdline);
            }
        }
    }
}

/// `spawn_server_signal_all_children()`.
fn signal_all(requests: &[Request], signal: Signal) {
    for rq in requests {
        if let Err(e) = kill(rq.pid, signal) {
            let priority = if signal == Signal::SIGKILL { Priority::Err } else { Priority::Warning };
            nd_log!(Source::Collector, priority, errno = e as i32;
                "SPAWN SERVER: failed to send signal {} to child pid {} (request {}): {}",
                signal as i32, rq.pid, rq.request_id, netdata_agent_log::strerror(e as i32));
        }
    }
}

/// `spawn_server_wait_for_children_to_exit()`: reaps every 10 ms until no request is left or `timeout` passes.
fn wait_for_children_to_exit(requests: &mut Vec<Request>, chld: &AtomicBool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !requests.is_empty() {
        process_sigchld(requests, chld);
        if requests.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// What one `spawn_server_recvmsg_fully()` brought: the control message's level, type and length, and its descriptors.
#[derive(Default)]
struct Control {
    cmsg: Option<(i32, i32, usize)>,
    fds: Vec<OwnedFd>,
}

/// `spawn_server_recvmsg_fully()` (`spawn_server_nofork.c:641-723`): fills `buf`, taking a control message (up to
/// [`wire::TRANSFER_FDS`] descriptors) until one arrives when `control` is set; any control message past that, or
/// one that did not fit, fails the read with C's record, as does an error or the peer's end.
fn recv_fully(sock: &UnixStream, buf: &mut [u8], control: bool, description: &str) -> Option<Control> {
    let mut got = Control::default();
    let mut at = 0;
    while at < buf.len() {
        let room = if control && got.cmsg.is_none() { wire::TRANSFER_FDS } else { 0 };
        let received = match sys::recv_with_fds(sock.as_fd(), &mut [IoSliceMut::new(&mut buf[at..])], room) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = netdata_agent_log::errno_of(&e);
                    "SPAWN SERVER: failed to recvmsg() {description}.");
                return None;
            }
            Ok(r) => r,
        };
        if received.bytes == 0 {
            nd_log!(Source::Collector, Priority::Err, "SPAWN SERVER: peer closed socket while receiving {description}.");
            return None;
        }
        if received.truncated {
            nd_log!(Source::Collector, Priority::Err,
                "SPAWN SERVER: received truncated control message while receiving {description}.");
            return None;
        }
        if got.cmsg.is_none() && received.first_cmsg.is_some() {
            got.cmsg = received.first_cmsg;
        }
        got.fds.extend(received.fds);
        at += received.bytes;
    }
    Some(got)
}

/// `spawn_server_receive_request()` (`spawn_server_nofork.c:927-1126`): reads a request, checks it in C's order with
/// C's records (a rejected one gets no report), and executes it. Every descriptor it received is closed on the way out
/// unless a child took it.
fn receive_request(sock: UnixStream, magic: &[u8; 16], requests: &mut Vec<Request>) {
    let mut header = [0u8; wire::HEADER_LEN];
    let Some(control) = recv_fully(&sock, &mut header, true, "the first part of the request") else {
        return;
    };
    let Some(h) = wire::Header::decode(&header) else {
        return;
    };
    if h.msg_type == wire::MSG_PING {
        drop(control);
        if let Err(errno) = send_report(&sock, wire::Report::ping()) {
            nd_log!(Source::Collector, Priority::Err, errno = errno; "SPAWN SERVER: Cannot send ping reply.");
        }
        return;
    }
    if h.msg_type != wire::MSG_REQUEST {
        nd_log!(Source::Collector, Priority::Err,
            "SPAWN SERVER: invalid request message type {}. Rejecting request.", h.msg_type);
        return;
    }
    let expected = wire::cmsg_len(wire::TRANSFER_FDS);
    let Some((level, kind, _)) = control.cmsg.filter(|&(_, _, len)| len == expected) else {
        nd_log!(Source::Collector, Priority::Err,
            "SPAWN SERVER: Received invalid control message (expected {expected} bytes, received {} bytes)",
            control.cmsg.map_or(0, |(_, _, len)| len));
        return;
    };
    if level != libc::SOL_SOCKET || kind != libc::SCM_RIGHTS {
        nd_log!(Source::Collector, Priority::Err, "SPAWN SERVER: Received unexpected control message type.");
        return;
    }
    let Ok([stdin, stdout, stderr, custom]): Result<[OwnedFd; wire::TRANSFER_FDS], _> = control.fds.try_into() else {
        return;
    };
    if h.magic != *magic {
        nd_log!(Source::Collector, Priority::Err,
            "SPAWN SERVER: Invalid authorization key for request {}. Rejecting request.", h.request_id);
        return;
    }
    // this server executes, it runs no callbacks (out:external-I1)
    if h.instance_type == wire::TYPE_CALLBACK {
        nd_log!(Source::Collector, Priority::Err,
            "SPAWN SERVER: Request {} wants to run a callback, but callbacks are not allowed for this spawn server. \
             Rejecting request.", h.request_id);
        return;
    }
    if h.env_size == 0 || h.argv_size == 0 {
        nd_log!(Source::Collector, Priority::Err,
            "SPAWN SERVER: invalid encoded request sizes, env = {}, argv = {}", h.env_size, h.argv_size);
        return;
    }
    let mut env = vec![0u8; h.env_size];
    let mut argv = vec![0u8; h.argv_size];
    let mut data = vec![0u8; h.data_size];
    for buf in [&mut env, &mut argv, &mut data] {
        if recv_fully(&sock, buf, false, "the second part of the request").is_none() {
            return;
        }
    }
    let to_c = |list: Vec<&[u8]>| list.into_iter().map(|s| CString::new(s).ok()).collect::<Option<Vec<_>>>();
    let (Some(env), Some(argv)) =
        (wire::decode_list(&env).and_then(to_c), wire::decode_list(&argv).and_then(to_c))
    else {
        nd_log!(Source::Collector, Priority::Err,
            "SPAWN SERVER: received malformed encoded argv/envp buffers for request {}", h.request_id);
        return;
    };
    drop(custom);
    let started = match h.instance_type {
        wire::TYPE_EXEC => spawn_external_command(&argv, &env, [stdin, stdout, stderr]),
        _ => Err((Errno::EINVAL as i32, None)),
    };
    match started {
        Ok((pid, cmdline)) => {
            if let Err(errno) = send_report(&sock, wire::Report::started(pid.as_raw())) {
                nd_log!(Source::Collector, Priority::Err, errno = errno;
                    "SPAWN SERVER: Cannot send success status report for pid {pid}, request {}: {cmdline}",
                    h.request_id);
            }
            requests.push(Request { pid, request_id: h.request_id, sock, cmdline });
        }
        Err((errno, cmdline)) => {
            if let Err(errno) = send_report(&sock, wire::Report::failed(errno)) {
                nd_log!(Source::Collector, Priority::Err, errno = errno;
                    "SPAWN SERVER: Cannot send failure status report for request {}: {}",
                    h.request_id, cmdline.as_deref().unwrap_or("(null)"));
            }
        }
    }
}

/// `spawn_external_command()` (`spawn_server_nofork.c:270-366`): the child and its command line, or the errno its
/// FAILED report carries (0: C's records clear it) and the command line when one was made.
fn spawn_external_command(
    argv: &[CString],
    env: &[CString],
    fds: [OwnedFd; 3],
) -> Result<(Pid, String), (i32, Option<String>)> {
    let cmdline = wire::cmdline(&argv.iter().map(|a| a.as_bytes()).collect::<Vec<_>>());
    let [stdin, stdout, stderr] = &fds;
    match exec::spawn_child(argv, env, stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd()) {
        Ok(pid) => {
            drop(fds);
            nd_log!(Source::Collector, Priority::Debug, "SPAWN SERVER: process created with pid {pid}: {cmdline}");
            Ok((pid, cmdline))
        }
        Err(e) if e.step == "posix_spawn()" => {
            nd_log!(Source::Collector, Priority::Err, "SPAWN SERVER: posix_spawn() failed: {cmdline}");
            Err((0, Some(cmdline)))
        }
        Err(e) => {
            nd_log!(Source::Collector, Priority::Err, "SPAWN PARENT: {} failed: {cmdline}", e.step);
            Err((0, Some(cmdline)))
        }
    }
}

/// `spawn_server_sigchld_handler()`'s records for a reaped child (`spawn_server_nofork.c:1166-1208`), with C's texts
/// and `[request not found]` without a command line: whether it ended (exited or killed), so its request is answered
/// and removed.
pub fn log_reaped(pid: i32, request_id: usize, raw: i32, cmdline: Option<&str>) -> bool {
    let cmd = cmdline.unwrap_or("[request not found]");
    if libc::WIFEXITED(raw) {
        let code = libc::WEXITSTATUS(raw);
        if code != 0 {
            nd_log!(
                Source::Collector,
                Priority::Warning,
                "SPAWN SERVER: child with pid {pid} (request {request_id}) exited with exit code {code}: {cmd}"
            );
        }
        true
    } else if libc::WIFSIGNALED(raw) {
        // SIGPIPE and SIGTERM are how children are stopped on purpose: not warnings
        let sig = libc::WTERMSIG(raw);
        if libc::WCOREDUMP(raw) {
            nd_log!(
                Source::Collector,
                Priority::Warning,
                "SPAWN SERVER: child with pid {pid} (request {request_id}) coredump'd due to signal {sig}: {cmd}"
            );
        } else {
            let priority = if matches!(sig, libc::SIGPIPE | libc::SIGTERM) { Priority::Debug } else { Priority::Warning };
            nd_log!(
                Source::Collector,
                priority,
                "SPAWN SERVER: child with pid {pid} (request {request_id}) killed by signal {sig}: {cmd}"
            );
        }
        true
    } else if libc::WIFSTOPPED(raw) {
        let sig = libc::WSTOPSIG(raw);
        nd_log!(
            Source::Collector,
            Priority::Warning,
            "SPAWN SERVER: child with pid {pid} (request {request_id}) stopped due to signal {sig}: {cmd}"
        );
        false
    } else if libc::WIFCONTINUED(raw) {
        nd_log!(
            Source::Collector,
            Priority::Warning,
            "SPAWN SERVER: child with pid {pid} (request {request_id}) continued due to signal {}: {cmd}",
            libc::SIGCONT
        );
        false
    } else {
        nd_log!(
            Source::Collector,
            Priority::Warning,
            "SPAWN SERVER: child with pid {pid} (request {request_id}) reports unhandled status: {cmd}"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::fd::RawFd;

    use nix::sys::socket::{ControlMessage, MsgFlags, UnixAddr, sendmsg};

    use super::*;

    const MAGIC: [u8; 16] = [7; 16];

    /// A request's bytes: its header (request 7) and blobs.
    fn request(msg_type: u8, magic: [u8; 16], instance_type: u8, env: &[u8], argv: &[u8]) -> Vec<u8> {
        let header = wire::Header {
            msg_type,
            magic,
            request_id: 7,
            env_size: env.len(),
            argv_size: argv.len(),
            data_size: 0,
            instance_type,
        };
        [header.encode().as_slice(), env, argv].concat()
    }

    /// Sends `bytes` from `client` with `fds` (none: no control message), then ends the client's sending side.
    fn send(client: &UnixStream, bytes: &[u8], fds: &[RawFd]) {
        let rights = [ControlMessage::ScmRights(fds)];
        let cmsgs: &[ControlMessage] = if fds.is_empty() { &[] } else { &rights };
        if !bytes.is_empty() {
            let iov = [std::io::IoSlice::new(bytes)];
            let n = sendmsg::<UnixAddr>(client.as_raw_fd(), &iov, cmsgs, MsgFlags::empty(), None).unwrap();
            assert_eq!(n, bytes.len());
        }
        client.shutdown(std::net::Shutdown::Write).unwrap();
    }

    /// Feeds one request to `receive_request()`: its records, the report bytes the client got, the requests kept, and
    /// whether every descriptor it passed (copies of one pipe's write end) was closed again.
    fn serve_one(bytes: &[u8], fds: usize) -> (Vec<(Priority, String)>, Vec<u8>, usize, bool) {
        let (client, server) = UnixStream::pair().unwrap();
        let (pipe_read, pipe_write) = nix::unistd::pipe().unwrap();
        let passed = vec![pipe_write.as_raw_fd(); fds];
        send(&client, bytes, &passed);
        drop(pipe_write);
        let mut requests = Vec::new();
        let ((), records) = netdata_agent_log::capture(|| receive_request(server, &MAGIC, &mut requests));
        // a kept request holds its socket open: the client reads to its end only once it is gone
        let kept = requests.len();
        drop(requests);
        let mut report = Vec::new();
        // a server that closes with the request unread resets the connection: no report either way
        if let Err(e) = (&client).read_to_end(&mut report) {
            assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset);
        }
        // no writer left: the pipe reads as ended at once
        let mut fd = [PollFd::new(pipe_read.as_fd(), PollFlags::POLLIN)];
        let closed = poll(&mut fd, PollTimeout::ZERO).unwrap() == 1
            && fd[0].revents().is_some_and(|r| r.contains(PollFlags::POLLHUP));
        let records = records.into_iter().map(|r| (r.priority, r.message.unwrap_or_default())).collect();
        (records, report, kept, closed)
    }

    /// Every way C's server refuses a request, each with C's record, no report and the descriptors closed.
    #[test]
    fn rejected_requests_are_logged_and_unanswered_as_c() {
        let env = wire::encode_list(&["A=1"]);
        let argv = wire::encode_list(&["/bin/true"]);
        let valid = request(wire::MSG_REQUEST, MAGIC, wire::TYPE_EXEC, &env, &argv);
        let cases: [(&str, Vec<u8>, usize, &str); 12] = [
            ("no descriptors", valid.clone(), 0,
                "SPAWN SERVER: Received invalid control message (expected 32 bytes, received 0 bytes)"),
            ("three descriptors", valid.clone(), 3,
                "SPAWN SERVER: Received invalid control message (expected 32 bytes, received 28 bytes)"),
            ("wrong key", request(wire::MSG_REQUEST, [1; 16], wire::TYPE_EXEC, &env, &argv), 4,
                "SPAWN SERVER: Invalid authorization key for request 7. Rejecting request."),
            ("unknown message", request(7, MAGIC, wire::TYPE_EXEC, &env, &argv), 4,
                "SPAWN SERVER: invalid request message type 7. Rejecting request."),
            ("callback", request(wire::MSG_REQUEST, MAGIC, wire::TYPE_CALLBACK, &env, &argv), 4,
                "SPAWN SERVER: Request 7 wants to run a callback, but callbacks are not allowed for this spawn server. \
                 Rejecting request."),
            ("no environment", request(wire::MSG_REQUEST, MAGIC, wire::TYPE_EXEC, b"", &argv), 4,
                "SPAWN SERVER: invalid encoded request sizes, env = 0, argv = 11"),
            ("malformed arguments", request(wire::MSG_REQUEST, MAGIC, wire::TYPE_EXEC, &env, b"x\0"), 4,
                "SPAWN SERVER: received malformed encoded argv/envp buffers for request 7"),
            ("nothing sent", Vec::new(), 0,
                "SPAWN SERVER: peer closed socket while receiving the first part of the request."),
            ("cut short", valid[..valid.len() - 1].to_vec(), 4,
                "SPAWN SERVER: peer closed socket while receiving the second part of the request."),
            // more descriptors than the control buffer holds
            ("five descriptors", valid.clone(), 5,
                "SPAWN SERVER: received truncated control message while receiving the first part of the request."),
            // the key is checked before the instance type, which is checked before the sizes (`:1003-1048`)
            ("wrong key, callback", request(wire::MSG_REQUEST, [1; 16], wire::TYPE_CALLBACK, &env, &argv), 4,
                "SPAWN SERVER: Invalid authorization key for request 7. Rejecting request."),
            ("callback, no environment", request(wire::MSG_REQUEST, MAGIC, wire::TYPE_CALLBACK, b"", &argv), 4,
                "SPAWN SERVER: Request 7 wants to run a callback, but callbacks are not allowed for this spawn server. \
                 Rejecting request."),
        ];
        for (name, bytes, fds, record) in cases {
            let got = serve_one(&bytes, fds);
            assert_eq!(got, (vec![(Priority::Err, record.to_string())], vec![], 0, true), "{name}");
        }
    }

    /// A PING is answered whatever its key; an instance type C knows neither as exec nor as callback passes both gates
    /// and is answered FAILED with EINVAL, unlogged (`spawn_server_nofork.c:529-546`).
    #[test]
    fn a_ping_is_answered_and_an_unknown_instance_type_fails() {
        let ping = wire::Header::ping().encode();
        assert_eq!(serve_one(&ping, 0), (vec![], wire::Report::ping().encode().to_vec(), 0, true));
        let env = wire::encode_list(&["A=1"]);
        let argv = wire::encode_list(&["/bin/true"]);
        let odd = request(wire::MSG_REQUEST, MAGIC, 5, &env, &argv);
        assert_eq!(serve_one(&odd, 4), (vec![], wire::Report::failed(libc::EINVAL).encode().to_vec(), 0, true));
    }

    /// The records and the answer per kind of end, as C's handler writes them.
    #[test]
    fn reaped_children_are_logged_as_c() {
        let cases = [
            (0x300, Some((Priority::Warning, "exited with exit code 3: cmd")), true),
            (0, None, true),
            (libc::SIGTERM, Some((Priority::Debug, "killed by signal 15: cmd")), true),
            (libc::SIGPIPE, Some((Priority::Debug, "killed by signal 13: cmd")), true),
            (34, Some((Priority::Warning, "killed by signal 34: cmd")), true),
            (libc::SIGKILL, Some((Priority::Warning, "killed by signal 9: cmd")), true),
            (libc::SIGSEGV | 0x80, Some((Priority::Warning, "coredump'd due to signal 11: cmd")), true),
            (libc::SIGSTOP << 8 | 0x7f, Some((Priority::Warning, "stopped due to signal 19: cmd")), false),
            (0xffff, Some((Priority::Warning, "continued due to signal 18: cmd")), false),
            // none of the macros' cases: the request stays
            (0xff, Some((Priority::Warning, "reports unhandled status: cmd")), false),
        ];
        for (raw, record, ended) in cases {
            let (got, records) = netdata_agent_log::capture(|| log_reaped(42, 7, raw, Some("cmd")));
            let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap())).collect();
            let want: Vec<_> = record
                .map(|(p, tail)| (Source::Collector, p, format!("SPAWN SERVER: child with pid 42 (request 7) {tail}")))
                .into_iter()
                .collect();
            assert_eq!((got, records), (ended, want), "{raw:#x}");
        }
        let (_, records) = netdata_agent_log::capture(|| log_reaped(42, 0, 0x100, None));
        assert!(records[0].message.as_deref().unwrap().ends_with("exit code 1: [request not found]"));
    }
}
