//! `aclk_synchronization_init()` (`src/database/sqlite/sqlite_aclk.c`) without ACLK: the stored children
//! (`hops > 0`) become archived hosts, with C's records, rules and field defaults (`load_archived_host_from_row()`).

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_info};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_metadata::read::{HostRow, NodeId};
use netdata_agent_rrd::host::{Host, HostInfo, Hosts};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};
use netdata_agent_rrd::system_info::SystemInfo;
use std::sync::{Arc, mpsc};

use crate::metasync::MetaSync;

/// `NETDATA_VIRTUAL_HOST`: the operating system of a virtual node.
const VIRTUAL_HOST_OS: &str = "Netdata Virtual Host 1.0";

/// What archived hosts get from the daemon's configuration.
pub struct Defaults {
    /// `default_rrd_memory_mode` after its fallback: the receivers' default.
    pub db_mode: DbMode,
    pub page_size: i64,
    /// `rrdhost_free_ephemeral_time_s`.
    pub free_ephemeral_time_s: i64,
}

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Creates the archived hosts, then has METASYNC load their contexts (the flags clear at once when it cannot, as in
/// C), and waits up to a minute for the vnodes among them.
pub fn load(meta: &MetaDb, hosts: &Arc<Hosts>, defaults: &Defaults, metasync: Option<&MetaSync>) {
    netdata_log_info!("Creating archived hosts");
    let (mut children, mut vnodes) = (0, 0);
    for row in meta.archived_hosts() {
        if let Some(host) = load_row(meta, hosts, row, defaults) {
            if host.info().os == VIRTUAL_HOST_OS {
                vnodes += 1;
            } else {
                children += 1;
            }
        }
    }
    netdata_log_info!(
        "Created {} archived hosts ({children} children and {vnodes} vnodes)",
        children + vnodes
    );
    let loaded = queue_context_load(hosts, metasync);
    // what the ACLKSYNC thread does first, on every start
    meta.drop_legacy_aclk_tables();
    if let Some(loaded) = loaded {
        wait_for_vnodes(&loaded, vnodes);
    }
    netdata_log_info!("ACLK sync initialization completed");
}

/// `metadata_queue_load_host_context()`, or, when it fails, `reset_host_context_load_flag()`. The receiver the vnodes
/// report on when queued.
fn queue_context_load(
    hosts: &Arc<Hosts>,
    metasync: Option<&MetaSync>,
) -> Option<mpsc::Receiver<()>> {
    let (tx, rx) = mpsc::channel();
    if metasync.is_some_and(|m| m.load_host_contexts(hosts, tx)) {
        return Some(rx);
    }
    nd_log!(
        Source::Daemon,
        Priority::Warning,
        "Failed to queue command to load contexts for archived hosts"
    );
    for host in hosts.all() {
        host.clear_pending_context_load();
    }
    None
}

/// The vnodes' context loads, for at most a minute.
fn wait_for_vnodes(loaded: &mpsc::Receiver<()>, vnodes: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    for _ in 0..vnodes {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if loaded.recv_timeout(left).is_err() {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "Vnodes context load still in progress, continue with agent start"
            );
            break;
        }
    }
}

/// `aclk_synchronization_init()` when `netdata-meta.db` could not be opened: C's statement fails on its NULL handle.
pub fn load_without_database(hosts: &Arc<Hosts>, metasync: Option<&MetaSync>) {
    netdata_log_info!("Creating archived hosts");
    nd_log!(
        Source::Daemon,
        Priority::Err,
        "Failed to prepare statement, rc=21 in aclk_synchronization_init"
    );
    nd_log!(
        Source::Daemon,
        Priority::Err,
        "SQLite error when preparing statement to load archived hosts: out of memory"
    );
    netdata_log_info!("Created 0 archived hosts (0 children and 0 vnodes)");
    let _ = queue_context_load(hosts, metasync);
    netdata_log_info!("ACLK sync initialization completed");
}

/// `load_archived_host_from_row()`: `None` for an unregistered ephemeral host past its time.
fn load_row(meta: &MetaDb, hosts: &Hosts, row: HostRow, defaults: &Defaults) -> Option<Arc<Host>> {
    let guid = uuid::Uuid::from_bytes(row.host_id).hyphenated().to_string();
    let now = now_s();
    let last_connected = if row.last_connected == 0 {
        now
    } else {
        row.last_connected
    };
    let age = now - last_connected;
    let hostname = row.hostname.clone().unwrap_or_else(|| "(null)".to_string());
    if row.is_ephemeral
        && ((!row.is_registered && last_connected == 1)
            || (defaults.free_ephemeral_time_s != 0 && age > defaults.free_ephemeral_time_s))
    {
        netdata_log_info!(
            "{} ephemeral hostname \"{hostname}\" with GUID \"{guid}\", age = {age} seconds (limit {} seconds)",
            if row.is_registered {
                "Loading registered"
            } else {
                "Skipping unregistered"
            },
            defaults.free_ephemeral_time_s
        );
        if !row.is_registered {
            return None;
        }
    }
    let mut system_info = SystemInfo {
        hops: row.hops as i16,
        ..SystemInfo::default()
    };
    for (key, value) in meta.host_info(&row.host_id) {
        system_info.set_by_name(&key, &value);
    }
    // rrdhost_create()'s defaults for what the row lacks
    let text = |v: &Option<String>, default: &str| v.clone().unwrap_or_else(|| default.to_string());
    let non_empty = |v: &Option<String>, default: &str| {
        v.as_deref()
            .filter(|v| !v.is_empty())
            .unwrap_or(default)
            .to_string()
    };
    let info = HostInfo {
        hostname: hostname.clone(),
        registry_hostname: text(&row.registry_hostname, &hostname),
        os: text(&row.os, "unknown"),
        timezone: non_empty(&row.timezone, "unknown"),
        abbrev_timezone: non_empty(&row.abbrev_timezone, "UTC"),
        utc_offset: row.utc_offset,
        program_name: non_empty(&row.program_name, "unknown"),
        program_version: non_empty(&row.program_version, "unknown"),
        update_every: row.update_every,
        db_mode: defaults.db_mode,
        history_entries: align_entries_to_pagesize(
            defaults.db_mode,
            i64::from(row.entries),
            defaults.page_size,
        ),
        health_enabled: false,
        system_info,
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    };
    let host = hosts.add_archived(&guid, info, |host| match meta.node_id(&row.host_id) {
        NodeId::Set(id) => host.set_node_id(id),
        NodeId::Cleared => host.set_node_id([0; 16]),
        NodeId::Absent => {}
    });
    if row.is_ephemeral {
        host.set_ephemeral(true);
    }
    let labels = meta.host_labels(&row.host_id);
    host.update_labels(|l| {
        for (name, value, source) in &labels {
            l.add(name.as_bytes(), value.as_bytes(), *source);
        }
    });
    host.set_last_connected_s(last_connected);
    Some(host)
}
