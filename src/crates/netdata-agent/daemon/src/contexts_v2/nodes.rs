//! The node objects of the contexts v2 engine: `rrdcontext_to_json_v2_rrdhost()` and
//! `buffer_json_node_add_v2_mcp()` (`src/database/contexts/api_v2_contexts.c`).

use netdata_agent_health::alerts::HostAlerts;
use netdata_agent_health::api::alert_counts;
use netdata_agent_query::jsonwrap_v2::node_add_v2;
use netdata_agent_query::keys::Keys;
use netdata_agent_query::tables::contexts_options::MCP;
use netdata_agent_rrd::host::{Host, pending_flags};
use netdata_agent_text::json::JsonWriter;

use super::{Request, mode};
use crate::capas;
use crate::server::Shared;

/// `buffer_json_node_add_v2_mcp()`.
fn node_add_v2_mcp(w: &mut JsonWriter, host: &Host) {
    w.member_add_string("machine_guid", host.machine_guid());
    let node_id = host.node_id();
    if node_id != [0; 16] {
        w.member_add_uuid("node_id", &node_id);
    }
    w.member_add_string("hostname", host.hostname());
    w.member_add_string(
        "relationship",
        if host.is_localhost() {
            "localhost"
        } else if host.is_virtual() {
            "virtual"
        } else {
            "child"
        },
    );
    w.member_add_boolean("connected", host.is_online());
}

/// `rrdcontext_to_json_v2_rrdhost()` for the node modes served.
pub(super) fn node_to_json(
    w: &mut JsonWriter,
    host: &Host,
    shared: &Shared,
    ni: usize,
    k: Keys,
    req: &Request,
    mode: u32,
) {
    w.add_array_item_object();
    if req.options & MCP != 0 {
        node_add_v2_mcp(w, host);
    } else {
        let show_status = mode & mode::AGENTS != 0 && mode & mode::NODE_INSTANCES == 0;
        node_add_v2(w, k, host, ni, 0, show_status);
    }
    if mode & (mode::NODES_INFO | mode::NODES_STREAM_PATH) != 0 {
        let info = host.info();
        w.member_add_string("v", &info.program_version);
        // host_labels2json()
        w.member_add_object("labels");
        host.labels().to_json_members(w);
        w.object_close();
        info.system_info.to_json_v2(w);
        w.member_add_string(
            "state",
            if host.is_online() {
                "reachable"
            } else {
                "stale"
            },
        );
    }
    if mode & mode::NODES_INFO != 0 {
        health_to_json(w, b"health", host, shared.health.host(host).as_deref());
        capas::to_json(w, b"capabilities", host);
    }
    if mode & mode::NODES_STREAM_PATH != 0 {
        netdata_agent_ingest::stream_path::to_json(
            w,
            host,
            shared.hosts.localhost(),
            b"streaming_path",
            false,
            None,
        );
    }
    w.object_close();
}

/// `rrdhost_health_to_json_v2()` over the health part of `rrdhost_status()` (`rrdhost-status.c:314-366`): a host
/// without health says `disabled`; one with it says `initializing` while a chart of it waits for its alerts and
/// `online` after (C's RUNNING), with its alerts counted by status as this request finds them.
pub(super) fn health_to_json(w: &mut JsonWriter, key: &[u8], host: &Host, alerts: Option<&HostAlerts>) {
    w.member_add_object(key);
    if host.health_enabled() {
        let initializing = host.pending_flags() & pending_flags::HEALTH_INITIALIZATION != 0;
        w.member_add_string("status", if initializing { "initializing" } else { "online" });
        let counts = alert_counts(alerts);
        w.member_add_object("alerts");
        w.member_add_uint64("critical", u64::from(counts.critical));
        w.member_add_uint64("warning", u64::from(counts.warning));
        w.member_add_uint64("clear", u64::from(counts.clear));
        w.member_add_uint64("undefined", u64::from(counts.undefined));
        w.member_add_uint64("uninitialized", u64::from(counts.uninitialized));
        w.object_close();
    } else {
        w.member_add_string("status", "disabled");
    }
    w.object_close();
}

#[cfg(test)]
mod tests {
    use netdata_agent_rrd::host::HostInfo;
    use netdata_agent_text::json::JsonOptions;

    use super::*;

    /// A host says `disabled` without health and prints no count; with health it says `initializing` while one of
    /// its charts waits for its alerts and `online` after, each with the five counts in the writer's order
    /// (`rrdhost_health_to_json_v2()`; the status texts are `rrdhost-status.c:57-62`).
    #[test]
    fn the_health_object_follows_the_host_s_health() {
        let info = HostInfo {
            hostname: "box".into(),
            registry_hostname: "box".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: netdata_agent_rrd::mode::DbMode::Ram,
            history_entries: 3600,
            health_enabled: false,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        };
        let host = Host::new("0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e", true, info);
        let rendered = || {
            let mut w = JsonWriter::new(JsonOptions::MINIFY);
            health_to_json(&mut w, b"health", &host, None);
            w.finalize();
            String::from_utf8(w.into_bytes()).unwrap()
        };
        assert_eq!(rendered(), r#"{"health":{"status":"disabled"}}"#);
        let counts = r#""alerts":{"critical":0,"warning":0,"clear":0,"undefined":0,"uninitialized":0}"#;
        host.set_health_enabled(true);
        assert_eq!(rendered(), format!(r#"{{"health":{{"status":"online",{counts}}}}}"#));
        host.raise_pending_flags(pending_flags::HEALTH_INITIALIZATION);
        assert_eq!(rendered(), format!(r#"{{"health":{{"status":"initializing",{counts}}}}}"#));
        // the pending flag of a host without health does not count
        host.set_health_enabled(false);
        assert_eq!(rendered(), r#"{"health":{"status":"disabled"}}"#);
    }
}
