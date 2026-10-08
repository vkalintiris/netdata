//! Pieces the daemon's unit tests share.

use netdata_agent_rrd::host::HostInfo;
use netdata_agent_rrd::mode::DbMode;

/// A host's info for a test: a RAM database of 3600 entries collected every second, health off, no streaming.
pub(crate) fn host_info(hostname: &str) -> HostInfo {
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
