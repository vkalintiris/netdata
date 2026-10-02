//! The intercept (`src/daemon/dyncfg/dyncfg-intercept.c`): the handler of every `config <id>` method. It checks a
//! call in C's order, answers the template-wide commands and the schemas itself, and hands the rest to the node's
//! plugin, keeping the user's changes when the plugin accepts them.

use std::sync::Weak;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::call::{Request, dispatch};
use netdata_agent_nrpc::reply::{ContentType, Payload as CallPayload};
use netdata_agent_nrpc::{Handler, Transport};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};

use crate::files::{self, Payload};
use crate::model::{Cmds, SourceType, Status, Type, default_response, resp_success};
use crate::{AddSpec, Dyncfg, audit, echo};

/// The intercept as the methods' handler.
pub(crate) struct Intercept(pub(crate) Weak<Dyncfg>);

impl Transport for Intercept {
    fn dispatch(&self, req: Request) -> u16 {
        match self.0.upgrade() {
            Some(dyncfg) => dyncfg.intercept(req),
            // DynCfg outlives every method in the daemon; a test that dropped it gets a dead plugin's answer
            None => {
                let mut reply = req.reply;
                let code = reply.error("The plugin that offered this function is not available.", 503);
                (req.done)(reply, code);
                code
            }
        }
    }
}

/// `struct dyncfg_call`: a forwarded call, for its answer.
pub(crate) struct Call {
    pub(crate) transaction: [u8; 16],
    pub(crate) function: Vec<u8>,
    /// The id the call was checked against (a nameless `test`'s without its last part).
    pub(crate) id: Vec<u8>,
    pub(crate) source: Vec<u8>,
    pub(crate) add_name: Option<Vec<u8>>,
    pub(crate) cmd: Cmds,
    payload: Option<CallPayload>,
    echo: bool,
}

/// What the intercept reads of the node under the lock.
struct Snapshot {
    kind: Type,
    cmds: Cmds,
    view_access: u32,
    edit_access: u32,
    template: Option<Vec<u8>>,
    template_user_disabled: bool,
    handler: Option<Handler>,
}

/// What a job a user added takes of its template.
struct Template {
    host_uuid: [u8; 16],
    path: Vec<u8>,
    cmds: Cmds,
    sync: bool,
    view_access: u32,
    edit_access: u32,
    handler: Option<Handler>,
}

/// `dyncfg_intercept_early_error()`: C's DynCfg answer, handed to the caller.
fn refuse(mut req: Request, code: u16, msg: &str) -> u16 {
    let code = default_response(&mut req.reply, code, msg);
    (req.done)(req.reply, code);
    code
}

/// A call's payload as a node saves it.
fn saved_payload(payload: CallPayload) -> Payload {
    Payload { bytes: payload.body, content_type: Some(payload.content_type) }
}

impl Dyncfg {
    /// `dyncfg_function_intercept_cb()`: it hands the caller its answer on every path.
    pub(crate) fn intercept(&self, mut req: Request) -> u16 {
        let echo = self.calls.has_tag(&req.call_id, echo::TAG);
        let has_payload = req.payload.as_ref().is_some_and(|p| !p.body.is_empty());
        let words = quoted_strings_splitter(&req.function, 20, Separators::Whitespace);
        let word = |i: usize| words.get(i).map(Vec::as_slice);
        if word(0) != Some(b"config") {
            return refuse(req, 400, "dyncfg functions intercept: this is not a dyncfg request");
        }
        let cmd = Cmds::parse(word(2).unwrap_or_default());
        if cmd == Cmds::NONE {
            return refuse(req, 400, "dyncfg functions intercept: invalid command received");
        }
        let mut id = word(1).unwrap_or_default().to_vec();
        let mut add_name = word(3).map(<[u8]>::to_vec);
        if cmd == Cmds::ADD || cmd == Cmds::TEST || cmd == Cmds::USERCONFIG {
            if cmd == Cmds::TEST && add_name.as_ref().is_none_or(Vec::is_empty) {
                // backwards compatibility for a test without a name
                match id.iter().rposition(|&c| c == b':') {
                    Some(colon) => {
                        add_name = Some(id[colon + 1..].to_vec());
                        id.truncate(colon);
                    }
                    None => add_name = Some(b"test".to_vec()),
                }
            }
            let Some(name) = add_name.as_ref().filter(|n| !n.is_empty()) else {
                return refuse(req, 400, "dyncfg functions intercept: this action requires a name");
            };
            if !echo && cmd == Cmds::ADD && self.nodes.lock().contains_key(&[id.as_slice(), b":", name].concat()) {
                return refuse(req, 400, "dyncfg functions intercept: a configuration with this name already exists");
            }
        }
        let data = cmd == Cmds::ADD || cmd == Cmds::UPDATE || cmd == Cmds::TEST || cmd == Cmds::USERCONFIG;
        if data && !has_payload {
            return refuse(req, 400, "dyncfg functions intercept: this action requires a payload");
        }
        if !data && has_payload {
            return refuse(req, 400, "dyncfg functions intercept: this action does not require a payload");
        }
        let snapshot = {
            let map = self.nodes.lock();
            let node = map.get(&id).or_else(|| {
                // a test of a new job: its template
                if cmd != Cmds::TEST && cmd != Cmds::USERCONFIG {
                    return None;
                }
                let colon = id.iter().rposition(|&c| c == b':')?;
                map.get(&id[..colon]).filter(|t| t.kind == Type::Template)
            });
            node.map(|n| Snapshot {
                kind: n.kind,
                cmds: n.cmds,
                view_access: n.view_access,
                edit_access: n.edit_access,
                template: n.template.clone(),
                template_user_disabled: n
                    .template
                    .as_deref()
                    .and_then(|t| map.get(t))
                    .is_some_and(|t| t.stored.user_disabled),
                handler: n.handler.clone(),
            })
        };
        let Some(node) = snapshot else {
            return refuse(req, 404, "dyncfg functions intercept: id is not found");
        };
        let function = String::from_utf8_lossy(&req.function).into_owned();

        // the caller's permissions, then the command against the node
        let user = req.user_access;
        let refused = match cmd {
            Cmds::GET | Cmds::SCHEMA | Cmds::USERCONFIG => (user & node.view_access != node.view_access)
                .then_some((403, "dyncfg: you don't have enough view permissions to execute this command")),
            Cmds::ENABLE | Cmds::DISABLE | Cmds::ADD | Cmds::TEST | Cmds::UPDATE | Cmds::REMOVE | Cmds::RESTART => {
                (user & node.edit_access != node.edit_access)
                    .then_some((403, "dyncfg: you don't have enough edit permissions to execute this command"))
            }
            _ => Some((500, "dyncfg: permissions for this command are not set")),
        };
        let refused = refused.or_else(|| {
            if !node.cmds.intersects(cmd) {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "DYNCFG: this command is not supported by the configuration node: {function}"
                );
                Some((400, "dyncfg functions intercept: this command is not supported by this configuration node"))
            } else if cmd == Cmds::ADD && node.kind != Type::Template {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "DYNCFG: add command can only be applied on templates, not {}: {function}",
                    node.kind.name()
                );
                Some((400, "dyncfg functions intercept: add command is only allowed in templates"))
            } else if cmd == Cmds::ENABLE && node.kind == Type::Job && node.template_user_disabled {
                nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot enable a job of a disabled template: {function}");
                Some((400, "dyncfg functions intercept: this job belongs to disabled template"))
            } else {
                None
            }
        });
        if let Some((code, msg)) = refused {
            return refuse(req, code, msg);
        }

        // the commands DynCfg answers itself
        if cmd.intersects(Cmds::ENABLE | Cmds::DISABLE | Cmds::RESTART) && node.kind == Type::Template {
            if !echo {
                {
                    let mut map = self.nodes.lock();
                    if let Some(template) = map.get_mut(&id) {
                        let old = template.stored.user_disabled;
                        if cmd == Cmds::ENABLE {
                            template.stored.user_disabled = false;
                        } else if cmd == Cmds::DISABLE {
                            template.stored.user_disabled = true;
                        }
                        if template.stored.user_disabled != old {
                            self.nodes.save(&id, template);
                        }
                    }
                }
                let call = Call {
                    transaction: req.call_id,
                    function: req.function.clone(),
                    id: id.clone(),
                    source: req.source.clone(),
                    add_name: add_name.clone(),
                    cmd,
                    payload: None,
                    echo,
                };
                audit::user_action(&self.localhost_name(), node.kind, &call);
            }
            self.apply_on_all_template_jobs(&req, &id, cmd);
            return refuse(req, 200, "applied to all template job");
        }
        if cmd == Cmds::SCHEMA {
            let of = if node.kind == Type::Job { node.template.as_deref() } else { Some(id.as_slice()) };
            if let Some(schema) = of.and_then(|of| files::schema(&self.user_config_dir, &self.stock_config_dir, of)) {
                req.reply.body = schema;
                req.reply.content_type = ContentType::ApplicationJson;
                req.reply.expires = now_realtime_s();
                (req.done)(req.reply, 200);
                return 200;
            }
        }

        // the node's plugin
        let Some(handler) = node.handler else {
            // a node with a method always has its plugin's handler; a dead plugin's answer otherwise
            let mut reply = req.reply;
            let code = reply.error("The plugin that offered this function is not available.", 503);
            (req.done)(reply, code);
            return code;
        };
        let call = Call {
            transaction: req.call_id,
            function: req.function.clone(),
            id,
            source: req.source.clone(),
            add_name,
            cmd,
            payload: req.payload.clone(),
            echo,
        };
        let me = self.me.clone();
        let caller = std::mem::replace(&mut req.done, Box::new(|_, _| {}));
        req.done = Box::new(move |reply, code| {
            if let Some(dyncfg) = me.upgrade() {
                dyncfg.answered(call, code);
            }
            caller(reply, code);
        });
        dispatch(&handler, req, None)
    }

    /// `dyncfg_apply_action_on_all_template_jobs()`: every job of the template, in the nodes' order, told to enable
    /// (a user-disabled one to disable), disable or restart, the caller's progress counting them.
    fn apply_on_all_template_jobs(&self, req: &Request, template_id: &[u8], cmd: Cmds) {
        let jobs: Vec<(Vec<u8>, bool)> = self
            .nodes
            .lock()
            .iter()
            .filter(|(_, n)| n.kind == Type::Job && n.template.as_deref() == Some(template_id))
            .map(|(id, n)| (id.clone(), n.stored.user_disabled))
            .collect();
        let all = jobs.len();
        if let Some(progress) = &req.progress {
            progress(&req.call_id, 0, all);
        }
        for (done, (job_id, user_disabled)) in jobs.into_iter().enumerate() {
            let send = match cmd {
                Cmds::ENABLE if user_disabled => Cmds::DISABLE,
                other => other,
            };
            self.echo(&job_id, send);
            if let Some(progress) = &req.progress {
                progress(&req.call_id, done + 1, all);
            }
        }
    }

    /// `dyncfg_function_intercept_result_cb()`'s bookkeeping, before the caller gets the answer: a user's accepted
    /// `add` makes the job, `update` saves the payload, `enable` and `disable` the choice, `remove` deletes the node
    /// and its file; each recorded. A refusal is C's ERR. An echo's answer is the echo's.
    fn answered(&self, mut call: Call, code: u16) {
        let mut map = self.nodes.lock();
        let Some(node) = map.get_mut(&call.id) else {
            return;
        };
        if call.echo {
            return;
        }
        if !resp_success(code) {
            drop(map);
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DYNCFG: plugin returned code {code} to user initiated call: {}",
                String::from_utf8_lossy(&call.function)
            );
            return;
        }
        let kind = node.kind;
        let old_user_disabled = node.stored.user_disabled;
        let mut save = false;
        match call.cmd {
            Cmds::ADD => {
                let template = Template {
                    host_uuid: node.host_uuid,
                    path: node.path.clone(),
                    cmds: node.cmds,
                    sync: node.sync,
                    view_access: node.view_access,
                    edit_access: node.edit_access,
                    handler: node.handler.clone(),
                };
                drop(map);
                self.job_added(template, code, &mut call);
                audit::user_action(&self.localhost_name(), kind, &call);
                return;
            }
            Cmds::UPDATE => {
                node.stored.status = Status::from_successful_response(code);
                node.stored.source_type = SourceType::Dyncfg;
                node.stored.payload = call.payload.take().map(saved_payload);
                node.stored.source = call.source.clone();
                node.on_successful_add_or_update(code);
                node.cmds = node.cmds.sanitize(node.kind, node.current.source_type);
                save = true;
            }
            Cmds::ENABLE => node.stored.user_disabled = false,
            Cmds::DISABLE => node.stored.user_disabled = true,
            Cmds::REMOVE => {
                self.nodes.delete_file(&call.id);
                map.shift_remove(&call.id);
                drop(map);
                audit::user_action(&self.localhost_name(), kind, &call);
                return;
            }
            _ => {}
        }
        if save || old_user_disabled != node.stored.user_disabled {
            self.nodes.save(&call.id, node);
        }
        drop(map);
        audit::user_action(&self.localhost_name(), kind, &call);
    }

    /// `dyncfg_function_intercept_job_successfully_added()`: the job `<template>:<name>` set on the template's host
    /// with the template's commands as a job's, its handler and accesses; then its saved state is the user's payload
    /// and source, saved, and made current.
    fn job_added(&self, template: Template, code: u16, call: &mut Call) {
        let name = call.add_name.as_deref().unwrap_or_default();
        let id = [call.id.as_slice(), b":", name].concat();
        let shown = String::from_utf8_lossy(&id);
        let Some(host) = self.host_by_uuid(&template.host_uuid) else {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot add job '{shown}' because host is missing");
            return;
        };
        let Some(handler) = template.handler else {
            return;
        };
        let status = Status::from_successful_response(code);
        self.add_internal(
            &AddSpec {
                host: &host,
                id: &id,
                path: &template.path,
                status,
                kind: Type::Job,
                source_type: SourceType::Dyncfg,
                source: &call.source,
                cmds: (template.cmds - Cmds::ADD)
                    | Cmds::GET
                    | Cmds::UPDATE
                    | Cmds::TEST
                    | Cmds::ENABLE
                    | Cmds::DISABLE
                    | Cmds::REMOVE,
                sync: template.sync,
                view_access: template.view_access,
                edit_access: template.edit_access,
                handler,
            },
            false,
        );
        let mut map = self.nodes.lock();
        if let Some(job) = map.get_mut(&id) {
            job.stored.payload = call.payload.take().map(saved_payload);
            job.stored.source = call.source.clone();
            job.stored.user_disabled = false;
            job.stored.source_type = SourceType::Dyncfg;
            job.stored.status = status;
            self.nodes.save(&id, job);
            job.on_successful_add_or_update(code);
        }
    }
}
