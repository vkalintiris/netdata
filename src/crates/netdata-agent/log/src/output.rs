//! Where records go: the per-source state (`struct nd_log`), output selection (`nd_logger_select_output()`), opening
//! and reopening (`nd_log_open()`, `nd_log_initialize()` in `nd_log-init.c`), the journal's direct socket
//! (`nd_log-to-systemd-journal.c`, `systemd-journal-helpers.c`) and syslog (`nd_log-to-syslog.c`).

use std::fs::{File, OpenOptions};
use std::io::IoSlice;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use crate::limit::Limits;
use crate::model::{Format, Method, Priority, SOURCES, Source};

/// A source's descriptor (`fd` and `fp` in `struct nd_log_source`).
#[derive(Debug, Clone)]
pub(crate) enum Fd {
    /// -1
    Unset,
    Stdout,
    Stderr,
    File(Arc<File>),
}

impl Fd {
    fn number(&self) -> i32 {
        match self {
            Fd::Unset => -1,
            Fd::Stdout => 1,
            Fd::Stderr => 2,
            Fd::File(file) => file.as_raw_fd(),
        }
    }
}

/// `struct nd_log_source` without the limits, which have their own lock.
#[derive(Debug, Clone)]
pub(crate) struct SourceState {
    pub(crate) method: Method,
    pub(crate) format: Format,
    pub(crate) filename: Option<String>,
    pub(crate) fd: Fd,
    pub(crate) min_priority: Priority,
}

/// The direct journal socket (`nd_log.journal_direct`), kept for the process lifetime once found.
struct Journal {
    socket: UnixDatagram,
    filename: String,
}

enum SyslogSocket {
    Dgram(UnixDatagram),
    Stream(UnixStream),
}

/// `openlog(program_name, LOG_PID, facility)` state: glibc connects at the first message.
struct Syslog {
    facility: i32,
    socket: Option<SyslogSocket>,
}

pub(crate) struct Globals {
    pub(crate) sources: RwLock<[SourceState; SOURCES]>,
    /// The sources' minimum priorities, read without the lock by every log call.
    min_priority: [AtomicU8; SOURCES],
    pub(crate) limits: [Mutex<Limits>; SOURCES],
    /// Write serialization: per source for its own file, shared for the std streams.
    writers: [Mutex<()>; SOURCES],
    stdout: Mutex<()>,
    stderr: Mutex<()>,
    /// `nd_log.std_output.initialized` / `std_error.initialized`: fd 1 / fd 2 was replaced by a log file.
    std_output_initialized: AtomicBool,
    std_error_initialized: AtomicBool,
    journal: RwLock<Option<Journal>>,
    syslog: Mutex<Option<Syslog>>,
    /// `nd_log.syslog.facility`.
    pub(crate) facility: std::sync::atomic::AtomicI32,
    pub(crate) host_prefix: RwLock<Option<String>>,
}

fn source_defaults(log_dir: &str) -> [SourceState; SOURCES] {
    let s = |method, format, file: Option<&str>, fd, min_priority| SourceState {
        method,
        format,
        filename: file.map(|f| format!("{log_dir}/{f}")),
        fd,
        min_priority,
    };
    // The static `nd_log` initializer (release build: collector owns fd 2, debug owns fd 1).
    [
        SourceState {
            method: Method::Disabled,
            format: Format::Journal,
            filename: None,
            fd: Fd::Unset,
            min_priority: Priority::Emerg,
        },
        s(
            Method::Default,
            Format::Logfmt,
            Some("access.log"),
            Fd::Unset,
            Priority::Debug,
        ),
        s(
            Method::File,
            Format::Logfmt,
            Some("aclk.log"),
            Fd::Unset,
            Priority::Debug,
        ),
        s(
            Method::Default,
            Format::Logfmt,
            Some("collector.log"),
            Fd::Stderr,
            Priority::Info,
        ),
        s(
            Method::Default,
            Format::Logfmt,
            Some("daemon.log"),
            Fd::Unset,
            Priority::Info,
        ),
        s(
            Method::Default,
            Format::Logfmt,
            Some("health.log"),
            Fd::Unset,
            Priority::Debug,
        ),
        s(
            Method::Disabled,
            Format::Logfmt,
            Some("debug.log"),
            Fd::Stdout,
            Priority::Debug,
        ),
    ]
}

pub(crate) static G: LazyLock<Globals> = LazyLock::new(|| {
    let sources = source_defaults("/var/log/netdata");
    let limits = |source: Source| {
        Mutex::new(match source {
            Source::Collector | Source::Daemon => Limits::default_limits(),
            _ => Limits::unlimited(),
        })
    };
    Globals {
        min_priority: std::array::from_fn(|i| AtomicU8::new(sources[i].min_priority as u8)),
        sources: RwLock::new(sources),
        limits: Source::ALL.map(limits),
        writers: std::array::from_fn(|_| Mutex::new(())),
        stdout: Mutex::new(()),
        stderr: Mutex::new(()),
        std_output_initialized: AtomicBool::new(false),
        std_error_initialized: AtomicBool::new(false),
        journal: RwLock::new(None),
        syslog: Mutex::new(None),
        facility: std::sync::atomic::AtomicI32::new(crate::model::FACILITY_DAEMON),
        host_prefix: RwLock::new(None),
    }
});

/// Locks that are only held for plain assignments or writes, so a poisoned one is still consistent.
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn read<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn write<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

/// The compiled `LOG_DIR` of the default file names (`LOG_DIR "/daemon.log"`, ...), before any configuration.
pub fn set_default_log_dir(log_dir: &str) {
    let defaults = source_defaults(log_dir);
    let mut sources = write(&G.sources);
    for (state, default) in sources.iter_mut().zip(defaults) {
        state.filename = default.filename;
    }
}

pub(crate) fn sync_priorities(sources: &[SourceState; SOURCES]) {
    for (atomic, state) in G.min_priority.iter().zip(sources) {
        atomic.store(state.min_priority as u8, Ordering::Relaxed);
    }
}

/// `source != NDLS_DEBUG && priority > min_priority`.
pub(crate) fn filtered(source: Source, priority: Priority) -> bool {
    source != Source::Debug
        && priority as u8 > G.min_priority[source as usize].load(Ordering::Relaxed)
}

/// What a record is written to.
pub(crate) enum Target {
    Disabled,
    Journal,
    Syslog,
    Fd { fd: Fd, lock: Lock },
}

#[derive(Clone, Copy)]
pub(crate) enum Lock {
    Source(usize),
    Stdout,
    Stderr,
}

const STDERR: Target = Target::Fd {
    fd: Fd::Stderr,
    lock: Lock::Stderr,
};

/// `nd_logger_select_output()`: the target and the source's format. Anything that cannot be written to falls back to
/// stderr.
pub(crate) fn select(source: Source) -> (Target, Format) {
    let sources = read(&G.sources);
    let e = &sources[source as usize];
    let target = match e.method {
        Method::Journal if read(&G.journal).is_none() => STDERR,
        Method::Journal => Target::Journal,
        Method::Syslog if lock(&G.syslog).is_none() => STDERR,
        Method::Syslog => Target::Syslog,
        Method::File => match &e.fd {
            Fd::Unset => STDERR,
            fd => Target::Fd {
                fd: fd.clone(),
                lock: Lock::Source(source as usize),
            },
        },
        Method::Stdout => Target::Fd {
            fd: Fd::Stdout,
            lock: Lock::Stdout,
        },
        Method::Default | Method::Stderr => STDERR,
        Method::Disabled | Method::DevNull => Target::Disabled,
    };
    (target, e.format)
}

fn borrowed(fd: &Fd) -> Option<BorrowedFd<'_>> {
    match fd {
        Fd::Unset => None,
        Fd::Stdout => Some(std_fd(1)),
        Fd::Stderr => Some(std_fd(2)),
        Fd::File(file) => Some(file.as_fd()),
    }
}

static STDOUT: LazyLock<std::io::Stdout> = LazyLock::new(std::io::stdout);
static STDERR_HANDLE: LazyLock<std::io::Stderr> = LazyLock::new(std::io::stderr);

/// fd 1 and fd 2 as the std streams borrow them; std never closes them.
fn std_fd(n: i32) -> BorrowedFd<'static> {
    if n == 1 {
        STDOUT.as_fd()
    } else {
        STDERR_HANDLE.as_fd()
    }
}

/// `nd_logger_file()`'s write loop: raw `write(2)` calls under the target's lock; an error other than `EINTR`, or a
/// write of 0 bytes, loses the rest of the record. False when not everything was written.
pub(crate) fn write_fd(fd: &Fd, lock_of: Lock, mut bytes: &[u8]) -> bool {
    let Some(fd) = borrowed(fd) else {
        return false;
    };
    let _guard = match lock_of {
        Lock::Source(i) => lock(&G.writers[i]),
        Lock::Stdout => lock(&G.stdout),
        Lock::Stderr => lock(&G.stderr),
    };
    while !bytes.is_empty() {
        match nix::unistd::write(fd, bytes) {
            Ok(0) => break,
            Ok(n) => bytes = &bytes[n..],
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => break,
        }
    }
    bytes.is_empty()
}

pub(crate) fn write_stderr(bytes: &[u8]) -> bool {
    write_fd(&Fd::Stderr, Lock::Stderr, bytes)
}

/// A single raw `write()` to fd 2 without the stderr lock: the fatal paths cannot wait for a writer that may never
/// finish.
pub(crate) fn write_stderr_raw(bytes: &[u8]) {
    let _ = nix::unistd::write(std_fd(2), bytes);
}

// ------------------------------------------------------------------------------------------------------------------
// journal

/// `is_path_unix_socket()`.
fn is_unix_socket(path: &str) -> bool {
    !path.is_empty() && std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

/// `journal_direct_fd()`: a datagram socket connected to `path`.
fn journal_connect(path: &str) -> Option<UnixDatagram> {
    if !is_unix_socket(path) {
        return None;
    }
    let socket = UnixDatagram::unbound().ok()?;
    socket.connect(path).ok()?;
    Some(socket)
}

/// `nd_log_journal_direct_fd_find_and_open()`: the host prefix's netdata namespace and default socket, then the
/// host's.
fn journal_find_and_open() -> Option<(UnixDatagram, String)> {
    let prefix = read(&G.host_prefix).clone().filter(|p| !p.is_empty());
    let mut candidates = Vec::new();
    if let Some(prefix) = &prefix {
        candidates.push(format!("{prefix}/run/systemd/journal.netdata/socket"));
        candidates.push(format!("{prefix}/run/systemd/journal/socket"));
    }
    candidates.push("/run/systemd/journal.netdata/socket".to_string());
    candidates.push("/run/systemd/journal/socket".to_string());
    candidates
        .into_iter()
        .find_map(|path| journal_connect(&path).map(|socket| (socket, path)))
}

/// `nd_log_journal_direct_set_env()`: plugins inherit the socket when the collector source logs to the journal.
fn journal_set_env(collector_journal: bool, filename: &str) {
    if collector_journal {
        // Refused once threads run (a reopen); the value is the one exported at startup.
        let _ = netdata_agent_sys::setenv("NETDATA_SYSTEMD_JOURNAL_PATH", filename);
    }
}

/// `nd_log_journal_direct_init()`.
pub(crate) fn journal_direct_init(path: Option<&str>) -> bool {
    // read before the journal lock: `select()` takes the sources lock first
    let collector_journal = read(&G.sources)[Source::Collector as usize].method == Method::Journal;
    if let Some(journal) = read(&G.journal).as_ref() {
        journal_set_env(collector_journal, &journal.filename);
        return true;
    }
    let found = match path.filter(|p| is_unix_socket(p)) {
        Some(path) => journal_connect(path).map(|socket| (socket, path.to_string())),
        None => journal_find_and_open(),
    };
    let Some((socket, filename)) = found else {
        return false;
    };
    journal_set_env(collector_journal, &filename);
    *write(&G.journal) = Some(Journal { socket, filename });
    true
}

/// `journal_direct_send()`: one datagram; one too large for a datagram goes as a sealed memfd.
pub(crate) fn journal_send(bytes: &[u8]) -> bool {
    let journal = read(&G.journal);
    let Some(journal) = journal.as_ref() else {
        return false;
    };
    match journal.socket.send(bytes) {
        Ok(_) => true,
        Err(err) if err.raw_os_error() == Some(nix::libc::EMSGSIZE) => {
            journal_send_with_memfd(&journal.socket, bytes)
        }
        // The socket's peer was re-created. C then logs through libsystemd, which sends each datagram to the
        // default socket's path on an unconnected socket, and so recovers.
        Err(err)
            if matches!(
                err.raw_os_error(),
                Some(nix::libc::ECONNREFUSED | nix::libc::ENOTCONN)
            ) =>
        {
            UnixDatagram::unbound()
                .and_then(|s| s.send_to(bytes, "/run/systemd/journal/socket"))
                .is_ok()
        }
        Err(_) => false,
    }
}

fn journal_send_with_memfd(socket: &UnixDatagram, bytes: &[u8]) -> bool {
    use nix::fcntl::{FcntlArg, SealFlag, fcntl};
    use nix::sys::memfd::{MFdFlags, memfd_create};
    use nix::sys::socket::{ControlMessage, MsgFlags, UnixAddr, sendmsg};

    let Ok(memfd) = memfd_create(c"journald", MFdFlags::MFD_ALLOW_SEALING) else {
        return false;
    };
    let mut file = File::from(memfd);
    if std::io::Write::write_all(&mut file, bytes).is_err() {
        return false;
    }
    let seals = SealFlag::F_SEAL_SHRINK | SealFlag::F_SEAL_GROW | SealFlag::F_SEAL_WRITE;
    if fcntl(&file, FcntlArg::F_ADD_SEALS(seals)).is_err() {
        return false;
    }
    let fds = [file.as_raw_fd()];
    let iov = [IoSlice::new(&[])];
    sendmsg::<UnixAddr>(
        socket.as_raw_fd(),
        &iov,
        &[ControlMessage::ScmRights(&fds)],
        MsgFlags::empty(),
        None,
    )
    .is_ok()
}

/// `nd_log_collectors_fd()`: where children write their stderr, the collectors' log file when collectors log to a
/// file, else stderr. `None` for stderr itself; otherwise a duplicate that stays open while the caller holds it, even
/// if the log reopens meanwhile.
pub fn collectors_fd() -> Option<std::os::fd::OwnedFd> {
    let sources = read(&G.sources);
    let e = &sources[Source::Collector as usize];
    match &e.fd {
        Fd::File(file) if e.method == Method::File => file.try_clone().ok().map(Into::into),
        Fd::Stdout if e.method == Method::File => std_fd(1).try_clone_to_owned().ok(),
        _ => None,
    }
}

/// `is_stderr_connected_to_journal()`: `JOURNAL_STREAM` is `<dev>:<ino>` of fd 2.
pub fn is_stderr_connected_to_journal() -> bool {
    let Some(stream) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Ok(stat) = nix::sys::stat::fstat(std_fd(2)) else {
        return false;
    };
    let bytes = stream.as_encoded_bytes();
    // strtol(): whitespace, sign, digits
    let (dev, used) = netdata_agent_text::parse::str2ll(bytes);
    if bytes.get(used) != Some(&b':') {
        return false;
    }
    let (ino, _) = netdata_agent_text::parse::str2ll(&bytes[used + 1..]);
    stat.st_dev == dev as u64 && stat.st_ino == ino as u64
}

// ------------------------------------------------------------------------------------------------------------------
// syslog

/// `nd_log_init_syslog()`: `openlog()` once, with the facility of that moment.
pub(crate) fn syslog_init() {
    let mut syslog = lock(&G.syslog);
    if syslog.is_none() {
        *syslog = Some(Syslog {
            facility: G.facility.load(Ordering::Relaxed),
            socket: None,
        });
    }
}

/// `syslog(priority, "%s", line)` as glibc and musl send it to `/dev/log`: `<PRI>Mmm dd hh:mm:ss ident[pid]: line`.
pub(crate) fn syslog_send(priority: Priority, line: &[u8], ident: &str) {
    let mut guard = lock(&G.syslog);
    let Some(syslog) = guard.as_mut() else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let stamp = netdata_agent_sys::localtime(now).map_or_else(String::new, |tm| {
        format!(
            "{} {:>2} {:02}:{:02}:{:02}",
            netdata_agent_text::datetime::MONTHS
                .get(tm.month0 as usize)
                .copied()
                .unwrap_or("Jan"),
            tm.mday,
            tm.hour,
            tm.min,
            tm.sec
        )
    });
    let mut message = format!(
        "<{}>{stamp} {ident}[{}]: ",
        priority as i32 | syslog.facility,
        std::process::id()
    )
    .into_bytes();
    message.extend_from_slice(line);

    let send = |socket: &mut SyslogSocket| match socket {
        SyslogSocket::Dgram(s) => s.send(&message).is_ok(),
        SyslogSocket::Stream(s) => {
            // stream sockets get the terminating NUL too
            let mut framed = message.clone();
            framed.push(0);
            std::io::Write::write_all(s, &framed).is_ok()
        }
    };
    let connect = || {
        UnixDatagram::unbound()
            .and_then(|s| s.connect("/dev/log").map(|()| SyslogSocket::Dgram(s)))
            .or_else(|_| UnixStream::connect("/dev/log").map(SyslogSocket::Stream))
            .ok()
    };
    if syslog.socket.is_none() {
        syslog.socket = connect();
    }
    let sent = syslog.socket.as_mut().is_some_and(&send);
    if !sent {
        // glibc reconnects once, then gives up until the next message
        syslog.socket = connect();
        if !syslog.socket.as_mut().is_some_and(&send) {
            syslog.socket = None;
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------
// opening

/// Messages `nd_log_open()` logs (daemon, error) with the errno of the failed call.
pub(crate) type OpenErrors = Vec<(String, i32)>;

type Dup2 = fn(&File) -> nix::Result<()>;

/// `nd_log_replace_existing_fd()`: moves a newly opened file onto the source's descriptor. Collector's fd 2 and
/// debug's fd 1 are replaced only once per process; the file is handed back when it was not used.
fn replace_existing_fd(
    e: &mut SourceState,
    file: File,
    errors: &mut OpenErrors,
) -> Result<(), File> {
    let (dup2, initialized, old): (Dup2, &AtomicBool, i32) = match &e.fd {
        Fd::Unset => return Err(file),
        Fd::Stdout => (
            |f| nix::unistd::dup2_stdout(f),
            &G.std_output_initialized,
            1,
        ),
        Fd::Stderr => (|f| nix::unistd::dup2_stderr(f), &G.std_error_initialized, 2),
        Fd::File(_) => {
            // C dup2()s onto the old number; a new descriptor is the same file for every writer
            e.fd = Fd::File(Arc::new(file));
            return Ok(());
        }
    };
    if initialized.load(Ordering::Relaxed) {
        return Err(file);
    }
    let result = dup2(&file);
    initialized.store(true, Ordering::Relaxed);
    match result {
        Ok(()) => Ok(()),
        Err(errno) => {
            errors.push((
                format!(
                    "Cannot dup2() new fd {} to old fd {old} for '{}'",
                    file.as_raw_fd(),
                    e.filename.as_deref().unwrap_or("")
                ),
                errno as i32,
            ));
            Err(file)
        }
    }
}

/// What `nd_log_open()` must do after the source lock is released.
pub(crate) enum AfterOpen {
    Nothing,
    Syslog,
    Journal,
}

/// The file part of `nd_log_open()`, for the `file` and `/dev/null` methods.
pub(crate) fn open_file(e: &mut SourceState, errors: &mut OpenErrors) {
    let filename = e.filename.clone().unwrap_or_default();
    match OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o664)
        .open(&filename)
    {
        Err(err) => {
            let errno = err.raw_os_error().unwrap_or(0);
            if matches!(e.fd, Fd::Stdout | Fd::Stderr) {
                errors.push((
                    format!(
                        "Cannot open log file '{filename}'. Leaving fd {} as-is.",
                        e.fd.number()
                    ),
                    errno,
                ));
            } else {
                e.fd = Fd::Stderr;
                e.method = Method::Stderr;
                errors.push((
                    format!("Cannot open log file '{filename}'. Falling back to stderr."),
                    errno,
                ));
            }
        }
        Ok(file) => {
            if let Err(file) = replace_existing_fd(e, file, errors) {
                match e.fd {
                    Fd::Stdout => e.method = Method::Stdout,
                    Fd::Stderr => e.method = Method::Stderr,
                    _ => e.fd = Fd::File(Arc::new(file)),
                }
            }
        }
    }
}

/// The part of `nd_log_open()` after the default method was resolved.
pub(crate) fn open_resolved(e: &mut SourceState, errors: &mut OpenErrors) -> AfterOpen {
    if (e.method == Method::File && e.filename.is_none())
        || (e.method == Method::DevNull && matches!(e.fd, Fd::Unset))
    {
        e.method = Method::Disabled;
    }
    match e.method {
        Method::Syslog => AfterOpen::Syslog,
        Method::Journal => AfterOpen::Journal,
        Method::Stdout => {
            e.fd = Fd::Stdout;
            AfterOpen::Nothing
        }
        Method::Disabled => AfterOpen::Nothing,
        Method::Default | Method::Stderr => {
            e.method = Method::Stderr;
            e.fd = Fd::Stderr;
            AfterOpen::Nothing
        }
        Method::DevNull | Method::File => {
            open_file(e, errors);
            AfterOpen::Nothing
        }
    }
}

/// `nd_log_stdin_init(STDIN_FILENO, "/dev/null")`.
pub(crate) fn stdin_init() {
    if let Ok(file) = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o664)
        .open("/dev/null")
    {
        let _ = nix::unistd::dup2_stdin(&file);
    }
}

/// `chown_open_file()`: a regular file open on `fd` that another account owns goes to `uid:gid`.
pub(crate) fn chown_fd(fd: BorrowedFd<'_>, uid: u32, gid: u32, errors: &mut OpenErrors) {
    let number = fd.as_raw_fd();
    let meta = match nix::sys::stat::fstat(fd) {
        Ok(meta) => meta,
        Err(errno) => {
            errors.push((format!("Cannot fstat() fd {number}"), errno as i32));
            return;
        }
    };
    let regular = meta.st_mode & nix::libc::S_IFMT == nix::libc::S_IFREG;
    if (meta.st_uid != uid || meta.st_gid != gid) && regular {
        if let Err(errno) = nix::unistd::fchown(
            fd,
            Some(nix::unistd::Uid::from_raw(uid)),
            Some(nix::unistd::Gid::from_raw(gid)),
        ) {
            errors.push((format!("Cannot fchown() fd {number}."), errno as i32));
        }
    }
}

/// `nd_log_chown_log_files()`: every open log descriptor.
pub(crate) fn chown_log_files(uid: u32, gid: u32) -> OpenErrors {
    let fds: Vec<Fd> = read(&G.sources)
        .iter()
        .map(|s| s.fd.clone())
        .filter(|fd| !matches!(fd, Fd::Unset))
        .collect();
    let mut errors = OpenErrors::new();
    for fd in &fds {
        if let Some(borrowed) = borrowed(fd) {
            chown_fd(borrowed, uid, gid, &mut errors);
        }
    }
    errors
}
