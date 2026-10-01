//! The only agent crate that contains `unsafe` (decisions D12, D21): system calls no safe crate wraps, each behind a
//! safe function whose preconditions it checks itself.
//!
//! - `fork()` and `setenv()` are sound only while the process has one thread; both refuse otherwise.
//! - `gethostid()` and the scheduling calls (`nice`, `getpriority`, `sched_*`) pass plain integers and read nothing back through
//!   pointers except a stack `sched_param`.
//! - `localtime_r()` (decision D22.3) fills a stack `struct tm`; it reads `TZ`, which only `setenv()` changes, and that
//!   refuses once a second thread exists. Its `tm_zone` string is copied out immediately.
//! - `pthread_attr_init/getstacksize/destroy()` (decision D28) work on a stack attribute object, initialised before
//!   use and destroyed once.
//! - `mallopt()` (decision D28, glibc only) passes two integers; glibc serialises it against its own allocator.
//! - `malloc_trim()` (decision D84.1, glibc on Linux only) passes an integer; glibc locks every arena it trims.
//! - `setsockopt(TCP_DEFER_ACCEPT)` (decision D47) passes a stack `int` with its size on a borrowed descriptor.
//! - `close_range()`, `close()` and `fcntl(FD_CLOEXEC)` of descriptors (decisions D52, D136.4): closing runs only while
//!   the process has one thread, before it holds a descriptor it keeps; marking close-on-exec changes no ownership.
//! - `recvmsg()` with `SCM_RIGHTS` (decision D136.4) reads into the caller's slices and a control buffer it owns; every
//!   descriptor the kernel installed for the call is wrapped in an `OwnedFd` at once, so each is closed exactly once.
//! - `signal_flag()` (decision D136.4) calls `sigaction()` with a handler that only stores into a static atomic, which
//!   is async-signal-safe.
//! - `sqlite3_status64()` (decision D82.4) fills two stack integers; SQLite serialises it under its allocator's
//!   mutex once initialised (before that no other SQLite call runs).
//! - `sqlite3_recover_init/run/finish()` (decision D59.2) use a connection borrowed for the whole call; recover copies
//!   its string arguments at init and is freed by finish, both inside the call.
//! - `Alloc` (decisions D87 F5, D91.1), the process's `GlobalAlloc`, hands every call to `System` unchanged.
//! - `install_deadly()` and `die_by()` (decision D91.1) call `sigaction()`; the handler's trampoline reads the
//!   `siginfo_t` the kernel passes it. `default_dispositions()` (decision D123) calls `sigaction()` with the default
//!   action, only while the process has one thread and the signals are blocked.
//! - `exit_now()` (review R34) calls `_exit()` with an integer: no atexit handler or flush runs, as C's fatal paths
//!   want when another thread is already exiting.
//! - `SocketSsl` (decisions D97.2, D99.1): `SSL_set_fd()`, `SSL_accept/read/peek/write/shutdown()` and
//!   `SSL_get_error()` on a connection it owns, with buffers passed as slices and their lengths, over a socket it owns
//!   and never hands out mutably; the connection is freed before its socket.

mod alloc;
mod deadly;
pub mod exit;
mod tls;

pub use alloc::{Alloc, allocation_failed};
pub use deadly::{Deadly, die_by, install_deadly};
pub use tls::SocketSsl;
#[cfg(feature = "alloc-count")]
pub use alloc::allocations;

use std::io;

/// Scheduling policies (`SCHED_*`).
pub const SCHED_OTHER: i32 = libc::SCHED_OTHER;
pub const SCHED_BATCH: i32 = libc::SCHED_BATCH;
pub const SCHED_IDLE: i32 = libc::SCHED_IDLE;
pub const SCHED_FIFO: i32 = libc::SCHED_FIFO;
pub const SCHED_RR: i32 = libc::SCHED_RR;

/// How many threads this process has (`Threads:` of `/proc/self/status`).
pub fn thread_count() -> io::Result<usize> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("Threads:"))
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| io::Error::other("no Threads: line in /proc/self/status"))
}

fn require_single_thread(what: &str) -> io::Result<()> {
    match thread_count()? {
        1 => Ok(()),
        n => Err(io::Error::other(format!(
            "{what} needs a single-threaded process, this one has {n} threads"
        ))),
    }
}

/// What `fork()` returned in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forked {
    Parent { child: i32 },
    Child,
}

/// `fork()`, refused unless the process is single-threaded (so the child cannot inherit a lock another thread held).
pub fn fork() -> io::Result<Forked> {
    require_single_thread("fork()")?;
    // SAFETY: with one thread there is no other thread whose state (locks, allocator) the child could inherit
    // half-updated; the caller only continues or exits in either branch.
    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Parent { child }) => Ok(Forked::Parent {
            child: child.as_raw(),
        }),
        Ok(nix::unistd::ForkResult::Child) => Ok(Forked::Child),
        Err(e) => Err(io::Error::from(e)),
    }
}

/// `setenv()` for the environment the plugins inherit, refused unless the process is single-threaded.
pub fn setenv(key: &str, value: &str) -> io::Result<()> {
    require_single_thread("setenv()")?;
    // SAFETY: no other thread exists that could read the environment concurrently.
    unsafe { std::env::set_var(key, value) };
    Ok(())
}

/// The default action for each of `signals` (decision D123), as C's `sigaction()` handlers replace an ignore the
/// launcher left: refused unless the process is single-threaded and each signal is blocked on this thread, so none can
/// be delivered with its default action (which for most of them ends the process) until a thread waits for it. A
/// failure stops at its signal; the ones before it are reset.
pub fn default_dispositions(signals: &[nix::sys::signal::Signal]) -> io::Result<()> {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, sigaction};
    require_single_thread("default_dispositions()")?;
    let blocked = SigSet::thread_get_mask().map_err(io::Error::from)?;
    if let Some(open) = signals.iter().find(|s| !blocked.contains(**s)) {
        return Err(io::Error::other(format!("default_dispositions() needs {open} blocked")));
    }
    let action = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
    for &signal in signals {
        // SAFETY: the default action installs no handler, and the previous action returned is dropped unread.
        unsafe { sigaction(signal, &action) }.map_err(io::Error::from)?;
    }
    Ok(())
}

/// `PTHREAD_STACK_MIN`.
pub const PTHREAD_STACK_MIN: usize = libc::PTHREAD_STACK_MIN;

/// `netdata_threads_init()`: the stack size of a thread created with default attributes.
pub fn default_thread_stack_size() -> io::Result<usize> {
    let mut attr = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
    // SAFETY: pthread_attr_init() initialises the object it is given.
    let r = unsafe { libc::pthread_attr_init(attr.as_mut_ptr()) };
    if r != 0 {
        return Err(io::Error::from_raw_os_error(r));
    }
    let mut size: libc::size_t = 0;
    // SAFETY: the attribute object was initialised above and `size` is a live out location.
    let r = unsafe { libc::pthread_attr_getstacksize(attr.as_ptr(), &mut size) };
    // SAFETY: the attribute object was initialised above and is destroyed only here.
    unsafe { libc::pthread_attr_destroy(attr.as_mut_ptr()) };
    if r != 0 {
        return Err(io::Error::from_raw_os_error(r));
    }
    Ok(size)
}

/// `mallopt(M_ARENA_MAX)` and `mallopt(M_TRIM_THRESHOLD)`, as `netdata_conf_glibc_malloc_initialize()` sets them.
#[cfg(target_env = "gnu")]
pub fn mallopt_arenas(arenas: i32, trim_threshold: i32) {
    // SAFETY: integer arguments only; glibc locks its own state.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, arenas);
        libc::mallopt(libc::M_TRIM_THRESHOLD, trim_threshold);
    }
}

/// `mallocz_release_as_much_memory_to_the_system()`: `malloc_trim(0)`, skipped while another thread runs it.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn malloc_trim() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static TRIMMING: AtomicBool = AtomicBool::new(false);
    if TRIMMING.swap(true, Ordering::Acquire) {
        return;
    }
    // SAFETY: an integer argument only; glibc locks each arena as it trims it.
    unsafe {
        libc::malloc_trim(0);
    }
    TRIMMING.store(false, Ordering::Release);
}

/// Without glibc there is nothing to trim.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn malloc_trim() {}

/// `OS_SYSTEM_DISK_SPACE`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskSpace {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub read_only: bool,
}

/// `os_disk_space()`: the filesystem of `path` as `statvfs()` reports it (the bytes free to unprivileged users);
/// zeros when it cannot be read.
// `fsblkcnt_t`, `fsfilcnt_t` and `c_ulong` are 32-bit on the armv7l and i386 targets, where the conversions are not
// no-ops.
#[allow(clippy::useless_conversion)]
pub fn disk_space(path: &std::path::Path) -> DiskSpace {
    let Ok(s) = nix::sys::statvfs::statvfs(path) else {
        return DiskSpace::default();
    };
    let fragment = u64::from(s.fragment_size());
    DiskSpace {
        total_bytes: u64::from(s.blocks()).wrapping_mul(fragment),
        free_bytes: u64::from(s.blocks_available()).wrapping_mul(fragment),
        total_inodes: u64::from(s.files()),
        free_inodes: u64::from(s.files_available()),
        read_only: s.flags().contains(nix::sys::statvfs::FsFlags::ST_RDONLY),
    }
}

/// `waitpid(pid, &status, nohang ? WNOHANG : 0)`: the child reaped and the `int` status the kernel filled, or `None`
/// when `nohang` found none ended. nix decodes the status after the kernel reaped the child and has no signal for
/// 32-64, so a child killed by a real-time signal would come back as an error, its pid and status lost (R56 B1).
pub fn waitpid_raw(pid: i32, nohang: bool) -> io::Result<Option<(i32, i32)>> {
    let mut status: libc::c_int = 0;
    let flags = if nohang { libc::WNOHANG } else { 0 };
    // SAFETY: `status` is a local the call writes once; the other arguments are integers.
    match unsafe { libc::waitpid(pid, &raw mut status, flags) } {
        -1 => Err(io::Error::last_os_error()),
        0 => Ok(None),
        reaped => Ok(Some((reaped, status))),
    }
}

/// `now_monotonic_usec()`: `CLOCK_MONOTONIC` in microseconds (never 0 on a running system).
pub fn now_monotonic_usec() -> u64 {
    use nix::time::{ClockId, clock_gettime};
    clock_gettime(ClockId::CLOCK_MONOTONIC).map_or(0, |ts| ts.tv_sec() as u64 * 1_000_000 + ts.tv_nsec() as u64 / 1_000)
}

/// `_exit(code)`: the process ends now, without atexit handlers or buffer flushes.
pub fn exit_now(code: i32) -> ! {
    // SAFETY: `_exit()` takes an integer and never returns.
    unsafe { libc::_exit(code) }
}

/// `gethostid()`.
// `c_long` is 32-bit on the armv7l and i386 targets, where the conversion is not a no-op.
#[allow(clippy::useless_conversion)]
pub fn gethostid() -> i64 {
    // SAFETY: no arguments, no memory is shared.
    i64::from(unsafe { libc::gethostid() })
}

/// `nice(inc)`: the new nice value.
pub fn nice(inc: i32) -> io::Result<i32> {
    // nice() may legitimately return -1, so errno decides.
    nix::errno::Errno::clear();
    // SAFETY: an integer argument, no memory is shared.
    let n = unsafe { libc::nice(inc) };
    if n == -1 && nix::errno::Errno::last_raw() != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n)
}

/// `getpriority(PRIO_PROCESS, 0)`.
pub fn getpriority_self() -> io::Result<i32> {
    nix::errno::Errno::clear();
    // SAFETY: integer arguments, no memory is shared.
    let n = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
    if n == -1 && nix::errno::Errno::last_raw() != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n)
}

/// `sched_getscheduler(0)`.
pub fn sched_getscheduler() -> io::Result<i32> {
    // SAFETY: an integer argument, no memory is shared.
    let policy = unsafe { libc::sched_getscheduler(0) };
    if policy == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(policy)
}

/// `sched_getparam(0, &param)`: the priority.
pub fn sched_getparam_priority() -> io::Result<i32> {
    let mut param = libc::sched_param { sched_priority: 0 };
    // SAFETY: `param` is a valid, exclusively borrowed `sched_param` for the duration of the call.
    if unsafe { libc::sched_getparam(0, &mut param) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(param.sched_priority)
}

/// `sched_setscheduler(0, policy, {priority})`.
pub fn sched_setscheduler(policy: i32, priority: i32) -> io::Result<()> {
    let param = libc::sched_param {
        sched_priority: priority,
    };
    // SAFETY: `param` is a valid `sched_param` that outlives the call; the kernel only reads it.
    if unsafe { libc::sched_setscheduler(0, policy, &param) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `sched_get_priority_min(policy)` (-1 on error, as C compares it).
pub fn sched_get_priority_min(policy: i32) -> i32 {
    // SAFETY: an integer argument, no memory is shared.
    unsafe { libc::sched_get_priority_min(policy) }
}

/// `sched_get_priority_max(policy)` (-1 on error, as C compares it).
pub fn sched_get_priority_max(policy: i32) -> i32 {
    // SAFETY: an integer argument, no memory is shared.
    unsafe { libc::sched_get_priority_max(policy) }
}

/// A broken-down local time (`struct tm`): the calendar year, the month counted from 0, and the other fields as C
/// has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month0: i32,
    pub mday: i32,
    pub hour: i32,
    pub min: i32,
    pub sec: i32,
    /// Seconds east of UTC (`tm_gmtoff`) and the zone abbreviation (`tm_zone`, what `strftime("%Z")` prints).
    pub gmtoff: i64,
    pub zone: String,
}

/// `localtime_r()` in the process time zone (`TZ`); `None` when it fails.
pub fn localtime(t: i64) -> Option<LocalTime> {
    let t = libc::time_t::try_from(t).ok()?;
    // SAFETY: `tm` is plain integers and a pointer, for which all zeroes are valid; `localtime_r` writes only
    // through the two pointers it is given, both to live stack values, and returns NULL on failure.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::localtime_r(&t, &mut tm) };
    if r.is_null() {
        return None;
    }
    let zone = if tm.tm_zone.is_null() {
        String::new()
    } else {
        // SAFETY: a non-NULL `tm_zone` from `localtime_r` points to a NUL-terminated abbreviation in libc's time zone
        // data, which only `tzset()` replaces; it is copied out at once.
        unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }
            .to_string_lossy()
            .into_owned()
    };
    Some(LocalTime {
        year: tm.tm_year + 1900,
        month0: tm.tm_mon,
        mday: tm.tm_mday,
        hour: tm.tm_hour,
        min: tm.tm_min,
        sec: tm.tm_sec,
        // `c_long`: 32 bits on some targets.
        #[allow(clippy::useless_conversion)]
        gmtoff: i64::from(tm.tm_gmtoff),
        zone,
    })
}

/// `sock_set_tcp_defer_accept()`: the kernel hands a listener's connection over only once data arrives, or after
/// `seconds`.
pub fn set_tcp_defer_accept(fd: std::os::fd::BorrowedFd<'_>, seconds: i32) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let value: libc::c_int = seconds;
    // SAFETY: the option value is a live stack `int` whose size is passed with it, and the borrowed descriptor stays
    // open for the call.
    let r = unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_DEFER_ACCEPT,
            (&raw const value).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `os_close_all_non_std_open_fds_except(NULL, 0, 0)`: closes every descriptor above stderr, as C does at startup for
/// what its launcher left open. Refused unless the process is single-threaded (see [`close_fds_except`]).
pub fn close_inherited_fds() -> io::Result<()> {
    close_fds_except(&[], CloseMode::Close)
}

/// What [`close_fds_except`] does to each descriptor it reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseMode {
    /// `close()`.
    Close,
    /// `FD_CLOEXEC`: open here, closed by the next exec (`CLOSE_RANGE_CLOEXEC`).
    Cloexec,
}

/// `os_close_all_non_std_open_fds_except()` (`libnetdata/os/close_range.c:85-114`): every descriptor above stderr but
/// `keep`, by `close_range()` over each gap between them, else each one `/proc/self/fd` lists, else each up to the
/// open-files limit. `Close` is refused unless the process is single-threaded: descriptors are closed without their
/// owners knowing, so the caller must hold none it keeps outside `keep`.
pub fn close_fds_except(keep: &[i32], mode: CloseMode) -> io::Result<()> {
    if mode == CloseMode::Close {
        require_single_thread("close_fds_except()")?;
    }
    let mut keep = keep.to_vec();
    keep.sort_unstable();
    let mut start = 3;
    for fd in keep.into_iter().filter(|&fd| fd >= 3) {
        if fd > start {
            close_range(start, (fd - 1) as u32, mode);
        }
        start = fd + 1;
    }
    close_range(start, u32::MAX, mode);
    Ok(())
}

/// `os_close_range()`: `[first, last]`, `u32::MAX` meaning no end.
fn close_range(first: i32, last: u32, mode: CloseMode) {
    let flags = match mode {
        CloseMode::Close => 0,
        CloseMode::Cloexec => libc::CLOSE_RANGE_CLOEXEC,
    };
    // SAFETY: integer arguments; `Close` runs single-threaded (checked by the caller), `Cloexec` moves no ownership.
    if unsafe { libc::syscall(libc::SYS_close_range, first as u32, last, flags) } == 0 {
        return;
    }
    let in_range = |fd: i32| fd >= first && (last == u32::MAX || fd as u32 <= last);
    // the listing's own descriptor is closed when the list is collected: its number then fails F_GETFD
    let listed: Option<Vec<i32>> = std::fs::read_dir("/proc/self/fd")
        .ok()
        .map(|dir| dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect());
    match listed {
        Some(fds) => fds.into_iter().filter(|&fd| in_range(fd)).for_each(|fd| close_one(fd, mode)),
        None => {
            let last = if last == u32::MAX { fd_open_max() } else { last as i32 };
            (first..=last).for_each(|fd| close_one(fd, mode));
        }
    }
}

/// `fd_is_valid()`, then `close()` or `setcloexec()`.
fn close_one(fd: i32, mode: CloseMode) {
    // SAFETY: integer arguments, as close_range()'s.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 && nix::errno::Errno::last() == nix::errno::Errno::EBADF {
        return;
    }
    match mode {
        // SAFETY: as above.
        CloseMode::Close => unsafe {
            libc::close(fd);
        },
        CloseMode::Cloexec if flags != -1 => {
            // SAFETY: as above.
            unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        }
        CloseMode::Cloexec => {}
    }
}

/// `os_get_fd_open_max()`: the hard open-files limit, else `_SC_OPEN_MAX`, else 65535.
fn fd_open_max() -> i32 {
    use nix::sys::resource::{Resource, getrlimit};
    match getrlimit(Resource::RLIMIT_NOFILE) {
        Ok((_, max)) if max != libc::RLIM_INFINITY => max as i32,
        _ => {
            // SAFETY: an integer argument.
            match unsafe { libc::sysconf(libc::_SC_OPEN_MAX) } {
                -1 => 65535,
                n => n as i32,
            }
        }
    }
}

/// What [`recv_with_fds`] read.
#[derive(Debug)]
pub struct Received {
    pub bytes: usize,
    /// `MSG_CTRUNC`: the control data did not fit.
    pub truncated: bool,
    /// The first control message's level, type and length (`cmsg_len`).
    pub first_cmsg: Option<(i32, i32, usize)>,
    /// Every descriptor of every `SCM_RIGHTS` message, owned: dropping the result closes them.
    pub fds: Vec<std::os::fd::OwnedFd>,
}

/// `recvmsg()` into `bufs` with room for `max_fds` passed descriptors (`SCM_RIGHTS`), as C's spawn server reads a
/// request's header (`spawn_server_nofork.c:943-975`); each descriptor received is owned by the result, so rejecting
/// the message closes them.
pub fn recv_with_fds(
    sock: std::os::fd::BorrowedFd<'_>,
    bufs: &mut [io::IoSliceMut<'_>],
    max_fds: usize,
) -> io::Result<Received> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: arithmetic on the length.
    let space = unsafe { libc::CMSG_SPACE((max_fds * size_of::<i32>()) as u32) } as usize;
    // u64 cells: the control buffer is aligned for `cmsghdr`
    let mut control = vec![0u64; space.div_ceil(8)];
    // SAFETY: an all-zero msghdr is a valid empty one; its pointers are set below.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    // IoSliceMut is ABI-compatible with iovec on Unix
    msg.msg_iov = bufs.as_mut_ptr().cast();
    msg.msg_iovlen = bufs.len() as _;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: the buffers and the control buffer outlive the call; their lengths are theirs.
    let n = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut out = Received {
        bytes: n as usize,
        truncated: msg.msg_flags & libc::MSG_CTRUNC != 0,
        first_cmsg: None,
        fds: Vec::new(),
    };
    // SAFETY: the kernel filled `msg` and its control buffer; the CMSG_* walk stays within msg_controllen.
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !cmsg.is_null() {
        // SAFETY: a header the walk returned lies inside the control buffer.
        let h = unsafe { &*cmsg };
        out.first_cmsg.get_or_insert((h.cmsg_level, h.cmsg_type, h.cmsg_len as usize));
        if h.cmsg_level == libc::SOL_SOCKET && h.cmsg_type == libc::SCM_RIGHTS {
            // SAFETY: the data follows the header, `cmsg_len` bytes from its start in all.
            let data = unsafe { libc::CMSG_DATA(cmsg) };
            let len = (h.cmsg_len as usize).saturating_sub(data as usize - cmsg as usize);
            for i in 0..len / size_of::<i32>() {
                // SAFETY: inside the message's data; each number is a descriptor the kernel installed for this call
                // and nobody else holds.
                out.fds.push(unsafe {
                    OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i * size_of::<i32>()).cast::<i32>()))
                });
            }
        }
        // SAFETY: as CMSG_FIRSTHDR.
        cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }
    Ok(out)
}

static SIGNAL_FLAGS: [std::sync::atomic::AtomicBool; 65] = [const { std::sync::atomic::AtomicBool::new(false) }; 65];

extern "C" fn raise_flag(signal: libc::c_int) {
    if let Some(flag) = SIGNAL_FLAGS.get(signal as usize) {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A `sigaction()` handler for `signal` that only raises a flag, with `flags` and an empty mask, as C's spawn server
/// catches SIGTERM and SIGCHLD (`spawn_server_nofork.c:1253-1271`); the flag stays raised until the caller lowers it.
pub fn signal_flag(
    signal: nix::sys::signal::Signal,
    flags: nix::sys::signal::SaFlags,
) -> io::Result<&'static std::sync::atomic::AtomicBool> {
    use nix::sys::signal::{SigAction, SigHandler, SigSet, sigaction};
    let action = SigAction::new(SigHandler::Handler(raise_flag), flags, SigSet::empty());
    // SAFETY: the handler only stores into a static atomic (async-signal-safe); the previous action is dropped.
    unsafe { sigaction(signal, &action) }.map_err(io::Error::from)?;
    Ok(&SIGNAL_FLAGS[signal as usize])
}

/// `wait_on_socket_or_cancel_with_timeout()` without TLS (`libnetdata/socket/socket.c:438-490`), shared by the
/// streaming connector and the spawn client: 0 when `fd` has one of `events`, 1 on a timeout (errno ETIMEDOUT), -1
/// once `cancelled()` (ECANCELED), 2 on a failed poll (its errno) or another event (0); with the errno C leaves. A
/// timeout of 0 or less waits forever; `cancelled` is asked every 100 ms.
pub fn wait_fd(
    fd: std::os::fd::BorrowedFd<'_>,
    timeout_ms: i64,
    events: nix::poll::PollFlags,
    cancelled: &dyn Fn() -> bool,
) -> (i32, i32) {
    use nix::errno::Errno;
    use nix::poll::{PollFd, PollTimeout, poll};
    const CHECK_MS: i64 = 100;
    let forever = timeout_ms <= 0;
    let mut timeout_ms = timeout_ms;
    while timeout_ms > 0 || forever {
        if cancelled() {
            return (-1, Errno::ECANCELED as i32);
        }
        let wait_ms = if timeout_ms >= CHECK_MS || forever { CHECK_MS } else { timeout_ms };
        // C clears errno before each poll: an interrupted one is retried, a successful one leaves 0
        let mut fds = [PollFd::new(fd, events)];
        match poll(&mut fds, PollTimeout::try_from(wait_ms as i32).unwrap_or(PollTimeout::MAX)) {
            Err(Errno::EINTR | Errno::EAGAIN) => {}
            Err(e) => return (2, e as i32),
            Ok(0) => {
                if !forever {
                    timeout_ms -= wait_ms;
                }
            }
            Ok(_) if fds[0].revents().is_some_and(|r| r.intersects(events)) => return (0, 0),
            Ok(_) => return (2, 0),
        }
    }
    (1, Errno::ETIMEDOUT as i32)
}

/// `sqlite3_status64(SQLITE_STATUS_MEMORY_USED, &current, &highwater, 1)`: the most memory SQLite held since the
/// last call, which resets it to the current use.
pub fn sqlite_memory_highwater() -> i64 {
    let (mut current, mut highwater) = (0, 0);
    // SAFETY: both pointers are to locals that outlive the call; the operation code is a constant SQLite knows.
    unsafe {
        rusqlite::ffi::sqlite3_status64(
            rusqlite::ffi::SQLITE_STATUS_MEMORY_USED,
            &mut current,
            &mut highwater,
            1,
        );
    }
    highwater
}

unsafe extern "C" {
    fn sqlite3_recover_init(
        db: *mut rusqlite::ffi::sqlite3,
        schema: *const std::ffi::c_char,
        dst: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
    fn sqlite3_recover_run(recover: *mut std::ffi::c_void) -> std::ffi::c_int;
    fn sqlite3_recover_finish(recover: *mut std::ffi::c_void) -> std::ffi::c_int;
}

/// SQLite's recover extension (`ext/recover`, compiled into the vendored SQLite): what `conn` can read of its main
/// database, written to a new database at `dst`. The result codes of `sqlite3_recover_run()` and
/// `sqlite3_recover_finish()`, or `None` when the recover object could not be created.
pub fn sqlite_recover(conn: &rusqlite::Connection, dst: &str) -> Option<(i32, i32)> {
    let dst = std::ffi::CString::new(dst).ok()?;
    // SAFETY: the handle belongs to `conn`, borrowed until this returns; the recover object lives only between init
    // and finish below, and copies both strings at init.
    unsafe {
        let recover = sqlite3_recover_init(conn.handle(), c"main".as_ptr(), dst.as_ptr());
        if recover.is_null() {
            return None;
        }
        let run = sqlite3_recover_run(recover);
        let finish = sqlite3_recover_finish(recover);
        Some((run, finish))
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::{AsFd, AsRawFd};

    fn cloexec(fd: &impl AsRawFd) -> bool {
        // SAFETY: an integer argument
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) & libc::FD_CLOEXEC != 0 }
    }

    /// Close-on-exec reaches every descriptor above stderr but the kept ones (a plain `pipe()` opens without it).
    #[test]
    fn cloexec_skips_the_kept_descriptors() {
        let (keep_r, keep_w) = nix::unistd::pipe().unwrap();
        let (other_r, _other_w) = nix::unistd::pipe().unwrap();
        assert!(!cloexec(&keep_r) && !cloexec(&other_r));
        close_fds_except(&[keep_r.as_raw_fd(), keep_w.as_raw_fd()], CloseMode::Cloexec).unwrap();
        assert_eq!([cloexec(&keep_r), cloexec(&keep_w), cloexec(&other_r)], [false, false, true]);
    }

    /// Descriptors passed with SCM_RIGHTS arrive owned, with the first message's header; a control buffer too small
    /// sets MSG_CTRUNC and still owns what did fit.
    #[test]
    fn passed_descriptors_arrive_owned() {
        use nix::sys::socket::{ControlMessage, MsgFlags, sendmsg};
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let (r, w) = nix::unistd::pipe().unwrap();
        let send = |fds: &[i32]| {
            let iov = [io::IoSlice::new(b"hello")];
            sendmsg::<()>(a.as_raw_fd(), &iov, &[ControlMessage::ScmRights(fds)], MsgFlags::empty(), None).unwrap();
        };
        send(&[r.as_raw_fd(), w.as_raw_fd()]);
        let mut buf = [0u8; 8];
        let got = recv_with_fds(b.as_fd(), &mut [io::IoSliceMut::new(&mut buf)], 4).unwrap();
        // SAFETY: arithmetic
        let len = unsafe { libc::CMSG_LEN(8) } as usize;
        assert_eq!(
            (got.bytes, got.truncated, got.first_cmsg, got.fds.len()),
            (5, false, Some((libc::SOL_SOCKET, libc::SCM_RIGHTS, len)), 2)
        );
        assert!(got.fds.iter().all(|fd| fd.as_raw_fd() != r.as_raw_fd() && fd.as_raw_fd() != w.as_raw_fd()));
        send(&[r.as_raw_fd(), w.as_raw_fd(), r.as_raw_fd()]);
        // room for one rounds up to the 8-byte alignment: two of the three fit, the third is cut
        let got = recv_with_fds(b.as_fd(), &mut [io::IoSliceMut::new(&mut buf)], 1).unwrap();
        assert!(got.truncated && got.fds.len() == 2, "{got:?}");
    }

    /// The handler raises the signal's flag and nothing else.
    #[test]
    fn a_signal_raises_its_flag() {
        use nix::sys::signal::{SaFlags, Signal, raise};
        let flag = signal_flag(Signal::SIGUSR2, SaFlags::SA_RESTART).unwrap();
        assert!(!flag.load(std::sync::atomic::Ordering::SeqCst));
        raise(Signal::SIGUSR2).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !flag.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(std::time::Instant::now() < deadline, "no flag");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// `wait_fd()`'s outcomes with C's errno: a timeout, the event, a cancellation.
    #[test]
    fn waits_end_as_c_s() {
        use nix::poll::PollFlags;
        use std::io::Write;
        let (a, mut b) = std::os::unix::net::UnixStream::pair().unwrap();
        let never = || false;
        assert_eq!(wait_fd(a.as_fd(), 150, PollFlags::POLLIN, &never), (1, libc::ETIMEDOUT));
        assert_eq!(wait_fd(a.as_fd(), 150, PollFlags::POLLIN, &|| true), (-1, libc::ECANCELED));
        b.write_all(b"x").unwrap();
        assert_eq!(wait_fd(a.as_fd(), 150, PollFlags::POLLIN, &never), (0, 0));
    }

    use super::*;

    /// The test harness runs tests on threads of its own, so the reset is refused here (its effect is proven by the
    /// single-threaded `dispositions` test).
    #[test]
    fn default_dispositions_needs_a_single_thread() {
        let err = default_dispositions(&[nix::sys::signal::Signal::SIGHUP]).unwrap_err();
        assert!(err.to_string().contains("single-threaded"), "{err}");
    }

    /// The high-water mark covers what SQLite held since the last reading, and a reading resets it to the use then.
    #[test]
    fn sqlite_memory_highwater_resets() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (zeroblob(1000000));")
            .unwrap();
        drop(conn);
        let peak = sqlite_memory_highwater();
        let after = sqlite_memory_highwater();
        assert!(peak > 1_000_000 && after < peak, "{peak} {after}");
    }

    #[test]
    fn multithreaded_processes_refuse_fork_and_setenv() {
        // The test harness runs tests on worker threads, so this process has more than one.
        assert!(thread_count().unwrap() > 1);
        assert!(fork().is_err());
        assert!(setenv("NETDATA_SYS_TEST", "1").is_err());
        assert!(close_inherited_fds().is_err());
        assert!(sched_getscheduler().is_ok());
        assert!(getpriority_self().is_ok());
    }

    #[test]
    fn localtime_breaks_down_the_epoch() {
        let tm = localtime(0).unwrap();
        // Within a day of the epoch in any zone.
        assert!(
            matches!((tm.year, tm.month0), (1970, 0) | (1969, 11)),
            "{tm:?}"
        );
        assert!((0..24).contains(&tm.hour) && (0..60).contains(&tm.min) && tm.sec == 0);
    }
}
