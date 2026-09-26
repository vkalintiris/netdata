//! The contexts v2 engine, ported from `api_v2_contexts_internal()` (`src/web/api/v2/api_v2_contexts.c`),
//! `rrdcontext_to_json_v2()` and its host walk (`src/database/contexts/api_v2_contexts.c`, `query_scope.c`) for the
//! modes the agent serves so far: `/api/v3/stream_path`. Decisions D51 in the status repository.

use std::sync::Arc;
use std::time::Instant;

use netdata_agent_query::jsonwrap_v2::{cloud_timings, node_add_v2};
use netdata_agent_query::keys::Keys;
use netdata_agent_query::request::pairs;
use netdata_agent_query::tables::{
    contexts_options::{DEBUG, JSON_LONG_KEYS, MCP, MINIFY, RFC3339},
    contexts_options_to_json_array, parse_contexts_options,
};
use netdata_agent_query::target::{host_matches, matches_retention};
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::parse::str2l;
use netdata_agent_text::simple_pattern::SimplePattern;
use netdata_agent_text::time_window::relative_window_to_absolute_query;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;

use crate::router::Route;
use crate::server::{self, Reply};
use crate::startup;

/// `CONTEXTS_V2_MODE`.
pub mod mode {
    pub const SEARCH: u32 = 1 << 1;
    pub const NODES: u32 = 1 << 2;
    pub const NODES_INFO: u32 = 1 << 3;
    pub const NODE_INSTANCES: u32 = 1 << 4;
    pub const NODES_STREAM_PATH: u32 = 1 << 5;
    pub const CONTEXTS: u32 = 1 << 6;
    pub const AGENTS: u32 = 1 << 7;
    pub const AGENTS_INFO: u32 = 1 << 8;
    pub const VERSIONS: u32 = 1 << 9;
    pub const FUNCTIONS: u32 = 1 << 10;
    pub const ALERTS: u32 = 1 << 11;
    pub const ALERT_TRANSITIONS: u32 = 1 << 12;
}

/// `buffer_json_contexts_v2_mode_to_array()`'s names, in its order.
const MODE_NAMES: [(u32, &str); 11] = [
    (mode::VERSIONS, "versions"),
    (mode::AGENTS, "agents"),
    (mode::AGENTS_INFO, "agents-info"),
    (mode::NODES, "nodes"),
    (mode::NODES_INFO, "nodes-info"),
    (mode::NODES_STREAM_PATH, "nodes-stream-path"),
    (mode::NODE_INSTANCES, "nodes-instances"),
    (mode::CONTEXTS, "contexts"),
    (mode::SEARCH, "search"),
    (mode::ALERTS, "alerts"),
    (mode::ALERT_TRANSITIONS, "alert_transitions"),
];

/// `struct api_v2_contexts_request`, the parts the served modes read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Request {
    scope_nodes: Option<Vec<u8>>,
    nodes: Option<Vec<u8>>,
    scope_contexts: Option<Vec<u8>>,
    contexts: Option<Vec<u8>>,
    options: u64,
    after: i64,
    before: i64,
    timeout_ms: i64,
}

/// `api_v2_contexts_internal()`'s parameter loop: the last occurrence of a value wins, options accumulate.
fn parse(query: &[u8], mode: u32, options: u64) -> Request {
    let context_modes =
        mode::NODES | mode::CONTEXTS | mode::SEARCH | mode::ALERTS | mode::ALERT_TRANSITIONS;
    let mut req = Request {
        options,
        ..Request::default()
    };
    for (name, value) in pairs(query) {
        match name {
            b"scope_nodes" => req.scope_nodes = Some(value.to_vec()),
            b"nodes" => req.nodes = Some(value.to_vec()),
            b"scope_contexts" if mode & context_modes != 0 => {
                req.scope_contexts = Some(value.to_vec())
            }
            b"contexts" if mode & context_modes != 0 => req.contexts = Some(value.to_vec()),
            b"options" => req.options |= parse_contexts_options(value),
            b"after" => req.after = str2l(value),
            b"before" => req.before = str2l(value),
            b"timeout" => req.timeout_ms = str2l(value),
            _ => {}
        }
    }
    req
}

/// The per-host timeout check: C compares `now > received + timeout_ms * 1000ULL` in unsigned arithmetic, so a
/// negative timeout wraps below the clock and fires at once.
fn timed_out(now_ut: u64, received_ut: u64, timeout_ms: i64) -> bool {
    timeout_ms != 0 && now_ut > received_ut.wrapping_add((timeout_ms as u64).wrapping_mul(1000))
}

/// The query window (`ctl.window`) and C's `ctl.now`.
#[derive(Debug, Clone, Copy)]
struct Window {
    range: Option<(i64, i64)>,
    now: i64,
}

/// `query_scope_foreach_context()` with `rrdcontext_to_json_v2_add_context()` for the node modes: whether any context
/// of the host counts. `scope_contexts` first names one context exactly, else its pattern filters them; the
/// `contexts` selector does not filter nodes (C ignores whether a context is queryable here).
fn any_context(
    host: &Host,
    scope_contexts: Option<&[u8]>,
    scope: Option<&SimplePattern>,
    window: Window,
) -> bool {
    let counts = |rc: &netdata_agent_rrd::contexts::Context| match window.range {
        None => true,
        Some((after, before)) => {
            let state = rc.state();
            let last = if rc.flags.is_collected() {
                window.now
            } else {
                state.last_time_s
            };
            matches_retention(after, before, state.first_time_s, last, 0)
        }
    };
    let exact = scope_contexts
        .map(String::from_utf8_lossy)
        .and_then(|id| host.contexts().get(&id));
    match exact {
        Some(rc) => counts(&rc),
        None => host
            .contexts()
            .all()
            .iter()
            .filter(|rc| scope.is_none_or(|sp| sp.matches(rc.id().as_bytes())))
            .any(|rc| counts(rc)),
    }
}

/// `buffer_json_node_add_v2_mcp()`.
fn node_add_v2_mcp(w: &mut JsonWriter, host: &Host) {
    w.member_add_string("machine_guid", host.machine_guid());
    let node_id = host.node_id();
    if node_id != [0; 16] {
        w.member_add_uuid("node_id", &node_id);
    }
    w.member_add_string("hostname", host.hostname());
    // no vnodes here
    w.member_add_string(
        "relationship",
        if host.is_localhost() {
            "localhost"
        } else {
            "child"
        },
    );
    w.member_add_boolean("connected", host.is_online());
}

/// `rrdcontext_to_json_v2_rrdhost()` for the node modes served.
fn node_to_json(
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

/// The `request` object of `options=debug`.
fn request_to_json(w: &mut JsonWriter, req: &Request, mode: u32) {
    let text = |v: &Option<Vec<u8>>| v.clone();
    w.member_add_object("request");
    w.member_add_array(Some(b"mode"));
    for (bit, name) in MODE_NAMES {
        if mode & bit != 0 {
            w.add_array_item_string(name);
        }
    }
    w.array_close();
    contexts_options_to_json_array(w, b"options", req.options);
    let listed = mode & (mode::CONTEXTS | mode::SEARCH | mode::ALERTS) != 0;
    w.member_add_object("scope");
    w.member_add_string_opt("scope_nodes", text(&req.scope_nodes).as_deref());
    if listed {
        w.member_add_string_opt("scope_contexts", text(&req.scope_contexts).as_deref());
    }
    w.object_close();
    w.member_add_object("selectors");
    w.member_add_string_opt("nodes", text(&req.nodes).as_deref());
    if listed {
        w.member_add_string_opt("contexts", text(&req.contexts).as_deref());
    }
    w.object_close();
    w.member_add_object("filters");
    let rfc3339 = req.options & RFC3339 != 0;
    w.member_add_time_t_formatted("after", req.after, rfc3339);
    w.member_add_time_t_formatted("before", req.before, rfc3339);
    w.object_close();
    w.object_close();
}

/// `rrdcontext_to_json_v2()` for the node modes served.
fn render(hosts: &[Arc<Host>], localhost: &Host, req: &Request, mode: u32, wall_s: i64) -> Reply {
    let received = Instant::now();
    let received_ut = startup::now_ut();
    let pattern = |v: &Option<Vec<u8>>| v.as_deref().and_then(SimplePattern::from_web);
    let (scope_nodes, nodes) = (pattern(&req.scope_nodes), pattern(&req.nodes));
    let (contexts, scope_contexts) = (pattern(&req.contexts), pattern(&req.scope_contexts));
    let window = if req.after != 0 || req.before != 0 {
        let (after, before, _) = relative_window_to_absolute_query(req.after, req.before, wall_s);
        Window {
            range: Some((after, before)),
            now: wall_s - 1,
        }
    } else {
        Window {
            range: None,
            now: wall_s,
        }
    };
    // query_scope_foreach_host() with rrdcontext_to_json_v2_add_host()
    let mut selected = Vec::new();
    for host in hosts {
        if scope_nodes
            .as_ref()
            .is_some_and(|sp| !host_matches(sp, host))
            || nodes.as_ref().is_some_and(|sp| !host_matches(sp, host))
        {
            continue;
        }
        if let Some((after, before)) = window.range {
            let (first, last) = host.contexts().retention();
            let last = if host.is_online() { window.now } else { last };
            if !matches_retention(after, before, first, last, 0) {
                continue;
            }
        }
        if timed_out(startup::now_ut(), received_ut, req.timeout_ms) {
            // the buffer is flushed but keeps its JSON content type
            return Reply {
                code: status::GATEWAY_TIMEOUT,
                content_type: ContentType::ApplicationJson,
                body: b"query timeout".to_vec(),
                ..Reply::default()
            };
        }
        let patterns = contexts.is_some() || scope_contexts.is_some();
        let mut matched = mode & (mode::NODES | mode::FUNCTIONS | mode::ALERTS) != 0
            && !patterns
            && window.range.is_none();
        if mode & (mode::CONTEXTS | mode::SEARCH | mode::ALERTS) != 0 || patterns {
            matched |= any_context(
                host,
                req.scope_contexts.as_deref(),
                scope_contexts.as_ref(),
                window,
            );
        } else if window.range.is_some() {
            // C checks the host's retention against the window again: it passed above
            matched = true;
        }
        if matched {
            selected.push(Arc::clone(host));
        }
    }
    let debug = req.options & DEBUG != 0;
    let mut w = JsonWriter::new(if req.options & MINIFY != 0 && !debug {
        JsonOptions::MINIFY
    } else {
        JsonOptions::DEFAULT
    });
    let mcp = req.options & MCP != 0;
    if !mcp {
        w.member_add_uint64("api", 2);
    }
    if debug {
        request_to_json(&mut w, req, mode);
    }
    let k = Keys::with_long(req.options & JSON_LONG_KEYS != 0);
    w.member_add_array(Some(b"nodes"));
    for (ni, host) in selected.iter().enumerate() {
        node_to_json(&mut w, host, localhost, ni, k, req, mode);
    }
    w.array_close();
    if !mcp {
        cloud_timings(&mut w, "timings", received, Instant::now());
    }
    w.finalize();
    Reply {
        code: status::OK,
        content_type: ContentType::ApplicationJson,
        body: w.into_bytes(),
        ..Reply::default()
    }
}

/// `api_v3_stream_path()`: the nodes with their stream paths; the host in the URL does not matter.
pub fn stream_path(route: &Route<'_>, query: &[u8]) -> Reply {
    let hosts = &route.shared.hosts;
    let stream_path_mode = mode::NODES | mode::NODES_STREAM_PATH;
    let req = parse(query, stream_path_mode, 0);
    render(
        &hosts.all(),
        hosts.localhost(),
        &req,
        stream_path_mode,
        server::now(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_as_c_reads_them() {
        let req = parse(
            b"nodes=a&nodes=b&options=minify&options=debug|long-keys&scope_contexts=x&q=y&timeout=-1&after=-10&nodes=",
            mode::NODES | mode::NODES_STREAM_PATH,
            0,
        );
        assert_eq!(
            req,
            Request {
                nodes: Some(b"b".to_vec()),
                scope_contexts: Some(b"x".to_vec()),
                options: MINIFY | DEBUG | JSON_LONG_KEYS,
                after: -10,
                timeout_ms: -1,
                ..Request::default()
            }
        );
        // scope_contexts and contexts are read only by the modes that have them
        assert_eq!(parse(b"contexts=x", mode::VERSIONS, 0).contexts, None);
    }

    #[test]
    fn a_negative_timeout_fires_at_once() {
        let r = 1_000_000_000;
        assert!(timed_out(r + 1, r, -1));
        assert!(!timed_out(r + 10, r, 0));
        assert!(!timed_out(r + 999, r, 1));
        assert!(timed_out(r + 1001, r, 1));
        // a huge negative timeout wraps past the clock
        assert!(!timed_out(r + 1, r, -1_000_000_000_000_000));
    }
}
