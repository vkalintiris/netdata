//! The tier rollup against C's run1 fixture (D72, S4a commit 1): the fixture generator's three days replayed through
//! the dimensions of a fresh three-tier engine give, after finalize, the tier-1 and tier-2 records C wrote for the
//! same stream. `NETDATA_DBENGINE_FIXTURES` points at the private fixtures; without it the test says it skipped.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_rrd::storage::StorageLayout;
use netdata_agent_rrd::system_info::SystemInfo;
use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
use netdata_agent_storage::dbengine::engine::mrg::Mrg;
use netdata_agent_storage::dbengine::engine::query::{Dbengine, EngineConfig, Priority};
use netdata_agent_storage::dbengine::engine::v2index::{populate, readiness};
use netdata_agent_storage::storage_number::{SN_DEFAULT_FLAGS, SN_EMPTY_SLOT};

const START: i64 = 1_789_980_541;
const END: i64 = 1_790_239_740;
const GAP_FROM: i64 = START + 100_000;
const GAP_TO: i64 = START + 105_000;
/// After run1, so every page validates.
const NOW: i64 = END + 3600;

fn fixtures() -> Option<PathBuf> {
    let dir = std::env::var_os("NETDATA_DBENGINE_FIXTURES").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: NETDATA_DBENGINE_FIXTURES unset");
    }
    dir
}

fn tier_dir(tier: usize) -> String {
    if tier == 0 {
        "dbengine".to_string()
    } else {
        format!("dbengine-tier{tier}")
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        std::fs::copy(entry.path(), &target).unwrap();
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o644);
        std::fs::set_permissions(&target, perms).unwrap();
    }
}

/// A three-tier engine over `root`, copying each tier from `seed` (and populating its registry) when given.
fn engine(root: &Path, seed: Option<&Path>) -> Arc<Dbengine> {
    let mrg = Mrg::new();
    let pool = WorkPool::new(4, 256 * 1024);
    let tiers = (0..3)
        .map(|tier| {
            let dir = root.join(tier_dir(tier));
            match seed {
                Some(seed) => copy_dir(&seed.join(tier_dir(tier)), &dir),
                None => std::fs::create_dir_all(&dir).unwrap(),
            }
            let cfg = TierConfig {
                max_disk_space: 25 * 1024 * 1024,
                ..TierConfig::new(tier, dir)
            };
            let mut loaded = load(cfg, &mrg, NOW).unwrap();
            if seed.is_some() {
                populate(&mut loaded, &mrg, &pool, 4, NOW);
                readiness(&mut loaded, NOW);
            }
            loaded
        })
        .collect();
    Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            main_cache_bytes: 256 * 1024 * 1024,
            extent_cache_bytes: 16 * 1024 * 1024,
            ..EngineConfig::new(|| NOW)
        },
    )
}

/// The b6 child's metrics from the checkers' map: (chart, dimension, UUID).
fn metrics(fx: &Path) -> Vec<(String, String, [u8; 16])> {
    let map = std::fs::read_to_string(fx.join("results/run1/metric-map.tsv")).unwrap();
    map.lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let mut u = [0u8; 16];
            for (i, b) in u.iter_mut().enumerate() {
                *b = u8::from_str_radix(&f[0][2 * i..2 * i + 2], 16).unwrap();
            }
            (f[1].to_string(), f[2].to_string(), u)
        })
        .filter(|(chart, _, _)| chart.starts_with("b6."))
        .collect()
}

/// A tier's records with a value: (end, sum, min, max, count, anomaly count).
type Record = (i64, f64, f64, f64, u32, u32);

fn records(e: &Arc<Dbengine>, uuid: &[u8; 16], tier: usize) -> Vec<Record> {
    let Some(metric) = e.mrg.get_and_acquire(uuid, tier) else {
        return Vec::new();
    };
    let mut q = e.query(&metric, START, NOW, Priority::Normal);
    let mut out = Vec::new();
    while !q.is_finished() {
        let p = q.next_metric();
        if p.sum.is_finite() {
            out.push((p.end_time_s, p.sum, p.min, p.max, p.count, p.anomaly_count));
        }
    }
    out
}

fn host_info() -> HostInfo {
    HostInfo {
        hostname: "b6child".into(),
        registry_hostname: "b6child".into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "netdata".into(),
        program_version: "v0".into(),
        update_every: 1,
        db_mode: DbMode::Dbengine,
        history_entries: 3600,
        health_enabled: false,
        system_info: SystemInfo::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    }
}

#[test]
fn replayed_tiers_equal_run1() {
    let Some(fx) = fixtures() else {
        return;
    };
    let work = tempfile::tempdir().unwrap();
    let c = engine(&work.path().join("c"), Some(&fx.join("run1/cache")));
    let ours = engine(&work.path().join("rust"), None);
    let storage = Arc::new(StorageLayout::new(Some(Arc::clone(&ours))));
    let host = Host::with_storage(
        "b6b6b6b6-1111-4111-8111-000000000001",
        false,
        host_info(),
        &storage,
    );
    let mut dims = Vec::new();
    for c_idx in 0..4 {
        let id = format!("c{c_idx}");
        let (chart, _) = host.charts().create(&ChartSpec {
            type_: "b6",
            id: &id,
            name: None,
            family: None,
            context: Some("b6.ctx"),
            title: "b6",
            units: "u",
            plugin: "b6",
            module: None,
            priority: 1,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: DbMode::Dbengine,
            history_entries: 3600,
            page_size: 4096,
        });
        for d_idx in 0..5 {
            let (dim, _) = chart.dim_add(&format!("d{d_idx}"), None, 1, 1, Algorithm::Absolute);
            dims.push((c_idx, d_idx, dim));
        }
    }
    // the generator (fixture README): (t/10 % 1000) + c/2 + d/8; c1.d4 empty at t % 1000 == 500, c2 anomalous at
    // t % 97 == 0, c3 silent over its gap
    for t in START..=END {
        for (c_idx, d_idx, dim) in &dims {
            if *c_idx == 3 && (GAP_FROM..GAP_TO).contains(&t) {
                continue;
            }
            let t_ut = t as u64 * 1_000_000;
            if *c_idx == 1 && *d_idx == 4 && t % 1000 == 500 {
                dim.store_metric(t_ut, f64::NAN, SN_EMPTY_SLOT);
                continue;
            }
            let value =
                (t / 10 % 1000) as f64 + f64::from(*c_idx) * 0.5 + f64::from(*d_idx) * 0.125;
            let flags = if *c_idx == 2 && t % 97 == 0 {
                0
            } else {
                SN_DEFAULT_FLAGS
            };
            dim.store_metric(t_ut, value, flags);
        }
    }
    for (_, _, dim) in &dims {
        dim.finalize_collection();
    }
    let by_id = |c_idx: u32, d_idx: u32| {
        dims.iter()
            .find(|(c, d, _)| *c == c_idx && *d == d_idx)
            .map(|(_, _, dim)| *dim.uuid())
            .unwrap()
    };
    let (mut tier1, mut tier2) = (0, 0);
    for (chart, dim, c_uuid) in metrics(&fx) {
        let c_idx: u32 = chart.trim_start_matches("b6.c").parse().unwrap();
        let d_idx: u32 = dim.trim_start_matches('d').parse().unwrap();
        let r_uuid = by_id(c_idx, d_idx);
        for tier in 1..3 {
            let (want, got) = (records(&c, &c_uuid, tier), records(&ours, &r_uuid, tier));
            assert!(
                !want.is_empty(),
                "{chart}.{dim} tier {tier}: C wrote no records"
            );
            if let Some(i) = (0..want.len().max(got.len())).find(|&i| want.get(i) != got.get(i)) {
                panic!(
                    "{chart}.{dim} tier {tier}: record {i} of {} (ours {}): C {:?}, ours {:?}",
                    want.len(),
                    got.len(),
                    want.get(i),
                    got.get(i)
                );
            }
            *if tier == 1 { &mut tier1 } else { &mut tier2 } += want.len();
        }
    }
    eprintln!("compared tier-1 records {tier1}, tier-2 records {tier2}");
    // run1's checker counts (results/run1/expected.json)
    assert_eq!((tier1, tier2), (85_965, 1_440));
}
