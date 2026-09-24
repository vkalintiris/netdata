//! Test fixtures shared by the query modules.

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_storage::storage_number::SN_FLAG_NOT_ANOMALOUS;

use crate::request::{parse_v1, parse_v2};
use crate::target::{QueryTarget, Source, create};
use crate::window::{Window, calculate};

pub const T0: i64 = 1_700_000_000;
pub const E: f64 = f64::NAN;

/// The spec's worked example (§5.9): one ram dimension with `10, E, 20, 30, E, 40` at `T0+1..=T0+6`.
pub fn host() -> Arc<Host> {
    let info = HostInfo {
        hostname: "child".into(),
        registry_hostname: "child".into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "p".into(),
        program_version: "1".into(),
        update_every: 1,
        db_mode: DbMode::Ram,
        history_entries: 3600,
        health_enabled: false,
        system_info: Default::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
    };
    let h = Arc::new(Host::new("guid-1", false, info));
    let (chart, _) = h.charts().create(&ChartSpec {
        type_: "t",
        id: "a",
        name: None,
        family: Some("f"),
        context: Some("ctx.a"),
        title: "T",
        units: "u",
        plugin: "p",
        module: None,
        priority: 1000,
        update_every: 1,
        chart_type: ChartType::Line,
        mode: DbMode::Ram,
        history_entries: 3600,
        page_size: 4096,
    });
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    for (i, v) in [10.0, E, 20.0, 30.0, E, 40.0].into_iter().enumerate() {
        dim.store_metric(
            (T0 + 1 + i as i64) as u64 * 1_000_000,
            v,
            SN_FLAG_NOT_ANOMALOUS,
        );
    }
    h.contexts().process_queued();
    h
}

/// A v1 context query on `ctx.a` of `h`, built at `T0 + 7`, with its window.
pub fn v1_target(h: &Arc<Host>, query: &str) -> (QueryTarget, Window) {
    let p = parse_v1(format!("context=ctx.a&{query}").as_bytes(), 1);
    let qt = create(
        p.request,
        Source::V1 {
            host: h,
            chart: None,
        },
        T0 + 7,
    );
    let window = calculate(&qt, T0 + 7).expect("window");
    (qt, window)
}

/// A v2 query over every host (here `h`), built at `T0 + 7`, with its window.
pub fn v2_target(h: &Arc<Host>, query: &str) -> (QueryTarget, Window) {
    let qt = create(
        parse_v2(query.as_bytes(), 2, 1),
        Source::V2 {
            hosts: vec![Arc::clone(h)],
            nodes_hard_hash: 1,
        },
        T0 + 7,
    );
    let window = calculate(&qt, T0 + 7).expect("window");
    (qt, window)
}
