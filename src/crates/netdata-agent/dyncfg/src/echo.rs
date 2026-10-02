//! The echo (`src/daemon/dyncfg/dyncfg-echo.c`): commands DynCfg sends a node's plugin with no caller - the first
//! enable or disable, the saved update of a stock or user configuration, the saved jobs of a template - and what their
//! answers change.

use std::sync::{Arc, Weak};

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::access;
use netdata_agent_nrpc::call::CallSpec;
use netdata_agent_nrpc::reply::{ContentType, Payload as CallPayload, Reply};
use netdata_agent_rrd::host::Host;

use crate::Dyncfg;
use crate::files::Payload;
use crate::model::{Cmds, SourceType, Status, Type, resp_success};

/// The calls table's mark on an echo (C tells them apart by their result callback, `dyncfg_echo_cb`).
pub(crate) const TAG: &str = "dyncfg-echo";

/// The node an echo was sent for, as C's callback data holds it: its answer changes that node, not one set under the
/// same id since.
struct Target {
    id: Vec<u8>,
    serial: u64,
    cmd: Cmds,
    cmd_str: String,
}

/// A saved payload as a call's: C sends `CT_NONE` as `text/plain`.
pub(crate) fn call_payload(payload: &Payload) -> CallPayload {
    CallPayload { body: payload.bytes.clone(), content_type: payload.content_type.unwrap_or(ContentType::TextPlain) }
}

impl Dyncfg {
    /// `dyncfg_echo()`: `config <id> <cmd>`, when the node has the command and it is one.
    pub(crate) fn echo(&self, id: &[u8], cmd: Cmds) {
        let Some((serial, host_uuid, function, cmds, source)) = self.nodes.lock().get(id).map(|n| {
            (n.serial, n.host_uuid, n.function.clone(), n.cmds, n.stored.source.clone())
        }) else {
            return;
        };
        let shown = String::from_utf8_lossy(id);
        let Some(host) = self.host_by_uuid(&host_uuid) else {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot find host of configuration id '{shown}'");
            return;
        };
        if !cmds.intersects(cmd) {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: attempted to echo a cmd that is not supported");
            return;
        }
        let Some(cmd_str) = cmd.name_one() else {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: command given does not resolve to a known command");
            return;
        };
        let target = Target { id: id.to_vec(), serial, cmd, cmd_str: cmd_str.to_string() };
        self.send(&host, &function, target, &source, None);
    }

    /// `dyncfg_echo_update()`: `config <id> update` with the saved payload.
    fn echo_update(&self, id: &[u8]) {
        let found = self.nodes.lock().get(id).map(|n| {
            let payload = n.stored.payload.as_ref().map(call_payload);
            (n.serial, n.host_uuid, n.function.clone(), n.stored.source.clone(), payload)
        });
        let Some((serial, host_uuid, function, source, payload)) = found else {
            return;
        };
        let shown = String::from_utf8_lossy(id);
        let Some(host) = self.host_by_uuid(&host_uuid) else {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot find host of configuration id '{shown}'");
            return;
        };
        let Some(payload) = payload else {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DYNCFG: requested to send an update to '{shown}', but there is no payload"
            );
            return;
        };
        let target = Target { id: id.to_vec(), serial, cmd: Cmds::UPDATE, cmd_str: "update".to_string() };
        self.send(&host, &function, target, &source, Some(payload));
    }

    /// `dyncfg_echo_add()`: `config <template> add <name>` with the job's saved payload (an empty one is sent).
    fn echo_add(&self, template_id: &[u8], job_id: &[u8], name: &[u8]) {
        let (template, job) = {
            let map = self.nodes.lock();
            let template = map.get(template_id).map(|t| (t.host_uuid, t.function.clone()));
            let job = map.get(job_id).map(|j| {
                (j.serial, j.stored.source.clone(), j.stored.payload.as_ref().map(call_payload))
            });
            (template, job)
        };
        let (Some((host_uuid, function)), Some((serial, source, payload))) = (template, job) else {
            return;
        };
        let shown = String::from_utf8_lossy(template_id);
        let Some(host) = self.host_by_uuid(&host_uuid) else {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot find host of configuration id '{shown}'");
            return;
        };
        let cmd_str = format!("add {}", String::from_utf8_lossy(name));
        let Some(payload) = payload else {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DYNCFG: requested to send a '{cmd_str}' to '{shown}', but there is no payload"
            );
            return;
        };
        let target = Target { id: job_id.to_vec(), serial, cmd: Cmds::ADD, cmd_str };
        self.send(&host, &function, target, &source, Some(payload));
    }

    /// `dyncfg_send_updates()`: a single or a job DynCfg saved with a payload gets it as an update; a template gets
    /// every job DynCfg made of it, in the nodes' order.
    pub(crate) fn send_updates(&self, id: &[u8]) {
        enum Updates {
            One,
            Jobs(Vec<Vec<u8>>),
            None,
        }
        let updates = {
            let map = self.nodes.lock();
            let Some(node) = map.get(id) else {
                drop(map);
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "DYNCFG: asked to update plugin for configuration '{}', but it is not found.",
                    String::from_utf8_lossy(id)
                );
                return;
            };
            match node.kind {
                Type::Single | Type::Job => {
                    let saved = node.stored.payload.as_ref().is_some_and(|p| !p.bytes.is_empty());
                    if node.cmds.intersects(Cmds::UPDATE) && node.stored.source_type == SourceType::Dyncfg && saved {
                        Updates::One
                    } else {
                        Updates::None
                    }
                }
                Type::Template if node.cmds.intersects(Cmds::ADD) => Updates::Jobs(
                    map.iter()
                        .filter(|(job_id, job)| {
                            job.kind == Type::Job
                                && job.current.source_type == SourceType::Dyncfg
                                && job.template.as_deref() == Some(id)
                                && job_id.starts_with(id)
                                && job_id.get(id.len()) == Some(&b':')
                                && job_id.len() > id.len() + 1
                        })
                        .map(|(job_id, _)| job_id.clone())
                        .collect(),
                ),
                Type::Template => Updates::None,
            }
        };
        match updates {
            Updates::One => self.echo_update(id),
            Updates::Jobs(jobs) => {
                for job_id in jobs {
                    self.echo_add(id, &job_id, &job_id[id.len() + 1..]);
                }
            }
            Updates::None => {}
        }
    }

    /// The echo's call: no caller waits, every permission, 10 seconds, the node's saved source, marked as an echo.
    fn send(&self, host: &Host, function: &[u8], target: Target, source: &[u8], payload: Option<CallPayload>) {
        let mut cmd = function.to_vec();
        cmd.push(b' ');
        cmd.extend_from_slice(target.cmd_str.as_bytes());
        let me: Weak<Dyncfg> = self.me.clone();
        let hostname = host.hostname();
        let _ = Arc::clone(&self.calls).call(CallSpec {
            owner: Some((host.functions(), &hostname)),
            cmd: &cmd,
            source,
            user_access: access::ALL,
            timeout_s: 10,
            wait: false,
            allow_restricted: false,
            call_id: None,
            payload,
            reply: Reply::new(ContentType::TextPlain),
            done: Some(Box::new(move |_reply, code| answered(&me, &target, code))),
            progress: None,
            is_cancelled: None,
            tag: Some(TAG),
        });
    }
}

/// `dyncfg_echo_cb()`: a success of `add` or `update` makes the saved state current, of `disable` disables, of `enable`
/// takes the answer's status; a failure is C's ERR, and of `add` or `update` marks the node rejected.
fn answered(me: &Weak<Dyncfg>, target: &Target, code: u16) {
    let success = resp_success(code);
    if !success {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "DYNCFG: received response code {code} on request to id '{}', cmd: {}",
            String::from_utf8_lossy(&target.id),
            target.cmd_str
        );
    }
    let Some(dyncfg) = me.upgrade() else {
        return;
    };
    let mut map = dyncfg.nodes.lock();
    let Some(node) = map.get_mut(&target.id).filter(|n| n.serial == target.serial) else {
        return;
    };
    match (success, target.cmd) {
        (true, Cmds::ADD | Cmds::UPDATE) => {
            node.stored.status = Status::from_successful_response(code);
            node.on_successful_add_or_update(code);
        }
        (true, Cmds::DISABLE) => {
            node.stored.status = Status::Disabled;
            node.current.status = Status::Disabled;
        }
        (true, Cmds::ENABLE) => {
            node.stored.status = Status::from_successful_response(code);
            node.current.status = node.stored.status;
        }
        (false, Cmds::ADD | Cmds::UPDATE) => node.stored.plugin_rejected = true,
        _ => {}
    }
}
