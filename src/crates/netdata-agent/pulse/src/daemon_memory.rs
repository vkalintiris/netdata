//! `pulse-daemon-memory.c`: the agent's memory by owner, its buffers, and the memory it keeps free for the system. The
//! figures are Rust's own where a cheap source exists, 0 where Rust has no such structure (D80.2, D82).

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, Chart, ChartType, Dim};

use crate::chart::{Def, Localhost, WithDims, dim, set};

/// The memory chart's dimensions (a release build's, without `DICT_WITH_STATS`).
const MEMORY: [&str; 16] = [
    "dbengine",
    "rrd",
    "sqlite3",
    "metadata",
    "uuid",
    "labels",
    "ML",
    "strings",
    "streaming",
    "buffers",
    "workers",
    "aral",
    "judy",
    "slots",
    "other",
    "health log",
];

/// The buffers chart's dimensions: Rust accounts for none of them.
const BUFFERS: [&str; 15] = [
    "queries",
    "collection",
    "aclk",
    "api",
    "functions",
    "sqlite",
    "exporters",
    "health",
    "streaming",
    "streaming cbuf",
    "replication",
    "web",
    "aral-by-size free",
    "aral-judy free",
    "uuid",
];

#[derive(Default)]
pub(crate) struct Charts {
    memory: Option<WithDims>,
    buffers: Option<WithDims>,
    out_of_memory: Option<(Arc<Chart>, Arc<Dim>)>,
}

/// A stacked chart of `Memory Usage` in bytes, its dimensions absolute; `values` are read once it exists, as C reads
/// them after creating it (its own rings count in `rrd`).
fn update<const N: usize>(
    slot: &mut Option<WithDims>,
    localhost: &Localhost<'_>,
    (id, title, priority): (&str, &str, i64),
    dims: &[&str; N],
    values: impl FnOnce() -> [i64; N],
) {
    let (chart, rds) = slot.get_or_insert_with(|| {
        let chart = localhost.create(&Def {
            id,
            family: "Memory Usage",
            context: None,
            title,
            units: "bytes",
            module: "pulse",
            priority,
            chart_type: ChartType::Stacked,
        });
        let rds = dims
            .iter()
            .map(|id| dim(&chart, id, 1, 1, Algorithm::Absolute))
            .collect();
        (chart, rds)
    });
    for (rd, value) in rds.iter().zip(values()) {
        set(rd, value);
    }
    localhost.done(chart);
}

impl Charts {
    /// `pulse_daemon_memory_do()`.
    pub fn update(&mut self, localhost: &Localhost<'_>) {
        let storage = localhost.host.storage();
        update(
            &mut self.memory,
            localhost,
            ("memory", "Netdata Memory", 130100),
            &MEMORY,
            || {
                let mut memory = [0; MEMORY.len()];
                // pulse_dbengine_total_memory: the main and extent caches (the open cache has no figure, the metric
                // registry is left out as C leaves it out when not extended), 0 without the engine
                memory[0] = storage
                    .dbengine()
                    .map_or(0, |e| e.main.bytes() + e.extents.bytes())
                    as i64;
                memory[1] = storage.pulse().rrd_memory.read();
                memory[2] = netdata_agent_sys::sqlite_memory_highwater();
                memory
            },
        );
        update(
            &mut self.buffers,
            localhost,
            ("memory_buffers", "Netdata Memory Buffers", 130102),
            &BUFFERS,
            || [0; BUFFERS.len()],
        );
        self.out_of_memory(localhost);
    }

    /// The out of memory protection, while the system's memory is known and the dbengine's start set a protection.
    fn out_of_memory(&mut self, localhost: &Localhost<'_>) {
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
