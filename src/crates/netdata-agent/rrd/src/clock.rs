//! The wall clock C reads as `now_realtime_usec()` and `now_realtime_sec()`: the objects' times and every deadline
//! the agent keeps in wall-clock time; and the boot clock of `now_boottime_sec()`.

use std::time::{SystemTime, UNIX_EPOCH};

use nix::time::{ClockId, clock_gettime};

/// `now_realtime_usec()`: microseconds since the epoch (0 before it).
pub fn now_realtime_ut() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// `now_realtime_sec()`.
pub fn now_realtime_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `now_boottime_sec()`: whole seconds of `CLOCK_BOOTTIME`.
pub fn now_boottime_s() -> i64 {
    clock_gettime(ClockId::CLOCK_BOOTTIME).map_or(0, |t| t.tv_sec())
}
