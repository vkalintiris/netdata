//! Test fixtures shared by the query modules.

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType, dim_flags};
use netdata_agent_rrd::contexts::{SqlChart, SqlDim};
use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_rrd::storage::{AlertClass, ChartAlert, StorageLayout};
use netdata_agent_storage::dbengine::engine::query::Dbengine;
use netdata_agent_storage::storage_number::SN_FLAG_NOT_ANOMALOUS;

use crate::request::{parse_v1, parse_v2};
use crate::target::{QueryTarget, Source, create};
use crate::window::{Window, calculate};

pub const T0: i64 = 1_700_000_000;
pub const E: f64 = f64::NAN;

/// A host's info: `hostname` collected every `update_every` seconds into `db_mode`.
pub fn info(hostname: &str, update_every: i32, db_mode: DbMode) -> HostInfo {
    HostInfo {
        hostname: hostname.into(),
        registry_hostname: hostname.into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "p".into(),
        program_version: "1".into(),
        update_every,
        db_mode,
        history_entries: 3600,
        health_enabled: false,
        system_info: Default::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    }
}

/// `ctx.a`'s chart `t.a` in ram mode, collected every second, keeping `history` points.
fn ram_chart(history: i64) -> ChartSpec<'static> {
    ChartSpec {
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
        history_entries: history,
        page_size: 4096,
    }
}

/// The spec's worked example (§5.9): one ram dimension with `10, E, 20, 30, E, 40` at `T0+1..=T0+6`.
pub fn host() -> Arc<Host> {
    let h = Arc::new(Host::new("guid-1", false, info("child", 1, DbMode::Ram)));
    let (chart, _) = h.charts().create(&ram_chart(3600));
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

/// How many points [`weights_host`] holds, and the wall clock of its queries: the second after its last point.
pub const W_POINTS: i64 = 240;
pub const W_NOW: i64 = T0 + W_POINTS + 1;

/// A host for the queries the weights endpoints make per metric: chart `t.w` of `ctx.w` in ram mode, a point a
/// second at `T0+1..=T0+240`, the last 60 of them anomalous. `a` rises by one from 1; `b` stays at 5; `z` stays
/// at zero; `hid` is hidden and stays at 3; `step` is zero for the first 120 points and 10 after.
pub fn weights_host() -> Arc<Host> {
    weights_host_as("guid-w", "weights")
}

/// [`weights_host`] under another machine GUID and name.
pub fn weights_host_as(guid: &str, hostname: &str) -> Arc<Host> {
    let h = Arc::new(Host::new(guid, false, info(hostname, 1, DbMode::Ram)));
    let (chart, _) = h.charts().create(&ChartSpec {
        id: "w",
        context: Some("ctx.w"),
        ..ram_chart(3600)
    });
    for id in ["a", "b", "z", "hid", "step"] {
        chart.dim_add(id, None, 1, 1, Algorithm::Absolute);
    }
    let dim = |id: &str| chart.dim(id).expect("a dimension of the fixture");
    dim("hid").update_meta(|m| m.flags |= dim_flags::HIDDEN);
    for i in 1..=W_POINTS {
        let at = (T0 + i) as u64 * 1_000_000;
        let flags = if i > W_POINTS - 60 { 0 } else { SN_FLAG_NOT_ANOMALOUS };
        let step = if i > W_POINTS / 2 { 10.0 } else { 0.0 };
        for (id, value) in [("a", i as f64), ("b", 5.0), ("z", 0.0), ("hid", 3.0), ("step", step)] {
            dim(id).store_metric(at, value, flags);
        }
    }
    h.contexts().process_queued();
    h
}

/// A host whose chart and dimension have names that are not their ids: chart `t.n` named `t.named` of `ctx.n`
/// (units `things`), its dimension `d` named `dee`, with one point at `T0+1`.
pub fn weights_named_host(guid: &str, hostname: &str) -> Arc<Host> {
    let h = Arc::new(Host::new(guid, false, info(hostname, 1, DbMode::Ram)));
    let (chart, _) = h.charts().create(&ChartSpec {
        id: "n",
        name: Some("named"),
        context: Some("ctx.n"),
        units: "things",
        ..ram_chart(3600)
    });
    let (dim, _) = chart.dim_add("d", Some("dee"), 1, 1, Algorithm::Absolute);
    dim.store_metric((T0 + 1) as u64 * 1_000_000, 1.0, SN_FLAG_NOT_ANOMALOUS);
    h.contexts().process_queued();
    h
}

/// What health would show of [`host`]'s chart: three alerts, one in each of three classes, in link order, with
/// the values 12.5, 0 and none.
struct ThreeAlerts;

impl netdata_agent_rrd::storage::AlertView for ThreeAlerts {
    fn versions(&self, _: &Host) -> (u64, u64) {
        (7, 9)
    }

    fn chart_alerts(&self, _: &Host, chart: &netdata_agent_rrd::chart::Chart) -> Vec<ChartAlert> {
        if chart.id() != "t.a" {
            return Vec::new();
        }
        let alerts = [
            ("a_warn", AlertClass::Warning, "WARNING", 12.5),
            ("a_clear", AlertClass::Clear, "CLEAR", 0.0),
            ("a_undef", AlertClass::Other, "UNDEFINED", f64::NAN),
        ];
        let alert = |(name, class, status_name, value): (&str, AlertClass, &'static str, f64)| ChartAlert {
            name: name.into(),
            class,
            status_name,
            at_least_clear: class != AlertClass::Other,
            value,
            units: b"things".to_vec(),
        };
        alerts.into_iter().map(alert).collect()
    }
}

/// [`host`], with health showing three alerts on its chart.
pub fn host_with_alerts() -> Arc<Host> {
    let h = host();
    h.storage().set_alert_view(Arc::new(ThreeAlerts));
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
        Source::V2 { hosts: vec![Arc::clone(h)] },
        T0 + 7,
    );
    let window = calculate(&qt, T0 + 7).expect("window");
    (qt, window)
}

/// The metric of [`dbengine_host`] and [`dbengine_pages_host`].
pub const U: [u8; 16] = [0x11; 16];

/// An engine of three empty tiers over temporary directories, loaded and clocked at `now()`, for charts collected
/// every `update_every_s`.
fn engine(update_every_s: u32, now: fn() -> i64) -> (Vec<tempfile::TempDir>, Arc<Dbengine>) {
    use netdata_agent_storage::dbengine::engine::cache::CacheConfig;
    use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
    use netdata_agent_storage::dbengine::engine::mrg::Mrg;
    use netdata_agent_storage::dbengine::engine::query::EngineConfig;
    let mrg = Mrg::new();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let tiers = dirs
        .iter()
        .enumerate()
        .map(|(tier, dir)| {
            load(TierConfig::new(tier, dir.path().to_path_buf()), &mrg, now()).unwrap()
        })
        .collect();
    let engine = Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            caches: CacheConfig::new(1 << 20, 1 << 20),
            update_every_s,
            ..EngineConfig::new(now)
        },
    );
    (dirs, engine)
}

/// A dbengine host over `engine` (grouping 3 then 2, so a chart of update every 10 has tiers of 10, 30 and 60 s),
/// with `ctx.a`'s chart `t.a` and its metric `d` of `algorithm` loaded from SQL rows.
fn dbengine_host_over(engine: Arc<Dbengine>, algorithm: Algorithm) -> Arc<Host> {
    let storage = Arc::new(StorageLayout::new(Some(engine)).with_profile(vec![10, 3, 2], 10));
    let h = Arc::new(Host::with_storage(
        "guid-db",
        false,
        info("db", 10, DbMode::Dbengine),
        &storage,
    ));
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
        algorithm,
    });
    loader.finish("db", || false, |_| {});
    h
}

/// A [`dbengine_host_over`] three empty tiers, clocked at `T0 + 1000`; `retention` is the metric's (first, last) per
/// tier in the registry, (0, 0) for none.
pub fn dbengine_host(retention: [(i64, i64); 3]) -> (Vec<tempfile::TempDir>, Arc<Host>) {
    let (dirs, engine) = engine(10, || T0 + 1000);
    for (tier, (first, last)) in retention.into_iter().enumerate() {
        if first != 0 || last != 0 {
            drop(engine.mrg.add_and_acquire(&U, tier, first, last, 10));
        }
    }
    (dirs, dbengine_host_over(engine, Algorithm::Absolute))
}

/// The engine of [`dbengine_pages_host`]: its collections stay open, so their pages stay hot.
pub struct Pages {
    _collect: Vec<netdata_agent_storage::dbengine::engine::collect::CollectHandle>,
    _dirs: Vec<tempfile::TempDir>,
}

/// A [`dbengine_host_over`] tiers holding points in hot pages, clocked at `T0 + 100_000`: per tier, the end times
/// `(first, last)` of a point every tier update every, each the tier's grouping of samples of `value(tier, end)`;
/// (0, 0) stores nothing.
pub fn dbengine_pages_host(
    points: [(i64, i64); 3],
    value: impl Fn(usize, i64) -> f64,
    algorithm: Algorithm,
) -> (Pages, Arc<Host>) {
    use netdata_agent_storage::dbengine::engine::collect::{Alignment, CollectHandle};
    let (dirs, engine) = engine(10, || T0 + 100_000);
    let mut open = Vec::new();
    for (tier, (first, last)) in points.into_iter().enumerate() {
        if first == 0 && last == 0 {
            continue;
        }
        let ue = [10, 30, 60][tier];
        let samples = (ue / 10) as u16;
        let (metric, _) = engine.mrg.add_and_acquire(&U, tier, 0, 0, ue as u32);
        let mut collect = CollectHandle::init(
            &engine,
            &metric,
            ue as u32,
            Alignment::new("guid-db", "t.a", tier),
        );
        for t in (first..=last).step_by(ue as usize) {
            let value = value(tier, t);
            collect.store_next(
                t as u64 * 1_000_000,
                value * f64::from(samples),
                value,
                value,
                samples,
                0,
                SN_FLAG_NOT_ANOMALOUS,
            );
        }
        open.push(collect);
    }
    let h = dbengine_host_over(engine, algorithm);
    (
        Pages {
            _collect: open,
            _dirs: dirs,
        },
        h,
    )
}

/// A ram host whose chart `t.a` keeps `history` points in its ring and rolls tiers 1 and 2 up by `groupings`, with
/// `value(t)` stored at every second of `span`; the ring answers tier 0.
pub fn ram_tiers_host(
    history: i64,
    groupings: [u64; 2],
    span: std::ops::RangeInclusive<i64>,
    value: impl Fn(i64) -> f64,
) -> (Vec<tempfile::TempDir>, Arc<Host>) {
    let (dirs, engine) = engine(1, || T0 + 100_000);
    let storage = Arc::new(
        StorageLayout::new(Some(engine)).with_profile(vec![1, groupings[0], groupings[1]], 1),
    );
    let h = Arc::new(Host::with_storage(
        "guid-ram",
        false,
        info("ram", 1, DbMode::Ram),
        &storage,
    ));
    let (chart, _) = h.charts().create(&ram_chart(history));
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    for t in span {
        dim.store_metric(t as u64 * 1_000_000, value(t), SN_FLAG_NOT_ANOMALOUS);
    }
    h.contexts().process_queued();
    (dirs, h)
}

/// A v1 context query on `ctx.a` of `h` with three tiers in use, built at `now`, with its window.
pub fn v1_tiers_target(h: &Arc<Host>, query: &str, now: i64) -> (QueryTarget, Window) {
    let profile = crate::request::Profile {
        storage_tiers: 3,
        update_every: h.info().update_every.into(),
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
