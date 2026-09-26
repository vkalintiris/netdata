//! The metadata writer's host side (`sqlite_metadata.c`): what of a host waits to be stored, as its
//! `RRDHOST_FLAG_METADATA_*` say, mapped onto the metadata crate's records, with C's records when a store fails and
//! the flags raised again so that the next run retries. The main thread stores localhost at its creation; the
//! METASYNC job stores every host.

use netdata_agent_log::netdata_log_error;
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_metadata::write::{HostRecord, LabelRecord};
use netdata_agent_rrd::host::{Host, meta_flags};

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
}
