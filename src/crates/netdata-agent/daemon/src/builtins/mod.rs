//! C's built-in Functions (`src/web/api/functions/functions.c` `global_functions_add()`): five daemon methods on
//! localhost, registered by the main thread, which never ends their serving handle. Their handlers hold the hosts
//! weakly: localhost's registry holds the handlers, so a strong hold would keep the hosts alive.

use std::sync::{Arc, Weak};

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_nrpc::{Builtin, BuiltinDesc, access};
use netdata_agent_rrd::host::Hosts;
use netdata_agent_text::json::JsonWriter;

mod api_calls;
mod bearer_get_token;
mod cardinality;
mod streaming;

/// `NRPC_PRIORITY_DEFAULT + 1`, and `bearer_get_token`'s `+ 3`.
const PRIORITY: i32 = netdata_agent_nrpc::PRIORITY_DEFAULT + 1;
const BEARER_PRIORITY: i32 = netdata_agent_nrpc::PRIORITY_DEFAULT + 3;
/// The access of every built-in but cardinality: signed-in, same space, sensitive data.
const SENSITIVE: u32 = access::SIGNED_ID | access::SAME_SPACE | access::SENSITIVE_DATA;

const STREAMING_HELP: &str = "Shows real-time streaming connections and replication status between parent and child \
                              nodes, including connection health, data flow metrics, and ML status.";
const STREAMING_TOPOLOGY_HELP: &str = "Shows streaming topology relations across Netdata agents, including directional \
                                       parent/child links and transport metadata.";
const PROGRESS_HELP: &str =
    "Monitors active and recent Netdata API requests with transaction details, duration, and response sizes.";
const BEARER_GET_TOKEN_HELP: &str = "Get a bearer token for authenticated direct access to the agent";
const METRICS_CARDINALITY_HELP: &str = "Displays metrics cardinality statistics showing distribution of instances and \
                                        time-series across contexts and nodes. To change grouping, append parameter to \
                                        function name: 'netdata-metrics-cardinality' (default, group by context) or \
                                        'netdata-metrics-cardinality group:by-node' (group by node).";

/// What a built-in whose data lands with a later milestone answers (D176.3): nRPC's error shape.
pub(crate) const NOT_IMPLEMENTED: &str = "This feature is not implemented yet on this agent.";

fn not_implemented(reply: &mut Reply) -> u16 {
    reply.error(NOT_IMPLEMENTED, 501)
}

/// `buffer_json_finalize()` into the reply: JSON, not cacheable, no expiry (`buffer_json_initialize()` clears it).
fn json_reply(mut w: JsonWriter, reply: &mut Reply) -> u16 {
    w.finalize();
    reply.body = w.into_bytes();
    reply.content_type = ContentType::ApplicationJson;
    reply.expires = 0;
    reply.cacheable = false;
    200
}

/// `function_progress()`: localhost's progress table.
fn api_calls(hosts: Weak<Hosts>) -> Builtin {
    Arc::new(move |reply: &mut Reply, _: &[u8], _: Option<&_>, _: &[u8]| {
        let hostname = hosts.upgrade().map(|hosts| hosts.localhost().hostname()).unwrap_or_default();
        api_calls::render(netdata_agent_web::progress::Table::process(), reply, &hostname)
    })
}

/// `function_bearer_get_token()`: on localhost.
fn bearer_get_token(hosts: Weak<Hosts>) -> Builtin {
    Arc::new(move |reply: &mut Reply, _: &[u8], payload: Option<&_>, source: &[u8]| match hosts.upgrade() {
        Some(hosts) => bearer_get_token::call(hosts.localhost(), reply, payload, source),
        None => not_implemented(reply),
    })
}

/// `function_metrics_cardinality()`: every host's contexts.
fn cardinality(hosts: Weak<Hosts>) -> Builtin {
    Arc::new(move |reply: &mut Reply, function: &[u8], _: Option<&_>, _: &[u8]| match hosts.upgrade() {
        Some(hosts) => cardinality::render(&hosts, reply, function),
        None => not_implemented(reply),
    })
}

/// The five, in C's order.
fn descriptors(hosts: &Weak<Hosts>) -> [BuiltinDesc<'static>; 5] {
    let desc = |name: &'static str, help: &'static str, tags: &'static str, priority, access, handler| BuiltinDesc {
        name: name.as_bytes(),
        help: help.as_bytes(),
        tags: tags.as_bytes(),
        timeout_s: 10,
        priority,
        version: netdata_agent_nrpc::VERSION_DEFAULT,
        access,
        handler,
    };
    [
        desc("netdata-streaming", STREAMING_HELP, "top", PRIORITY, SENSITIVE, Arc::new(streaming::netdata_streaming)),
        desc("topology:streaming", STREAMING_TOPOLOGY_HELP, "top", PRIORITY, SENSITIVE, Arc::new(streaming::topology)),
        desc("netdata-api-calls", PROGRESS_HELP, "top", PRIORITY, SENSITIVE, api_calls(hosts.clone())),
        desc(
            "bearer_get_token",
            BEARER_GET_TOKEN_HELP,
            "hidden",
            BEARER_PRIORITY,
            SENSITIVE,
            bearer_get_token(hosts.clone()),
        ),
        desc(
            "netdata-metrics-cardinality",
            METRICS_CARDINALITY_HELP,
            "top",
            PRIORITY,
            access::ANONYMOUS_DATA,
            cardinality(hosts.clone()),
        ),
    ]
}

/// `global_functions_add()`: the five on localhost only (a vnode has none, a child's come as its streamed methods).
pub fn global_functions_add(hosts: &Arc<Hosts>) {
    let localhost = hosts.localhost();
    for desc in descriptors(&Arc::downgrade(hosts)) {
        if let Err(warning) = localhost.register_builtin(&desc) {
            nd_log!(Source::Daemon, Priority::Warning, "{warning}");
        }
    }
}

#[cfg(test)]
mod tests;
