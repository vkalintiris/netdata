//! The metadata writer's host side (`sqlite_metadata.c`): what of a host waits to be stored, as its
//! `RRDHOST_FLAG_METADATA_*` say, mapped onto the metadata crate's records, with C's records when a store fails and
//! the flags raised again so that the next run retries. The main thread stores localhost at its creation; the
//! METASYNC job stores every host.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use netdata_agent_log::{netdata_log_error, netdata_log_info};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_metadata::write::{ChartRecord, DimRecord, HostRecord, LabelRecord};
use netdata_agent_rrd::chart::{Chart, ChartMeta, Dim, DimMeta, dim_flags};
use netdata_agent_rrd::host::{Host, Hosts, meta_flags};
use netdata_agent_rrd::labels::Labels;

use crate::startup::now_ut;

/// The host's machine GUID as the 16 bytes SQL stores (`host->host_id`).
pub fn host_id(host: &Host) -> Option<[u8; 16]> {
    netdata_agent_text::parse::uuid_parse_flexi(host.machine_guid().as_bytes())
}

/// `meta_store_host_labels()`: the stored labels are replaced by the host's.
fn store_host_labels(meta: &MetaDb, host: &Host, id: &[u8; 16]) {
    if !meta.delete_host_labels(id) {
        netdata_log_error!(
            "METADATA: 'host:{}': failed to delete old host labels",
            host.hostname()
        );
        host.set_meta_flags(meta_flags::LABELS | meta_flags::UPDATE);
        return;
    }
    let labels = host.labels();
    let records: Vec<LabelRecord<'_>> = labels
        .iter()
        .map(|l| LabelRecord {
            name: &l.name,
            value: &l.value,
            source: l.flags,
        })
        .collect();
    if !meta.store_host_labels(id, &records) {
        netdata_log_error!(
            "METADATA: 'host:{}': failed to store host labels",
            host.hostname()
        );
        host.set_meta_flags(meta_flags::LABELS | meta_flags::UPDATE);
    }
}

/// `store_host_claim_id()`: the agent's own claim id (none: this agent does not claim yet, D61.3).
fn store_host_claim_id(meta: &MetaDb, host: &Host, id: &[u8; 16]) {
    if !meta.store_claim_id(id, None) {
        host.set_meta_flags(meta_flags::CLAIMID | meta_flags::UPDATE);
    }
}

/// `store_host_and_system_info()`: the system info rows, then the host row.
fn store_host_and_system_info(meta: &MetaDb, host: &Host, id: &[u8; 16]) {
    let info = host.info();
    if !meta.store_host_system_info(id, &info.system_info.stored_keys()) {
        netdata_log_error!(
            "METADATA: 'host:{}': Failed to store host updated system information in the database",
            host.hostname()
        );
        host.set_meta_flags(meta_flags::INFO | meta_flags::UPDATE);
    }
    // rrdhost_tz_get(): an unset timezone is "unknown", an unset abbreviation "UTC"
    let or = |value: &'_ str, default: &'static str| -> String {
        if value.is_empty() {
            default.to_string()
        } else {
            value.to_string()
        }
    };
    let (timezone, abbrev_timezone) = (
        or(&info.timezone, "unknown"),
        or(&info.abbrev_timezone, "UTC"),
    );
    let record = HostRecord {
        host_id: *id,
        hostname: &info.hostname,
        registry_hostname: &info.registry_hostname,
        update_every: info.update_every,
        os: &info.os,
        timezone: &timezone,
        hops: i32::from(host.ingestion_hops()),
        memory_mode: info.db_mode as i32,
        abbrev_timezone: &abbrev_timezone,
        utc_offset: info.utc_offset,
        program_name: &info.program_name,
        program_version: &info.program_version,
        entries: info.history_entries,
        health_enabled: info.health_enabled,
        last_connected: host.last_connected_s(),
    };
    if !meta.store_host(&record) {
        netdata_log_error!(
            "METADATA: 'host:{}': Failed to store host info in the database",
            host.hostname()
        );
        host.set_meta_flags(meta_flags::INFO | meta_flags::UPDATE);
    }
}

/// `store_host_info_and_metadata()`: the host's labels, claim id and info, each when its flag says so.
pub fn store_host_info_and_metadata(meta: &MetaDb, host: &Host) {
    let Some(id) = host_id(host) else {
        return;
    };
    if host.take_meta_flags(meta_flags::LABELS) {
        store_host_labels(meta, host, &id);
    }
    if host.take_meta_flags(meta_flags::CLAIMID) {
        store_host_claim_id(meta, host, &id);
    }
    if host.take_meta_flags(meta_flags::INFO) {
        store_host_and_system_info(meta, host, &id);
    }
}

/// What `metadata_scan_host()` stores of one chart, read before the database is locked: the chart and its labels when
/// the chart is flagged (the labels only when their version moved), and its flagged dimensions.
struct ChartScan {
    chart: Arc<Chart>,
    meta: Option<ChartMeta>,
    labels: Option<(u32, Labels)>,
    dims: Vec<(Arc<Dim>, DimMeta)>,
}

/// What of a scan failed, to be flagged again once the database is unlocked.
#[derive(Default)]
struct ScanOutcome {
    /// Per chart: whether its labels were stored (when tried).
    labels_stored: Vec<Option<bool>>,
    chart_failed: Vec<bool>,
    dims_failed: Vec<Vec<bool>>,
}

fn label_records(labels: &Labels) -> Vec<LabelRecord<'_>> {
    labels
        .iter()
        .map(|l| LabelRecord {
            name: &l.name,
            value: &l.value,
            source: l.flags,
        })
        .collect()
}

/// `metadata_scan_host()`: the host's flagged charts, their labels and flagged dimensions, in one transaction. A
/// normal scan that meets a shutdown stops and leaves the host flagged for the final store. The flags are taken and
/// the rows read first, then written with the database locked (no chart lock is taken under it), then what failed
/// is flagged again.
fn scan_host(
    meta: &MetaDb,
    host: &Host,
    host_id: &[u8; 16],
    shutdown: &AtomicBool,
    final_store: bool,
) {
    let mut recheck = false;
    let mut scans = Vec::new();
    for chart in host.charts().all() {
        if !final_store && shutdown.load(Ordering::Acquire) {
            recheck = true;
            break;
        }
        let mut scan = ChartScan {
            meta: None,
            labels: None,
            dims: Vec::new(),
            chart: Arc::clone(&chart),
        };
        if chart.take_metadata_update() {
            let m = chart.meta();
            if m.labels.version() != chart.labels_saved_version() {
                scan.labels = Some((m.labels.version(), m.labels.clone()));
            }
            scan.meta = Some(m);
        }
        for dim in chart.dims() {
            if let Some(m) = chart.take_dim_metadata_update(&dim) {
                scan.dims.push((dim, m));
            }
        }
        scans.push(scan);
    }
    let hostname = host.hostname();
    let outcome = meta.scan_host(|db| {
        let mut outcome = ScanOutcome::default();
        for scan in &scans {
            let chart = &scan.chart;
            let name = || {
                scan.meta
                    .as_ref()
                    .and_then(|m| m.name.clone())
                    .unwrap_or_else(|| chart.meta().name.unwrap_or_else(|| chart.id().to_string()))
            };
            let mut labels_stored = None;
            let mut chart_failed = false;
            if let Some(m) = &scan.meta {
                if let Some((_, labels)) = &scan.labels {
                    let stored = db.chart_labels(chart.uuid(), &label_records(labels));
                    if !stored {
                        netdata_log_error!(
                            "METADATA: 'host:{hostname}': Failed to update labels for chart {}",
                            name()
                        );
                    }
                    labels_stored = Some(stored);
                }
                let record = ChartRecord {
                    chart_id: *chart.uuid(),
                    host_id: *host_id,
                    type_: chart.type_(),
                    id: chart.id_part(),
                    name: chart.name_part(),
                    family: &m.family,
                    context: &m.context,
                    title: &m.title,
                    units: &m.units,
                    plugin: &m.plugin,
                    module: &m.module,
                    priority: m.priority as i32,
                    update_every: m.update_every,
                    chart_type: m.chart_type.id(),
                    memory_mode: chart.mode() as i32,
                    history_entries: chart.entries() as i32,
                };
                if !db.chart(&record) {
                    chart_failed = true;
                    netdata_log_error!(
                        "METADATA: 'host:{hostname}': Failed to store metadata for chart {}",
                        name()
                    );
                }
            }
            let mut dims_failed = Vec::with_capacity(scan.dims.len());
            for (dim, m) in &scan.dims {
                let record = DimRecord {
                    dim_id: *dim.uuid(),
                    chart_id: *chart.uuid(),
                    id: dim.id(),
                    name: &m.name,
                    multiplier: m.multiplier,
                    divisor: m.divisor,
                    algorithm: m.algorithm.id(),
                    hidden: m.flags & dim_flags::HIDDEN != 0,
                };
                let failed = !db.dimension(&record);
                if failed {
                    netdata_log_error!(
                        "METADATA: 'host:{hostname}': Failed to store dimension metadata for chart {}. dimension {}",
                        name(),
                        m.name
                    );
                }
                dims_failed.push(failed);
            }
            outcome.labels_stored.push(labels_stored);
            outcome.chart_failed.push(chart_failed);
            outcome.dims_failed.push(dims_failed);
        }
        outcome
    });
    for (i, scan) in scans.iter().enumerate() {
        if let (Some(true), Some((version, _))) = (outcome.labels_stored[i], &scan.labels) {
            scan.chart.set_labels_saved_version(*version);
        }
        if outcome.chart_failed[i] {
            recheck = true;
            scan.chart.set_metadata_update();
        }
        for ((dim, _), failed) in scan.dims.iter().zip(&outcome.dims_failed[i]) {
            if *failed {
                recheck = true;
                scan.chart.set_dim_metadata_update(dim);
            }
        }
    }
    if recheck {
        host.set_meta_flags(meta_flags::UPDATE);
    }
}

/// `store_hosts_metadata()`: every host that is not archived and has something to store, in the index's order. The
/// final store (`!is_worker`) reports each stored host's position and the total with the time it took; a normal
/// store stops at a shutdown, leaving the rest for the final one.
pub fn store_hosts_metadata(
    meta: &MetaDb,
    hosts: &Hosts,
    shutdown: &AtomicBool,
    is_worker: bool,
    final_store: bool,
) {
    let started = now_ut();
    let all = hosts.all();
    let host_count = all.len().max(1);
    let mut count = 0;
    for host in &all {
        count += 1;
        let mut store = false;
        let mut stop = false;
        if !host.is_archived() && host.meta_flags() & meta_flags::UPDATE != 0 {
            if !final_store && shutdown.load(Ordering::Acquire) {
                stop = true;
            } else {
                host.take_meta_flags(meta_flags::UPDATE);
                store = true;
            }
        }
        if stop {
            break;
        }
        if store {
            store_host_info_and_metadata(meta, host);
            if let Some(id) = host_id(host) {
                scan_host(meta, host, &id, shutdown, final_store);
            }
            if !is_worker {
                netdata_log_info!(
                    "METADATA: Progress of metadata storage: {:6.2}% completed",
                    100.0 * count as f64 / host_count as f64
                );
            }
        }
    }
    if !is_worker {
        netdata_log_info!(
            "METADATA: Progress of metadata storage: {:6.2}% completed in {}",
            100.0 * count as f64 / host_count as f64,
            crate::metasync::duration(now_ut().saturating_sub(started))
        );
    }
}

/// `SQLITE_MISUSE`: what every prepare on C's NULL `db_meta` returns.
const SQLITE_MISUSE: i32 = 21;

/// `store_host_info_and_metadata()` of localhost at its creation when `netdata-meta.db` could not be opened: C runs
/// it on a NULL handle, and every statement fails to prepare. The writer stays off afterwards (D61.8).
pub fn store_localhost_without_database(host: &Host) {
    if !host.take_meta_flags(meta_flags::INFO) {
        return;
    }
    for _ in 0..host.info().system_info.stored_keys().len() {
        netdata_log_error!(
            "Failed to prepare statement, rc={SQLITE_MISUSE} in add_host_sysinfo_key_value"
        );
    }
    netdata_log_error!(
        "METADATA: 'host:{}': Failed to store host updated system information in the database",
        host.hostname()
    );
    netdata_log_error!("Failed to prepare statement, rc={SQLITE_MISUSE} in store_host_metadata");
    netdata_log_error!(
        "METADATA: 'host:{}': Failed to store host info in the database",
        host.hostname()
    );
    host.set_meta_flags(meta_flags::INFO | meta_flags::UPDATE);
}

/// `load_claiming_state()`'s `invalidate_node_instances()` for an agent without a claimed id (D61.3): every node id
/// goes, as C's invalidation does not refer to the updated row. The claim id follows on METASYNC.
pub fn invalidate_node_instances(meta: Option<&MetaDb>, localhost: &Host) {
    let Some(id) = host_id(localhost) else {
        return;
    };
    match meta {
        Some(meta) => meta.invalidate_node_instances(&id, None),
        None => netdata_log_error!(
            "Failed to prepare statement, rc={SQLITE_MISUSE} in invalidate_node_instances"
        ),
    }
}

/// `store_claim_id()` of a queued `METADATA_STORE_CLAIM_ID`, on METASYNC.
pub fn store_claim_id(meta: Option<&MetaDb>, id: &[u8; 16]) {
    match meta {
        Some(meta) => {
            let _ = meta.store_claim_id(id, None);
        }
        None => {
            netdata_log_error!("Failed to prepare statement, rc={SQLITE_MISUSE} in store_claim_id")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_metadata::open::SqliteSettings;
    use netdata_agent_rrd::host::{HostInfo, Hosts};
    use netdata_agent_rrd::mode::DbMode;

    const GUID: &str = "5a1e0000-0000-4000-8000-0000000000aa";

    fn localhost() -> Hosts {
        let system_info = netdata_agent_rrd::system_info::SystemInfo {
            host_os_name: Some("Debian".into()),
            ..Default::default()
        };
        Hosts::new(Host::new(
            GUID,
            true,
            HostInfo {
                hostname: "parent".into(),
                registry_hostname: "parent".into(),
                os: "linux".into(),
                timezone: String::new(),
                abbrev_timezone: String::new(),
                utc_offset: 0,
                program_name: "netdata".into(),
                program_version: "v0".into(),
                update_every: 1,
                db_mode: DbMode::Alloc,
                history_entries: 3600,
                health_enabled: true,
                system_info,
                replication_enabled: false,
                replication_period: 0,
                replication_step: 0,
                stream_send: None,
                cache_dir: None,
            },
        ))
    }

    fn rows(meta: &MetaDb, sql: &str) -> Vec<String> {
        let c = meta.lock();
        let mut stmt = c.prepare(sql).unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// Localhost at its creation: its info and host rows, the labels and claim id only once flagged; the flags are
    /// consumed, and a failed store raises them again with C's records.
    #[test]
    fn host_stores_follow_the_flags() {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
        let hosts = localhost();
        let host = hosts.localhost();
        host.update_labels(|l| l.add(b"role", b"db", 2));
        store_host_info_and_metadata(&meta, host);
        assert_eq!(host.meta_flags(), meta_flags::UPDATE);
        assert_eq!(
            rows(
                &meta,
                "SELECT hostname || '|' || timezone || '|' || abbrev_timezone || '|' || hops || '|' || memory_mode || '|' || (last_connected > 0) FROM host"
            ),
            ["parent|unknown|UTC|0|4|1"]
        );
        assert_eq!(
            rows(
                &meta,
                "SELECT count(*) || '|' || count(DISTINCT system_key) FROM host_info"
            ),
            ["27|27"]
        );
        assert_eq!(
            rows(
                &meta,
                "SELECT system_value FROM host_info WHERE system_key = 'NETDATA_HOST_OS_NAME'"
            ),
            ["Debian"]
        );
        assert_eq!(
            rows(&meta, "SELECT CAST(count(*) AS TEXT) FROM host_label"),
            ["0"]
        );
        host.set_meta_flags(meta_flags::LABELS | meta_flags::CLAIMID);
        store_host_info_and_metadata(&meta, host);
        assert_eq!(
            rows(
                &meta,
                "SELECT label_key || '=' || label_value FROM host_label"
            ),
            ["role=db"]
        );
        assert_eq!(
            rows(&meta, "SELECT ifnull(claim_id, 'NULL') FROM node_instance"),
            ["NULL"]
        );

        meta.lock().execute_batch("DROP TABLE host").unwrap();
        host.set_meta_flags(meta_flags::INFO);
        let ((), records) =
            netdata_agent_log::capture(|| store_host_info_and_metadata(&meta, host));
        let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
        assert_eq!(
            messages,
            [
                "Failed to prepare statement, rc=1 in store_host_metadata",
                "METADATA: 'host:parent': Failed to store host info in the database"
            ]
        );
        assert_eq!(host.meta_flags(), meta_flags::INFO | meta_flags::UPDATE);
    }

    /// Without `netdata-meta.db`, C's statements fail on a NULL handle: 27 system info keys and the host row.
    #[test]
    fn localhost_without_a_database_logs_cs_records() {
        let hosts = localhost();
        let ((), records) =
            netdata_agent_log::capture(|| store_localhost_without_database(hosts.localhost()));
        let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
        assert_eq!(messages.len(), 30);
        assert!(
            messages[..27]
                .iter()
                .all(|m| m == "Failed to prepare statement, rc=21 in add_host_sysinfo_key_value")
        );
        assert_eq!(
            messages[27..],
            [
                "METADATA: 'host:parent': Failed to store host updated system information in the database",
                "Failed to prepare statement, rc=21 in store_host_metadata",
                "METADATA: 'host:parent': Failed to store host info in the database"
            ]
        );
    }

    fn spec<'a>(id: &'a str) -> netdata_agent_rrd::chart::ChartSpec<'a> {
        netdata_agent_rrd::chart::ChartSpec {
            type_: "t",
            id,
            name: None,
            family: None,
            context: None,
            title: "title",
            units: "units",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every: 1,
            chart_type: netdata_agent_rrd::chart::ChartType::Stacked,
            mode: DbMode::Alloc,
            history_entries: 3600,
            page_size: 4096,
        }
    }

    fn messages(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
        records.into_iter().filter_map(|r| r.message).collect()
    }

    /// A store writes each flagged host's charts, chart labels and dimensions (a hidden one as `'hidden'`), skips
    /// archived hosts, and consumes the flags; the final store reports each stored host's position and the total.
    /// The next store has nothing to write but what changed since.
    #[test]
    fn stores_write_what_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
        let hosts = localhost();
        let archived = hosts.add_archived(
            "5a1e0000-0000-4000-8000-0000000000bb",
            hosts.localhost().info(),
            |_| {},
        );
        archived.set_meta_flags(meta_flags::UPDATE | meta_flags::INFO);
        let child = hosts.find_or_create(
            "5a1e0000-0000-4000-8000-0000000000cc",
            DbMode::Alloc,
            || {
                let mut info = hosts.localhost().info();
                info.hostname = "child".into();
                info
            },
            |_| {},
        );
        let (chart, _) = child.charts().create(&spec("c"));
        chart.update_meta(|m| m.labels.add(b"role", b"db", 2));
        let (_d1, _) = chart.dim_add(
            "d1",
            None,
            3,
            7,
            netdata_agent_rrd::chart::Algorithm::Incremental,
        );
        let (d2, _) = chart.dim_add(
            "d2",
            None,
            1,
            1,
            netdata_agent_rrd::chart::Algorithm::Absolute,
        );
        chart.dim_set_hidden(&d2, true);
        let shutdown = AtomicBool::new(false);
        let ((), records) = netdata_agent_log::capture(|| {
            store_hosts_metadata(&meta, &hosts, &shutdown, false, true)
        });
        let got = messages(records);
        assert_eq!(
            got[..2],
            [
                "METADATA: Progress of metadata storage:  33.33% completed",
                "METADATA: Progress of metadata storage: 100.00% completed"
            ]
        );
        assert!(
            got[2].starts_with("METADATA: Progress of metadata storage: 100.00% completed in ")
        );
        assert_eq!(got.len(), 3);
        assert_eq!(
            rows(&meta, "SELECT hostname FROM host ORDER BY rowid"),
            ["parent", "child"]
        );
        assert_eq!(
            rows(
                &meta,
                "SELECT type || '|' || id || '|' || ifnull(name, 'NULL') || '|' || context || '|' || module || '|' || chart_type || '|' || memory_mode || '|' || history_entries FROM chart"
            ),
            ["t|c|NULL|t.c||2|4|3600"]
        );
        assert_eq!(
            rows(
                &meta,
                "SELECT id || '|' || multiplier || '|' || divisor || '|' || algorithm || '|' || ifnull(options, 'NULL') FROM dimension ORDER BY id"
            ),
            ["d1|3|7|1|NULL", "d2|1|1|0|hidden"]
        );
        assert_eq!(
            rows(
                &meta,
                "SELECT label_key || '=' || label_value FROM chart_label ORDER BY label_key"
            ),
            ["_collect_module=[none]", "_collect_plugin=p", "role=db"]
        );
        assert_eq!(child.meta_flags(), 0);
        assert_eq!(archived.meta_flags(), meta_flags::UPDATE | meta_flags::INFO);
        assert_eq!(chart.labels_saved_version(), chart.meta().labels.version());

        // only the unhidden dimension waits now
        chart.dim_set_hidden(&d2, false);
        meta.lock().execute_batch("DELETE FROM dimension").unwrap();
        let ((), records) = netdata_agent_log::capture(|| {
            store_hosts_metadata(&meta, &hosts, &shutdown, true, false)
        });
        assert!(messages(records).is_empty());
        assert_eq!(
            rows(
                &meta,
                "SELECT id || '|' || ifnull(options, 'NULL') FROM dimension"
            ),
            ["d2|NULL"]
        );
    }

    /// A normal store stops at a shutdown and leaves the host flagged for the final store; a failed dimension store
    /// flags the dimension and its host again.
    #[test]
    fn shutdowns_and_failures_leave_the_flags() {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
        let hosts = localhost();
        let (chart, _) = hosts.localhost().charts().create(&spec("c"));
        let (dim, _) = chart.dim_add(
            "d",
            None,
            1,
            1,
            netdata_agent_rrd::chart::Algorithm::Absolute,
        );
        let shutdown = AtomicBool::new(true);
        store_hosts_metadata(&meta, &hosts, &shutdown, true, false);
        assert_eq!(
            hosts.localhost().meta_flags(),
            meta_flags::INFO | meta_flags::UPDATE
        );
        assert_eq!(
            rows(&meta, "SELECT CAST(count(*) AS TEXT) FROM chart"),
            ["0"]
        );

        shutdown.store(false, Ordering::Release);
        meta.lock()
            .execute_batch("CREATE TRIGGER no_dims BEFORE INSERT ON dimension BEGIN SELECT RAISE(ABORT, 'no'); END")
            .unwrap();
        let ((), records) = netdata_agent_log::capture(|| {
            store_hosts_metadata(&meta, &hosts, &shutdown, true, false)
        });
        assert_eq!(
            messages(records),
            [
                "Failed to store dimension, rc = 19",
                "METADATA: 'host:parent': Failed to store dimension metadata for chart t.c. dimension d"
            ]
        );
        assert_eq!(hosts.localhost().meta_flags(), meta_flags::UPDATE);
        assert!(chart.take_dim_metadata_update(&dim).is_some());
        assert_eq!(
            rows(&meta, "SELECT CAST(count(*) AS TEXT) FROM chart"),
            ["1"]
        );
    }
}
