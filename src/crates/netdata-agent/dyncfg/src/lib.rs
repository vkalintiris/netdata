//! DynCfg, ported from `src/daemon/dyncfg/` and `src/libnetdata/inicfg/dyncfg.c`: the dynamic configuration plugins
//! and the agent offer through the `config` Function, and the user's changes saved across restarts.
//!
//! Every callback here may run inside a call DynCfg itself made, on the same thread (a refusal, a dead plugin, the
//! catch-all, a plugin's answer to an echo): the node lock is a leaf, never held across a call, a dispatch, a `done`
//! or a registration.

#![forbid(unsafe_code)]

pub mod files;
pub mod model;
pub mod nodes;

mod audit;
mod echo;
mod intercept;
mod tree;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::call::Calls;
use netdata_agent_nrpc::{Handler, MethodDesc, Source as NrpcSource, access};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::print::print_uuid_lower;

use crate::intercept::Intercept;
use crate::model::{Cmds, SourceType, Status, Type, is_valid_id};
use crate::nodes::{Current, Node, Nodes};
use crate::tree::Tree;

/// `DYNCFG_FUNCTIONS_VERSION`.
const FUNCTIONS_VERSION: u32 = 0;

/// What the daemon gives DynCfg at its start (`dyncfg_init()`).
pub struct Init<'a> {
    /// `netdata_configured_varlib_dir`: the saved files go in its `config`.
    pub varlib: &'a Path,
    /// `netdata_configured_user_config_dir` and `netdata_configured_stock_config_dir`: their `schema.d`.
    pub user_config_dir: &'a Path,
    pub stock_config_dir: &'a Path,
    /// Load the saved files.
    pub load_saved: bool,
    /// The calls table every caller uses: an echo is told apart by its tag on it.
    pub calls: Arc<Calls>,
}

/// `struct dyncfg_add_spec`: a node a plugin (or the agent) declares.
pub struct AddSpec<'a> {
    pub host: &'a Host,
    pub id: &'a [u8],
    pub path: &'a [u8],
    pub status: Status,
    pub kind: Type,
    pub source_type: SourceType,
    pub source: &'a [u8],
    pub cmds: Cmds,
    pub sync: bool,
    pub view_access: u32,
    pub edit_access: u32,
    /// What runs the node's commands: the plugin's transport.
    pub handler: Handler,
}

/// `dyncfg_globals` and the functions around them.
pub struct Dyncfg {
    /// This, for the callbacks of the calls it makes.
    me: Weak<Dyncfg>,
    nodes: Nodes,
    calls: Arc<Calls>,
    /// The hosts, known once they exist (after DynCfg's start): a node's host is found by its UUID.
    hosts: OnceLock<Arc<Hosts>>,
    user_config_dir: PathBuf,
    stock_config_dir: PathBuf,
    /// The handler of every `config <id>` method: one, so a re-registration stays the same handler (C's function
    /// pointer).
    intercept: Arc<Intercept>,
}

static PROCESS: OnceLock<Arc<Dyncfg>> = OnceLock::new();

impl Dyncfg {
    /// `dyncfg_init_low_level()`: the directory made and, with `load_saved`, the saved files loaded.
    pub fn new(init: Init<'_>) -> Arc<Dyncfg> {
        Arc::new_cyclic(|me: &Weak<Dyncfg>| Dyncfg {
            me: me.clone(),
            nodes: Nodes::init(init.varlib, init.load_saved),
            calls: init.calls,
            hosts: OnceLock::new(),
            user_config_dir: init.user_config_dir.to_path_buf(),
            stock_config_dir: init.stock_config_dir.to_path_buf(),
            intercept: Arc::new(Intercept(me.clone())),
        })
    }

    /// The process's DynCfg, made at the daemon's `dyncfg` step; none before it, as C's dictionary.
    pub fn process() -> Option<&'static Arc<Dyncfg>> {
        PROCESS.get()
    }

    /// Makes this the process's DynCfg; the first one stays.
    pub fn install(self: Arc<Self>) -> &'static Arc<Dyncfg> {
        PROCESS.get_or_init(|| self)
    }

    pub fn nodes(&self) -> &Nodes {
        &self.nodes
    }

    /// The hosts, once the daemon made them.
    pub fn set_hosts(&self, hosts: Arc<Hosts>) {
        let _ = self.hosts.set(hosts);
    }

    /// `dyncfg_rrdhost_by_uuid()`: the host whose machine GUID is this UUID, else C's ERR.
    fn host_by_uuid(&self, uuid: &[u8; 16]) -> Option<Arc<Host>> {
        let mut guid = Vec::with_capacity(36);
        print_uuid_lower(&mut guid, uuid);
        let guid = String::from_utf8_lossy(&guid);
        let host = self.hosts.get().and_then(|hosts| hosts.find_by_guid(&guid));
        if host.is_none() {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot find host with UUID '{guid}'");
        }
        host
    }

    /// `localhost->hostname`, for the records.
    fn localhost_name(&self) -> String {
        self.hosts.get().map(|hosts| hosts.localhost().hostname()).unwrap_or_default()
    }

    /// `dyncfg_host_init()`: the `config` function (the tree and the catch-all) on a host, async, as it may call
    /// another function itself.
    pub fn host_init(&self, host: &Arc<Host>) {
        let tree = Arc::new(Tree { dyncfg: self.me.clone(), host: Arc::downgrade(host) });
        let registered = host.register_function(&MethodDesc {
            name: b"config",
            help: b"Dynamic configuration",
            tags: b"config",
            timeout_s: 120,
            priority: 1000,
            version: FUNCTIONS_VERSION,
            access: access::ANONYMOUS_DATA,
            sync: false,
            source: NrpcSource::Daemon,
            handler: Handler::Transport(tree),
        });
        if let Err(warning) = registered {
            nd_log!(Source::Daemon, Priority::Warning, "{warning}");
        }
    }

    /// `dyncfg_add_low_level()`: unset accesses take C's defaults, the id is checked (a job's template must be
    /// registered), the commands sanitized (C's NOTICE when they changed); the node set, its `config <id>` method
    /// registered, then a non-template told to enable or disable itself and the saved changes sent to the plugin.
    pub fn add_low_level(&self, spec: AddSpec<'_>) -> bool {
        let view_access = if spec.view_access == access::NONE {
            access::SIGNED_ID | access::SAME_SPACE | access::VIEW_AGENT_CONFIG
        } else {
            spec.view_access
        };
        let edit_access = if spec.edit_access == access::NONE {
            access::SIGNED_ID | access::SAME_SPACE | access::EDIT_AGENT_CONFIG | access::COMMERCIAL_SPACE
        } else {
            spec.edit_access
        };
        let id = spec.id;
        let shown = String::from_utf8_lossy(id);
        if !is_valid_id(id) {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: id '{shown}' is invalid. Ignoring dynamic configuration for it.");
            return false;
        }
        if spec.kind == Type::Job && !self.nodes.job_has_registered_template(id) {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DYNCFG: job id '{shown}' does not have a registered template. Ignoring dynamic configuration for it."
            );
            return false;
        }
        let cmds = spec.cmds.sanitize(spec.kind, spec.source_type);
        if cmds != spec.cmds {
            let mut text = format!("DYNCFG: id '{shown}' was declared with cmds: ").into_bytes();
            spec.cmds.write_joined(&mut text);
            text.extend_from_slice(b", but they have sanitized to: ");
            cmds.write_joined(&mut text);
            nd_log!(Source::Daemon, Priority::Notice, "{}", String::from_utf8_lossy(&text));
        }
        let (host, sync) = (spec.host, spec.sync);
        if !self.add_internal(&AddSpec { cmds, view_access, edit_access, ..spec }, true) {
            nd_log!(
                Source::Daemon,
                Priority::Notice,
                "DYNCFG: cannot add configuration '{shown}' - the dyncfg registry is not available"
            );
            return false;
        }

        // the node as the set left it (merged into an existing one, maybe)
        let (function, kind, cmds, disabled, template, dyncfg_job) = {
            let map = self.nodes.lock();
            let Some(node) = map.get(id) else {
                return true;
            };
            (
                node.function.clone(),
                node.kind,
                node.cmds,
                node.stored.user_disabled || node.current.status == Status::Disabled,
                node.template.clone(),
                node.current.source_type == SourceType::Dyncfg && node.kind == Type::Job,
            )
        };
        let registered = host.register_function(&MethodDesc {
            name: &function,
            help: b"Dynamic configuration",
            tags: b"config",
            timeout_s: 120,
            priority: 1000,
            version: FUNCTIONS_VERSION,
            access: view_access & edit_access,
            sync,
            source: NrpcSource::Daemon,
            handler: Handler::Transport(Arc::clone(&self.intercept) as _),
        });
        if let Err(warning) = registered {
            nd_log!(Source::Daemon, Priority::Warning, "{warning}");
        }
        if kind != Type::Template && cmds.intersects(Cmds::ENABLE | Cmds::DISABLE) {
            let disable =
                disabled || template.as_deref().is_some_and(|template| self.nodes.is_user_disabled(template));
            self.echo(id, if disable { Cmds::DISABLE } else { Cmds::ENABLE });
        }
        if !dyncfg_job {
            self.send_updates(id);
        }
        true
    }

    /// `dyncfg_add_internal()`: the spec as a node, set with its current state (times stamped by the set), its
    /// handler replacing a different one only with `overwrite_handler`; false when the set refused it.
    fn add_internal(&self, spec: &AddSpec<'_>, overwrite_handler: bool) -> bool {
        let node = Node {
            host_uuid: host_uuid(spec.host),
            path: spec.path.to_vec(),
            cmds: spec.cmds,
            kind: spec.kind,
            view_access: spec.view_access,
            edit_access: spec.edit_access,
            current: Current {
                status: spec.status,
                source_type: spec.source_type,
                source: spec.source.to_vec(),
                ..Current::default()
            },
            sync: spec.sync,
            handler: Some(spec.handler.clone()),
            ..Node::default()
        };
        self.nodes.set(spec.id, node, overwrite_handler).is_some()
    }

    /// `dyncfg_del_low_level()`: the node's method unregistered, the node gone when it was never saved.
    pub fn del_low_level(&self, host: &Host, id: &[u8]) {
        if !is_valid_id(id) {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DYNCFG: id '{}' is invalid. Ignoring dynamic configuration for it.",
                String::from_utf8_lossy(id)
            );
            return;
        }
        if let Some(function) = self.nodes.delete(id) {
            host.unregister_function(&function, NrpcSource::Daemon);
        }
    }

    /// `dyncfg_status_low_level()`.
    pub fn status_low_level(&self, id: &[u8], status: Status) {
        self.nodes.status(id, status);
    }
}

/// `host->host_id`: the host's machine GUID as a UUID.
fn host_uuid(host: &Host) -> [u8; 16] {
    uuid_parse_flexi(host.machine_guid().as_bytes()).unwrap_or([0; 16])
}

#[cfg(test)]
mod tests;
