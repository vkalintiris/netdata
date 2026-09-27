//! The agent's own charts on localhost, ported from `src/daemon/pulse/`: each cycle of the `PULSE` thread updates
//! them in C's order, creating each on first use. Only the charts of D80 are ported: the extended ones (`[pulse]
//! extended`), ML, gorilla, heartbeat, the dbengine caches, the registry, strings and ARAL are not (D80.4).

mod chart;
mod http_api;
mod ingestion;
mod network;
mod queries;

use std::sync::Arc;

use netdata_agent_rrd::host::Host;

/// What the charts take from the daemon's configuration.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// `[db] gap when lost iterations above`.
    pub gap_when_lost_iterations_above: i64,
    /// The system's page size, to which ram charts align their history.
    pub page_size: i64,
}

/// The pulse charts, each created on its first update as C's `static RRDSET *`.
pub struct Pulse {
    localhost: Arc<Host>,
    settings: Settings,
    ingestion: ingestion::Charts,
    http_api: http_api::Charts,
    queries: queries::Charts,
    network: network::Charts,
}

impl Pulse {
    pub fn new(localhost: Arc<Host>, settings: Settings) -> Self {
        Pulse {
            localhost,
            settings,
            ingestion: Default::default(),
            http_api: http_api::Charts::new(),
            queries: Default::default(),
            network: Default::default(),
        }
    }

    /// One cycle of `pulse_thread_main()`.
    pub fn cycle(&mut self) {
        let localhost = chart::Localhost::new(&self.localhost, &self.settings);
        self.ingestion.update(&localhost);
        self.http_api.update(&localhost);
        self.queries.update(&localhost);
        self.network.update(&localhost);
    }
}

#[cfg(test)]
mod tests;
