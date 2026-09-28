//! The Cloud status (`src/claim/cloud-status.c`) and the claiming failure reason (`src/claim/claim.c`) of an agent
//! that is never claimed (D61.3): ACLK, claiming and the claimed states come with the Cloud milestone (D92.3).

use netdata_agent_inicfg::{Config, SECTION_GLOBAL};
use netdata_agent_text::json::JsonWriter;

use crate::conf::DEFAULT_CLOUD_BASE_URL;
use crate::status_file::cloud_status;

/// `claim_agent_failure_reason_get()` while no claim has failed.
pub const CLAIM_FAILURE_REASON: &str = "Agent is not claimed yet";

/// `cloud_status()`: without ACLK, a claim or a stream sender, the agent is available.
pub fn status() -> u8 {
    cloud_status::AVAILABLE
}

/// `cloud_config_url_get()`: cloud.conf's `[global] url`.
pub fn url(cloud_conf: &mut Config) -> Vec<u8> {
    cloud_conf
        .get(SECTION_GLOBAL, "url", Some(DEFAULT_CLOUD_BASE_URL))
        .unwrap_or_default()
}

/// `buffer_json_cloud_status()` of an available agent; `url` is [`url()`]'s.
pub fn status_to_json(w: &mut JsonWriter, now_s: i64, url: &[u8]) {
    // cloud_last_change(): no connection or disconnection yet, so the agent's start
    let since = netdata_agent_rrd::host::netdata_start_time();
    w.member_add_object("cloud");
    // cloud_connection_id()
    w.member_add_uint64("id", 0);
    w.member_add_string("status", cloud_status::name(status()));
    w.member_add_time_t("since", since);
    w.member_add_time_t("age", now_s - since);
    w.member_add_string("url", url);
    w.member_add_string("reason", CLAIM_FAILURE_REASON);
    w.object_close();
}
