//! The contexts v2 engine, ported from `api_v2_contexts_internal()` (`src/web/api/v2/api_v2_contexts.c`),
//! `rrdcontext_to_json_v2()` and its host walk (`src/database/contexts/api_v2_contexts.c`, `query_scope.c`) for the
//! modes the agent serves so far: `/api/v3/stream_path`, `/api/v2/info` (`/api/v3/info`), `/api/v2/functions`
//! (`/api/v3/functions`), `/api/v2/versions` (`/api/v3/versions`), `/api/v2/nodes` (`/api/v3/nodes`) and
//! `/api/v2/contexts` (`/api/v3/contexts`), and `/api/v2/alerts` (`/api/v3/alerts`) without `transition=` and
//! `options=mcp` yet. Decisions D51, D92, D160, D231 and D234 in the status repository.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Instant;

use netdata_agent_nrpc::catalog;

use netdata_agent_query::jsonwrap_v2::{cloud_timings, version_hashes_v2};
use netdata_agent_query::keys::Keys;
use netdata_agent_query::request::pairs;
use netdata_agent_query::tables::{
    alert_statuses_to_json_array,
    contexts_options::{
        CONFIGURATIONS, DEBUG, FAMILY, JSON_LONG_KEYS, LIVENESS, MCP, MINIFY, PRIORITIES, RETENTION, RFC3339, UNITS,
    },
    contexts_options_to_json_array, parse_alert_statuses, parse_contexts_options,
};
use netdata_agent_query::target::{Versions, foreach_context, foreach_host, matches_retention};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::contexts::{Context, ContextState};
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::parse::{str2l, str2ul};
use netdata_agent_text::simple_pattern::SimplePattern;
use netdata_agent_text::time_window::relative_window_to_absolute_query;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;

use crate::router::Route;
use crate::server::{Reply, Shared};
use crate::startup;

mod agents;
mod alerts;
mod contexts;
mod functions;
mod labels;
mod nodes;

use agents::agents;
use contexts::ContextsDict;
use functions::Functions;
use nodes::node_to_json;

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
    /// `cardinality` or `cardinality_limit`: how many contexts, and items of each of their lists, are printed; 0 for
    /// all.
    cardinality_limit: u64,
    /// `alert`: a pattern on the alerts' names (the two alert modes).
    alert: Option<Vec<u8>>,
    /// `transition`: the id of one transition of the alert log (the two alert modes).
    transition: Option<Vec<u8>>,
    /// `status`: the bits of the status words an alerts request keeps; 0 keeps every alert.
    status: u64,
}

/// `api_v2_contexts_internal()`'s parameter loop: the last occurrence of a value wins, options accumulate.
fn parse(query: &[u8], mode: u32, options: u64) -> Request {
    let context_modes =
        mode::NODES | mode::CONTEXTS | mode::SEARCH | mode::ALERTS | mode::ALERT_TRANSITIONS;
    let alert_modes = mode::ALERTS | mode::ALERT_TRANSITIONS;
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
            b"cardinality" | b"cardinality_limit" => req.cardinality_limit = str2ul(value),
            b"alert" if mode & alert_modes != 0 => req.alert = Some(value.to_vec()),
            b"transition" if mode & alert_modes != 0 => req.transition = Some(value.to_vec()),
            // the words of one `status` add up; a later `status` replaces an earlier one
            b"status" if mode & mode::ALERTS != 0 => req.status = parse_alert_statuses(value),
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

/// `rrdcontext_to_json_v2_add_context()`'s first test: with a window, a context counts only when its retention meets
/// it, a collected context's reaching the walk's clock.
fn context_in_window(rc: &Context, state: &ContextState, window: Window) -> bool {
    let Some((after, before)) = window.range else {
        return true;
    };
    let last = if rc.flags.is_collected() {
        window.now
    } else {
        state.last_time_s
    };
    matches_retention(after, before, state.first_time_s, last, 0)
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
    if mode & (mode::ALERTS | mode::ALERT_TRANSITIONS) != 0 {
        w.member_add_object("alerts");
        if mode & mode::ALERTS != 0 {
            alert_statuses_to_json_array(w, b"status", req.status);
        }
        w.member_add_string_opt("alert", text(&req.alert).as_deref());
        w.member_add_string_opt("transition", text(&req.transition).as_deref());
        w.object_close();
    }
    w.object_close();
    w.member_add_object("filters");
    let rfc3339 = req.options & RFC3339 != 0;
    w.member_add_time_t_formatted("after", req.after, rfc3339);
    w.member_add_time_t_formatted("before", req.before, rfc3339);
    w.object_close();
    w.object_close();
}

/// `rrdcontext_to_json_v2()` for the modes served.
fn render(shared: &Shared, req: &Request, mode: u32, wall_s: i64) -> Reply {
    let received = Instant::now();
    let hosts = shared.hosts.all();
    let received_ut = startup::now_ut();
    let pattern = |v: &Option<Vec<u8>>| v.as_deref().and_then(SimplePattern::from_web);
    let (scope_nodes, nodes) = (pattern(&req.scope_nodes), pattern(&req.nodes));
    let (contexts, scope_contexts) = (pattern(&req.contexts), pattern(&req.scope_contexts));
    // the alerts an alerts request keeps: by a pattern on their names and by their status
    let alert_name = pattern(&req.alert);
    let filters = alerts::Filters { name: alert_name.as_ref(), alarm_id: 0, status: req.status };
    let mut alerts = (mode & mode::ALERTS != 0).then(|| alerts::Collector::new(req.options));
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
    let mut functions = Functions::default();
    // C makes the dictionary for the search too (commit 6)
    let mut dict = (mode & mode::CONTEXTS != 0).then(ContextsDict::default);
    let mut versions = Versions::default();
    let walked = foreach_host(&hosts, scope_nodes.as_ref(), nodes.as_ref(), &mut versions, |host, queryable| {
        if !queryable {
            return ControlFlow::Continue(());
        }
        if let Some((after, before)) = window.range {
            let (first, last) = host.contexts().retention();
            let last = if host.is_online() { window.now } else { last };
            if !matches_retention(after, before, first, last, 0) {
                return ControlFlow::Continue(());
            }
        }
        if timed_out(startup::now_ut(), received_ut, req.timeout_ms) {
            return ControlFlow::Break(());
        }
        // With status words, a host whose last complete health pass counted no alert of a requested status
        // cannot have one the request keeps: its contexts are not walked (`rrdhost_alert_status_snapshot_read()`
        // and its filter). A host without a complete pass may have one.
        let host_alerts = if mode & mode::ALERTS != 0 { shared.health.host(host) } else { None };
        let may_have_alerts = req.status & alerts::STATUSES == 0
            || alerts::host_may_match(req.status, host_alerts.as_ref().and_then(|alerts| alerts.pass_counts()));
        let patterns = contexts.is_some() || scope_contexts.is_some();
        let matched_modes = mode::NODES | mode::FUNCTIONS | if may_have_alerts { mode::ALERTS } else { 0 };
        let mut matched = mode & matched_modes != 0 && !patterns && window.range.is_none();
        let walk_contexts = mode & (mode::CONTEXTS | mode::SEARCH | mode::ALERTS) != 0 || patterns;
        if walk_contexts && (mode & mode::ALERTS == 0 || may_have_alerts) {
            // query_scope_foreach_context() with rrdcontext_to_json_v2_add_context(): a host with a context that
            // counts is matched, and the contexts answer collects each one that does. The `contexts` selector
            // does not filter here (C ignores whether a context is queryable), so it only keeps a host without any
            // context out. Where nothing is collected, the first context that counts settles the host. An alerts
            // request counts a context only when it keeps an alert of it, and walks them all.
            let stop_at_first = dict.is_none() && alerts.is_none();
            let ni = selected.len();
            let mut counted = false;
            let first = foreach_context(
                host,
                req.scope_contexts.as_deref(),
                scope_contexts.as_ref(),
                // what `contexts` selects only marks a context, and this walk does not read the mark
                None,
                true,
                |rc, _| {
                    if stop_at_first && window.range.is_none() {
                        return ControlFlow::Break(());
                    }
                    let state = rc.state();
                    if !context_in_window(rc, &state, window) {
                        return ControlFlow::Continue(());
                    }
                    if let Some(alerts) = alerts.as_mut() {
                        // rrdcontext_matches_alert(): an instance's `ni` is the index the host is about to get
                        if !alerts.context(rc, host, host_alerts.as_deref(), ni, &filters) {
                            return ControlFlow::Continue(());
                        }
                    }
                    if let Some(dict) = dict.as_mut() {
                        dict.add(rc, state, req.options, window);
                    }
                    counted = true;
                    if stop_at_first { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
                },
            );
            matched |= counted || first.is_break();
        } else if window.range.is_some() {
            // C checks the host's retention against the window again: it passed above
            matched = true;
        }
        if matched {
            if mode & mode::FUNCTIONS != 0 {
                functions.add(catalog::to_dict(host.functions()), selected.len());
            }
            selected.push(Arc::clone(host));
        }
        ControlFlow::Continue(())
    });
    if walked.is_break() {
        // the buffer is flushed but keeps its JSON content type
        return Reply {
            code: status::GATEWAY_TIMEOUT,
            content_type: ContentType::ApplicationJson,
            body: b"query timeout".to_vec(),
            ..Reply::default()
        };
    }
    let executed = Instant::now();
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
    if mode & mode::NODES != 0 {
        let k = Keys::with_long(req.options & JSON_LONG_KEYS != 0);
        w.member_add_array(Some(b"nodes"));
        for (ni, host) in selected.iter().enumerate() {
            node_to_json(&mut w, host, shared, ni, k, req, mode);
        }
        w.array_close();
    }
    if mode & mode::FUNCTIONS != 0 {
        functions.to_json(&mut w, mcp);
    }
    if let Some(dict) = &dict {
        dict.to_json(&mut w, req, window.now);
    }
    if let Some(alerts) = &mut alerts {
        // contexts_v2_alerts_to_json(): a summary's hosts are printed as their indexes in `nodes`
        let node_index = |guid: &str| selected.iter().position(|host| host.machine_guid() == guid);
        alerts.count_prototypes(&shared.health.prototypes());
        alerts.to_json(&mut w, req.options, node_index);
    }
    if mode & mode::VERSIONS != 0 {
        // the host index's version as the answer is written
        versions.nodes_hard_hash = u64::from(shared.hosts.version());
        version_hashes_v2(&mut w, &versions);
    }
    // the agents' timings end the query; the cloud timings end there too
    let finished = (mode & mode::AGENTS != 0)
        .then(|| agents(&mut w, shared, req, mode, window.now, received, executed));
    if !mcp {
        cloud_timings(
            &mut w,
            "timings",
            received,
            finished.unwrap_or_else(Instant::now),
        );
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
    let stream_path_mode = mode::NODES | mode::NODES_STREAM_PATH;
    let req = parse(query, stream_path_mode, 0);
    render(route.shared, &req, stream_path_mode, now_realtime_s())
}

/// `api_v2_info()` (`/api/v2/info`, `/api/v3/info`): the agent with its info; the host in the URL does not matter.
pub fn info(route: &Route<'_>, query: &[u8]) -> Reply {
    let info_mode = mode::AGENTS | mode::AGENTS_INFO;
    let req = parse(query, info_mode, 0);
    render(route.shared, &req, info_mode, now_realtime_s())
}

/// `api_v2_alerts()` (`/api/v2/alerts`, `/api/v3/alerts`): the alerts of the hosts in scope that the request's
/// name pattern and status words keep, between the nodes and the timings: with `options=summary` summarised by
/// name and counted by type, component, classification, recipient and collecting module, with `instances` or
/// `values` listed one by one. No versions and no agents. The host in the URL does not matter.
pub fn alerts(route: &Route<'_>, query: &[u8]) -> Reply {
    let alerts_mode = mode::ALERTS | mode::NODES;
    let mut req = parse(query, alerts_mode, 0);
    // rrdcontext_to_json_v2() strips `config` for this mode before anything reads the options, the echo too
    req.options &= !CONFIGURATIONS;
    render(route.shared, &req, alerts_mode, now_realtime_s())
}

/// `api_v2_contexts()` (`/api/v2/contexts`, `/api/v3/contexts`): the contexts of the hosts in scope, merged by id,
/// with the nodes (every host in scope that `nodes` selects while no context pattern and no window is given, else
/// those of them with a context that counts), the versions and the agent; the dashboard's chart menu. `options`
/// adds to the route's defaults, so a request cannot remove one. The host in the URL does not matter.
pub fn contexts(route: &Route<'_>, query: &[u8]) -> Reply {
    let contexts_mode = mode::CONTEXTS | mode::NODES | mode::AGENTS | mode::VERSIONS;
    let req = parse(query, contexts_mode, PRIORITIES | RETENTION | LIVENESS | FAMILY | UNITS);
    render(route.shared, &req, contexts_mode, now_realtime_s())
}

/// `api_v2_nodes()` (`/api/v2/nodes`, `/api/v3/nodes`): the hosts in scope, each with its version, labels, system
/// info and state, its health and its capabilities; the host in the URL does not matter.
pub fn nodes(route: &Route<'_>, query: &[u8]) -> Reply {
    let nodes_mode = mode::NODES | mode::NODES_INFO;
    let req = parse(query, nodes_mode, 0);
    render(route.shared, &req, nodes_mode, now_realtime_s())
}

/// `api_v2_versions()` (`/api/v2/versions`, `/api/v3/versions`): the version hashes of the hosts in scope, none of
/// them listed; the host in the URL does not matter.
pub fn versions(route: &Route<'_>, query: &[u8]) -> Reply {
    let req = parse(query, mode::VERSIONS, 0);
    render(route.shared, &req, mode::VERSIONS, now_realtime_s())
}

/// `api_v2_functions()` (`/api/v2/functions`, `/api/v3/functions`): the methods of every host selected, merged by
/// version and name, with the nodes, the versions and the agent; the host in the URL does not matter.
pub fn functions(route: &Route<'_>, query: &[u8]) -> Reply {
    let functions_mode = mode::FUNCTIONS | mode::NODES | mode::AGENTS | mode::VERSIONS;
    let req = parse(query, functions_mode, 0);
    render(route.shared, &req, functions_mode, now_realtime_s())
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
