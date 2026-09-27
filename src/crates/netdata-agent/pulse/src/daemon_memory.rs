//! `pulse-daemon-memory.c`: the memory the agent keeps free for the system.

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, Chart, ChartType, Dim};

use crate::chart::{Def, Localhost, dim, set};

#[derive(Default)]
pub(crate) struct Charts {
    out_of_memory: Option<(Arc<Chart>, Arc<Dim>)>,
}

impl Charts {
    /// `pulse_daemon_memory_do()`: the out of memory protection, while the system's memory is known and the
    /// dbengine's start set a protection.
    pub fn update(&mut self, localhost: &Localhost<'_>) {
        let (Some(available), true) = (
            localhost.system_memory_available(),
            localhost.out_of_memory_protection() != 0,
        ) else {
            return;
        };
        let (chart, rd) = self.out_of_memory.get_or_insert_with(|| {
            let chart = localhost.create(&Def {
                id: "out_of_memory_protection",
                family: "Memory Usage",
                context: None,
                title: "Out of Memory Protection",
                units: "bytes",
                module: "pulse",
                priority: 130103,
                chart_type: ChartType::Area,
            });
            let rd = dim(&chart, "available", 1, 1, Algorithm::Absolute);
            (chart, rd)
        });
        set(rd, available as i64);
        localhost.done(chart);
    }
}
