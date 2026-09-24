//! The only agent crate that contains `unsafe` (decisions D12, D21): system calls no safe crate wraps, each behind a
//! safe function whose preconditions it checks itself.
//!
//! - `fork()` and `setenv()` are sound only while the process has one thread; both refuse otherwise.
//! - `gethostid()` and the scheduling calls (`nice`, `getpriority`, `sched_*`) pass plain integers and read nothing back through
//!   pointers except a stack `sched_param`.
//! - `localtime_r()` (decision D22.3) fills a stack `struct tm`; it reads `TZ`, which only `setenv()` changes, and that
//!   refuses once a second thread exists.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month0: i32,
    pub mday: i32,
    pub hour: i32,
    pub min: i32,
    pub sec: i32,
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
    Some(LocalTime {
        year: tm.tm_year + 1900,
        month0: tm.tm_mon,
        mday: tm.tm_mday,
        hour: tm.tm_hour,
        min: tm.tm_min,
        sec: tm.tm_sec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multithreaded_processes_refuse_fork_and_setenv() {
        // The test harness runs tests on worker threads, so this process has more than one.
        assert!(thread_count().unwrap() > 1);
        assert!(fork().is_err());
        assert!(setenv("NETDATA_SYS_TEST", "1").is_err());
        assert!(sched_getscheduler().is_ok());
        assert!(getpriority_self().is_ok());
    }

    #[test]
    fn localtime_breaks_down_the_epoch() {
        let tm = localtime(0).unwrap();
        // Within a day of the epoch in any zone.
        assert!(matches!((tm.year, tm.month0), (1970, 0) | (1969, 11)), "{tm:?}");
        assert!((0..24).contains(&tm.hour) && (0..60).contains(&tm.min) && tm.sec == 0);
    }
}
