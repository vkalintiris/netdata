//! API handlers. `/api/v1/info` is ported from `src/web/api/v1/api_v1_info.c`
//! (`web_client_api_request_v1_info_fill_buffer()`).

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use crate::analytics::Analytics;
use crate::server::Shared;
use crate::{cloud, v1_charts};

/// `sizeof(name)` in `host_collectors()`: a plugin and module pair is told apart by its first 499 bytes.
const COLLECTOR_KEY_MAX: usize = 499;

/// `host_collectors()`: the plugin and module of the first chart of each pair among the charts available for viewers,
/// in creation order; that chart's last access is the walk's time. The pair is keyed as `plugin:module`, so `a:b`
/// and `c` hide `a` and `b:c`.
fn host_collectors(w: &mut JsonWriter, host: &Host) {
    w.member_add_array(Some(b"collectors"));
    let mut seen = HashSet::new();
    let now = now_realtime_s();
    for st in host.charts().all() {
        if !v1_charts::available_for_viewers(&st) {
            continue;
        }
        let meta = st.meta();
        let mut key = format!("{}:{}", meta.plugin, meta.module).into_bytes();
        key.truncate(COLLECTOR_KEY_MAX);
        if seen.insert(key) {
            st.set_last_accessed_s(now);
            w.add_array_item_object();
            w.member_add_string("plugin", &meta.plugin);
            w.member_add_string("module", &meta.module);
            w.object_close();
        }
    }
    w.array_close();
}

/// `analytics_set_data_str()`'s stored form of a text: wrapped in quotes.
fn quoted(text: &str) -> Vec<u8> {
    format!("\"{text}\"").into_bytes()
}

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

/// The members of `/api/v1/info`: every host in creation order, then the reachable ones before the orphans; then, of
/// the routed host, the alert summary, the system info, the labels, the functions and the collectors; the agent's
/// Cloud flags; the routed host's memory mode with the dbengine's disk quota and page cache size; the agent's web,
/// streaming and analytics members, the routed host's sender compression and its ML.
pub fn info_json(host: &Host, shared: &Shared) -> Vec<u8> {
    let all = shared.hosts.all();
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
    // web_client_api_request_v1_info_summary_alarm_statuses()
    let counts = netdata_agent_health::api::status_counts(shared.health.host(host).as_deref());
    w.member_add_object("alarms");
    w.member_add_uint64("normal", counts.normal);
    w.member_add_uint64("warning", counts.warning);
    w.member_add_uint64("critical", counts.critical);
    w.object_close();
    info.system_info.to_json_v1(&mut w);
    // host_labels2json()
    w.member_add_object("host_labels");
    host.labels().to_json_members(&mut w);
    w.object_close();
    // nrpc_catalog_host2json(): omitted for a host without a registry
    netdata_agent_nrpc::catalog::to_json(host.functions(), &mut w);
    host_collectors(&mut w, host);
    // C's literals: the pair says nothing about the build (D251 F3)
    w.member_add_boolean("cloud-enabled", true);
    w.member_add_boolean("cloud-available", true);
    w.member_add_boolean("agent-claimed", cloud::agent_claimed());
    w.member_add_boolean("aclk-available", cloud::aclk_online());
    w.member_add_string("memory-mode", info.db_mode.name());
    w.member_add_uint64("multidb-disk-quota", shared.multidb_disk_quota_mb);
    w.member_add_uint64("page-cache-size", shared.page_cache_mb);
    w.member_add_boolean("web-enabled", shared.web_enabled);
    w.member_add_boolean("stream-enabled", shared.stream_enabled);
    // stream_sender_has_compression(): the dispatched session's compressor, else the last connect's
    let compression = host.upstream().is_some_and(|up| up.status().compression);
    w.member_add_boolean("stream-compression", compression);
    w.member_add_boolean("https-enabled", true);
    let analytics = &shared.analytics;
    w.member_add_quoted_string("buildinfo", Some(quoted(&shared.build_info.analytics()).as_slice()));
    w.member_add_quoted_string("release-channel", Some(quoted(shared.release_channel).as_slice()));
    let methods = Analytics::text(&analytics.notification_methods);
    w.member_add_quoted_string("notification-methods", methods.as_deref());
    // exporting.conf is not read until the exporting milestone (D251 F4, DEFECTS)
    w.member_add_boolean("exporting-enabled", false);
    let connectors = Analytics::text(&analytics.exporting_connectors);
    w.member_add_quoted_string("exporting-connectors", connectors.as_deref());
    // the allmetrics endpoints come with the exporting milestone: nothing counts their hits yet
    w.member_add_uint64("allmetrics-prometheus-used", 0);
    w.member_add_uint64("allmetrics-shell-used", 0);
    w.member_add_uint64("allmetrics-json-used", 0);
    w.member_add_uint64("dashboard-used", analytics.dashboard_hits.load(Ordering::Relaxed));
    w.member_add_uint64("charts-count", analytics.charts_count.load(Ordering::Relaxed));
    w.member_add_uint64("metrics-count", analytics.metrics_count.load(Ordering::Relaxed));
    // ml_host_get_info() of a host without an ML host, as every host is until the ML milestone
    w.member_add_object("ml-info");
    w.member_add_boolean("enabled", false);
    w.object_close();
    w.finalize();
    w.into_bytes()
}
