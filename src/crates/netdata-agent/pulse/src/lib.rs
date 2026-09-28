//! The agent's own charts on localhost, ported from `src/daemon/pulse/`: each cycle of the `PULSE` thread updates
//! them in C's order, creating each on first use. Only the charts of D80 are ported: the extended ones (`[pulse]
//! extended`), ML, gorilla, heartbeat, the dbengine caches, the registry, strings and ARAL are not (D80.4).

#![forbid(unsafe_code)]

mod chart;
mod daemon;
mod daemon_memory;
mod http_api;
mod ingestion;
mod network;
mod parents;
mod queries;
mod retention;

use std::sync::Arc;

use netdata_agent_rrd::host::Hosts;

pub use parents::Gates;

/// What the charts take from the daemon's configuration.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// `[db] gap when lost iterations above`.
    pub gap_when_lost_iterations_above: i64,
    /// The system's page size, to which ram charts align their history.
    pub page_size: i64,
    /// Where the parents module runs.
    pub parents: Gates,
    /// `dbengine_out_of_memory_protection`: what the dbengine's start keeps free, 0 without it.
    pub out_of_memory_protection: u64,
    /// `os_system_memory(true)`: the available bytes, while the total is known.
    pub system_memory: fn() -> Option<u64>,
}

/// The pulse charts, each created on its first update as C's `static RRDSET *`.
pub struct Pulse {
    hosts: Arc<Hosts>,
    settings: Settings,
    ingestion: ingestion::Charts,
    http_api: http_api::Charts,
    queries: queries::Charts,
    network: network::Charts,
    retention: retention::Charts,
    parents: parents::Charts,
    daemon: daemon::Charts,
}

impl Pulse {
    pub fn new(hosts: Arc<Hosts>, settings: Settings) -> Self {
        Pulse {
            hosts,
            settings,
            ingestion: Default::default(),
            http_api: http_api::Charts::new(),
            queries: Default::default(),
            network: Default::default(),
            retention: Default::default(),
            parents: Default::default(),
            daemon: Default::default(),
        }
    }

    /// One cycle of `pulse_thread_main()`.
    pub fn cycle(&mut self) {
        let localhost = chart::Localhost::new(self.hosts.localhost(), &self.settings);
        self.ingestion.update(&localhost);
        self.http_api.update(&localhost);
        self.queries.update(&localhost);
        self.network.update(&localhost);
        self.retention.update(&localhost);
        self.parents
            .update(&localhost, &self.hosts, self.settings.parents);
        self.daemon.update(&localhost);
    }
}

#[cfg(test)]
mod tests;
