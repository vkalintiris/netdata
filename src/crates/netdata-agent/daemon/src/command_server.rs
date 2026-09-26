//! The netdatacli command server (`commands_init()`, `command_thread()` and `commands_exit()` of
//! `src/daemon/commands.c`): a unix stream socket at the pipe name, served by the `DAEMON_COMMAND` thread. One
//! command per connection: the request is what the client sends before it shuts down its side; the command runs on
//! the `UV_WORKER` pool (D57.1) and the thread writes the reply, then closes the connection.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{JoinHandle, ThreadId};

use mio::net::{UnixListener, UnixStream};
use mio::{Events, Interest, Poll, Token, Waker};
use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{Priority, Source, errno_of, nd_log, netdata_log_error, netdata_log_info};
use nix::errno::Errno;
use socket2::{Domain, SockAddr, Socket, Type};

use crate::commands::{self, Status};
use crate::{shutdown, system};

const WAKE: Token = Token(0);
const LISTENER: Token = Token(1);

/// `SOMAXCONN`.
const BACKLOG: i32 = 4096;

/// libuv's read size.
const READ_SIZE: usize = 64 * 1024;

/// libuv's reads per readiness event before it polls again.
const READS_PER_EVENT: usize = 32;

/// `daemon_pipename()`: `NETDATA_PIPENAME` verbatim, even when empty, else `<run dir>/netdata.pipe`.
pub fn pipename() -> &'static [u8] {
    static PIPENAME: OnceLock<Vec<u8>> = OnceLock::new();
    PIPENAME.get_or_init(|| match std::env::var_os("NETDATA_PIPENAME") {
        Some(name) => name.into_vec(),
        None => format!(
            "{}/netdata.pipe",
            system::run_dir(false).unwrap_or_default()
        )
        .into_bytes(),
    })
}

/// `uv_strerror()` of a negated errno.
pub fn uv_strerror(errno: i32) -> String {
    let text = match Errno::from_raw(errno) {
        Errno::E2BIG => "argument list too long",
        Errno::EACCES => "permission denied",
        Errno::EADDRINUSE => "address already in use",
        Errno::EADDRNOTAVAIL => "address not available",
        Errno::EAFNOSUPPORT => "address family not supported",
        Errno::EAGAIN => "resource temporarily unavailable",
        Errno::EALREADY => "connection already in progress",
        Errno::EBADF => "bad file descriptor",
        Errno::EBUSY => "resource busy or locked",
        Errno::ECANCELED => "operation canceled",
        Errno::ECONNABORTED => "software caused connection abort",
        Errno::ECONNREFUSED => "connection refused",
        Errno::ECONNRESET => "connection reset by peer",
        Errno::EDESTADDRREQ => "destination address required",
        Errno::EEXIST => "file already exists",
        Errno::EFAULT => "bad address in system call argument",
        Errno::EFBIG => "file too large",
        Errno::EHOSTUNREACH => "host is unreachable",
        Errno::EINTR => "interrupted system call",
        Errno::EINVAL => "invalid argument",
        Errno::EIO => "i/o error",
        Errno::EISCONN => "socket is already connected",
        Errno::EISDIR => "illegal operation on a directory",
        Errno::ELOOP => "too many symbolic links encountered",
        Errno::EMFILE => "too many open files",
        Errno::EMSGSIZE => "message too long",
        Errno::ENAMETOOLONG => "name too long",
        Errno::ENETDOWN => "network is down",
        Errno::ENETUNREACH => "network is unreachable",
        Errno::ENFILE => "file table overflow",
        Errno::ENOBUFS => "no buffer space available",
        Errno::ENODEV => "no such device",
        Errno::ENOENT => "no such file or directory",
        Errno::ENOMEM => "not enough memory",
        Errno::ENONET => "machine is not on the network",
        Errno::ENOPROTOOPT => "protocol not available",
        Errno::ENOSPC => "no space left on device",
        Errno::ENOSYS => "function not implemented",
        Errno::ENOTCONN => "socket is not connected",
        Errno::ENOTDIR => "not a directory",
        Errno::ENOTEMPTY => "directory not empty",
        Errno::ENOTSOCK => "socket operation on non-socket",
        Errno::EOPNOTSUPP => "operation not supported on socket",
        Errno::EOVERFLOW => "value too large for defined data type",
        Errno::EPERM => "operation not permitted",
        Errno::EPIPE => "broken pipe",
        Errno::EPROTO => "protocol error",
        Errno::EPROTONOSUPPORT => "protocol not supported",
        Errno::EPROTOTYPE => "protocol wrong type for socket",
        Errno::ERANGE => "result too large",
        Errno::EROFS => "read-only file system",
        Errno::ESHUTDOWN => "cannot send after transport endpoint shutdown",
        Errno::ESPIPE => "invalid seek",
        Errno::ESRCH => "no such process",
        Errno::ETIMEDOUT => "connection timed out",
        Errno::ETXTBSY => "text file is busy",
        Errno::EXDEV => "cross-device link not permitted",
        Errno::ENXIO => "no such device or address",
        Errno::EMLINK => "too many links",
        Errno::EHOSTDOWN => "host is down",
        Errno::EREMOTEIO => "remote I/O error",
        Errno::ENOTTY => "inappropriate ioctl for device",
        Errno::EILSEQ => "illegal byte sequence",
        Errno::ESOCKTNOSUPPORT => "socket type not supported",
        Errno::ENODATA => "no data available",
        Errno::EUNATCH => "protocol driver not attached",
        Errno::ENOEXEC => "exec format error",
        _ => return format!("Unknown system error {}", -errno),
    };
    text.to_string()
}

/// The running `DAEMON_COMMAND` thread.
struct Running {
    thread: JoinHandle<()>,
    id: ThreadId,
    waker: Arc<Waker>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// `command_thread_shutdown`: the loop ends at its next wake-up.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// `commands_exit_in_progress`.
static EXIT_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// `commands_in_flight`: dispatched commands whose reply has not started.
static IN_FLIGHT: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// `this_thread_runs_a_command`.
    static IN_COMMAND: Cell<bool> = const { Cell::new(false) };
}

fn running() -> std::sync::MutexGuard<'static, Option<Running>> {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `commands_init()`: at the first call (status OFF) the liveness server (INIT): the thread, bound and listening,
/// or C's failure records and OFF again; at the next the full server (FULL).
pub fn init(pool: &WorkPool, stack_size: usize) {
    if shutdown::exiting() {
        netdata_log_info!("Not initializing the command server: shutdown has already begun.");
        return;
    }
    match commands::status() {
        commands::FULL => return,
        commands::OFF => {
            netdata_log_info!("Initializing command server for liveness CHECK");
            EXIT_IN_PROGRESS.store(false, Ordering::Release);
            commands::set_status(commands::INIT);
        }
        _ => {
            netdata_log_info!("Initializing full command server.");
            commands::set_status(commands::FULL);
            return;
        }
    }
    let (ready_tx, ready_rx) = mpsc::channel();
    let pool = pool.clone();
    let spawned = std::thread::Builder::new()
        .name("DAEMON_COMMAND".into())
        .stack_size(stack_size)
        .spawn(move || command_thread(&pool, &ready_tx));
    let started = match spawned {
        Ok(thread) => match ready_rx.recv() {
            Ok(Some(waker)) => {
                let id = thread.thread().id();
                *running() = Some(Running { thread, id, waker });
                true
            }
            _ => {
                let _ = thread.join();
                false
            }
        },
        Err(err) => {
            netdata_log_error!("uv_thread_create(): {}", uv_strerror(errno_of(&err)));
            false
        }
    };
    if !started {
        netdata_log_error!(
            "Failed to initialize command server. The netdata cli tool will be unable to send commands."
        );
        commands::set_status(commands::OFF);
        return;
    }
    if shutdown::exiting() {
        netdata_log_info!("Command server came up during shutdown; stopping it again.");
        exit();
    }
}

/// `commands_exit()`: stops the thread and waits for it, unless called from it or from a command, which would wait
/// for itself.
pub fn exit() {
    if commands::status() == commands::OFF {
        return;
    }
    let mut guard = running();
    let Some(id) = guard.as_ref().map(|r| r.id) else {
        netdata_log_info!("Command server thread was never created; nothing to stop.");
        return;
    };
    if id == std::thread::current().id() || IN_COMMAND.with(Cell::get) {
        match IN_FLIGHT.load(Ordering::Acquire) {
            0 => netdata_log_info!(
                "Command server cannot be joined from this thread; no netdatacli command is in flight."
            ),
            n => netdata_log_info!(
                "Command server cannot be joined from this thread; {n} command(s) still in flight."
            ),
        }
        return;
    }
    if EXIT_IN_PROGRESS.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(server) = guard.take() else { return };
    drop(guard);
    SHUTDOWN.store(true, Ordering::Release);
    netdata_log_info!("Shutting down command server.");
    let _ = server.waker.wake();
    let _ = server.thread.join();
    netdata_log_info!("Command server has stopped.");
    commands::set_status(commands::OFF);
}

/// Step 21 of the exit sequence: the socket file, unless libuv already removed it.
pub fn remove_socket_file() {
    let name = pipename();
    if name.is_empty() {
        return;
    }
    if let Err(err) = std::fs::remove_file(OsStr::from_bytes(name))
        && err.kind() != io::ErrorKind::NotFound
    {
        nd_log!(Source::Daemon, Priority::Err, errno = errno_of(&err);
            "EXIT: cannot unlink netdatacli socket file '{}'.", String::from_utf8_lossy(name));
    }
}

/// `uv_pipe_bind()` and `uv_listen()`: a non-blocking unix stream socket at `name`, listening.
fn listen(name: &[u8]) -> Result<UnixListener, ()> {
    let fail = |call: &str, err: &io::Error| {
        let errno = errno_of(err);
        // libuv reports a missing directory as a permission error
        let reported = if call == "uv_pipe_bind" && errno == Errno::ENOENT as i32 {
            Errno::EACCES as i32
        } else {
            errno
        };
        nd_log!(Source::Daemon, Priority::Err, errno = errno; "{call}(): {}", uv_strerror(reported));
    };
    let socket = Socket::new(Domain::UNIX, Type::STREAM.nonblocking().cloexec(), None)
        .map_err(|err| fail("uv_pipe_init", &err))?;
    // C binds a name of 108 bytes or more cut to 108 with no NUL; the safe address stops at 107, so such a name
    // takes the bind failure path (D57.4)
    SockAddr::unix(OsStr::from_bytes(name))
        .map_err(|_| io::Error::from_raw_os_error(Errno::ENAMETOOLONG as i32))
        .and_then(|addr| socket.bind(&addr))
        .map_err(|err| fail("uv_pipe_bind", &err))?;
    if socket.listen(BACKLOG).is_err() {
        netdata_log_info!(
            "uv_listen() failed with backlog = {BACKLOG}, falling back to backlog = 1."
        );
        socket.listen(1).map_err(|err| fail("uv_listen", &err))?;
    }
    Ok(UnixListener::from_std(
        std::os::unix::net::UnixListener::from(OwnedFd::from(socket)),
    ))
}

enum State {
    /// Collecting the request (at most `MAX_COMMAND_LENGTH - 1` bytes) until the client's EOF.
    Reading(Vec<u8>),
    /// The command runs on the pool.
    Dispatched,
    /// Writing the reply, from this offset.
    Writing(Vec<u8>, usize),
}

struct Client {
    stream: UnixStream,
    state: State,
}

/// A finished command: the client's token, the command and its outcome.
type Done = (Token, usize, Status, Option<Vec<u8>>);

struct Loop<'a> {
    poll: Poll,
    waker: Arc<Waker>,
    pool: &'a WorkPool,
    clients: HashMap<Token, Client>,
    next_token: usize,
    done_tx: mpsc::Sender<Done>,
    done_rx: mpsc::Receiver<Done>,
}

/// `command_thread()`: tells `init()` whether it listens (with the waker that stops it), then serves until `exit()`.
fn command_thread(pool: &WorkPool, ready: &mpsc::Sender<Option<Arc<Waker>>>) {
    let poll = match Poll::new() {
        Ok(poll) => poll,
        Err(err) => {
            netdata_log_error!("uv_loop_init(): {}", uv_strerror(errno_of(&err)));
            let _ = ready.send(None);
            return;
        }
    };
    let waker = match Waker::new(poll.registry(), WAKE) {
        Ok(waker) => Arc::new(waker),
        Err(err) => {
            netdata_log_error!("uv_async_init(): {}", uv_strerror(errno_of(&err)));
            let _ = ready.send(None);
            return;
        }
    };
    let name = pipename();
    let _ = std::fs::remove_file(OsStr::from_bytes(name));
    let Ok(mut listener) = listen(name) else {
        let _ = ready.send(None);
        return;
    };
    if let Err(err) = poll
        .registry()
        .register(&mut listener, LISTENER, Interest::READABLE)
    {
        netdata_log_error!("uv_listen(): {}", uv_strerror(errno_of(&err)));
        let _ = ready.send(None);
        return;
    }
    SHUTDOWN.store(false, Ordering::Release);
    let (done_tx, done_rx) = mpsc::channel();
    let mut lp = Loop {
        poll,
        waker: Arc::clone(&waker),
        pool,
        clients: HashMap::new(),
        next_token: 2,
        done_tx,
        done_rx,
    };
    let _ = ready.send(Some(waker));
    let mut events = Events::with_capacity(64);
    while !SHUTDOWN.load(Ordering::Acquire) {
        if lp.poll_once(&mut events, Some(&mut listener)).is_err() {
            break;
        }
    }
    netdata_log_info!("Shutting down command event loop.");
    // stop accepting first (libuv unlinks the path as it closes), then drop the idle clients, then flush
    let _ = lp.poll.registry().deregister(&mut listener);
    drop(listener);
    if !name.is_empty() && name[0] != 0 {
        let _ = std::fs::remove_file(OsStr::from_bytes(name));
    }
    lp.clients
        .retain(|_, c| !matches!(c.state, State::Reading(_)));
    while !lp.clients.is_empty() {
        if lp.poll_once(&mut events, None).is_err() {
            break;
        }
    }
    netdata_log_info!("Shutting down command loop complete.");
}

impl Loop<'_> {
    fn poll_once(
        &mut self,
        events: &mut Events,
        mut listener: Option<&mut UnixListener>,
    ) -> io::Result<()> {
        match self.poll.poll(events, None) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(err) => return Err(err),
        }
        for event in events.iter() {
            match event.token() {
                WAKE => {}
                LISTENER => {
                    if let Some(listener) = listener.as_deref_mut() {
                        self.accept(listener);
                    }
                }
                token => self.client_event(token),
            }
        }
        // replies of the commands that finished
        while let Ok((token, idx, status, message)) = self.done_rx.try_recv() {
            let out = commands::reply(Some(idx), status, message.as_deref());
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            self.start_reply(token, out);
        }
        Ok(())
    }

    /// `connection_cb()`: every pending connection.
    fn accept(&mut self, listener: &mut UnixListener) {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let token = Token(self.next_token);
                    self.next_token += 1;
                    if let Err(err) =
                        self.poll
                            .registry()
                            .register(&mut stream, token, Interest::READABLE)
                    {
                        netdata_log_error!("uv_read_start(): {}", uv_strerror(errno_of(&err)));
                        continue;
                    }
                    self.clients.insert(
                        token,
                        Client {
                            stream,
                            state: State::Reading(Vec::new()),
                        },
                    );
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    netdata_log_error!("uv_accept(): {}", uv_strerror(errno_of(&err)));
                    return;
                }
            }
        }
    }

    fn client_event(&mut self, token: Token) {
        let Some(client) = self.clients.get_mut(&token) else {
            return;
        };
        match &mut client.state {
            State::Reading(request) => match read_request(&mut client.stream, request) {
                Ok(false) => {}
                Ok(true) => {
                    let request = std::mem::take(request);
                    self.parse(token, &request);
                }
                Err(()) => {
                    self.clients.remove(&token);
                }
            },
            State::Writing(..) => self.write(token),
            State::Dispatched => {}
        }
    }

    /// `parse_commands()`: an illegal request is answered at once; a command goes to the pool, `shutdown-agent`
    /// first running here, as C runs it on this thread.
    fn parse(&mut self, token: Token, request: &[u8]) {
        let Some((idx, args)) = commands::parse(request) else {
            let out = commands::reply(None, commands::FAILURE, Some(commands::ILLEGAL.as_bytes()));
            self.start_reply(token, out);
            return;
        };
        if idx == commands::EXIT {
            commands::execute(commands::EXIT, b"");
        }
        if let Some(client) = self.clients.get_mut(&token) {
            client.state = State::Dispatched;
        }
        IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
        let args = args.to_vec();
        let (done_tx, waker) = (self.done_tx.clone(), Arc::clone(&self.waker));
        let queued = self.pool.queue(move || {
            IN_COMMAND.with(|c| c.set(true));
            let (status, message) = commands::execute(idx, &args);
            IN_COMMAND.with(|c| c.set(false));
            let _ = done_tx.send((token, idx, status, message));
            let _ = waker.wake();
        });
        if queued.is_err() {
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            self.start_reply(token, commands::reply(Some(idx), commands::FAILURE, None));
        }
    }

    /// `send_command_reply()`: the reply goes out as far as the socket takes it now, the rest when it is writable.
    fn start_reply(&mut self, token: Token, out: Vec<u8>) {
        let Some(client) = self.clients.get_mut(&token) else {
            return;
        };
        client.state = State::Writing(out, 0);
        self.write(token);
    }

    fn write(&mut self, token: Token) {
        let Some(client) = self.clients.get_mut(&token) else {
            return;
        };
        let State::Writing(out, offset) = &mut client.state else {
            return;
        };
        while *offset < out.len() {
            match client.stream.write(&out[*offset..]) {
                Ok(n) => *offset += n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    // write errors are not reported: the connection just closes
                    if self
                        .poll
                        .registry()
                        .reregister(&mut client.stream, token, Interest::WRITABLE)
                        .is_ok()
                    {
                        return;
                    }
                    break;
                }
                Err(_) => break,
            }
        }
        self.clients.remove(&token);
    }
}

/// libuv's reads on a readable client (`uv__read()` and `pipe_read_cb()`): `Ok(true)` at EOF, `Ok(false)` to wait
/// for more, `Err` after a read error (recorded; the connection closes without a reply). mio reports readiness once,
/// so this reads until the socket is drained, where libuv's level-triggered loop would be woken again. Within one
/// wake-up libuv follows a read that fills its buffer with another, up to 32, and when that one finds nothing yet it
/// reports zero bytes, which C records; after a shorter read it waits for the next wake-up, and finds nothing only
/// when nothing came.
fn read_request(stream: &mut UnixStream, request: &mut Vec<u8>) -> Result<bool, ()> {
    let mut buf = vec![0u8; READ_SIZE];
    // full reads in a row, within libuv's current wake-up
    let mut full = 0;
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                let room = commands::MAX_COMMAND_LENGTH - 1 - request.len();
                request.extend_from_slice(&buf[..n.min(room)]);
                full = if n == READ_SIZE { full + 1 } else { 0 };
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if full > 0 && full % READS_PER_EVENT != 0 {
                    nd_log!(Source::Daemon, Priority::Info, errno = Errno::EAGAIN as i32;
                        "pipe_read_cb: Zero bytes read by command pipe.");
                }
                return Ok(false);
            }
            Err(err) => {
                let errno = errno_of(&err);
                nd_log!(Source::Daemon, Priority::Err, errno = errno;
                    "pipe_read_cb: {}", uv_strerror(errno));
                return Err(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream as StdUnixStream;

    fn pair() -> (StdUnixStream, UnixStream) {
        let (client, server) = StdUnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        (client, UnixStream::from_std(server))
    }

    #[test]
    fn a_request_and_its_eof_are_read_in_one_wake_up() {
        let (mut client, mut server) = pair();
        client.write_all(b"ping ").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let mut request = Vec::new();
        assert_eq!(read_request(&mut server, &mut request), Ok(true));
        assert_eq!(request, b"ping ");
    }

    #[test]
    fn a_full_read_then_nothing_is_cs_zero_bytes_record() {
        let zero_bytes = |len: usize| {
            let (mut client, mut server) = pair();
            client.write_all(&vec![b' '; len]).unwrap();
            let mut request = Vec::new();
            let (result, records) =
                netdata_agent_log::capture(|| read_request(&mut server, &mut request));
            assert_eq!(result, Ok(false), "{len}");
            assert_eq!(request.len(), commands::MAX_COMMAND_LENGTH - 1, "{len}");
            records
                .iter()
                .map(|r| (r.errno, r.message.clone().unwrap_or_default()))
                .collect::<Vec<_>>()
        };
        let record = (
            Errno::EAGAIN as i32,
            "pipe_read_cb: Zero bytes read by command pipe.".to_string(),
        );
        assert_eq!(zero_bytes(READ_SIZE), vec![record.clone()]);
        assert_eq!(zero_bytes(2 * READ_SIZE), [record]);
        assert_eq!(zero_bytes(READ_SIZE - 1), []);
    }

    #[test]
    fn a_name_c_would_cut_fails_to_bind() {
        let name = format!("/tmp/{}", "p".repeat(110));
        let (result, records) = netdata_agent_log::capture(|| listen(name.as_bytes()));
        assert!(result.is_err());
        let records: Vec<_> = records
            .iter()
            .map(|r| (r.errno, r.message.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            records,
            [(
                Errno::ENAMETOOLONG as i32,
                "uv_pipe_bind(): name too long".to_string()
            )]
        );
    }

    #[test]
    fn errors_read_as_libuv_texts() {
        assert_eq!(uv_strerror(Errno::EACCES as i32), "permission denied");
        assert_eq!(
            uv_strerror(Errno::EADDRINUSE as i32),
            "address already in use"
        );
        assert_eq!(
            uv_strerror(Errno::EOPNOTSUPP as i32),
            "operation not supported on socket"
        );
        assert_eq!(uv_strerror(4095), "Unknown system error -4095");
    }
}
