//! The `PULSE` thread, ported from `pulse_thread_main()` (`src/daemon/pulse/pulse.c`): on the wall-clock grid of a
//! second, every `[pulse] update every` seconds, a cycle of localhost's pulse charts, until the exit starts.

use std::sync::Arc;
use std::time::Duration;

use netdata_agent_pulse::{Pulse, Settings};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::Hosts;

use crate::heartbeat::{Phase, Thread};
use crate::timezone::Timezone;
use crate::{shutdown, startup, system};

/// `os_system_memory(true)` for the pulse charts: the available bytes, while the total is known.
pub fn system_memory_available() -> Option<u64> {
    let memory = system::system_memory_cached(true);
    (memory.total > 0).then_some(memory.available)
}

/// Starts `PULSE`. `update_every` runs first on the thread, as C reads `[pulse] update every` there; each cycle ends
/// with the time zone's refresh, the last step of C's (`pulse_daemon_timezone_do()`).
pub fn spawn(
    hosts: Arc<Hosts>,
    stack_size: usize,
    update_every: impl FnOnce() -> i64 + Send + 'static,
    settings: Settings,
    mut timezone: Timezone,
) -> std::io::Result<Thread> {
    // keep the randomness at zero, to make sure we are not close to any other thread (C)
    Thread::spawn(
        "PULSE",
        stack_size,
        Duration::from_secs(1),
        Phase::OnTheTick,
        move |ticker| {
            let step = update_every();
            let localhost = Arc::clone(hosts.localhost());
            let mut pulse = Pulse::new(hosts, settings);
            let mut real_step = 1;
            // service_running(SERVICE_COLLECTORS), false once the exit starts, is tested before the wait: the cycle
            // after the exit's start still runs, its stores dropped by `timed_done` (D81.3, D110.5)
            while !shutdown::exiting() {
                if !ticker.next() {
                    break;
                }
                if real_step < step {
                    real_step += 1;
                    continue;
                }
                real_step = 1;
                pulse.cycle();
                timezone.pulse_refresh(&localhost, startup::now_ut(), now_realtime_s());
            }
        },
    )
}
