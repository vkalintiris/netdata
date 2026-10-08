//! The contexts v2 engine, ported from `api_v2_contexts_internal()` (`src/web/api/v2/api_v2_contexts.c`),
//! `rrdcontext_to_json_v2()` and its host walk (`src/database/contexts/api_v2_contexts.c`, `query_scope.c`) for the
//! modes the agent serves so far: `/api/v3/stream_path`, `/api/v2/info` (`/api/v3/info`), `/api/v2/functions`
//! (`/api/v3/functions`), `/api/v2/versions` (`/api/v3/versions`), `/api/v2/nodes` (`/api/v3/nodes`),
//! `/api/v2/node_instances` (`/api/v3/node_instances`), `/api/v2/contexts` (`/api/v3/contexts`), `/api/v2/alerts`
//! (`/api/v3/alerts`), `/api/v2/alert_transitions` (`/api/v3/alert_transitions`) and `/api/v2/q` (`/api/v3/q`).
//! Decisions D51, D92, D160, D231 and D234 in the status repository.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Instant;

use netdata_agent_log::netdata_log_error;
use netdata_agent_metadata::health_log::TransitionsOf;
use netdata_agent_nrpc::catalog;

use netdata_agent_query::jsonwrap_v2::{cloud_timings, version_hashes_v2};
use netdata_agent_query::request::pairs;
use netdata_agent_query::tables::{
    alert_statuses_to_json_array,
    contexts_options::{
        CONFIGURATIONS, DEBUG, DIMENSIONS, FAMILY, INSTANCES, LABELS, LIVENESS, MCP, MINIFY, PRIORITIES,
        RETENTION, RFC3339, SUMMARY, TITLES, UNITS,
    },
    contexts_options_to_json_array, parse_alert_statuses, parse_contexts_options,
};
use netdata_agent_query::target::{Versions, foreach_context, foreach_host, matches_retention};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::contexts::{Context, ContextState};
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::parse::{str2l, str2uint64, str2ul, strtoul0, uuid_parse_flexi};
use netdata_agent_text::print::uuid_lower_text;
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
mod instances;
mod labels;
mod nodes;
mod search;
mod transitions;

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
    /// `alert`: for the alerts mode a pattern on the alerts' names; for the transitions mode one alert name,
    /// compared whole by the statement.
    alert: Option<Vec<u8>>,
    /// `transition`: the id of one transition of the alert log (the two alert modes).
    transition: Option<Vec<u8>>,
    /// `status`: the bits of the status words an alerts request keeps; 0 keeps every alert.
    status: u64,
    /// `q`: the words a search looks for (the search mode).
    q: Option<Vec<u8>>,
    /// `last`: how many transitions an `alert_transitions` request keeps; 1 when it is 0 or not given.
    last: u32,
    /// `anchor_gi`: the global id the kept transitions are newer than.
    anchor_gi: u64,
    /// The request's text for each facet of `alert_transitions`, in the order of `transitions::FACETS`.
    facets: [Option<Vec<u8>>; 9],
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
            b"q" if mode & mode::SEARCH != 0 => req.q = Some(value.to_vec()),
            b"options" => req.options |= parse_contexts_options(value),
            b"after" => req.after = str2l(value),
            b"before" => req.before = str2l(value),
            b"timeout" => req.timeout_ms = str2l(value),
            b"cardinality" | b"cardinality_limit" => req.cardinality_limit = str2ul(value),
            b"alert" if mode & alert_modes != 0 => req.alert = Some(value.to_vec()),
            b"transition" if mode & alert_modes != 0 => req.transition = Some(value.to_vec()),
            // the words of one `status` add up; a later `status` replaces an earlier one
            b"status" if mode & mode::ALERTS != 0 => req.status = parse_alert_statuses(value),
            // the transitions' own: `last` as `strtoul(value, NULL, 0)` cut to 32 bits; `context` writes the
            // request's `contexts`, so the later of the two wins; a facet's name is its parameter
            b"last" if mode & mode::ALERT_TRANSITIONS != 0 => req.last = strtoul0(value).0 as u32,
            b"context" if mode & mode::ALERT_TRANSITIONS != 0 => req.contexts = Some(value.to_vec()),
            b"anchor_gi" if mode & mode::ALERT_TRANSITIONS != 0 => req.anchor_gi = str2uint64(value).0,
            name if mode & mode::ALERT_TRANSITIONS != 0 => {
                let facet = transitions::FACETS.iter().position(|(id, _, _)| id.as_bytes() == name);
                if let Some(facet) = facet {
                    req.facets[facet] = Some(value.to_vec());
                }
            }
            _ => {}
        }
    }
    if mode & mode::ALERT_TRANSITIONS != 0 && req.last == 0 {
        req.last = 1;
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
        if mode & mode::ALERT_TRANSITIONS != 0 {
            w.member_add_string_opt("context", text(&req.contexts).as_deref());
            w.member_add_uint64("anchor_gi", req.anchor_gi);
            w.member_add_uint64("last", u64::from(req.last));
        }
        w.member_add_string_opt("alert", text(&req.alert).as_deref());
        w.member_add_string_opt("transition", text(&req.transition).as_deref());
        w.object_close();
    }
    w.object_close();
    w.member_add_object("filters");
    if mode & mode::SEARCH != 0 {
        w.member_add_string_opt("q", text(&req.q).as_deref());
    }
    let rfc3339 = req.options & RFC3339 != 0;
    w.member_add_time_t_formatted("after", req.after, rfc3339);
    w.member_add_time_t_formatted("before", req.before, rfc3339);
    w.object_close();
    if mode & mode::ALERT_TRANSITIONS != 0 {
        w.member_add_object("facets");
        for ((id, _, _), value) in transitions::FACETS.iter().zip(&req.facets) {
            w.member_add_string_opt(id, value.as_deref());
        }
        w.object_close();
    }
    w.object_close();
}

/// `contexts_v2_alert_transitions_to_json()` with its query (`sql_alert_transitions()`): the alert log's
/// transitions of the hosts selected, inside the window's two ends (none given: both 0, which only an entry
/// stored with a global id of 0 meets, as in C), of
/// the request's `contexts` text as one chart context and of its `alert` text as one alert name, each compared
/// whole; or, with `transition=`, the entries with that id whatever their host and time (a text that is no UUID is
/// reported and finds nothing). Without a metadata database there is no entry. The rules of `options=config` are
/// read after the rows, as C reads them.
fn alert_transitions_to_json(
    w: &mut JsonWriter,
    shared: &Shared,
    req: &Request,
    selected: &[Arc<Host>],
    ends: Option<(i64, i64)>,
) {
    let defaults = shared.health.host_defaults(shared.hosts.localhost());
    let mut collector = transitions::Collector::new(&req.facets, defaults.1, req.last, req.anchor_gi);
    let meta = shared.meta.as_ref().and_then(std::sync::Weak::upgrade);
    let hosts: Vec<[u8; 16]> = selected.iter().filter_map(|host| crate::meta_store::host_id(host)).collect();
    let (after_s, before_s) = ends.unwrap_or((0, 0));
    let id = req.transition.as_deref().map(uuid_parse_flexi);
    let of = match &id {
        None => Some(TransitionsOf::Window {
            hosts: &hosts,
            after_s,
            before_s,
            context: req.contexts.as_deref(),
            alert_name: req.alert.as_deref(),
        }),
        Some(Some(id)) => Some(TransitionsOf::Id(id)),
        Some(None) => {
            let text = String::from_utf8_lossy(req.transition.as_deref().unwrap_or_default());
            netdata_log_error!("Invalid transition given {text}");
            None
        }
    };
    if let (Some(of), Some(meta)) = (&of, &meta) {
        meta.alert_transitions(of, |row| collector.row(row));
    }
    let rules = (req.options & CONFIGURATIONS != 0).then(|| {
        let mut rules = Vec::new();
        if let Some(meta) = &meta {
            let _ = meta.alert_configs(&collector.config_hashes(), |rule| rules.push(rule));
        }
        rules
    });
    let host = |guid: &str| shared.hosts.find_by_guid(guid).map(|host| (host.hostname(), host.node_id()));
    collector.to_json(w, req.options, host, defaults, rules.as_deref());
}

/// `rrdcontext_to_json_v2()` for the modes served.
fn render(shared: &Shared, req: &Request, mode: u32, wall_s: i64) -> Reply {
    let received = Instant::now();
    let hosts = shared.hosts.all();
    let received_ut = startup::now_ut();
    let pattern = |v: &Option<Vec<u8>>| v.as_deref().and_then(SimplePattern::from_web);
    let (mut scope_nodes, mut nodes) = (pattern(&req.scope_nodes), pattern(&req.nodes));
    let (mut contexts, mut scope_contexts) = (pattern(&req.contexts), pattern(&req.scope_contexts));
    // rrdcontexts_v2_init_alert_dictionaries(): `transition=` names one transition of the alert log. The host and
    // the context of its alert replace the request's two scopes and drop its two selectors, and its alarm id
    // becomes a filter (the request's own `scope_contexts` text stays, and the walk still tries it as an exact
    // id). A text that is no UUID, a database that cannot be asked and an id no entry has answer 404, with nothing.
    let mut alarm_id = 0;
    if mode & mode::ALERTS != 0
        && let Some(transition) = &req.transition
    {
        let meta = shared.meta.as_ref().and_then(std::sync::Weak::upgrade);
        let found = uuid_parse_flexi(transition)
            .zip(meta)
            .map(|(id, meta)| meta.find_alert_transition(&id))
            .unwrap_or_default();
        if found.is_empty() {
            return Reply { code: status::NOT_FOUND, ..Reply::default() };
        }
        // rrdcontext_v2_set_transition_filter(), once per entry with the id: the last one's values stay
        for alert in found {
            let (guid, len) = uuid_lower_text(&alert.host_id, false);
            scope_nodes = SimplePattern::from_web(&guid[..len]);
            nodes = None;
            if let Some(context) = alert.context.filter(|context| !context.is_empty()) {
                scope_contexts = SimplePattern::from_web(&context);
                contexts = None;
            }
            alarm_id = i64::from(alert.alarm_id);
        }
    }
    // the alerts an alerts request keeps: by a pattern on their names, by their status, by the transition's alarm
    let alert_name = pattern(&req.alert);
    let filters = alerts::Filters { name: alert_name.as_ref(), alarm_id, status: req.status };
    let mut alerts = (mode & mode::ALERTS != 0).then(|| alerts::Collector::new(req.options));
    // The window's two ends as the request means them. The transitions' statement reads them; for that mode C
    // leaves its window disabled, so no host and no context is tested against them.
    let ends = (req.after != 0 || req.before != 0).then(|| {
        let (after, before, _) = relative_window_to_absolute_query(req.after, req.before, wall_s);
        (after, before)
    });
    let transitions_mode = mode & mode::ALERT_TRANSITIONS != 0;
    let window = Window {
        range: ends.filter(|_| !transitions_mode),
        now: if ends.is_some() { wall_s - 1 } else { wall_s },
    };
    // query_scope_foreach_host() with rrdcontext_to_json_v2_add_host()
    let mut selected = Vec::new();
    let mut functions = Functions::default();
    let mut dict = (mode & (mode::CONTEXTS | mode::SEARCH) != 0).then(ContextsDict::default);
    // The search's words: parts of a text, whatever the case. Without a word (no `q`, `q=*`, only separators) there
    // is no search: every context in scope is stored with no match.
    let q = req.q.as_deref().and_then(SimplePattern::from_web_nocase_substring);
    let mut fts = search::Fts::default();
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
                    // rrdcontext_to_json_v2_full_text_search(): a context nothing of which matches does not count
                    let found = q.as_ref().map(|q| search::search(rc, &state, q, req.options, window, &mut fts));
                    if found.as_ref().is_some_and(|found| !found.any()) {
                        return ControlFlow::Continue(());
                    }
                    if let Some(alerts) = alerts.as_mut() {
                        // rrdcontext_matches_alert(): an instance's `ni` is the index the host is about to get
                        if !alerts.context(rc, host_alerts.as_deref(), ni, &filters) {
                            return ControlFlow::Continue(());
                        }
                    }
                    if let Some(dict) = dict.as_mut() {
                        if mode & mode::SEARCH != 0 {
                            dict.add_searched(rc, state, req.options, found.unwrap_or_default());
                        } else {
                            dict.add(rc, state, req.options, window);
                        }
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
    if transitions_mode {
        // the transitions' answer has nothing of the other modes': the nodes select the hosts and are not listed
        alert_transitions_to_json(&mut w, shared, req, &selected, ends);
    }
    if mode & mode::NODES != 0 && !transitions_mode {
        w.member_add_array(Some(b"nodes"));
        for (ni, host) in selected.iter().enumerate() {
            node_to_json(&mut w, host, shared, ni, req, mode, window.now);
        }
        w.array_close();
    }
    if mode & mode::FUNCTIONS != 0 {
        functions.to_json(&mut w, mcp);
    }
    if let Some(dict) = &dict {
        if mode & mode::SEARCH != 0 {
            dict.search_to_json(&mut w, req);
        } else {
            dict.to_json(&mut w, req, window.now);
        }
    }
    if let Some(alerts) = &mut alerts {
        if mcp {
            // an instance names its host, which is listed at the index the instance took
            let hostname = |ni: usize| selected.get(ni).map(|host| host.hostname()).unwrap_or_default();
            alerts.to_json_mcp(&mut w, req.options, req.cardinality_limit, hostname);
        } else {
            // the rules' names count under their texts in a summary's groupings alone
            if req.options & SUMMARY != 0 {
                alerts.count_prototypes(&shared.health.prototypes());
            }
            alerts.to_json(&mut w, req.options);
        }
    }
    if mode & mode::SEARCH != 0 {
        fts.to_json(&mut w);
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
/// `values` listed one by one; with `options=mcp` as headers and rows. `transition=` narrows the request to the
/// alert of one transition of the alert log. No versions and no agents. The host in the URL does not matter.
pub fn alerts(route: &Route<'_>, query: &[u8]) -> Reply {
    let alerts_mode = mode::ALERTS | mode::NODES;
    let mut req = parse(query, alerts_mode, 0);
    // rrdcontext_to_json_v2() strips `config` for this mode before anything reads the options, the echo too
    req.options &= !CONFIGURATIONS;
    render(route.shared, &req, alerts_mode, now_realtime_s())
}

/// `api_v2_alert_transitions()` (`/api/v2/alert_transitions`, `/api/v3/alert_transitions`): the alert log's
/// transitions of the hosts in scope inside the request's window, newest first: the nine facets with every value
/// the window's rows show and how many rows each would give, the `last` rows the facets select that are newer than
/// `anchor_gi`, with `options=config` their rules, and the counts of what was evaluated, matched and left before
/// and after. `transition=` asks for one transition by its id. No nodes, no versions and no agents. The host in the
/// URL does not matter.
pub fn alert_transitions(route: &Route<'_>, query: &[u8]) -> Reply {
    let transitions_mode = mode::ALERT_TRANSITIONS | mode::NODES;
    let mut req = parse(query, transitions_mode, 0);
    // rrdcontext_to_json_v2() strips `instances` for this mode before anything reads the options, the echo too
    req.options &= !INSTANCES;
    render(route.shared, &req, transitions_mode, now_realtime_s())
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

/// `api_v2_q()` (`/api/v2/q`, `/api/v3/q`): the full-text search of the contexts of the hosts in scope: each
/// context one of whose texts holds a word of `q` (its id, title, family, units, its instances' and dimensions'
/// ids and names, its labels' keys and values), with what matched of it; then how many texts were tested, the
/// versions and the agent. The nodes are the node list's while no context pattern and no window is given, else
/// the hosts with a context that matched. Without a word in `q` nothing is searched and every context in scope is
/// listed. The host in the URL does not matter.
pub fn q(route: &Route<'_>, query: &[u8]) -> Reply {
    let search_mode = mode::SEARCH | mode::NODES | mode::AGENTS | mode::VERSIONS;
    let req = parse(query, search_mode, FAMILY | UNITS | TITLES | LABELS | INSTANCES | DIMENSIONS);
    render(route.shared, &req, search_mode, now_realtime_s())
}

/// `api_v2_nodes()` (`/api/v2/nodes`, `/api/v3/nodes`): the hosts in scope, each with its version, labels, system
/// info and state, its health and its capabilities; the host in the URL does not matter.
pub fn nodes(route: &Route<'_>, query: &[u8]) -> Reply {
    let nodes_mode = mode::NODES | mode::NODES_INFO;
    let req = parse(query, nodes_mode, 0);
    render(route.shared, &req, nodes_mode, now_realtime_s())
}

/// `api_v2_node_instances()` (`/api/v2/node_instances`, `/api/v3/node_instances`): the hosts in scope, each with
/// this agent's instance of it (its database, what feeds it, its health, functions and capabilities), then the
/// versions and the agent with its info; the host in the URL does not matter.
pub fn node_instances(route: &Route<'_>, query: &[u8]) -> Reply {
    let instances_mode =
        mode::NODES | mode::NODE_INSTANCES | mode::AGENTS | mode::AGENTS_INFO | mode::VERSIONS;
    let req = parse(query, instances_mode, 0);
    render(route.shared, &req, instances_mode, now_realtime_s())
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
    use netdata_agent_query::tables::contexts_options::JSON_LONG_KEYS;

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
        // the alerts' own: the last `status` replaces the first, a word that is no status adds nothing, and the
        // modes that are no alert mode read none of the three
        let alerts_mode = mode::ALERTS | mode::NODES;
        let req = parse(b"alert=a*&transition=t&status=clear,warning&status=critical|bogus", alerts_mode, 0);
        let critical = netdata_agent_query::tables::alert_statuses::CRITICAL;
        assert_eq!((req.alert, req.transition, req.status), (Some(b"a*".to_vec()), Some(b"t".to_vec()), critical));
        assert_eq!(parse(b"status=bogus", alerts_mode, 0).status, 0);
        let other = parse(b"alert=a&transition=t&status=clear", mode::CONTEXTS | mode::NODES, 0);
        assert_eq!((other.alert, other.transition, other.status), (None, None, 0));
        // `q` is the search's alone; its last value counts
        assert_eq!(parse(b"q=a&q=b|c", mode::SEARCH | mode::NODES, 0).q, Some(b"b|c".to_vec()));
        assert_eq!(parse(b"q=a", mode::CONTEXTS | mode::NODES, 0).q, None);
    }

    /// The parameters of `alert_transitions` alone: `last` in any of C's bases, 1 when it is 0 or missing;
    /// `context` and `contexts` write one field, the later one wins; `anchor_gi`; a facet's name is its parameter.
    /// `status` is not read in this mode, and the alerts mode reads none of these.
    #[test]
    fn the_transitions_parameters_as_c_reads_them() {
        let transitions_mode = mode::ALERT_TRANSITIONS | mode::NODES;
        let query = b"last=0x10&anchor_gi=12&context=a&contexts=b&f_status=warning|critical&f_context=x&status=clear\
                      &alert=n&transition=t&f_node=&f_bogus=1";
        let mut facets: [Option<Vec<u8>>; 9] = Default::default();
        facets[0] = Some(b"warning|critical".to_vec());
        facets[8] = Some(b"x".to_vec());
        let expected = Request {
            contexts: Some(b"b".to_vec()),
            alert: Some(b"n".to_vec()),
            transition: Some(b"t".to_vec()),
            last: 16,
            anchor_gi: 12,
            facets,
            ..Request::default()
        };
        assert_eq!(parse(query, transitions_mode, 0), expected);
        assert_eq!(parse(b"contexts=b&context=a", transitions_mode, 0).contexts, Some(b"a".to_vec()));
        for (query, last) in [("", 1), ("last=0", 1), ("last=x", 1), ("last=010", 8), ("last=4294967297", 1)] {
            assert_eq!(parse(query.as_bytes(), transitions_mode, 0).last, last, "{query}");
        }
        let alerts = parse(b"last=5&anchor_gi=7&context=a&f_status=clear", mode::ALERTS | mode::NODES, 0);
        assert_eq!(alerts, Request::default());
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
