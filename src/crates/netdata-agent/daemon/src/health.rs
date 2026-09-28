//! The `HEALTH` thread, ported from `health_main()` and `health_event_loop()` (`src/health/health_event_loop.c`)
//! without alerts: health is not ported (M9), but C runs the loop with health off, and a disconnected child is
//! archived only after more than 10 of its passes (D93.2). It keeps C's pacing: a pass at most every `[health] run
//! at least every` seconds, waited in 1 s sleeps, and none while a backfill or more than one user query runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_rrd::clock::{now_realtime_s, now_realtime_ut};
use netdata_agent_rrd::storage::StorageLayout;
use netdata_agent_rrd::stream_control;

use crate::heartbeat::{Phase, Thread};

/// `check_if_resumed_from_suspension()`: the wall clock moved more than twice as far as the monotonic one since the
/// last pass.
struct Suspension {
    last: Option<(u64, Instant)>,
}

impl Suspension {
    fn resumed(&mut self, realtime_ut: u64, monotonic: Instant) -> bool {
        let resumed = self.last.is_some_and(|(last_realtime_ut, last_monotonic)| {
            let realtime_delta = realtime_ut.saturating_sub(last_realtime_ut);
            let monotonic_delta = monotonic.saturating_duration_since(last_monotonic).as_micros() as u64;
            realtime_ut > last_realtime_ut
                && realtime_delta > monotonic_delta
                && realtime_delta - monotonic_delta > monotonic_delta
        });
        self.last = Some((realtime_ut, monotonic));
        resumed
    }
}

/// Starts `HEALTH`, whose passes `storage` counts; `run_at_least_every_s` and `postpone_s` are `[health]`'s.
pub fn spawn(
    storage: Arc<StorageLayout>,
    stack_size: usize,
    run_at_least_every_s: i64,
    postpone_s: i64,
) -> std::io::Result<Thread> {
    Thread::spawn(
        "HEALTH",
        stack_size,
        Duration::from_secs(1),
        Phase::OnTheTick,
        move |ticker| {
            let mut suspension = Suspension { last: None };
            while ticker.running() {
                if !stream_control::health_should_be_running() {
                    ticker.sleep(stream_control::throttle_wait());
                    continue;
                }
                let next_run = now_realtime_s().saturating_add(run_at_least_every_s);
                if suspension.resumed(now_realtime_ut(), Instant::now()) {
                    nd_log!(
                        Source::Daemon,
                        Priority::Notice,
                        "Postponing alarm checks for {postpone_s} seconds, because it seems that the system was just resumed from suspension."
                    );
                }
                // health_event_loop_for_host(): with health off no host runs its alerts
                storage.next_health_iteration();
                // health_sleep()
                while now_realtime_s() < next_run && ticker.sleep(Duration::from_secs(1)) {}
            }
            nd_log!(Source::Daemon, Priority::Debug, "Health thread ended.");
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pass after the wall clock ran more than twice as far as the monotonic one is a resume; the first is not.
    #[test]
    fn a_resume_is_twice_the_monotonic_time() {
        let mut s = Suspension { last: None };
        let t0 = Instant::now();
        assert!(!s.resumed(1_000_000_000, t0));
        let t1 = t0 + Duration::from_secs(10);
        assert!(!s.resumed(1_000_000_000 + 10_000_000, t1), "as far");
        let t2 = t1 + Duration::from_secs(10);
        assert!(!s.resumed(1_000_000_000 + 30_000_000, t2), "twice as far is not more than twice");
        let t3 = t2 + Duration::from_secs(10);
        assert!(s.resumed(1_000_000_000 + 30_000_000 + 20_000_001, t3), "more than twice");
    }
}
