//! API handlers. `/api/v1/info` is ported from `src/web/api/v1/api_v1_info.c`
//! (`web_client_api_request_v1_info_fill_buffer()`); its members after `host_labels` follow as the subsystems that
//! own them are ported.

use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_text::json::{JsonOptions, JsonWriter};

/// `web_client_api_request_v1_info_mirrored_hosts_status()`.
fn mirrored_host_status(w: &mut JsonWriter, host: &Host) {
    w.add_array_item_object();
    w.member_add_string("hostname", host.hostname());
    w.member_add_int64("hops", i64::from(host.ingestion_hops()));
    w.member_add_boolean("reachable", host.is_localhost() || !host.is_orphan());
    w.member_add_string("guid", host.machine_guid());
    // the node id stored for the host, null when it has none
    match host.node_id() {
        id if id == [0; 16] => w.member_add_null("node_id"),
        id => {
            let mut text = Vec::new();
            netdata_agent_text::print::print_uuid_lower(&mut text, &id);
            w.member_add_string("node_id", &text);
        }
    }
    match host.claim_id() {
        Some(id) => {
            let mut text = Vec::new();
            netdata_agent_text::print::print_uuid_lower(&mut text, &id);
            w.member_add_string("claim_id", &text);
        }
        None => w.member_add_null("claim_id"),
    }
    w.object_close();
}

/// The members of `/api/v1/info` up to `host_labels`, about the routed host: every host in creation order, then the
/// reachable ones before the orphans, the alert summary, the host's system info and labels.
pub fn info_json(host: &Host, hosts: &Hosts) -> Vec<u8> {
    let all = hosts.all();
    let info = host.info();
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("version", &info.program_version);
    w.member_add_string("uid", host.machine_guid());
    w.member_add_uint64("hosts-available", all.len() as u64);
    w.member_add_array(Some(b"mirrored_hosts"));
    for host in &all {
        w.add_array_item_string(host.hostname());
    }
    w.array_close();
    w.member_add_array(Some(b"mirrored_hosts_status"));
    let reachable = |h: &Host| h.is_localhost() || !h.is_orphan();
    for host in all.iter().filter(|h| reachable(h)) {
        mirrored_host_status(&mut w, host);
    }
    for host in all.iter().filter(|h| !reachable(h)) {
        mirrored_host_status(&mut w, host);
    }
    w.array_close();
    // web_client_api_request_v1_info_summary_alarm_statuses(): no alert runs without the health engine
    w.member_add_object("alarms");
    for status in ["normal", "warning", "critical"] {
        w.member_add_uint64(status, 0);
    }
    w.object_close();
    info.system_info.to_json_v1(&mut w);
    // host_labels2json()
    w.member_add_object("host_labels");
    host.labels().to_json_members(&mut w);
    w.object_close();
    w.finalize();
    w.into_bytes()
}
