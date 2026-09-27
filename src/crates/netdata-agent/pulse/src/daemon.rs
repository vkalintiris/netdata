//! `pulse-daemon.c`: the agent's CPU and uptime, then its memory (`daemon_memory`). The timezone refresh, last in C's
//! step, runs in the daemon's PULSE loop (D82.5).

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, Chart, ChartType, Dim};
use nix::sys::resource::{UsageWho, getrusage};
use nix::sys::time::TimeValLike;
use nix::time::{ClockId, clock_gettime};

use crate::chart::{Def, Localhost, dim, set};
use crate::daemon_memory;

#[derive(Default)]
pub(crate) struct Charts {
    cpu: Option<(Arc<Chart>, Arc<Dim>, Arc<Dim>)>,
    uptime: Option<(Arc<Chart>, Arc<Dim>)>,
    /// `netdata_boottime_time`: the boot time clock at the first cycle.
    boottime_s: i64,
    memory: daemon_memory::Charts,
}

/// `now_boottime_sec()`.
fn now_boottime_s() -> i64 {
    clock_gettime(ClockId::CLOCK_BOOTTIME).map_or(0, |t| t.tv_sec())
}

impl Charts {
    /// `pulse_daemon_do()`.
    pub fn update(&mut self, localhost: &Localhost<'_>) {
        self.cpu(localhost);
        self.uptime(localhost);
        self.memory.update(localhost);
    }

    /// `pulse_daemon_cpu_usage_do()`: the user and system time of the process, in microseconds.
    fn cpu(&mut self, localhost: &Localhost<'_>) {
        let (user, system) = getrusage(UsageWho::RUSAGE_SELF).map_or((0, 0), |usage| {
            (
                usage.user_time().num_microseconds(),
                usage.system_time().num_microseconds(),
            )
        });
        let (chart, user_dim, system_dim) = self.cpu.get_or_insert_with(|| {
            let chart = localhost.create(&Def {
                id: "server_cpu",
                family: "CPU usage",
                context: None,
                title: "Netdata CPU usage",
                units: "milliseconds/s",
                module: "pulse",
                priority: 130000,
                chart_type: ChartType::Stacked,
            });
            let user_dim = dim(&chart, "user", 1, 1000, Algorithm::Incremental);
            let system_dim = dim(&chart, "system", 1, 1000, Algorithm::Incremental);
            (chart, user_dim, system_dim)
        });
        set(user_dim, user);
        set(system_dim, system);
        localhost.done(chart);
    }

    /// `pulse_daemon_uptime_do()`: the seconds on the boot time clock since the first cycle.
    fn uptime(&mut self, localhost: &Localhost<'_>) {
        if self.boottime_s == 0 {
            self.boottime_s = now_boottime_s();
        }
        let uptime = now_boottime_s() - self.boottime_s;
        let (chart, rd) = self.uptime.get_or_insert_with(|| {
            let chart = localhost.create(&Def {
                id: "uptime",
                family: "Uptime",
                context: None,
                title: "Netdata uptime",
                units: "seconds",
                module: "pulse",
                priority: 130150,
                chart_type: ChartType::Line,
            });
            let rd = dim(&chart, "uptime", 1, 1, Algorithm::Absolute);
            (chart, rd)
        });
        set(rd, uptime);
        localhost.done(chart);
    }
}
