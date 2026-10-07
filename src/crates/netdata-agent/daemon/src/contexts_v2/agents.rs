//! The `agents` array of the contexts v2 engine: `buffer_json_agents_v2()` as `rrdcontext_to_json_v2()` calls it,
//! with the agent's info members (`src/database/contexts/api_v2_contexts_agents.c`).

use std::time::Instant;

use netdata_agent_query::jsonwrap::timings;
use netdata_agent_query::jsonwrap_v2::{Agent, agents_v2};
use netdata_agent_query::tables::contexts_options::RFC3339;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::retention::retention_stats;
use netdata_agent_text::json::JsonWriter;

use super::{Request, mode};
use crate::server::Shared;
use crate::{capas, cloud};

/// `buffer_json_agents_v2()` as `rrdcontext_to_json_v2()` calls it: localhost at `now_s`, the info members with
/// `AGENTS_INFO`, then the timings; when they end.
pub(super) fn agents(
    w: &mut JsonWriter,
    shared: &Shared,
    req: &Request,
    mode: u32,
    now_s: i64,
    received: Instant,
    executed: Instant,
) -> Instant {
    let localhost = shared.hosts.localhost();
    let hostname = localhost.hostname();
    let nodes_hard_hash = || u64::from(shared.hosts.version());
    let agent = Agent {
        machine_guid: localhost.machine_guid(),
        node_id: localhost.node_id(),
        hostname: &hostname,
        nodes_hard_hash: &nodes_hard_hash,
    };
    let rfc3339 = req.options & RFC3339 != 0;
    let mut finished = executed;
    agents_v2(w, agent, now_s, rfc3339, true, |w| {
        if mode & mode::AGENTS_INFO != 0 {
            agent_info(w, shared, now_s, rfc3339);
        }
        finished = Instant::now();
        // nothing is preprocessed
        timings(w, "timings", received, received, executed, finished);
    });
    finished
}

/// The `info` members of `buffer_json_agents_v2()`.
fn agent_info(w: &mut JsonWriter, shared: &Shared, now_s: i64, rfc3339: bool) {
    w.member_add_object("application");
    shared.build_info.to_json_object(w);
    w.object_close();
    let url = cloud::url(&mut shared.cloud_conf());
    cloud::status_to_json(w, now_s, &url);
    // rrdstats_metadata_collect()
    let m = shared.hosts.metadata_stats();
    w.member_add_object("nodes");
    w.member_add_uint64("total", m.nodes_total);
    w.member_add_uint64("receiving", m.nodes_receiving);
    w.member_add_uint64("sending", m.nodes_sending);
    w.member_add_uint64("archived", m.nodes_archived);
    w.object_close();
    for (key, c) in [("metrics", m.metrics), ("instances", m.instances)] {
        w.member_add_object(key);
        w.member_add_uint64("collected", c.collected);
        w.member_add_uint64("available", c.available);
        w.object_close();
    }
    w.member_add_object("contexts");
    w.member_add_uint64("collected", m.contexts.collected);
    w.member_add_uint64("available", m.contexts.available);
    w.member_add_uint64("unique", m.contexts_unique);
    w.object_close();
    // C writes localhost's capabilities here
    capas::to_json(w, b"capabilities", shared.hosts.localhost());
    w.member_add_object("api");
    w.member_add_uint64("version", capas::HTTP_API_V2_VERSION);
    w.member_add_boolean("bearer_protection", crate::auth::bearer_protection());
    w.object_close();
    db_size(w, shared, rfc3339);
}

/// `db_size` from `rrdstats_retention_collect()`: each tier with an engine.
fn db_size(w: &mut JsonWriter, shared: &Shared, rfc3339: bool) {
    let info = shared.hosts.localhost().info();
    let tiers = retention_stats(
        shared.hosts.storage(),
        info.db_mode,
        i64::from(info.update_every),
        shared.history_entries,
        now_realtime_s(),
    );
    w.member_add_array(Some(b"db_size"));
    for t in &tiers {
        w.add_array_item_object();
        w.member_add_uint64("tier", t.tier as u64);
        w.member_add_string("granularity", &t.granularity_human);
        w.member_add_uint64("metrics", t.metrics);
        w.member_add_uint64("samples", t.samples);
        let sized = t.disk_used != 0 || t.disk_max != 0;
        if sized {
            w.member_add_uint64("disk_used", t.disk_used);
            w.member_add_uint64("disk_max", t.disk_max);
            w.member_add_double("disk_percent", (t.disk_percent * 100.0 + 0.5).floor() / 100.0);
        }
        if t.retention != 0 {
            w.member_add_time_t_formatted("from", t.first_time_s, rfc3339);
            w.member_add_time_t_formatted("to", t.last_time_s, rfc3339);
            w.member_add_time_t("retention", t.retention);
            w.member_add_string("retention_human", &t.retention_human);
            if sized {
                w.member_add_time_t("requested_retention", t.requested_retention);
                w.member_add_string("requested_retention_human", &t.requested_retention_human);
                w.member_add_time_t("expected_retention", t.expected_retention);
                w.member_add_string("expected_retention_human", &t.expected_retention_human);
            }
        }
        w.object_close();
    }
    w.array_close();
}
