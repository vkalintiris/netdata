//! The daemon's side of a spawn server (`spawn_server_nofork.c:1346-1859`): creating one, asking it to start a child,
//! and waiting for or stopping that child through the report the server sends when it reaps it.

use std::ffi::{CString, OsString};
use std::fs::File;
use std::io::{IoSlice, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use netdata_agent_log::{Priority, Source, errno_of, nd_log, strerror};
use nix::errno::Errno;
use nix::fcntl::OFlag;
use nix::poll::PollFlags;
use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, posix_spawn};
use nix::sys::signal::{Signal, kill};
use nix::sys::socket::{
    AddressFamily, Backlog, ControlMessage, MsgFlags, SockFlag, SockType, UnixAddr, bind, connect, listen, sendmsg,
    socket, socketpair,
};
use nix::sys::wait::waitpid;
use nix::unistd::{Pid, pipe2};

use crate::server::{Bootstrap, MARKER};
use crate::{env, wire};

/// `sizeof(sun_path) - 1`: the longest socket path.
const MAX_SOCKET_PATH: usize = 107;

/// `spawn_server_id`: every server created in this process takes the next number.
static SERVER_ID: AtomicUsize = AtomicUsize::new(0);

/// What a server is started from: C's fork inherited both implicitly.
#[derive(Debug, Clone)]
pub struct Start {
    /// The binary to re-execute: `/proc/self/exe`, whose `main()` calls [`crate::server::run_if_requested`] first.
    pub exe: PathBuf,
    /// `os_run_dir(true)`, asked when `NETDATA_RUN_DIR` is unset or empty.
    pub run_dir: fn() -> Option<String>,
}

impl Default for Start {
    fn default() -> Start {
        Start {
            exe: PathBuf::from("/proc/self/exe"),
            run_dir: || None,
        }
    }
}

/// `spawn_server_id`'s next number, taken without a server: C's daemon creates an "init" server first, which the
/// agent does not (D134.3), so the plugins server keeps C's number.
pub fn skip_server_id() {
    SERVER_ID.fetch_add(1, Ordering::Relaxed);
}

/// A running spawn server (`SPAWN_SERVER`): dropping it is `spawn_server_destroy()`, which stops it.
#[derive(Debug)]
pub struct Server {
    name: String,
    path: PathBuf,
    magic: [u8; 16],
    pid: Option<Pid>,
    /// The status pipe's read end: the server sees the daemon's end when the last copy closes.
    status: Option<File>,
    request_id: AtomicUsize,
}

/// The runtime directory a server's socket goes to (`spawn_server_nofork.c:1445-1472`), with C's records.
fn runtime_directory(start: &Start) -> String {
    let dir = env::get("NETDATA_RUN_DIR")
        .map(|d| d.to_string_lossy().into_owned())
        .filter(|d| !d.is_empty())
        .or_else(start.run_dir);
    let Some(dir) = dir.filter(|d| !d.is_empty()) else {
        return "/tmp".into();
    };
    match std::fs::metadata(&dir) {
        Ok(meta) if meta.is_dir() => match nix::unistd::access(dir.as_str(), nix::unistd::AccessFlags::W_OK) {
            Ok(()) => dir,
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = e as i32;
                    "Runtime directory '{dir}' is not writable, falling back to '/tmp'");
                "/tmp".into()
            }
        },
        other => {
            let errno = other.err().map_or(0, |e| errno_of(&e));
            nd_log!(Source::Collector, Priority::Err, errno = errno;
                "Runtime directory '{dir}' does not exist, falling back to '/tmp'");
            "/tmp".into()
        }
    }
}

/// `connect_to_spawn_server()`: a stream to the server at `path`, with C's records when `log`.
fn connect_to(path: &Path, log: bool) -> Option<UnixStream> {
    let Ok(addr) = UnixAddr::new(path) else {
        if log {
            nd_log!(Source::Collector, Priority::Err, errno = Errno::ENAMETOOLONG as i32;
                "SPAWN PARENT: Cannot connect() to spawn server on path '{}': exceeds the {MAX_SOCKET_PATH}-byte \
                 AF_UNIX limit", path.display());
        }
        return None;
    };
    // close-on-exec where C's is not (D136.5)
    let sock = match socket(AddressFamily::Unix, SockType::Stream, SockFlag::SOCK_CLOEXEC, None) {
        Ok(sock) => sock,
        Err(e) => {
            if log {
                nd_log!(Source::Collector, Priority::Err, errno = e as i32;
                    "SPAWN PARENT: cannot create socket() to connect to spawn server: {} ({}).",
                    strerror(e as i32), e as i32);
            }
            return None;
        }
    };
    if let Err(e) = connect(sock.as_raw_fd(), &addr) {
        if log {
            nd_log!(Source::Collector, Priority::Err, errno = e as i32;
                "SPAWN PARENT: cannot connect() to spawn server on path '{}': {} ({}).",
                path.display(), strerror(e as i32), e as i32);
        }
        return None;
    }
    Some(UnixStream::from(sock))
}

/// `spawn_server_sendmsg_fully()`: `bytes`, with `fds` passed on the first `sendmsg()`; C's records name `what`, none
/// without it.
fn send_fully(sock: &UnixStream, bytes: &[u8], fds: &[i32], what: Option<&str>) -> bool {
    let mut at = 0;
    while at < bytes.len() {
        let rights = [ControlMessage::ScmRights(fds)];
        let cmsgs: &[ControlMessage] = if at == 0 && !fds.is_empty() { &rights } else { &[] };
        match sendmsg::<UnixAddr>(sock.as_raw_fd(), &[IoSlice::new(&bytes[at..])], cmsgs, MsgFlags::empty(), None) {
            Err(Errno::EINTR) => {}
            Err(e) => {
                if let Some(what) = what {
                    nd_log!(Source::Collector, Priority::Err, errno = e as i32;
                        "SPAWN PARENT: failed to sendmsg() {what}.");
                }
                return false;
            }
            Ok(0) => {
                if let Some(what) = what {
                    nd_log!(Source::Collector, Priority::Err,
                        "SPAWN PARENT: sendmsg() made no progress while sending {what}.");
                }
                return false;
            }
            Ok(n) => at += n,
        }
    }
    true
}

/// A report read whole from `from`, or why not.
fn read_report(mut from: impl Read) -> std::io::Result<wire::Report> {
    let mut b = [0u8; wire::Report::LEN];
    from.read_exact(&mut b)?;
    Ok(wire::Report::decode(&b))
}

/// The errno a failed read leaves: 0 when the stream ended early.
fn read_errno(e: &std::io::Error) -> i32 {
    if e.kind() == std::io::ErrorKind::UnexpectedEof { 0 } else { errno_of(e) }
}

/// `spawn_server_is_running()`: whether a server answers a PING at `path`.
fn is_running(path: &Path) -> bool {
    let Some(sock) = connect_to(path, false) else {
        return false;
    };
    if !send_fully(&sock, &wire::Header::ping().encode(), &[], Some("spawn server ping")) {
        return false;
    }
    read_report(&sock).is_ok_and(|r| r.status == wire::Status::Ping as i32)
}

/// `spawn_server_create_listening_socket()` (`spawn_server_nofork.c:1365-1404`).
fn listening_socket(path: &Path) -> Option<UnixListener> {
    if is_running(path) {
        nd_log!(Source::Collector, Priority::Err,
            "SPAWN SERVER: Server is already listening on path '{}'", path.display());
        return None;
    }
    let addr = UnixAddr::new(path).ok()?;
    let sock = match socket(AddressFamily::Unix, SockType::Stream, SockFlag::SOCK_CLOEXEC, None) {
        Ok(sock) => sock,
        Err(e) => {
            nd_log!(Source::Collector, Priority::Err, errno = e as i32;
                "SPAWN SERVER: Failed to create socket(): {} ({})", strerror(e as i32), e as i32);
            return None;
        }
    };
    let _ = std::fs::remove_file(path);
    if let Err(e) = bind(sock.as_raw_fd(), &addr) {
        nd_log!(Source::Collector, Priority::Err, errno = e as i32;
            "SPAWN SERVER: Failed to bind(): {} ({})", strerror(e as i32), e as i32);
        return None;
    }
    if let Err(e) = listen(&sock, Backlog::MAXCONN) {
        nd_log!(Source::Collector, Priority::Err, errno = e as i32;
            "SPAWN SERVER: Failed to listen(): {} ({})", strerror(e as i32), e as i32);
        return None;
    }
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o770)) {
        nd_log!(Source::Collector, Priority::Err, errno = errno_of(&e);
            "SPAWN SERVER: failed to chmod '{}' to 0770", path.display());
    }
    Some(UnixListener::from(sock))
}

/// `os_setproctitle(title, argc, argv)` (`libnetdata/os/setproctitle.c:17-30`) applied to `argv`: the first argument
/// becomes `title`, cut or blank-padded to its own length, every other one blanks of its length.
fn retitled(title: &str, argv: &[OsString]) -> Vec<Vec<u8>> {
    argv.iter()
        .enumerate()
        .map(|(i, arg)| {
            let len = arg.as_bytes().len();
            if i > 0 {
                return vec![b' '; len];
            }
            let mut first: Vec<u8> = title.as_bytes().iter().copied().take(len).collect();
            first.resize(len, b' ');
            first
        })
        .collect()
}

/// The environment this binary was executed with (`/proc/self/environ`), what C's forked server shows in its
/// `/proc`; the current one when it cannot be read. Read once: a later user switch makes the process non-dumpable and
/// `/proc/self/environ` unreadable (R57 M2), so [`crate::popen::configure`] reads it at the start.
pub(crate) fn exec_time_environment() -> &'static [Vec<u8>] {
    static AT_EXEC: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();
    AT_EXEC.get_or_init(|| match std::fs::read("/proc/self/environ") {
        Ok(block) => block.split(|&c| c == 0).filter(|e| !e.is_empty()).map(<[u8]>::to_vec).collect(),
        Err(_) => env::block().into_iter().map(CString::into_bytes).collect(),
    })
}

fn c_strings(items: impl IntoIterator<Item = Vec<u8>>) -> Option<Vec<CString>> {
    items.into_iter().map(|i| CString::new(i).ok()).collect()
}

/// `%#x`: C prints 0 without the prefix.
fn alt_hex(v: u32) -> String {
    if v == 0 { "0".into() } else { format!("{v:#x}") }
}

impl Server {
    /// `spawn_server_create(SPAWN_SERVER_OPTION_EXEC, name, NULL, argc, argv)` (`spawn_server_nofork.c:1433-1580`): a
    /// server listening on `<run dir>/netdata-spawn-<name>.sock` (or `-<pid>-<id>` unnamed), started from
    /// `start.exe` with this process's arguments, retitled as C's when `title` (C passes `argc`/`argv`).
    pub fn create(name: Option<&str>, title: bool, start: &Start) -> Option<Server> {
        let id = SERVER_ID.fetch_add(1, Ordering::Relaxed) + 1;
        let dir = runtime_directory(start);
        let name = name.filter(|n| !n.is_empty());
        let path = match name {
            Some(n) => format!("{dir}/netdata-spawn-{n}.sock"),
            None => format!("{dir}/netdata-spawn-{}-{id}.sock", std::process::id()),
        };
        let mut server = Server {
            name: name.unwrap_or("unnamed").to_string(),
            path: PathBuf::new(),
            magic: *uuid::Uuid::new_v4().as_bytes(),
            pid: None,
            status: None,
            request_id: AtomicUsize::new(0),
        };
        // C formats into a 1024-byte buffer
        if path.len() >= 1024 {
            nd_log!(Source::Collector, Priority::Err, errno = Errno::ENAMETOOLONG as i32;
                "SPAWN SERVER: socket path for '{}' in runtime directory '{dir}' was truncated (needed {} chars plus NUL, \
                 buffer is 1024 bytes)", server.name, path.len());
            return None;
        }
        if path.len() > MAX_SOCKET_PATH {
            nd_log!(Source::Collector, Priority::Err, errno = Errno::ENAMETOOLONG as i32;
                "SPAWN SERVER: socket path for '{}' in runtime directory '{dir}' exceeds the {MAX_SOCKET_PATH}-byte \
                 AF_UNIX limit", server.name);
            return None;
        }
        // from here a failure unlinks the path, as C's cleanup does (even another live server's)
        server.path = PathBuf::from(path);
        let listener = listening_socket(&server.path)?;
        let (status_read, status_write) = match pipe2(OFlag::O_CLOEXEC) {
            Ok(pipe) => pipe,
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = e as i32; "SPAWN SERVER: Cannot create status pipe()");
                return None;
            }
        };
        server.status = Some(File::from(status_read));
        let pid = match server.start(start, title, listener.as_fd(), status_write.as_fd()) {
            Ok(pid) => pid,
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = e as i32; "SPAWN SERVER: Cannot fork()");
                return None;
            }
        };
        server.pid = Some(pid);
        // the server holds its own copies now
        drop((listener, status_write));
        let report = match read_report(server.status.as_ref()?) {
            Ok(report) => report,
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = read_errno(&e);
                    "SPAWN SERVER: cannot read() initial status report from spawn server");
                return None;
            }
        };
        if report.status != wire::Status::Started as i32 {
            nd_log!(Source::Collector, Priority::Err, "SPAWN SERVER: server did not respond with success.");
            return None;
        }
        if report.value != pid.as_raw() {
            nd_log!(Source::Collector, Priority::Err,
                "SPAWN SERVER: server sent pid {} but we have created {pid}.", report.value);
            return None;
        }
        nd_log!(Source::Collector, Priority::Debug, "SPAWN SERVER: server created on pid {pid}");
        Some(server)
    }

    /// Starts the server process and hands it its bootstrap with the two descriptors (D137): stdin a socket, stderr the
    /// collectors' log when it is a file.
    fn start(&self, start: &Start, title: bool, listener: BorrowedFd<'_>, status: BorrowedFd<'_>) -> Result<Pid, Errno> {
        let (ours, theirs) = socketpair(AddressFamily::Unix, SockType::Stream, None, SockFlag::SOCK_CLOEXEC)?;
        let args: Vec<OsString> = std::env::args_os().collect();
        let args = if title {
            retitled(&crate::server::title(&self.name), &args)
        } else {
            args.into_iter().map(OsString::into_vec).collect()
        };
        let mut environment = exec_time_environment().to_vec();
        environment.push(format!("{MARKER}=1").into_bytes());
        let (Some(args), Some(environment), Ok(exe)) =
            (c_strings(args), c_strings(environment), CString::new(start.exe.as_os_str().as_bytes()))
        else {
            return Err(Errno::EINVAL);
        };
        let collectors = netdata_agent_log::collectors_fd();
        let mut actions = PosixSpawnFileActions::init()?;
        actions.add_dup2(theirs.as_raw_fd(), 0)?;
        if let Some(fd) = &collectors {
            actions.add_dup2(fd.as_raw_fd(), 2)?;
        }
        let pid = posix_spawn(exe.as_c_str(), &actions, &PosixSpawnAttr::init()?, &args, &environment)?;
        drop((theirs, collectors));
        let boot = Bootstrap {
            magic: self.magic,
            name: self.name.clone(),
            path: self.path.clone(),
            env: env::block().into_iter().map(CString::into_bytes).collect(),
        };
        // a server that is already gone fails its handshake instead
        let ours = UnixStream::from(ours);
        send_fully(&ours, &boot.encode(), &[listener.as_raw_fd(), status.as_raw_fd()], None);
        Ok(pid)
    }

    /// `spawn_server_pid()`.
    pub fn pid(&self) -> i32 {
        self.pid.map_or(0, Pid::as_raw)
    }

    /// The listening socket's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `spawn_server_exec(server, stderr, custom, argv, NULL, 0, SPAWN_INSTANCE_TYPE_EXEC)`
    /// (`spawn_server_nofork.c:1760-1859`): the child running `argv` with this process's environment, its stdin and
    /// stdout on new pipes, `stderr` as its stderr; `custom` goes to the server, which closes it.
    pub fn exec<S: AsRef<[u8]>>(&self, stderr: BorrowedFd<'_>, custom: BorrowedFd<'_>, argv: &[S]) -> Option<Instance> {
        let argv: Vec<&[u8]> = argv.iter().map(AsRef::as_ref).collect();
        let cmdline = wire::cmdline(&argv);
        let sock = connect_to(&self.path, true)?;
        let (child_stdin, write) = match pipe2(OFlag::O_CLOEXEC) {
            Ok(pipe) => pipe,
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = e as i32; "SPAWN PARENT: Cannot create stdin pipe()");
                return None;
            }
        };
        let (read, child_stdout) = match pipe2(OFlag::O_CLOEXEC) {
            Ok(pipe) => pipe,
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = e as i32; "SPAWN PARENT: Cannot create stdout pipe()");
                return None;
            }
        };
        let request_id = self.request_id.fetch_add(1, Ordering::Relaxed) + 1;
        let env = wire::encode_list(&env::block().iter().map(|e| e.as_bytes()).collect::<Vec<_>>());
        let args = wire::encode_list(&argv);
        let header = wire::Header {
            msg_type: wire::MSG_REQUEST,
            magic: self.magic,
            request_id,
            env_size: env.len(),
            argv_size: args.len(),
            data_size: 0,
            instance_type: wire::TYPE_EXEC,
        };
        let request = [header.encode(), env, args].concat();
        let fds = [child_stdin.as_raw_fd(), child_stdout.as_raw_fd(), stderr.as_raw_fd(), custom.as_raw_fd()];
        if !send_fully(&sock, &request, &fds, Some("request to spawn server")) {
            return None;
        }
        drop((child_stdin, child_stdout));
        let mut instance = Instance {
            request_id,
            sock,
            write: Some(File::from(write)),
            read: Some(File::from(read)),
            pid: 0,
            cmdline,
        };
        let report = match read_report(&instance.sock) {
            Ok(report) => report,
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = read_errno(&e);
                    "SPAWN PARENT: Failed to exec spawn request {request_id} (cannot get initial status report)");
                return None;
            }
        };
        if report.magic != wire::REPORT_MAGIC {
            nd_log!(Source::Collector, Priority::Err,
                "SPAWN PARENT: Failed to exec spawn request {request_id} (invalid magic {} in response)",
                alt_hex(report.magic));
            return None;
        }
        match report.status {
            s if s == wire::Status::Started as i32 => {
                instance.pid = report.value;
                return Some(instance);
            }
            s if s == wire::Status::Failed as i32 => {
                nd_log!(Source::Collector, Priority::Err, errno = report.value;
                    "SPAWN PARENT: Failed to exec spawn request {request_id} (server reports failure, errno is updated)");
            }
            s if s == wire::Status::Exited as i32 => {
                nd_log!(Source::Collector, Priority::Err, errno = Errno::ENOEXEC as i32;
                    "SPAWN PARENT: Failed to exec spawn request {request_id} (server reports exit, errno is updated)");
            }
            _ => {
                nd_log!(Source::Collector, Priority::Err,
                    "SPAWN PARENT: Invalid status report to exec spawn request {request_id} (received invalid data)");
            }
        }
        None
    }

    /// `spawn_server_destroy()`: the same as dropping it.
    pub fn destroy(self) {}
}

impl Drop for Server {
    /// `spawn_server_destroy()` (`spawn_server_nofork.c:1346-1363`): closes the status pipe, stops the server with
    /// SIGTERM and reaps it, and unlinks the socket path.
    fn drop(&mut self) {
        self.status.take();
        if let Some(pid) = self.pid.take() {
            let _ = kill(pid, Signal::SIGTERM);
            let _ = waitpid(pid, None);
        }
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// How a timed wait ended (`SPAWN_TIMEDWAIT_RESULT`), with the instance when the caller keeps it.
#[derive(Debug)]
pub enum Waited<T> {
    /// The report came (the instance is gone): the raw wait status, or -1 when the report was unreadable.
    Exited(i32),
    /// No report within the timeout, or the wait was cancelled; not proof that the child lives.
    Running(T),
    /// The report channel failed (the server died): the caller reclaims the instance, typically with `kill`.
    Error(T),
}

/// What [`Instance`] waits learn from the report socket.
enum Ready {
    Yes,
    /// Not yet, with the errno C's wait leaves (ETIMEDOUT or ECANCELED).
    No(i32),
    Broken,
}

/// A child the server started (`SPAWN_INSTANCE`): its pipes and the socket its EXITED report comes on. Dropping it
/// unwaited sends the child SIGTERM (`spawn_server_exec_destroy()`).
#[derive(Debug)]
#[must_use = "a child must be waited for or killed"]
pub struct Instance {
    request_id: usize,
    sock: UnixStream,
    /// The child's stdin.
    write: Option<File>,
    /// The child's stdout.
    read: Option<File>,
    /// 0 once the child is known to be gone (or given up on): nothing to signal.
    pid: i32,
    cmdline: String,
}

impl Instance {
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// The child's stdin, until a wait closes it.
    pub fn stdin(&mut self) -> Option<&mut File> {
        self.write.as_mut()
    }

    /// The child's stdout, until a wait closes it.
    pub fn stdout(&mut self) -> Option<&mut File> {
        self.read.as_mut()
    }

    fn close_pipes(&mut self) {
        self.write.take();
        self.read.take();
    }

    /// `spawn_server_log_kill_failure()`.
    fn signal(&self, signal: Signal) {
        if let Err(e) = kill(Pid::from_raw(self.pid), signal) {
            let priority = if e == Errno::ESRCH { Priority::Debug } else { Priority::Err };
            let uid = if e == Errno::EPERM {
                " - the child runs with a uid we cannot signal, so it will not be terminated"
            } else {
                ""
            };
            nd_log!(Source::Collector, priority, errno = e as i32;
                "SPAWN PARENT: cannot send signal {} to child pid {} (request {}): {}{uid}: {}",
                signal as i32, self.pid, self.request_id, strerror(e as i32), self.cmdline);
        }
    }

    /// Waits up to `timeout_ms` for the report to be readable (`wait_on_socket_or_cancel_with_timeout()`).
    fn ready(&mut self, timeout_ms: i32, cancelled: &dyn Fn() -> bool) -> Ready {
        let waited = netdata_agent_sys::wait_fd(self.sock.as_fd(), i64::from(timeout_ms), PollFlags::POLLIN, cancelled);
        match (waited.rc, waited.errno) {
            (0, _) => Ready::Yes,
            (2, errno) => {
                nd_log!(Source::Collector, Priority::Err, errno = errno;
                    "SPAWN PARENT: status socket error for request No {}, pid {}", self.request_id, self.pid);
                Ready::Broken
            }
            (_, errno) => Ready::No(errno),
        }
    }

    /// `spawn_server_exec_wait()` (`spawn_server_nofork.c:1659-1696`): closes the pipes (so a child reading stdin
    /// ends) and blocks for the report: the raw wait status, or -1 with C's record when the report is missing or
    /// wrong.
    pub fn wait(mut self) -> i32 {
        self.close_pipes();
        let mut rc = -1;
        match read_report(&self.sock) {
            Err(e) => {
                nd_log!(Source::Collector, Priority::Err, errno = read_errno(&e);
                    "SPAWN PARENT: failed to read final status report for child {}, request {}",
                    self.pid, self.request_id);
            }
            Ok(report) if report.magic != wire::REPORT_MAGIC => {
                // the report's bytes as C prints them: control and non-printable characters as '_'
                let reads_like: String = report
                    .encode()
                    .iter()
                    .map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '_' })
                    .collect();
                nd_log!(Source::Collector, Priority::Err,
                    "SPAWN PARENT: invalid final status report for child {}, request {} (invalid magic {} in \
                     response, reads like '{reads_like}')", self.pid, self.request_id, alt_hex(report.magic));
            }
            Ok(report) if report.status == wire::Status::Exited as i32 => rc = report.value,
            Ok(report) => {
                nd_log!(Source::Collector, Priority::Err,
                    "SPAWN PARENT: invalid status report to exec spawn request {} for pid {} (status = {})",
                    self.request_id, self.pid, report.status as u32);
            }
        }
        self.pid = 0;
        rc
    }

    /// `spawn_server_exec_timedwait()` (`spawn_server_nofork.c:1619-1657`): closes the pipes and waits up to
    /// `timeout_ms` (at least 1) for the report; `cancelled` is asked every 100 ms.
    pub fn timedwait(mut self, timeout_ms: i32, cancelled: &dyn Fn() -> bool) -> Waited<Instance> {
        self.close_pipes();
        match self.ready(timeout_ms.max(1), cancelled) {
            Ready::Yes => Waited::Exited(self.wait()),
            Ready::No(_) => Waited::Running(self),
            Ready::Broken => Waited::Error(self),
        }
    }

    /// `spawn_server_exec_kill()` (`spawn_server_nofork.c:1704-1750`): closes the pipes, gives the child `timeout_ms`
    /// to end by itself, then SIGTERM and up to 2 s, then SIGKILL and up to 2 s; the raw wait status, or -1 when it
    /// gives up (the child left running, the instance reclaimed). The pid may have been reused meanwhile: C's
    /// accepted race.
    pub fn kill(mut self, timeout_ms: i32, cancelled: &dyn Fn() -> bool) -> i32 {
        self.close_pipes();
        // the pre-kill grace, silent whatever it finds (C ignores its result)
        if timeout_ms > 0 {
            let _ = netdata_agent_sys::wait_fd(self.sock.as_fd(), i64::from(timeout_ms), PollFlags::POLLIN, cancelled);
        }
        if self.pid == 0 {
            return self.wait();
        }
        let mut errno = 0;
        for signal in [Signal::SIGTERM, Signal::SIGKILL] {
            self.signal(signal);
            match self.ready(wire::KILL_DEFAULT_GRACE_MS, cancelled) {
                Ready::Yes => return self.wait(),
                Ready::No(e) => errno = e,
                // the record cleared it
                Ready::Broken => errno = 0,
            }
        }
        nd_log!(Source::Collector, Priority::Err, errno = errno;
            "SPAWN PARENT: giving up waiting for pid {} after SIGKILL (request No {}) - reclaiming, the child is left \
             running: {}", self.pid, self.request_id, self.cmdline);
        self.pid = 0;
        -1
    }
}

impl Drop for Instance {
    /// `spawn_server_exec_destroy()`: SIGTERM to a child still recorded.
    fn drop(&mut self) {
        if self.pid != 0 {
            self.signal(Signal::SIGTERM);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_replaces_the_arguments_as_c() {
        let argv: Vec<OsString> = ["/usr/sbin/netdata", "-D", "", "-W", "x y"].iter().map(Into::into).collect();
        let got = retitled("spawn-plugins", &argv);
        let want: Vec<Vec<u8>> =
            vec![b"spawn-plugins    ".to_vec(), b"  ".to_vec(), vec![], b"  ".to_vec(), b"   ".to_vec()];
        assert_eq!(got, want);
        // a shorter first argument cuts the title
        assert_eq!(retitled("spawn-plugins", &["nd".into()]), vec![b"sp".to_vec()]);
    }

    #[test]
    fn a_bootstrap_round_trips() {
        let boot = Bootstrap {
            magic: *uuid::Uuid::new_v4().as_bytes(),
            name: "plugins".into(),
            path: "/run/netdata/netdata-spawn-plugins.sock".into(),
            env: vec![b"A=1".to_vec(), b"B=x\ny".to_vec()],
        };
        assert_eq!(Bootstrap::decode(&boot.encode()), Some(boot.clone()));
        assert_eq!(Bootstrap::decode(b"other\0\0"), None);
        assert_eq!(crate::server::title("a-name-longer-than-fifteen"), "spawn-a-name-lo");
        assert_eq!(alt_hex(0), "0");
        assert_eq!(alt_hex(0xbada55ee), "0xbada55ee");
    }
}
