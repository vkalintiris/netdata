//! The `config` function (`src/daemon/dyncfg/dyncfg-tree.c`): the tree of a host's configurations, and the catch-all
//! of every `config <id> ...` call no live `config <id>` method takes.

use std::sync::Weak;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::Transport;
use netdata_agent_nrpc::access;
use netdata_agent_nrpc::call::Request;
use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_query::jsonwrap_v2::{Agent, agents_v2};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};

use crate::model::{Cmds, SourceType, Status, Type, default_response, is_valid_id};
use crate::nodes::Node;
use crate::{Dyncfg, host_uuid};

/// `MAX_FUNCTION_PARAMETERS`.
const MAX_FUNCTION_PARAMETERS: usize = 1024;

/// The text a source is replaced with for a caller who may not see it.
const HIDDEN: &str = "User details hidden in anonymous mode. Sign in to access configuration details.";

/// The `config` function of a host (C's handler data).
pub(crate) struct Tree {
    pub(crate) dyncfg: Weak<Dyncfg>,
    pub(crate) host: Weak<Host>,
}

impl Transport for Tree {
    fn dispatch(&self, req: Request) -> u16 {
        match (self.dyncfg.upgrade(), self.host.upgrade()) {
            (Some(dyncfg), Some(host)) => dyncfg.config(&host, req),
            _ => {
                let mut reply = req.reply;
                let code = reply.error("The plugin that offered this function is not available.", 503);
                (req.done)(reply, code);
                code
            }
        }
    }
}

impl Dyncfg {
    /// `dyncfg_config_execute_cb()`: `config tree [path] [id]`, or `config <id> <action> [name]` for the catch-all; the
    /// caller is answered on every path.
    fn config(&self, host: &Host, mut req: Request) -> u16 {
        let words = quoted_strings_splitter(&req.function, MAX_FUNCTION_PARAMETERS, Separators::Whitespace);
        let word = |i: usize| words.get(i).map(Vec::as_slice);
        let error = |req: Request, msg: &str| {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DYNCFG TREE: function call '{}': {msg}",
                String::from_utf8_lossy(&req.function)
            );
            let mut reply = req.reply;
            let code = default_response(&mut reply, 400, msg);
            (req.done)(reply, code);
            code
        };
        if word(0) != Some(b"config") {
            return error(req, "invalid function call, expected: config");
        }
        let Some(action) = word(1).filter(|a| !a.is_empty()) else {
            return error(req, "invalid function call, expected: config tree");
        };
        if action == b"tree" {
            let path = word(2).filter(|p| !p.is_empty()).unwrap_or(b"/");
            let id = word(3).filter(|id| !id.is_empty());
            if id.is_some_and(|id| !is_valid_id(id)) {
                return error(req, "invalid id given");
            }
            let anonymous = req.user_access & access::SENSITIVE_DATA == 0;
            self.tree_for_host(host, &mut req.reply, path, id, anonymous);
            (req.done)(req.reply, 200);
            return 200;
        }
        self.catch_all(host, req, action, word(2), word(3))
    }

    /// The catch-all: an orphan's `remove` deletes it; a live node's (or a new job's template's) `get`, `test` and
    /// `userconfig` go to the intercept under its own name; anything else is an unknown id.
    fn catch_all(&self, host: &Host, mut req: Request, id: &[u8], action: Option<&[u8]>, name: Option<&[u8]>) -> u16 {
        let action = action.unwrap_or_default();
        let cmd = Cmds::parse(action);
        let mut name = name.map(<[u8]>::to_vec);
        let item = {
            let mut map = self.nodes.lock();
            let found = match map.get_index_of(id) {
                Some(index) => Some(index),
                None => {
                    // a new job: its template
                    let template = id.iter().rposition(|&c| c == b':').and_then(|colon| {
                        map.get_index_of(&id[..colon]).filter(|&i| map[i].kind == Type::Template)
                    });
                    if let Some(index) = template
                        && name.as_ref().is_none_or(Vec::is_empty)
                    {
                        let template_id = map.get_index(index).map(|(k, _)| k.clone()).unwrap_or_default();
                        if id.starts_with(&template_id) && id.get(template_id.len()) == Some(&b':') {
                            name = Some(id[template_id.len() + 1..].to_vec());
                        }
                    }
                    template
                }
            };
            found.and_then(|index| map.get_index_mut(index)).map(|(item_id, node)| {
                if !host.functions().available(&node.function) {
                    node.current.status = Status::Orphan;
                }
                (item_id.clone(), node.current.status == Status::Orphan, node.edit_access)
            })
        };
        if let Some((item_id, orphan, edit_access)) = item {
            if cmd == Cmds::REMOVE && orphan {
                if req.user_access & edit_access != edit_access {
                    let mut reply = req.reply;
                    let code = default_response(
                        &mut reply,
                        403,
                        "dyncfg: you don't have enough edit permissions to execute this command",
                    );
                    (req.done)(reply, code);
                    return code;
                }
                self.nodes.lock().shift_remove(id);
                self.nodes.delete_file(id);
                let mut reply = req.reply;
                let code = default_response(&mut reply, 200, "");
                (req.done)(reply, code);
                return code;
            }
            if (cmd == Cmds::USERCONFIG || cmd == Cmds::TEST || cmd == Cmds::GET) && !orphan {
                let mut function = b"config ".to_vec();
                function.extend_from_slice(&item_id);
                function.push(b' ');
                function.extend_from_slice(action);
                if cmd != Cmds::GET {
                    function.push(b' ');
                    function.extend_from_slice(name.as_deref().unwrap_or_default());
                }
                // C's 2048-byte buffer
                function.truncate(2047);
                req.function = function;
                return self.intercept(req);
            }
        }
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "DYNCFG: unknown config id '{}' in call: '{}'. This can happen if the plugin that registered the dynamic \
             configuration is not running now.",
            String::from_utf8_lossy(id),
            String::from_utf8_lossy(&req.function)
        );
        let mut reply = req.reply;
        let code = reply.error("Unknown config id given.", 404);
        (req.done)(reply, code);
        code
    }

    /// `dyncfg_tree_for_host()`: the host's nodes under the path (a raw prefix), an orphan marked when its method is
    /// gone, those of the id (itself, or the jobs of a template of that id) sorted by path and id, with the counts that
    /// need attention and the agent.
    fn tree_for_host(&self, host: &Host, reply: &mut Reply, path: &[u8], id: Option<&[u8]>, anonymous: bool) {
        let uuid = host_uuid(host);
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        w.member_add_uint64("version", 1);
        let (mut restart_required, mut plugin_rejected, mut failed, mut incomplete) = (0u64, 0u64, 0u64, 0u64);
        {
            let mut map = self.nodes.lock();
            let mut listed = Vec::new();
            for (index, (node_id, node)) in map.iter_mut().enumerate() {
                if node.host_uuid != uuid || !node.path.starts_with(path) {
                    continue;
                }
                if !host.functions().available(&node.function) {
                    node.current.status = Status::Orphan;
                }
                if let Some(id) = id
                    && id != node_id.as_slice()
                    && node.template.as_deref() != Some(id)
                {
                    continue;
                }
                listed.push(index);
            }
            listed.sort_by(|&a, &b| {
                let (a_id, a) = map.get_index(a).unwrap_or_else(|| unreachable!());
                let (b_id, b) = map.get_index(b).unwrap_or_else(|| unreachable!());
                a.path.cmp(&b.path).then_with(|| a_id.cmp(b_id))
            });
            w.member_add_object("tree");
            let mut last_path: Option<&[u8]> = None;
            for &index in &listed {
                let Some((node_id, node)) = map.get_index(index) else {
                    continue;
                };
                if last_path != Some(node.path.as_slice()) {
                    if last_path.is_some() {
                        w.object_close();
                    }
                    last_path = Some(&node.path);
                    w.member_add_object(&node.path);
                }
                to_json(&mut w, node_id, node, anonymous);
                plugin_rejected += u64::from(node.stored.plugin_rejected);
                if node.current.status != Status::Orphan {
                    restart_required += u64::from(node.stored.restart_required);
                    failed += u64::from(node.current.status == Status::Failed);
                    incomplete += u64::from(node.current.status == Status::Incomplete);
                }
            }
            if last_path.is_some() {
                w.object_close();
            }
            w.object_close();
        }
        w.member_add_object("attention");
        w.member_add_boolean("degraded", restart_required + plugin_rejected + failed + incomplete > 0);
        w.member_add_uint64("restart_required", restart_required);
        w.member_add_uint64("plugin_rejected", plugin_rejected);
        w.member_add_uint64("status_failed", failed);
        w.member_add_uint64("status_incomplete", incomplete);
        w.object_close();
        if let Some(hosts) = self.hosts.get() {
            let localhost = hosts.localhost();
            let hostname = localhost.hostname();
            let version = || u64::from(hosts.version());
            let agent = Agent {
                machine_guid: localhost.machine_guid(),
                node_id: localhost.node_id(),
                hostname: &hostname,
                nodes_hard_hash: &version,
            };
            agents_v2(&mut w, agent, now_realtime_s(), false, false, |_| {});
        }
        w.finalize();
        reply.body = w.into_bytes();
        reply.content_type = ContentType::ApplicationJson;
    }
}

/// `dyncfg_to_json()`: one node under its id.
fn to_json(w: &mut JsonWriter, id: &[u8], node: &Node, anonymous: bool) {
    w.member_add_object(id);
    w.member_add_string("type", node.kind.name());
    if node.kind == Type::Job {
        w.member_add_string("template", node.template.as_deref().unwrap_or_default());
    }
    w.member_add_string("status", node.current.status.name());
    w.member_add_array(Some(b"cmds"));
    let cmds = if node.current.status == Status::Orphan { Cmds::REMOVE } else { node.cmds };
    for name in cmds.names() {
        w.add_array_item_string(name);
    }
    w.array_close();
    w.member_add_object("access");
    for (key, bits) in [(b"view".as_slice(), node.view_access), (b"edit".as_slice(), node.edit_access)] {
        w.member_add_array(Some(key));
        for name in access::names(bits) {
            w.add_array_item_string(name);
        }
        w.array_close();
    }
    w.object_close();
    let source = |source_type: SourceType, source: &[u8]| -> Vec<u8> {
        if source_type == SourceType::Dyncfg && anonymous { HIDDEN.as_bytes().to_vec() } else { source.to_vec() }
    };
    w.member_add_string("source_type", node.current.source_type.name());
    w.member_add_string("source", source(node.current.source_type, &node.current.source));
    w.member_add_boolean("sync", node.sync);
    w.member_add_boolean("user_disabled", node.stored.user_disabled);
    w.member_add_boolean("restart_required", node.stored.restart_required);
    w.member_add_boolean("plugin_rejected", node.stored.plugin_rejected);
    w.member_add_object("payload");
    match node.stored.payload.as_ref().filter(|p| !p.bytes.is_empty()) {
        Some(payload) => {
            w.member_add_boolean("available", true);
            w.member_add_string("status", node.stored.status.name());
            w.member_add_string("source_type", node.stored.source_type.name());
            w.member_add_string("source", source(node.stored.source_type, &node.stored.source));
            w.member_add_uint64("created_ut", node.stored.created_ut);
            w.member_add_uint64("modified_ut", node.stored.modified_ut);
            w.member_add_string("content_type", payload.content_type.unwrap_or(ContentType::TextPlain).name());
            w.member_add_uint64("content_length", payload.bytes.len() as u64);
        }
        None => w.member_add_boolean("available", false),
    }
    w.object_close();
    w.member_add_uint64("saves", u64::from(node.stored.saves));
    w.member_add_uint64("created_ut", node.current.created_ut);
    w.member_add_uint64("modified_ut", node.current.modified_ut);
    w.object_close();
}
