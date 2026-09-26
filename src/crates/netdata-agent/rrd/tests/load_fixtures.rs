//! The contexts loader over the C-written snapshots of the private fixtures (`<fx>/runR/cache/`), with tiers built
//! from the dbengine pages: `NETDATA_DBENGINE_FIXTURES` points at them, and without it the test says it skipped.
//! Brief `knowledge/brief-dbengine-s1.md` §5.2 item 5 in the status repository; C's counts come from its runR log.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use netdata_agent_metadata::open::{ContextDb, MetaDb, SqliteSettings};
use netdata_agent_metadata::read::{chart_list, context_list, dimension_list};
use netdata_agent_rrd::chart::{Algorithm, ChartType};
use netdata_agent_rrd::contexts::{
    Contexts, LoadReport, SqlChart, SqlContext, SqlDim, TierRetention,
};
use netdata_agent_storage::dbengine::format::descriptor::PAGE_TYPE_GORILLA_32BIT;
use netdata_agent_storage::dbengine::format::inspect::for_each_page;

/// A tier's retention per metric: the first page's start and the last page's end.
#[derive(Debug, Default)]
struct PagesTier(HashMap<[u8; 16], (i64, i64)>);

impl PagesTier {
    fn read(dir: &Path) -> PagesTier {
        let mut t = PagesTier::default();
        for_each_page(dir, |d, _| {
            let start = (d.start_time_ut / 1_000_000) as i64;
            let end = if d.page_type == PAGE_TYPE_GORILLA_32BIT {
                start + i64::from(d.gorilla_delta_s())
            } else {
                (d.end_time_ut() / 1_000_000) as i64
            };
            let e = t.0.entry(d.uuid).or_insert((start, end));
            e.0 = e.0.min(start);
            e.1 = e.1.max(end);
        })
        .unwrap();
        t
    }
}

impl TierRetention for PagesTier {
    fn retention_by_id(&self, uuid: &[u8; 16]) -> Option<(i64, i64)> {
        self.0.get(uuid).copied()
    }
}

fn load(
    meta: &MetaDb,
    context_db: &ContextDb,
    tiers: &[Arc<PagesTier>],
    host: &[u8; 16],
    name: &str,
) -> (LoadReport, Contexts) {
    let contexts = Contexts::default();
    contexts.set_tiers(
        tiers
            .iter()
            .map(|t| Arc::clone(t) as Arc<dyn TierRetention>)
            .collect(),
    );
    let mut loader = contexts.loader().unwrap();
    context_list(&context_db.lock(), host, |r| {
        loader.context(&SqlContext {
            id: r.id,
            version: r.version as u64,
            title: r.title.map(String::into_bytes),
            chart_type: r.chart_type,
            units: r.units,
            priority: r.priority as u64,
            first_time_s: r.first_time_s as u64,
            last_time_s: r.last_time_s as u64,
            deleted: r.deleted,
            family: r.family.map(String::into_bytes),
        });
    });
    chart_list(&meta.lock(), host, |r| {
        loader.chart(&SqlChart {
            chart_id: r.chart_id,
            id: r.id,
            name: r.name,
            context: r.context,
            title: r.title,
            units: r.units,
            priority: r.priority,
            update_every: r.update_every,
            chart_type: ChartType::from_id(r.chart_type),
            family: r.family,
        });
    });
    dimension_list(&meta.lock(), host, |r| {
        loader.dim(&SqlDim {
            dim_id: r.dim_id,
            id: r.id,
            name: r.name,
            hidden: r.hidden,
            chart_id: r.chart_id,
            context: r.context,
            algorithm: Algorithm::from_id(r.algorithm),
        });
    });
    (loader.finish(name, || false), contexts)
}

/// The loader reproduces C's `RRDCONTEXT: metadata for node` counts for runR's parent and child, and the child's
/// context retention C's API shows.
#[test]
fn the_loader_counts_as_c_on_a_c_cache() {
    let Some(fx) = std::env::var_os("NETDATA_DBENGINE_FIXTURES").map(PathBuf::from) else {
        eprintln!("skipped: NETDATA_DBENGINE_FIXTURES unset");
        return;
    };
    let cache = fx.join("runR/cache");
    let dir = tempfile::tempdir().unwrap();
    for file in ["netdata-meta.db", "context-meta.db"] {
        let copy = dir.path().join(file);
        std::fs::copy(cache.join(file), &copy).unwrap();
        std::fs::set_permissions(&copy, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .unwrap();
    }
    let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
    let context_db = ContextDb::open(dir.path(), &SqliteSettings::default()).unwrap();
    let tiers: Vec<Arc<PagesTier>> = ["dbengine", "dbengine-tier1", "dbengine-tier2"]
        .iter()
        .map(|t| Arc::new(PagesTier::read(&cache.join(t))))
        .collect();
    let hosts: Vec<([u8; 16], String, i64)> = {
        let c = meta.lock();
        let mut stmt = c
            .prepare("SELECT host_id, hostname, hops FROM host ORDER BY hops")
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?.try_into().unwrap(),
                r.get(1)?,
                r.get(2)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    };
    let counts = |r: &LoadReport| {
        (
            r.contexts,
            r.contexts_deleted,
            r.instances,
            r.instances_deleted,
            r.instances_ignored,
            r.metrics,
            r.metrics_ignored,
            r.metrics_zero_retention,
        )
    };
    let (parent, _) = load(&meta, &context_db, &tiers, &hosts[0].0, &hosts[0].1);
    assert_eq!(
        counts(&parent),
        (18, 0, 22, 0, 0, 96, 0, 0),
        "{}",
        hosts[0].1
    );
    let child = hosts.iter().find(|h| h.1 == "b6child").unwrap();
    let (report, contexts) = load(&meta, &context_db, &tiers, &child.0, &child.1);
    assert_eq!(counts(&report), (1, 0, 4, 0, 0, 20, 0, 0));
    let rc = contexts.get("b6.ctx").unwrap();
    assert_eq!(
        (rc.state().first_time_s, rc.state().last_time_s),
        (1_789_980_541, 1_790_239_740)
    );
}
