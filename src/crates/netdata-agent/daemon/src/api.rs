//! API handlers. `/api/v1/info` is ported from `src/web/api/v1/api_v1_info.c`
//! (`web_client_api_request_v1_info_fill_buffer()`); its members after `mirrored_hosts_status` follow as the
//! subsystems that own them are ported.

use netdata_agent_text::json::{JsonOptions, JsonWriter};

/// The hosts `/api/v1/info` describes; only localhost until streaming lands.
pub struct Info {
    pub version: &'static str,
    pub machine_guid: String,
    pub hostname: String,
}

pub fn info_json(info: &Info) -> Vec<u8> {
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("version", info.version);
    w.member_add_string("uid", &info.machine_guid);
    w.member_add_uint64("hosts-available", 1);
    w.member_add_array(Some(b"mirrored_hosts"));
    w.add_array_item_string(&info.hostname);
    w.array_close();
    w.member_add_array(Some(b"mirrored_hosts_status"));
    w.add_array_item_object();
    w.member_add_string("hostname", &info.hostname);
    w.member_add_int64("hops", 0);
    w.member_add_boolean("reachable", true);
    w.member_add_string("guid", &info.machine_guid);
    w.member_add_null("node_id");
    w.member_add_null("claim_id");
    w.object_close();
    w.array_close();
    w.finalize();
    w.into_bytes()
}
