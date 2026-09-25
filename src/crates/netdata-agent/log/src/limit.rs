//! Flood protection (`src/libnetdata/log/nd_log_limit.c`) and the per-call-site limiter (`ERROR_LIMIT`,
//! `netdata_logger_with_limit()` in `nd_log.c`).

use std::sync::Mutex;

use nix::time::{ClockId, clock_gettime};

use crate::model::msgid;

/// `ND_LOG_DEFAULT_THROTTLE_LOGS` / `ND_LOG_DEFAULT_THROTTLE_PERIOD`.
pub const DEFAULT_THROTTLE_LOGS: u32 = 1000;
pub const DEFAULT_THROTTLE_PERIOD: u32 = 60;

fn clock_usec(clock: ClockId) -> u64 {
    clock_gettime(clock).map_or(0, |ts| {
        ts.tv_sec() as u64 * 1_000_000 + ts.tv_nsec() as u64 / 1_000
    })
}

/// `now_monotonic_usec()`.
pub(crate) fn now_monotonic_usec() -> u64 {
    clock_usec(ClockId::CLOCK_MONOTONIC)
}

/// `struct nd_log_limit`, plus the source's pending message, which C guards with the same spinlock.
#[derive(Debug, Clone, Default)]
pub(crate) struct Limits {
    started_monotonic_ut: u64,
    counter: u32,
    prevented: u32,
    pub(crate) throttle_period: u32,
    pub(crate) logs_per_period: u32,
    pub(crate) logs_per_period_backup: u32,
    /// The flood-protection message to write after the current record.
    pub(crate) pending: Option<String>,
}

impl Limits {
    /// `ND_LOG_LIMITS_DEFAULT`.
    pub(crate) fn default_limits() -> Self {
        Limits {
            throttle_period: DEFAULT_THROTTLE_PERIOD,
            logs_per_period: DEFAULT_THROTTLE_LOGS,
            logs_per_period_backup: DEFAULT_THROTTLE_LOGS,
            ..Limits::default()
        }
    }

    /// `ND_LOG_LIMITS_UNLIMITED`.
    pub(crate) fn unlimited() -> Self {
        Limits::default()
    }

    /// Assigning a new `struct nd_log_limit` in C leaves the pending message, which lives outside it.
    pub(crate) fn replace(&mut self, limits: Limits) {
        let pending = self.pending.take();
        *self = limits;
        self.pending = pending;
    }

    /// `nd_log_limits_reset()` for one source: a new period starts now, with the configured logs per period.
    pub(crate) fn reset(&mut self, now_ut: u64) {
        self.prevented = 0;
        self.counter = 0;
        self.started_monotonic_ut = now_ut;
        self.logs_per_period = self.logs_per_period_backup;
    }

    /// `nd_log_limit_reached()`: true drops the record; the first dropped record of a period and the first record
    /// after a period with drops queue a message.
    pub(crate) fn reached(&mut self, now_ut: u64, program_name: &str) -> bool {
        if self.throttle_period == 0 || self.logs_per_period == 0 {
            return false;
        }
        if self.started_monotonic_ut == 0 {
            self.started_monotonic_ut = now_ut;
        }
        self.counter = self.counter.wrapping_add(1);

        let period_ut = u64::from(self.throttle_period) * 1_000_000;
        if now_ut.wrapping_sub(self.started_monotonic_ut) > period_ut {
            if self.prevented != 0 {
                self.pending = Some(format!(
                    "LOG FLOOD PROTECTION: resuming logging (prevented {} logs in the last {} seconds).",
                    self.prevented, self.throttle_period
                ));
            }
            self.started_monotonic_ut = now_ut;
            self.counter = 1;
            self.prevented = 0;
            return false;
        }

        if self.counter > self.logs_per_period {
            if self.prevented == 0 {
                // C computes both in unsigned microseconds and prints them as int64 seconds.
                let elapsed = now_ut.wrapping_sub(self.started_monotonic_ut) / 1_000_000;
                let remaining = self
                    .started_monotonic_ut
                    .wrapping_add(period_ut)
                    .wrapping_sub(now_ut)
                    / 1_000_000;
                self.pending = Some(format!(
                    "LOG FLOOD PROTECTION: too many logs ({} logs in {} seconds, threshold is set to {} logs in {} \
                     seconds). Preventing more logs from process '{}' for {} seconds.",
                    self.counter,
                    elapsed as i64,
                    self.logs_per_period,
                    self.throttle_period,
                    program_name,
                    remaining as i64
                ));
            }
            self.prevented = self.prevented.wrapping_add(1);
            return true;
        }
        false
    }
}

/// The `MESSAGE_ID` of the flood-protection messages.
pub(crate) const PENDING_MSGID: [u8; 16] = msgid::LOG_FLOOD_PROTECTION;

/// `ERROR_LIMIT`: a call site that logs at most once every `log_every` seconds of boot time, optionally sleeping
/// before each attempt. Suppressed calls leave no trace.
#[derive(Debug)]
pub struct ErrorLimit {
    log_every: i64,
    sleep_ut: u64,
    state: Mutex<(u64, i64)>,
}

impl ErrorLimit {
    /// `nd_log_limit_static_global_var(var, log_every_secs, sleep_usecs)`.
    pub const fn new(log_every_secs: i64, sleep_usecs: u64) -> Self {
        ErrorLimit {
            log_every: log_every_secs,
            sleep_ut: sleep_usecs,
            state: Mutex::new((0, 0)),
        }
    }

    /// The part of `netdata_logger_with_limit()` after the priority check: the boot-time second when this call logs.
    pub(crate) fn admit(&self) -> Option<i64> {
        if self.sleep_ut != 0 {
            std::thread::sleep(std::time::Duration::from_micros(self.sleep_ut));
        }
        let now = (clock_usec(ClockId::CLOCK_BOOTTIME) / 1_000_000) as i64;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.0 += 1;
        (now - state.1 >= self.log_every).then_some(now)
    }

    /// After an admitted call logged: C stores the time taken before logging and resets the count.
    pub(crate) fn logged(&self, now: i64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = (0, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(logs: u32, period: u32) -> Limits {
        Limits {
            throttle_period: period,
            logs_per_period: logs,
            logs_per_period_backup: logs,
            ..Limits::default()
        }
    }

    #[test]
    fn the_first_dropped_record_queues_the_c_message() {
        let mut l = limits(5, 60);
        let t0 = 1_000_000;
        for i in 0..5 {
            assert!(!l.reached(t0 + i, "netdata"));
        }
        assert!(l.reached(t0 + 10, "netdata"));
        assert_eq!(
            l.pending.take().as_deref(),
            Some(
                "LOG FLOOD PROTECTION: too many logs (6 logs in 0 seconds, threshold is set to 5 logs in 60 seconds). \
                 Preventing more logs from process 'netdata' for 59 seconds."
            )
        );
        // later drops in the same period queue nothing
        assert!(l.reached(t0 + 20, "netdata"));
        assert_eq!(l.pending, None);
        // after the period the record is logged and the resume message queued
        assert!(!l.reached(t0 + 60_000_001, "netdata"));
        assert_eq!(
            l.pending.take().as_deref(),
            Some(
                "LOG FLOOD PROTECTION: resuming logging (prevented 2 logs in the last 60 seconds)."
            )
        );
    }

    #[test]
    fn zero_logs_or_period_disable_the_limit() {
        for mut l in [limits(0, 60), limits(5, 0), Limits::unlimited()] {
            for i in 0..100 {
                assert!(!l.reached(1 + i, "netdata"));
            }
        }
    }

    #[test]
    fn reset_restores_the_configured_logs_per_period() {
        let mut l = limits(5, 60);
        l.logs_per_period = 0;
        l.reset(42);
        assert_eq!(
            (l.logs_per_period, l.counter, l.started_monotonic_ut),
            (5, 0, 42)
        );
    }
}
