//! The startup step lines of `netdata_main()` (`delta_startup_time()` in `src/daemon/main.c`) and the completion
//! line. Every C step prints its line, including the steps whose bodies are not ported yet (D36).

use std::sync::atomic::{AtomicBool, Ordering};

use netdata_agent_log::netdata_log_info;
use nix::time::{ClockId, clock_gettime};

use crate::build;

/// `now_monotonic_usec()`: `CLOCK_MONOTONIC_RAW`, nanoseconds truncated to microseconds.
pub(crate) fn now_ut() -> u64 {
    clock_gettime(ClockId::CLOCK_MONOTONIC_RAW).map_or(0, |ts| {
        ts.tv_sec() as u64 * 1_000_000 + ts.tv_nsec() as u64 / 1_000
    })
}

pub struct Startup {
    started_ut: u64,
    last_ut: u64,
    prev: Option<&'static str>,
}

impl Startup {
    /// At `netdata_main()` entry, before the command line is parsed.
    pub fn new() -> Self {
        let now = now_ut();
        Startup {
            started_ut: now,
            last_ut: now,
            prev: None,
        }
    }

    /// `delta_startup_time(msg)`: the step line, then the status file saved with `startup(<msg>)` as its step.
    pub fn step(&mut self, msg: &'static str) {
        self.step_line(msg);
        crate::status_file::startup_step(Some(&format!("startup({msg})")));
    }

    /// The line of `delta_startup_time(msg)`: the time since the previous step line, the first one without it.
    pub fn step_line(&mut self, msg: &'static str) {
        let now = now_ut();
        match self.prev {
            Some(prev) => netdata_log_info!(
                "NETDATA STARTUP: in {:>7} ms, {prev} - next: {msg}",
                now.saturating_sub(self.last_ut) / 1000
            ),
            None => netdata_log_info!("NETDATA STARTUP: next: {msg}"),
        }
        self.last_ut = now;
        self.prev = Some(msg);
    }

    /// Microseconds since the daemon started.
    pub fn elapsed_us(&self) -> u64 {
        now_ut().saturating_sub(self.started_ut)
    }

    /// The line of the "agent start timings" step, with the median start time of earlier starts.
    pub fn completed(&self, elapsed_us: u64, median_us: u64) {
        netdata_log_info!(
            "NETDATA STARTUP: version '{}', sqlite '{}', completed in {} ms (median start up time is {} ms). Enjoy \
             X-Ray Vision for your infrastructure!",
            build::NETDATA_VERSION,
            netdata_agent_metadata::library::version(),
            elapsed_us / 1000,
            median_us / 1000
        );
    }
}

/// `netdata_anonymous_statistics_enabled`: on at the start, and off for good once a check finds it off.
static ANONYMOUS_STATISTICS: AtomicBool = AtomicBool::new(true);

/// `analytics_check_enabled()`: off when the opt-out file is readable or `DISABLE_TELEMETRY` is set to anything.
pub fn analytics_check_enabled(user_config_dir: &str) -> bool {
    check_enabled(&ANONYMOUS_STATISTICS, user_config_dir)
}

/// The flag as the last check left it (the registry's hello reads it).
pub fn anonymous_statistics() -> bool {
    ANONYMOUS_STATISTICS.load(Ordering::Relaxed)
}

fn check_enabled(flag: &AtomicBool, user_config_dir: &str) -> bool {
    if !flag.load(Ordering::Relaxed) {
        return false;
    }
    let opt_out = format!("{user_config_dir}/.opt-out-from-anonymous-statistics");
    let enabled = nix::unistd::access(opt_out.as_str(), nix::unistd::AccessFlags::R_OK).is_err()
        && std::env::var_os("DISABLE_TELEMETRY").is_none_or(|s| s.is_empty());
    flag.store(enabled, Ordering::Relaxed);
    enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Once a check finds the statistics off they stay off, though the opt-out file goes away.
    #[test]
    fn the_statistics_flag_stays_off_once_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let flag = AtomicBool::new(true);
        let opt_out = dir.path().join(".opt-out-from-anonymous-statistics");
        std::fs::write(&opt_out, "").unwrap();
        assert!(!check_enabled(&flag, path));
        std::fs::remove_file(&opt_out).unwrap();
        assert!(!check_enabled(&flag, path));
        assert!(!flag.load(Ordering::Relaxed));
    }
}
