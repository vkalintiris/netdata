//! `EXIT_REASON` and `exit_initiated` (`src/libnetdata/exit/exit_initiated.{h,c}`): why the agent exits, a bitmap
//! every exit path adds to, readable from any thread. The bits and names are in C's table order.

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

pub const SIGBUS: u32 = 1 << 0;
pub const SIGSEGV: u32 = 1 << 1;
pub const SIGFPE: u32 = 1 << 2;
pub const SIGILL: u32 = 1 << 3;
pub const SIGABRT: u32 = 1 << 4;
pub const SIGSYS: u32 = 1 << 5;
pub const SIGXCPU: u32 = 1 << 6;
pub const SIGXFSZ: u32 = 1 << 7;
pub const OUT_OF_MEMORY: u32 = 1 << 8;
pub const ALREADY_RUNNING: u32 = 1 << 9;
pub const FATAL: u32 = 1 << 10;
pub const API_QUIT: u32 = 1 << 11;
pub const CMD_EXIT: u32 = 1 << 12;
pub const SIGQUIT: u32 = 1 << 13;
pub const SIGTERM: u32 = 1 << 14;
pub const SIGINT: u32 = 1 << 15;
pub const SERVICE_STOP: u32 = 1 << 16;
pub const SYSTEM_SHUTDOWN: u32 = 1 << 17;
pub const UPDATE: u32 = 1 << 18;
pub const SHUTDOWN_TIMEOUT: u32 = 1 << 19;

pub const NAMES: [(u32, &str); 20] = [
    (SIGBUS, "signal-bus-error"),
    (SIGSEGV, "signal-segmentation-fault"),
    (SIGFPE, "signal-floating-point-exception"),
    (SIGILL, "signal-illegal-instruction"),
    (SIGABRT, "signal-abort"),
    (SIGSYS, "signal-bad-system-call"),
    (SIGXCPU, "signal-cpu-time-limit-exceeded"),
    (SIGXFSZ, "signal-file-size-limit-exceeded"),
    (OUT_OF_MEMORY, "out-of-memory"),
    (ALREADY_RUNNING, "already-running"),
    (FATAL, "fatal"),
    (API_QUIT, "api-quit"),
    (CMD_EXIT, "cmd-exit"),
    (SIGQUIT, "signal-quit"),
    (SIGTERM, "signal-terminate"),
    (SIGINT, "signal-interrupt"),
    (SERVICE_STOP, "service-stop"),
    (SYSTEM_SHUTDOWN, "system-shutdown"),
    (UPDATE, "update"),
    (SHUTDOWN_TIMEOUT, "shutdown-timeout"),
];

/// `EXIT_REASON_NORMAL`.
pub const NORMAL: u32 =
    SIGINT | SIGTERM | SIGQUIT | API_QUIT | CMD_EXIT | SERVICE_STOP | SYSTEM_SHUTDOWN | UPDATE;
/// `EXIT_REASON_DEADLY_SIGNAL`.
pub const DEADLY_SIGNAL: u32 = SIGBUS | SIGSEGV | SIGFPE | SIGILL | SIGSYS | SIGXCPU | SIGXFSZ;
/// `EXIT_REASON_ABNORMAL`.
pub const ABNORMAL: u32 =
    DEADLY_SIGNAL | SIGABRT | FATAL | ALREADY_RUNNING | OUT_OF_MEMORY | SHUTDOWN_TIMEOUT;

/// `is_exit_reason_normal()`.
pub fn is_normal(reason: u32) -> bool {
    (reason == 0 || reason & NORMAL != 0) && reason & ABNORMAL == 0
}

/// `EXIT_REASON_2id_one()`: 0 for an unknown or empty name.
pub fn from_name(name: &[u8]) -> u32 {
    NAMES.iter().find(|(_, n)| n.as_bytes() == name).map_or(0, |(bit, _)| *bit)
}
/// `EXIT_REASON_2buffer()`: the names of the bits, in table order, joined by `separator`; empty for none.
pub fn names(reason: u32, separator: &str) -> String {
    crate::status_file::bitmap_names(&NAMES, reason).collect::<Vec<_>>().join(separator)
}

/// `exit_initiated`.
static EXIT_INITIATED: AtomicU32 = AtomicU32::new(0);

/// `self_path` and its `OS_FILE_METADATA` (modification time, size) at start, when both are known.
static SELF: OnceLock<Option<(PathBuf, (i64, u64))>> = OnceLock::new();

/// `os_get_file_metadata()` under `OS_FILE_METADATA_OK()`.
fn file_metadata(path: &std::path::Path) -> Option<(i64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    (m.mtime() > 0 && m.size() > 0).then_some((m.mtime(), m.size()))
}

/// `exit_initiated_init()`: no reason yet, and the executable's metadata, to tell an update at exit.
pub fn init() {
    EXIT_INITIATED.store(0, Ordering::Relaxed);
    // os_get_process_path(): libuv's uv_exepath() reads /proc/self/exe as the fallback does
    let _ = SELF.set(std::env::current_exe().ok().and_then(|p| file_metadata(&p).map(|m| (p, m))));
}

/// `exit_initiated_get()`.
pub fn get() -> u32 {
    EXIT_INITIATED.load(Ordering::Relaxed)
}

/// `exit_initiated_add()`.
pub fn add(reason: u32) {
    EXIT_INITIATED.fetch_or(reason, Ordering::Relaxed);
}

/// `is_system_shutdown()` on Linux: a file the shutdown creates exists.
fn system_shutdown() -> bool {
    ["/etc/nologin", "/etc/halt", "/run/nologin"].iter().any(|f| std::path::Path::new(f).exists())
}

/// `exit_initiated_set()`: the first reason also learns whether the system is shutting down and whether the
/// executable changed since start (an update); every reason is kept.
pub fn set(mut reason: u32) {
    let old = get();
    if old == 0 && reason & SYSTEM_SHUTDOWN == 0 && system_shutdown() {
        reason |= SYSTEM_SHUTDOWN;
    }
    if old == 0
        && let Some(Some((path, then))) = SELF.get()
        && file_metadata(path).is_some_and(|now| now != *then)
    {
        reason |= UPDATE;
    }
    add(reason);
}
