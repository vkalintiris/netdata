//! `rrdset_timed_done()`'s `service_running(SERVICE_COLLECTORS)` guard (D110): once the exit started a collection
//! stores nothing. Its own test binary, since the exit's start is process-wide.

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
use netdata_agent_rrd::collection::{set_value, timed_done};
use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_rrd::system_info::SystemInfo;
use netdata_agent_rrd::upstream::BufferSource;

#[test]
fn a_collection_after_the_exit_started_stores_nothing() {
    let info = HostInfo {
        hostname: "exit".into(),
        registry_hostname: "exit".into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "netdata".into(),
        program_version: "v0".into(),
        update_every: 1,
        db_mode: DbMode::Ram,
        history_entries: 60,
        health_enabled: false,
        system_info: SystemInfo::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    };
    let host = Host::with_storage("00000000-0000-0000-0000-000000000002", true, info, &Arc::default());
    let (chart, _) = host.charts().create(&ChartSpec {
        type_: "t",
        id: "c",
        name: None,
        family: None,
        context: None,
        title: "t",
        units: "u",
        plugin: "p",
        module: None,
        priority: 1,
        update_every: 1,
        chart_type: ChartType::Line,
        mode: DbMode::Ram,
        history_entries: 60,
        page_size: 4096,
    });
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    const T: i64 = 1_700_000_000;
    for i in 0..3 {
        set_value(&dim, (T + i, 0), 7);
        timed_done(&host, &chart, (T + i, 0), i != 0, 3, BufferSource::Thread);
    }
    let before = chart.collection();
    assert_eq!(before.counter_done, 3);
    netdata_agent_sys::exit::add(1);
    for i in 3..6 {
        set_value(&dim, (T + i, 0), 7);
        timed_done(&host, &chart, (T + i, 0), true, 3, BufferSource::Thread);
    }
    assert_eq!(chart.collection(), before, "nothing collected or stored after the exit started");
}
