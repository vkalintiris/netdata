//! The `PULSE` thread, ported from `pulse_thread_main()` (`src/daemon/pulse/pulse.c`): on the wall-clock grid of a
//! second, every `[pulse] update every` seconds, a cycle of localhost's pulse charts, until the exit starts.

use std::sync::Arc;
use std::time::Duration;

use netdata_agent_pulse::{Pulse, Settings};
use netdata_agent_rrd::host::Hosts;

use crate::heartbeat::{Phase, Thread};
use crate::shutdown;

/// Starts `PULSE`. `update_every` runs first on the thread, as C reads `[pulse] update every` there.
pub fn spawn(
    hosts: Arc<Hosts>,
    stack_size: usize,
    update_every: impl FnOnce() -> i64 + Send + 'static,
    settings: Settings,
) -> std::io::Result<Thread> {
    // keep the randomness at zero, to make sure we are not close to any other thread (C)
    Thread::spawn(
        "PULSE",
        stack_size,
        Duration::from_secs(1),
        Phase::OnTheTick,
        move |ticker| {
            let step = update_every();
            let mut pulse = Pulse::new(hosts, settings);
            let mut real_step = 1;
            // service_running(SERVICE_COLLECTORS), false once the exit starts
            while ticker.next() && !shutdown::exiting() {
                if real_step < step {
                    real_step += 1;
                    continue;
                }
                real_step = 1;
                pulse.cycle();
            }
        },
    )
}
