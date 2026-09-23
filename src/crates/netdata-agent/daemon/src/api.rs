//! API handlers. `/api/v1/info` is ported from `src/web/api/v1/api_v1_info.c`
//! (`web_client_api_request_v1_info_fill_buffer()`); its members after `mirrored_hosts_status` follow as the
//! subsystems that own them are ported.

use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_text::json::{JsonOptions, JsonWriter};

/// What `/api/v1/info` says about this agent.
pub struct Info {
    pub version: &'static str,
    pub machine_guid: String,
}

/// `web_client_api_request_v1_info_mirrored_hosts_status()`.
fn mirrored_host_status(w: &mut JsonWriter, host: &Host) {
    w.add_array_item_object();
    w.member_add_string("hostname", host.hostname());
    w.member_add_int64("hops", i64::from(host.ingestion_hops()));
    w.member_add_boolean("reachable", host.is_localhost() || !host.is_orphan());
    w.member_add_string("guid", host.machine_guid());
    // Node IDs arrive with claiming: all zero, printed as null.
    w.member_add_null("node_id");
    w.member_add_null("claim_id");
    w.object_close();
}

/// The leading members of `/api/v1/info`: every host in creation order, then the reachable ones before the orphans.
pub fn info_json(info: &Info, hosts: &Hosts) -> Vec<u8> {
    let all = hosts.all();
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("version", info.version);
    w.member_add_string("uid", &info.machine_guid);
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
    w.finalize();
    w.into_bytes()
}
