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
        stream_send: None,
        cache_dir: None,
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
    let p = parse_v1(
        format!("context=ctx.a&{query}").as_bytes(),
        &crate::request::Profile::default(),
    );
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
        parse_v2(query.as_bytes(), 2, &crate::request::Profile::default()),
        Source::V2 {
            hosts: vec![Arc::clone(h)],
            nodes_hard_hash: 1,
        },
        T0 + 7,
    );
    let window = calculate(&qt, T0 + 7).expect("window");
    (qt, window)
}

/// The metric of [`dbengine_host`].
pub const U: [u8; 16] = [0x11; 16];

/// A dbengine host over an engine of three empty tiers (grouping 3 then 2, so a chart of update every 10 has tiers of
/// 10, 30 and 60 s), with `ctx.a`'s chart `t.a` and its metric `d` loaded from SQL rows; `retention` is the metric's
/// (first, last) per tier in the registry, (0, 0) for none.
pub fn dbengine_host(retention: [(i64, i64); 3]) -> (Vec<tempfile::TempDir>, Arc<Host>) {
    use netdata_agent_rrd::contexts::{SqlChart, SqlDim};
    use netdata_agent_rrd::storage::StorageLayout;
    use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
    use netdata_agent_storage::dbengine::engine::mrg::Mrg;
    use netdata_agent_storage::dbengine::engine::query::{Dbengine, EngineConfig};
    let mrg = Mrg::new();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let tiers = dirs
        .iter()
        .enumerate()
        .map(|(tier, dir)| {
            let cfg = TierConfig::new(tier, dir.path().to_path_buf());
            load(cfg, &mrg, T0 + 1000).unwrap()
        })
        .collect();
    for (tier, (first, last)) in retention.into_iter().enumerate() {
        if first != 0 || last != 0 {
            drop(mrg.add_and_acquire(&U, tier, first, last, 10));
        }
    }
    let engine = Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            main_cache_bytes: 1 << 20,
            extent_cache_bytes: 1 << 20,
            update_every_s: 10,
            ..EngineConfig::new(|| T0 + 1000)
        },
    );
    let storage =
        Arc::new(StorageLayout::new(Some(engine)).with_profile(vec![10, 3, 2], 10));
    let info = HostInfo {
        hostname: "db".into(),
        registry_hostname: "db".into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "p".into(),
        program_version: "1".into(),
        update_every: 10,
        db_mode: DbMode::Dbengine,
        history_entries: 3600,
        health_enabled: false,
        system_info: Default::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    };
    let h = Arc::new(Host::with_storage("guid-db", false, info, &storage));
    let mut loader = h.contexts().loader().unwrap();
    loader.chart(&SqlChart {
        chart_id: [0x22; 16],
        id: Some("t.a".into()),
        name: Some("t.a".into()),
        context: Some("ctx.a".into()),
        title: Some("T".into()),
        units: Some("u".into()),
        priority: 1000,
        update_every: 10,
        chart_type: ChartType::Line,
        family: Some("f".into()),
    });
    loader.dim(&SqlDim {
        dim_id: U,
        id: Some("d".into()),
        name: Some("d".into()),
        hidden: false,
        chart_id: Some("t.a".into()),
        context: Some("ctx.a".into()),
        algorithm: Algorithm::Absolute,
    });
    loader.finish("db", || false, |_| {});
    (dirs, h)
}

/// A v1 context query on `ctx.a` of `h` with three tiers in use, built at `now`, with its window.
pub fn v1_tiers_target(h: &Arc<Host>, query: &str, now: i64) -> (QueryTarget, Window) {
    let profile = crate::request::Profile {
        storage_tiers: 3,
        update_every: 10,
    };
    let p = parse_v1(format!("context=ctx.a&{query}").as_bytes(), &profile);
    let qt = create(
        p.request,
        Source::V1 {
            host: h,
            chart: None,
        },
        now,
    );
    let window = calculate(&qt, now).expect("window");
    (qt, window)
}
