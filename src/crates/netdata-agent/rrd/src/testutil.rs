//! Test fixtures of the rrd crate: hosts, engine-backed storage, charts and tier readers.

use std::sync::Arc;

use crate::host::{Attach, Host, HostInfo};
use crate::mode::DbMode;
use crate::storage::StorageLayout;
use crate::system_info::SystemInfo;

pub(crate) fn info(hostname: &str) -> HostInfo {
    HostInfo {
        hostname: hostname.into(),
        registry_hostname: hostname.into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "netdata".into(),
        program_version: "v0".into(),
        update_every: 1,
        db_mode: DbMode::Ram,
        history_entries: 4096,
        health_enabled: false,
        system_info: SystemInfo::default(),
        replication_enabled: true,
        replication_period: 86400,
        replication_step: 3600,
        stream_send: None,
        cache_dir: None,
    }
}

/// An engine of `tiers` empty tiers over temporary directories, and its registry.
pub(crate) fn engine(tiers: usize) -> (Vec<tempfile::TempDir>, Arc<StorageLayout>) {
    use netdata_agent_storage::dbengine::engine::cache::CacheConfig;
    use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
    use netdata_agent_storage::dbengine::engine::mrg::Mrg;
    use netdata_agent_storage::dbengine::engine::query::{Dbengine, EngineConfig};
    let mrg = Mrg::new();
    let dirs: Vec<_> = (0..tiers).map(|_| tempfile::tempdir().unwrap()).collect();
    let tiers = dirs
        .iter()
        .enumerate()
        .map(|(tier, dir)| {
            let cfg = TierConfig::new(tier, dir.path().to_path_buf());
            load(cfg, &mrg, 1_800_000_000).unwrap()
        })
        .collect();
    let engine = Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            caches: CacheConfig::new(1 << 20, 1 << 20),
            ..EngineConfig::new(|| 1_800_000_000)
        },
    );
    (dirs, Arc::new(StorageLayout::new(Some(engine))))
}

/// The spec of the test chart `t.c`, of `mode`, collected every second.
pub(crate) fn chart_spec(mode: DbMode) -> crate::chart::ChartSpec<'static> {
    crate::chart::ChartSpec {
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
        chart_type: crate::chart::ChartType::Line,
        mode,
        history_entries: 60,
        page_size: 4096,
    }
}

/// A chart of `mode` collected every second.
pub(crate) fn collected_chart(host: &Host, mode: DbMode) -> Arc<crate::chart::Chart> {
    host.charts().create(&chart_spec(mode)).0
}

pub(crate) fn store(dim: &crate::chart::Dim, t: i64, v: f64) {
    use netdata_agent_storage::storage_number::SN_DEFAULT_FLAGS;
    dim.store_metric(t as u64 * 1_000_000, v, SN_DEFAULT_FLAGS);
}

/// The records with a value of a dimension's tier.
pub(crate) fn tier_records(
    e: &Arc<netdata_agent_storage::dbengine::engine::query::Dbengine>,
    dim: &crate::chart::Dim,
    tier: usize,
) -> Vec<(i64, f64, u32)> {
    use netdata_agent_storage::dbengine::engine::query::Priority;
    let metric = e.mrg.get_and_acquire(dim.uuid(), tier).unwrap();
    let mut q = e.query(&metric, 1, 1_900_000_000, Priority::Normal);
    let mut out = Vec::new();
    while !q.is_finished() {
        let p = q.next_metric();
        if p.sum.is_finite() {
            out.push((p.end_time_s, p.sum, p.count));
        }
    }
    out
}

/// A dimension of a host of `mode` over a three-tier engine with windows of 5 and 15 s, of the host's first
/// chart (flush modulo 1), with the backfill mode given; its host (streamed to by `slot`) and chart.
pub(crate) struct BackfillFixture {
    pub _dirs: Vec<tempfile::TempDir>,
    pub host: Arc<Host>,
    pub chart: Arc<crate::chart::Chart>,
    pub dim: Arc<crate::chart::Dim>,
    pub engine: Arc<netdata_agent_storage::dbengine::engine::query::Dbengine>,
    pub slot: Arc<crate::host::ReceiverSlot>,
}

pub(crate) fn backfill_fixture(
    backfill: crate::storage::Backfill,
    mode: DbMode,
) -> BackfillFixture {
    use crate::chart::Algorithm;
    use crate::host::{ReceiverLink, ReceiverSlot};
    let (dirs, storage) = engine(3);
    let storage = Arc::new(
        Arc::try_unwrap(storage)
            .unwrap()
            .with_profile(vec![1, 5, 3], 1)
            .with_backfill(backfill),
    );
    let e = Arc::clone(storage.dbengine().unwrap());
    let host = Host::with_storage(
        "guid-b",
        false,
        HostInfo {
            db_mode: mode,
            ..info("b")
        },
        &storage,
    )
    .into_shared();
    let chart = collected_chart(&host, mode);
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    let slot = Arc::new(ReceiverSlot::new(
        1,
        Default::default(),
        ReceiverLink::default(),
        Box::new(|| {}),
    ));
    assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
    BackfillFixture {
        _dirs: dirs,
        host,
        chart,
        dim,
        engine: e,
        slot,
    }
}

/// A fixture's dimension alone.
pub(crate) fn backfill_dim(
    backfill: crate::storage::Backfill,
    mode: DbMode,
) -> (
    Vec<tempfile::TempDir>,
    Arc<crate::chart::Dim>,
    Arc<netdata_agent_storage::dbengine::engine::query::Dbengine>,
) {
    let f = backfill_fixture(backfill, mode);
    (f._dirs, f.dim, f.engine)
}
