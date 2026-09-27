//! What every pulse chart shares: `rrdset_create_localhost()`, `rrddim_add()`, `rrddim_set_by_pointer()` and
//! `rrdset_done()`.

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, Chart, ChartSpec, ChartType, Dim};
use netdata_agent_rrd::collection::{now_realtime_timeval, set_value, timed_done};
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::mode::DbMode;

use crate::Settings;

/// A pulse chart: type `netdata`, no name, plugin `netdata`.
pub(crate) struct Def<'a> {
    pub id: &'a str,
    pub family: &'a str,
    pub context: Option<&'a str>,
    pub title: &'a str,
    pub units: &'a str,
    pub module: &'a str,
    pub priority: i64,
    pub chart_type: ChartType,
}

/// Localhost for one cycle, with what its charts take from it read once.
pub(crate) struct Localhost<'a> {
    pub host: &'a Host,
    hostname: String,
    settings: &'a Settings,
    /// `localhost->rrd_update_every`, `rrd_memory_mode` and `rrd_history_entries`.
    update_every: i32,
    mode: DbMode,
    history_entries: i64,
}

impl<'a> Localhost<'a> {
    pub fn new(host: &'a Host, settings: &'a Settings) -> Self {
        let info = host.info();
        Localhost {
            host,
            hostname: info.hostname,
            settings,
            update_every: info.update_every,
            mode: info.db_mode,
            history_entries: info.history_entries,
        }
    }

    /// localhost's `rrd_memory_mode`.
    pub fn mode(&self) -> DbMode {
        self.mode
    }

    /// `os_system_memory(true).ram_available_bytes`, while the system's memory is known.
    pub fn system_memory_available(&self) -> Option<u64> {
        (self.settings.system_memory)()
    }

    /// `dbengine_out_of_memory_protection`.
    pub fn out_of_memory_protection(&self) -> u64 {
        self.settings.out_of_memory_protection
    }

    /// `rrdset_create_localhost()` with localhost's update every.
    pub fn create(&self, def: &Def<'_>) -> Arc<Chart> {
        self.create_every(def, self.update_every)
    }

    /// `rrdset_create_localhost()`.
    pub fn create_every(&self, def: &Def<'_>, update_every: i32) -> Arc<Chart> {
        let spec = ChartSpec {
            type_: "netdata",
            id: def.id,
            name: None,
            family: Some(def.family),
            context: def.context,
            title: def.title,
            units: def.units,
            plugin: "netdata",
            module: Some(def.module),
            priority: def.priority,
            update_every,
            chart_type: def.chart_type,
            mode: self.mode,
            history_entries: self.history_entries,
            page_size: self.settings.page_size,
        };
        self.host.charts().create(&spec).0
    }

    /// `rrdset_done()`: the collection's time is now, and the chart's next is implicit after its first.
    pub fn done(&self, chart: &Chart) {
        let pending_next = chart.collection().counter_done != 0;
        timed_done(
            chart,
            &self.hostname,
            now_realtime_timeval(),
            pending_next,
            self.settings.gap_when_lost_iterations_above,
        );
    }
}

/// A created chart and its dimensions, in creation order.
pub(crate) type WithDims = (Arc<Chart>, Vec<Arc<Dim>>);

/// `rrddim_add()` without a name.
pub(crate) fn dim(
    chart: &Chart,
    id: &str,
    multiplier: i32,
    divisor: i32,
    algorithm: Algorithm,
) -> Arc<Dim> {
    chart.dim_add(id, None, multiplier, divisor, algorithm).0
}

/// `rrddim_set_by_pointer()`.
pub(crate) fn set(dim: &Dim, value: i64) {
    set_value(dim, now_realtime_timeval(), value);
}
