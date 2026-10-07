//! The node objects of the contexts v2 engine: `rrdcontext_to_json_v2_rrdhost()` and
//! `buffer_json_node_add_v2_mcp()` (`src/database/contexts/api_v2_contexts.c`).

use netdata_agent_query::jsonwrap_v2::node_add_v2;
use netdata_agent_query::keys::Keys;
use netdata_agent_query::tables::contexts_options::MCP;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::JsonWriter;

use super::{Request, mode};

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
    localhost: &Host,
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
    if mode & mode::NODES_STREAM_PATH != 0 {
        netdata_agent_ingest::stream_path::to_json(
            w,
            host,
            localhost,
            b"streaming_path",
            false,
            None,
        );
    }
    w.object_close();
}
