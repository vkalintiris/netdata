//! `rrdhost_load_rrdcontext_data()` (`src/database/contexts/rrdcontext-loading.c`): a dbengine host's contexts,
//! instances and metrics from the metadata databases, with retention from its storage tiers, and the SQL changes the
//! load asks for (D64). The SQL readers' rows are mapped to the contexts loader's here.

use std::sync::{Arc, Weak};

use netdata_agent_log::netdata_log_error;
use netdata_agent_metadata::Connection;
use netdata_agent_metadata::open::{ContextDb, MetaDb};
use netdata_agent_metadata::read::{self, ChartRow, ContextRow, DimRow};
use netdata_agent_rrd::chart::{Algorithm, ChartType};
use netdata_agent_rrd::contexts::{LabelSource, SqlChange, SqlChart, SqlContext, SqlDim};
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::mode::DbMode;

use crate::meta_store::{self, no_database};
use crate::shutdown;

/// Where a load reads and what it asks of METASYNC: the shared databases, the context-load thread's own read-only
/// handles when it has them (C's `db_meta_thread`, `db_context_thread`), and the cleanup queue.
pub struct Sources<'a> {
    pub meta: &'a MetaDb,
    pub context_db: Option<&'a Arc<ContextDb>>,
    pub meta_thread: Option<&'a Connection>,
    pub context_thread: Option<&'a Connection>,
    /// `metadata_queue_ctx_host_cleanup()`.
    pub cleanup: &'a dyn Fn([u8; 16], String),
}

/// `load_instance_labels_on_demand()`: a loaded instance's chart labels, read through the shared context database
/// when first used (a missing key or value reads as empty, which the labels refuse as C's `rrdlabels_add()` does).
#[derive(Debug)]
struct SqlLabels(Weak<ContextDb>);

impl LabelSource for SqlLabels {
    fn chart_labels(&self, chart_uuid: &[u8; 16]) -> Vec<(Vec<u8>, Vec<u8>, u32)> {
        let Some(db) = self.0.upgrade() else {
            no_database("ctx_get_label_list");
            return Vec::new();
        };
        read::chart_labels(&db.lock(), chart_uuid)
            .into_iter()
            .map(|(key, value, source)| {
                (
                    key.map(String::into_bytes).unwrap_or_default(),
                    value.map(String::into_bytes).unwrap_or_default(),
                    source,
                )
            })
            .collect()
    }
}

/// `rrdhost_load_rrdcontext_data()`: once per host; nothing is read for a host that is not dbengine. An exit that
/// starts between the three lists stops the load without its record, as in C.
pub fn load_host_contexts(host: &Host, src: &Sources<'_>) {
    let Some(mut loader) = host.contexts().loader() else {
        return;
    };
    if host.info().db_mode != DbMode::Dbengine {
        return;
    }
    let Some(host_id) = meta_store::host_id(host) else {
        return;
    };
    if let Some(db) = src.context_db {
        host.contexts()
            .set_label_source(Arc::new(SqlLabels(Arc::downgrade(db))));
    }
    let on_context = |r: ContextRow| loader.context(&sql_context(r));
    match (src.context_thread, src.context_db) {
        (Some(c), _) => read::context_list(c, &host_id, on_context),
        (None, Some(db)) => read::context_list(&db.lock(), &host_id, on_context),
        (None, None) => no_database("ctx_get_context_list"),
    }
    if shutdown::exiting() {
        return;
    }
    let on_chart = |r: ChartRow| loader.chart(&sql_chart(r));
    match src.meta_thread {
        Some(c) => read::chart_list(c, &host_id, on_chart),
        None => read::chart_list(&src.meta.lock(), &host_id, on_chart),
    }
    if shutdown::exiting() {
        return;
    }
    let on_dim = |r: DimRow| loader.dim(&sql_dim(r));
    match src.meta_thread {
        Some(c) => read::dimension_list(c, &host_id, on_dim),
        None => read::dimension_list(&src.meta.lock(), &host_id, on_dim),
    }
    if shutdown::exiting() {
        return;
    }
    loader.finish(&host.hostname(), shutdown::exiting, |change| match change {
        SqlChange::Cleanup(context) => (src.cleanup)(host_id, context.to_string()),
        SqlChange::Delete(context, version) => {
            delete_context(host, src.context_db, context, version, src.cleanup);
        }
    });
}

/// `rrdcontext_delete_from_sql_unsafe()` of a context the garbage collection removed: on a dbengine host, its row
/// leaves the context database (`ctx_delete_context()`, which queues the host's context cleanup once its statement
/// is prepared), with C's record when it cannot.
pub fn delete_context(
    host: &Host,
    context_db: Option<&Arc<ContextDb>>,
    context: &str,
    version: u64,
    cleanup: &dyn Fn([u8; 16], String),
) {
    if host.info().db_mode != DbMode::Dbengine {
        return;
    }
    let Some(host_id) = meta_store::host_id(host) else {
        return;
    };
    let deleted = match context_db {
        Some(db) => db.delete_context(&host_id, context, || cleanup(host_id, context.to_string())),
        None => {
            no_database("ctx_delete_context");
            false
        }
    };
    if !deleted {
        netdata_log_error!(
            "RRDCONTEXT: failed to delete context '{context}' version {version} from SQL."
        );
    }
}

/// A row of `CTX_GET_CONTEXT_LIST` as the loader takes it (C's integer conversions).
pub fn sql_context(r: ContextRow) -> SqlContext {
    SqlContext {
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
    }
}

/// A row of `CTX_GET_CHART_LIST` as the loader takes it.
pub fn sql_chart(r: ChartRow) -> SqlChart {
    SqlChart {
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
    }
}

/// A row of `CTX_GET_DIMENSION_LIST` as the loader takes it.
pub fn sql_dim(r: DimRow) -> SqlDim {
    SqlDim {
        dim_id: r.dim_id,
        id: r.id,
        name: r.name,
        hidden: r.hidden,
        chart_id: r.chart_id,
        context: r.context,
        algorithm: Algorithm::from_id(r.algorithm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_evloop::work::WorkPool;
    use netdata_agent_metadata::open::SqliteSettings;
    use netdata_agent_rrd::host::{HostInfo, Hosts};
    use netdata_agent_rrd::storage::StorageLayout;
    use netdata_agent_storage::dbengine::engine::cache::CacheConfig;
    use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
    use netdata_agent_storage::dbengine::engine::mrg::Mrg;
    use netdata_agent_storage::dbengine::engine::query::{Dbengine, EngineConfig};
    use netdata_agent_storage::dbengine::engine::v2index::{populate, readiness};
    use std::path::Path;

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            std::fs::copy(entry.path(), &target).unwrap();
            std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o644))
                .unwrap();
        }
    }

    /// runR's cache loaded by the engine an hour after runR's start, with its metadata databases.
    fn runr(fx: &Path, dir: &Path) -> (MetaDb, Arc<ContextDb>, Arc<StorageLayout>) {
        const NOW: i64 = 1_790_351_792 + 3600;
        let cache = fx.join("runR/cache");
        for file in ["netdata-meta.db", "context-meta.db"] {
            let copy = dir.join(file);
            std::fs::copy(cache.join(file), &copy).unwrap();
            std::fs::set_permissions(&copy, std::os::unix::fs::PermissionsExt::from_mode(0o644))
                .unwrap();
        }
        let mrg = Mrg::new();
        let pool = WorkPool::new(4, 256 * 1024);
        let tiers = ["dbengine", "dbengine-tier1", "dbengine-tier2"]
            .iter()
            .enumerate()
            .map(|(tier, name)| {
                let path = dir.join(name);
                copy_dir(&cache.join(name), &path);
                let cfg = TierConfig {
                    max_disk_space: 25 * 1024 * 1024,
                    ..TierConfig::new(tier, path)
                };
                let mut loaded = load(cfg, &mrg, NOW).unwrap();
                populate(&mut loaded, &mrg, &pool, 4, NOW);
                readiness(&mut loaded, NOW);
                loaded
            })
            .collect();
        let engine = Dbengine::new(
            mrg,
            tiers,
            EngineConfig {
                caches: CacheConfig::new(1 << 24, 1 << 22),
                ..EngineConfig::new(|| NOW)
            },
        );
        let settings = SqliteSettings::default();
        (
            MetaDb::open(dir, &settings).unwrap(),
            Arc::new(ContextDb::open(dir, &settings).unwrap()),
            Arc::new(StorageLayout::new(Some(engine))),
        )
    }

    /// Over runR's C-written cache, the load gives C's `RRDCONTEXT: metadata for node` records for the parent and
    /// the child (runR's log), with retention from the engine's registry, and the child's context the retention C's
    /// API shows (check `rrd.contexts-fixtures`).
    #[test]
    fn a_load_over_a_c_cache_counts_as_c() {
        let Some(fx) = std::env::var_os("NETDATA_DBENGINE_FIXTURES").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: NETDATA_DBENGINE_FIXTURES unset");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (meta, context_db, storage) = runr(&fx, dir.path());
        let rows: Vec<(String, String)> = {
            let c = meta.lock();
            let mut stmt = c
                .prepare("SELECT lower(hex(host_id)), hostname FROM host ORDER BY hops")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let info = |hostname: &str| HostInfo {
            hostname: hostname.into(),
            registry_hostname: hostname.into(),
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
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        };
        let guid = |hex: &str| {
            format!(
                "{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            )
        };
        let hosts = Hosts::with_storage(
            Host::with_storage(&guid(&rows[0].0), true, info(&rows[0].1), &storage),
            Arc::clone(&storage),
        );
        let mut cleanups = Vec::new();
        let mut notices = Vec::new();
        for (hex, name) in &rows {
            let host = if name == &rows[0].1 {
                Arc::clone(hosts.localhost())
            } else {
                hosts.add_archived(&guid(hex), info(name), |_| {})
            };
            let queued = std::cell::RefCell::new(Vec::new());
            let cleanup = |_, context: String| queued.borrow_mut().push(context);
            let ((), records) = netdata_agent_log::capture(|| {
                load_host_contexts(
                    &host,
                    &Sources {
                        meta: &meta,
                        context_db: Some(&context_db),
                        meta_thread: None,
                        context_thread: None,
                        cleanup: &cleanup,
                    },
                )
            });
            cleanups.extend(queued.into_inner());
            notices.extend(
                records
                    .into_iter()
                    .filter_map(|r| r.message)
                    .filter(|m| m.starts_with("RRDCONTEXT: metadata for node ")),
            );
        }
        let parent = format!(
            "RRDCONTEXT: metadata for node '{}': contexts 18 (deleted 0), instances 22 (deleted 0, ignored 0), and \
             metrics 96 (ignored 0, zero retention 0)",
            rows[0].1
        );
        assert!(notices.contains(&parent), "{notices:?}");
        assert!(
            notices.contains(
                &"RRDCONTEXT: metadata for node 'b6child': contexts 1 (deleted 0), instances 4 (deleted 0, ignored \
                  0), and metrics 20 (ignored 0, zero retention 0)"
                    .to_string()
            ),
            "{notices:?}"
        );
        assert_eq!(cleanups, Vec::<String>::new());
        let child = hosts
            .all()
            .into_iter()
            .find(|h| h.hostname() == "b6child")
            .unwrap();
        let rc = child.contexts().get("b6.ctx").unwrap();
        assert_eq!(
            (rc.state().first_time_s, rc.state().last_time_s),
            (1_789_980_541, 1_790_239_740)
        );
    }

    /// A host of `mode` with a valid machine GUID.
    fn host(mode: DbMode) -> Host {
        Host::new(
            "5a1e0000-0000-4000-8000-0000000000bb",
            false,
            HostInfo {
                hostname: "child".into(),
                registry_hostname: "child".into(),
                os: "linux".into(),
                timezone: String::new(),
                abbrev_timezone: String::new(),
                utc_offset: 0,
                program_name: "netdata".into(),
                program_version: "v0".into(),
                update_every: 1,
                db_mode: mode,
                history_entries: 3600,
                health_enabled: false,
                system_info: Default::default(),
                replication_enabled: false,
                replication_period: 0,
                replication_step: 0,
                stream_send: None,
                cache_dir: None,
            },
        )
    }

    /// `rrdcontext_delete_from_sql_unsafe()`: nothing for a host that is not dbengine; on a dbengine host the row goes
    /// and the host's context cleanup is queued once the statement is prepared; without the database, C's records.
    #[test]
    fn a_collected_context_leaves_the_context_database_of_a_dbengine_host() {
        let cleanups = std::cell::RefCell::new(Vec::new());
        let cleanup =
            |host_id: [u8; 16], context: String| cleanups.borrow_mut().push((host_id, context));
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(ContextDb::open(dir.path(), &SqliteSettings::default()).unwrap());
        let ((), records) = netdata_agent_log::capture(|| {
            delete_context(&host(DbMode::Ram), Some(&db), "c.ram", 7, &cleanup);
        });
        assert!(
            records.is_empty() && cleanups.borrow().is_empty(),
            "a ram host"
        );
        let dbengine = host(DbMode::Dbengine);
        let host_id = meta_store::host_id(&dbengine).unwrap();
        let ((), records) = netdata_agent_log::capture(|| {
            delete_context(&dbengine, Some(&db), "c.gone", 7, &cleanup);
        });
        assert!(records.is_empty());
        assert_eq!(*cleanups.borrow(), [(host_id, "c.gone".to_string())]);
        let ((), records) = netdata_agent_log::capture(|| {
            delete_context(&dbengine, None, "c.gone", 7, &cleanup);
        });
        assert_eq!(
            records
                .into_iter()
                .filter_map(|r| r.message)
                .collect::<Vec<_>>(),
            [
                "Failed to prepare statement, rc=21 in ctx_delete_context",
                "RRDCONTEXT: failed to delete context 'c.gone' version 7 from SQL."
            ]
        );
        assert_eq!(
            cleanups.borrow().len(),
            1,
            "no cleanup without the database"
        );
    }
}
