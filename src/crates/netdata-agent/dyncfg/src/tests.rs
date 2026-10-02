//! DynCfg's core against a recording plugin on localhost, through the calls table as every caller reaches it: the
//! registrations and their echoes, the intercept's checks and bookkeeping, the template commands, the tree and the
//! catch-all (UT's daemon-coupled scenarios, `src/daemon/dyncfg/dyncfg-unittest.c`).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use netdata_agent_log::{Captured, Field, Priority, capture};
use netdata_agent_nrpc::call::{CallSpec, Calls, Request, SystemClock};
use netdata_agent_nrpc::reply::{ContentType, Payload as CallPayload, Reply};
use netdata_agent_nrpc::{FLAG_DYNCFG, Handler, Transport, access};
use netdata_agent_rrd::host::{Host, HostInfo, Hosts};
use netdata_agent_rrd::mode::DbMode;

use super::*;
use crate::files::file_name;

const GUID: &str = "6f2a9c1e-0b7d-4e3a-9f51-2c8d4b7a1e90";
/// A caller as the web server describes one (`user_auth_to_source_buffer()`).
const SOURCE: &str = "method=api-bearer,role=admin,permissions=0x7ff,user=tester,ip=127.0.0.1";

fn info(hostname: &str) -> HostInfo {
    HostInfo {
        hostname: hostname.into(),
        registry_hostname: hostname.into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "netdata".into(),
        program_version: "v0".into(),
        update_every: 1,
        db_mode: DbMode::Ram,
        history_entries: 5,
        health_enabled: false,
        system_info: Default::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    }
}

/// What the plugin was sent.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    function: String,
    payload: Option<String>,
    source: String,
    user_access: u32,
    echo: bool,
}

/// A plugin that records every request and answers it at once with its code, or holds it.
struct Plugin {
    calls: Arc<Calls>,
    code: Mutex<Option<u16>>,
    seen: Mutex<Vec<Seen>>,
    held: Mutex<Vec<Request>>,
}

impl Plugin {
    fn seen(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }

    fn functions(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.function).collect()
    }

    fn hold(&self) {
        *self.code.lock().unwrap() = None;
    }

    fn answer_held(&self, code: u16) {
        let held = std::mem::take(&mut *self.held.lock().unwrap());
        for req in held {
            (req.done)(req.reply, code);
        }
    }
}

impl Transport for Plugin {
    fn dispatch(&self, req: Request) -> u16 {
        self.seen.lock().unwrap().push(Seen {
            function: String::from_utf8_lossy(&req.function).into_owned(),
            payload: req.payload.as_ref().map(|p| format!("{} {}", p.content_type.name(), String::from_utf8_lossy(&p.body))),
            source: String::from_utf8_lossy(&req.source).into_owned(),
            user_access: req.user_access,
            echo: self.calls.has_tag(&req.call_id, echo::TAG),
        });
        let code = *self.code.lock().unwrap();
        match code {
            Some(code) => {
                let mut reply = req.reply;
                reply.body = b"plugin".to_vec();
                (req.done)(reply, code);
            }
            None => self.held.lock().unwrap().push(req),
        }
        200
    }
}

struct Fx {
    dir: tempfile::TempDir,
    calls: Arc<Calls>,
    hosts: Arc<Hosts>,
    dyncfg: Arc<Dyncfg>,
    plugin: Arc<Plugin>,
}

impl Fx {
    fn localhost(&self) -> &Arc<Host> {
        self.hosts.localhost()
    }

    fn node(&self, id: &str) -> Option<Node> {
        self.dyncfg.nodes.lock().get(id.as_bytes()).cloned()
    }

    fn file(&self, id: &str) -> std::path::PathBuf {
        self.dyncfg.nodes.dir().join(String::from_utf8(file_name(id.as_bytes())).unwrap())
    }

    /// A plugin's `CONFIG <id> create`, its accesses left to C's defaults.
    fn add(&self, id: &str, kind: Type, cmds: &str, source_type: SourceType) -> bool {
        self.dyncfg.add_low_level(AddSpec {
            host: self.localhost(),
            id: id.as_bytes(),
            path: b"/collectors/go.d",
            status: Status::Accepted,
            kind,
            source_type,
            source: b"type=stock",
            cmds: Cmds::parse(cmds.as_bytes()),
            sync: false,
            view_access: 0,
            edit_access: 0,
            handler: Handler::Transport(Arc::clone(&self.plugin) as _),
        })
    }

    /// A user's call, as `/api/v1/config` makes it: waiting for the answer.
    fn call(&self, cmd: &str, payload: Option<&str>, user_access: u32) -> (u16, String) {
        self.call_with_progress(cmd, payload, user_access, None)
    }

    fn call_with_progress(
        &self,
        cmd: &str,
        payload: Option<&str>,
        user_access: u32,
        progress: Option<netdata_agent_nrpc::call::ProgressCb>,
    ) -> (u16, String) {
        let hostname = self.localhost().hostname();
        let called = self.calls.call(CallSpec {
            owner: Some((self.localhost().functions(), &hostname)),
            cmd: cmd.as_bytes(),
            source: SOURCE.as_bytes(),
            user_access,
            timeout_s: 10,
            wait: true,
            allow_restricted: false,
            call_id: None,
            payload: payload.map(|p| CallPayload { body: p.as_bytes().to_vec(), content_type: ContentType::ApplicationJson }),
            reply: Reply::new(ContentType::ApplicationJson),
            done: None,
            progress,
            is_cancelled: None,
            tag: None,
        });
        let body = called.reply.map(|r| String::from_utf8_lossy(&r.body).into_owned()).unwrap_or_default();
        (called.code, body)
    }
}

fn fx_in(dir: tempfile::TempDir) -> Fx {
    let calls = Calls::new(Box::new(SystemClock));
    let dyncfg = Dyncfg::new(Init {
        varlib: dir.path(),
        user_config_dir: &dir.path().join("user"),
        stock_config_dir: &dir.path().join("stock"),
        load_saved: true,
        calls: Arc::clone(&calls),
    });
    let hosts = Arc::new(Hosts::new(Host::new(GUID, true, info("dc-host"))));
    dyncfg.set_hosts(Arc::clone(&hosts));
    dyncfg.host_init(hosts.localhost());
    let plugin = Arc::new(Plugin {
        calls: Arc::clone(&calls),
        code: Mutex::new(Some(200)),
        seen: Mutex::default(),
        held: Mutex::default(),
    });
    Fx { dir, calls, hosts, dyncfg, plugin }
}

fn fx() -> Fx {
    fx_in(tempfile::tempdir().unwrap())
}

/// Runs a scenario on its own thread with its records captured, failing on a deadlock instead of hanging.
fn within<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> (R, Vec<Captured>) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(capture(f));
    });
    rx.recv_timeout(Duration::from_secs(20)).expect("the scenario deadlocked")
}

/// The records at NOTICE and above (the hosts' creation logs below it; DynCfg never does).
fn texts(records: &[Captured]) -> Vec<(Priority, String)> {
    records
        .iter()
        .filter(|r| r.priority <= Priority::Notice)
        .map(|r| (r.priority, r.message.clone().unwrap_or_default()))
        .collect()
}

fn field(record: &Captured, field: Field) -> Option<&str> {
    record.fields.iter().rev().find(|(f, _)| *f == field).map(|(_, v)| v.as_str())
}

fn echo(function: &str) -> Seen {
    Seen { function: function.into(), payload: None, source: String::new(), user_access: access::ALL, echo: true }
}

/// `dyncfg_add_low_level()` of a single (UT U1): C's default accesses, the method registered as DynCfg's, the first
/// `enable` echoed with every permission and its answer taken; nothing saved.
#[test]
fn a_single_is_registered_and_told_to_enable() {
    let ((), records) = within(|| {
        let fx = fx();
        assert!(fx.add("go.d:x", Type::Single, "get schema update enable disable", SourceType::Stock));
        assert_eq!(fx.plugin.seen(), [echo("config go.d:x enable")]);
        let method = fx.localhost().functions().get(b"config go.d:x").unwrap();
        assert_eq!(
            (method.access, method.flags, method.timeout_s, method.priority, method.source, method.sync),
            (access::SIGNED_ID | access::SAME_SPACE, FLAG_DYNCFG, 120, 1000, NrpcSource::Daemon, false)
        );
        let node = fx.node("go.d:x").unwrap();
        assert_eq!((node.current.status, node.stored.status, node.stored.saves), (Status::Running, Status::Running, 0));
        assert_eq!(
            (node.view_access, node.edit_access),
            (0x23, access::SIGNED_ID | access::SAME_SPACE | access::EDIT_AGENT_CONFIG | access::COMMERCIAL_SPACE)
        );
        assert!(!fx.file("go.d:x").exists());
    });
    assert!(texts(&records).is_empty(), "{records:?}");
}

/// A template (UT U2): its commands sanitized with C's NOTICE, no echo; the id checks' ERRs.
#[test]
fn a_template_is_sanitized_and_not_echoed_and_bad_ids_are_refused() {
    let (fx, records) = within(|| {
        let fx = fx();
        assert!(fx.add("go.d:T", Type::Template, "get schema enable disable", SourceType::Stock));
        assert!(!fx.add("go.d:a b", Type::Single, "get", SourceType::Stock));
        assert!(!fx.add("go.d:U:j", Type::Job, "get", SourceType::Stock));
        fx
    });
    assert!(fx.plugin.seen().is_empty());
    assert_eq!(fx.node("go.d:T").unwrap().cmds, Cmds::parse(b"schema add enable disable"));
    assert_eq!(
        texts(&records),
        [
            (
                Priority::Notice,
                "DYNCFG: id 'go.d:T' was declared with cmds: get schema enable disable, but they have sanitized to: \
                 schema add enable disable"
                    .into()
            ),
            (Priority::Err, "DYNCFG: id 'go.d:a b' is invalid. Ignoring dynamic configuration for it.".into()),
            (
                Priority::Err,
                "DYNCFG: job id 'go.d:U:j' does not have a registered template. Ignoring dynamic configuration for it."
                    .into()
            ),
        ]
    );
}

/// A user's `add` (UT U4): the job is made, saved and current, with the template's commands as a job's, but no method
/// of its own until the plugin creates it (K6); the change is recorded with the caller's fields.
#[test]
fn a_users_add_makes_a_saved_job_and_records_it() {
    let (fx, records) = within(|| {
        let fx = fx();
        fx.add("go.d:T", Type::Template, "schema add enable disable restart", SourceType::Stock);
        assert_eq!(fx.call("config go.d:T add j1", Some("{\"a\":1}"), access::ALL), (200, "plugin".into()));
        let exists = fx.call("config go.d:T add j1", Some("{}"), access::ALL);
        assert_eq!(
            exists,
            (400, "{\"status\":400,\"message\":\"dyncfg functions intercept: a configuration with this name already exists\"}".into())
        );
        fx
    });
    assert_eq!(
        fx.plugin.seen(),
        [Seen {
            function: "config go.d:T add j1".into(),
            payload: Some("application/json {\"a\":1}".into()),
            source: SOURCE.into(),
            user_access: access::ALL,
            echo: false,
        }]
    );
    let job = fx.node("go.d:T:j1").unwrap();
    assert_eq!((job.kind, job.template.as_deref()), (Type::Job, Some(b"go.d:T".as_slice())));
    assert_eq!(job.cmds, Cmds::parse(b"get schema update test remove enable disable restart"));
    assert_eq!(
        (job.current.status, job.current.source_type, job.current.source.as_slice()),
        (Status::Running, SourceType::Dyncfg, SOURCE.as_bytes())
    );
    assert_eq!(job.stored.saves, 1);
    assert_eq!(job.stored.payload.unwrap().bytes, b"{\"a\":1}");
    assert!(fx.file("go.d:T:j1").exists());
    assert!(fx.localhost().functions().get(b"config go.d:T:j1").is_none(), "K6");
    let action = records.iter().find(|r| r.message.as_deref().is_some_and(|m| m.starts_with("DYNCFG USER"))).unwrap();
    assert_eq!(
        action.message.as_deref(),
        Some("DYNCFG USER ACTION 'add' j1 on template 'go.d:T' by user 'tester', IP '127.0.0.1'")
    );
    assert_eq!(
        [Field::Module, Field::NidlNode, Field::Request, Field::UserName, Field::UserRole, Field::UserAccess, Field::SrcIp]
            .map(|f| field(action, f)),
        [
            Some("DYNCFG"),
            Some("dc-host"),
            Some("config go.d:T add j1"),
            Some("tester"),
            Some("admin"),
            Some("0x7ff"),
            Some("127.0.0.1")
        ]
    );
}

/// The intercept's refusals in C's order, with their codes, texts and records (map §1.2).
#[test]
fn the_intercept_refuses_as_c() {
    let (fx, records) = within(|| {
        let fx = fx();
        fx.add("go.d:x", Type::Single, "get schema update enable disable", SourceType::Stock);
        fx.add("go.d:T", Type::Template, "schema add enable disable", SourceType::Stock);
        fx.add("go.d:T:j2", Type::Job, "get schema update enable disable", SourceType::Stock);
        fx.plugin.seen();
        let view_only = access::SIGNED_ID | access::SAME_SPACE | access::VIEW_AGENT_CONFIG;
        let edit_only = access::SIGNED_ID | access::SAME_SPACE | access::EDIT_AGENT_CONFIG;
        let cases = [
            ("config go.d:x nope", None, access::ALL),
            ("config go.d:T add", None, access::ALL),
            ("config go.d:x update", None, access::ALL),
            ("config go.d:x get", Some("{}"), access::ALL),
            ("config go.d:x get", None, edit_only),
            ("config go.d:x update", Some("{}"), view_only),
            ("config go.d:x 'get schema'", None, access::ALL),
            ("config go.d:x restart", None, access::ALL),
        ];
        let answers: Vec<_> = cases.into_iter().map(|(cmd, payload, user)| fx.call(cmd, payload, user)).collect();
        fx.call("config go.d:T disable", None, access::ALL);
        fx.plugin.seen();
        let disabled_template = fx.call("config go.d:T:j2 enable", None, access::ALL);
        (fx, answers, disabled_template)
    });
    let (fx, answers, disabled_template) = fx;
    let json = |code: u16, msg: &str| (code, format!("{{\"status\":{code},\"message\":\"{msg}\"}}"));
    assert_eq!(
        answers,
        [
            json(400, "dyncfg functions intercept: invalid command received"),
            json(400, "dyncfg functions intercept: this action requires a name"),
            json(400, "dyncfg functions intercept: this action requires a payload"),
            json(400, "dyncfg functions intercept: this action does not require a payload"),
            json(403, "dyncfg: you don't have enough view permissions to execute this command"),
            json(403, "dyncfg: you don't have enough edit permissions to execute this command"),
            json(500, "dyncfg: permissions for this command are not set"),
            json(400, "dyncfg functions intercept: this command is not supported by this configuration node"),
        ]
    );
    assert_eq!(disabled_template, json(400, "dyncfg functions intercept: this job belongs to disabled template"));
    assert!(fx.plugin.seen().is_empty(), "nothing refused reaches the plugin");
    let errs: Vec<_> = texts(&records).into_iter().filter(|(p, _)| *p == Priority::Err).map(|(_, t)| t).collect();
    assert_eq!(
        errs,
        [
            "DYNCFG: this command is not supported by the configuration node: config go.d:x restart",
            "DYNCFG: cannot enable a job of a disabled template: config go.d:T:j2 enable",
        ]
    );
}

/// A template's `disable` and `enable` (UT U8, U9): DynCfg answers them, saving the template's choice, and echoes the
/// command to every job of it in the nodes' order, the caller's progress counting them; a user-disabled job is told
/// to disable on `enable`.
#[test]
fn a_template_command_goes_to_every_job() {
    let ((fx, answers, progress), records) = within(|| {
        let fx = fx();
        fx.add("go.d:T", Type::Template, "schema add enable disable", SourceType::Stock);
        fx.add("go.d:T:j2", Type::Job, "get enable disable", SourceType::Stock);
        fx.add("go.d:T:j3", Type::Job, "get enable disable", SourceType::Stock);
        fx.call("config go.d:T:j3 disable", None, access::ALL);
        fx.plugin.seen();
        let progress = Arc::new(Mutex::new(Vec::new()));
        let p = Arc::clone(&progress);
        let cb: netdata_agent_nrpc::call::ProgressCb = Arc::new(move |_, done, all| p.lock().unwrap().push((done, all)));
        let disable = fx.call_with_progress("config go.d:T disable", None, access::ALL, Some(Arc::clone(&cb)));
        let disabled = fx.plugin.functions();
        let enable = fx.call_with_progress("config go.d:T enable", None, access::ALL, Some(cb));
        let enabled = fx.plugin.functions();
        let progress = progress.lock().unwrap().clone();
        (fx, [(disable, disabled), (enable, enabled)], progress)
    });
    let applied = (200, "{\"status\":200,\"message\":\"applied to all template job\"}".to_string());
    assert_eq!(
        answers,
        [
            (applied.clone(), vec!["config go.d:T:j2 disable".to_string(), "config go.d:T:j3 disable".into()]),
            (applied, vec!["config go.d:T:j2 enable".to_string(), "config go.d:T:j3 disable".into()]),
        ]
    );
    assert_eq!(progress, [(0, 2), (1, 2), (2, 2), (0, 2), (1, 2), (2, 2)]);
    let template = fx.node("go.d:T").unwrap();
    assert_eq!((template.stored.user_disabled, template.stored.saves), (false, 2));
    assert_eq!(fx.node("go.d:T:j2").unwrap().stored.saves, 0, "an echo saves nothing");
    let actions: Vec<_> =
        texts(&records).into_iter().map(|(_, t)| t).filter(|t| t.starts_with("DYNCFG USER")).collect();
    assert_eq!(
        actions,
        [
            "DYNCFG USER ACTION 'disable' on job 'go.d:T:j3' by user 'tester', IP '127.0.0.1'",
            "DYNCFG USER ACTION 'disable' on template 'go.d:T' by user 'tester', IP '127.0.0.1'",
            "DYNCFG USER ACTION 'enable' on template 'go.d:T' by user 'tester', IP '127.0.0.1'",
        ]
    );
}

/// A user's `update` (UT U5, N1): the payload and the caller saved and made current before the save stamps them, so a
/// node never saved before keeps a current `created_ut` of 0; `enable`/`disable` change only the user's choice (N5).
#[test]
fn a_users_update_and_disable_are_saved() {
    let ((), _) = within(|| {
        let fx = fx();
        fx.add("go.d:x", Type::Single, "get schema update enable disable", SourceType::Stock);
        assert_eq!(fx.call("config go.d:x update", Some("{\"u\":2}"), access::ALL).0, 200);
        let node = fx.node("go.d:x").unwrap();
        assert_eq!(
            (node.stored.saves, node.current.created_ut, node.current.source_type, node.cmds),
            (1, 0, SourceType::Dyncfg, Cmds::parse(b"get schema update enable disable"))
        );
        assert_eq!(fx.call("config go.d:x disable", None, access::ALL).0, 200);
        let node = fx.node("go.d:x").unwrap();
        assert_eq!((node.stored.saves, node.stored.user_disabled, node.current.status), (2, true, Status::Running));
        fx.plugin.code.lock().unwrap().replace(400);
        assert_eq!(fx.call("config go.d:x enable", None, access::ALL).0, 400);
        assert!(fx.node("go.d:x").unwrap().stored.user_disabled, "a refused enable changes nothing");
    });
}

/// A user's `remove` of a job DynCfg made: node and file gone, its method left registered (K2), so the next call
/// finds no id.
#[test]
fn a_users_remove_deletes_the_job_and_leaves_its_method() {
    let ((fx, after), records) = within(|| {
        let fx = fx();
        fx.add("go.d:T", Type::Template, "schema add enable disable", SourceType::Stock);
        fx.call("config go.d:T add j1", Some("{}"), access::ALL);
        fx.add("go.d:T:j1", Type::Job, "get update enable disable", SourceType::Dyncfg);
        assert_eq!(fx.call("config go.d:T:j1 remove", None, access::ALL).0, 200);
        let after = fx.call("config go.d:T:j1 get", None, access::ALL);
        (fx, after)
    });
    assert!(fx.node("go.d:T:j1").is_none());
    assert!(!fx.file("go.d:T:j1").exists());
    assert!(fx.localhost().functions().get(b"config go.d:T:j1").is_some(), "K2");
    assert_eq!(after, (404, "{\"status\":404,\"message\":\"dyncfg functions intercept: id is not found\"}".into()));
    assert!(texts(&records).contains(&(
        Priority::Notice,
        "DYNCFG USER ACTION 'remove' on job 'go.d:T:j1' by user 'tester', IP '127.0.0.1'".into()
    )));
}

/// An echo's answer changes the node it was sent for: one deleted and set again since keeps its own state; a failed
/// one is C's ERR.
#[test]
fn an_echo_answer_follows_its_node() {
    let ((), records) = within(|| {
        let fx = fx();
        fx.plugin.hold();
        fx.add("go.d:x", Type::Single, "get schema enable disable", SourceType::Stock);
        fx.dyncfg.del_low_level(fx.localhost(), b"go.d:x");
        assert!(fx.node("go.d:x").is_none(), "never saved, so deleted");
        let first = std::mem::take(&mut *fx.plugin.held.lock().unwrap());
        fx.add("go.d:x", Type::Single, "get schema enable disable", SourceType::Stock);
        for req in first {
            (req.done)(req.reply, 298);
        }
        assert_eq!(fx.node("go.d:x").unwrap().current.status, Status::Accepted);
        fx.plugin.answer_held(503);
        assert_eq!(fx.node("go.d:x").unwrap().current.status, Status::Accepted);
    });
    assert_eq!(
        texts(&records),
        [(Priority::Err, "DYNCFG: received response code 503 on request to id 'go.d:x', cmd: enable".into())]
    );
}

/// The saved changes go back to the plugin when it registers again (`dyncfg_send_updates()`): a single's update with
/// its payload, a template's jobs as `add <name>`, in the files' order.
#[test]
fn saved_changes_are_sent_when_the_plugin_registers_again() {
    let ((), records) = within(|| {
        let first = fx();
        first.add("go.d:x", Type::Single, "get update enable disable", SourceType::Stock);
        first.call("config go.d:x update", Some("{\"u\":1}"), access::ALL);
        first.add("go.d:T", Type::Template, "schema add enable disable", SourceType::Stock);
        first.call("config go.d:T add j1", Some("{\"j\":1}"), access::ALL);
        let fx = fx_in(first.dir);
        assert_eq!(fx.node("go.d:x").unwrap().current.status, Status::Orphan, "loaded");
        fx.add("go.d:x", Type::Single, "get update enable disable", SourceType::Stock);
        fx.add("go.d:T", Type::Template, "schema add enable disable", SourceType::Stock);
        let seen = fx.plugin.seen();
        let updates: Vec<_> = seen.iter().map(|s| (s.function.as_str(), s.payload.as_deref(), s.echo)).collect();
        assert_eq!(
            updates,
            [
                ("config go.d:x enable", None, true),
                ("config go.d:x update", Some("application/json {\"u\":1}"), true),
                ("config go.d:T add j1", Some("application/json {\"j\":1}"), true),
            ]
        );
        assert_eq!(seen[1].source, SOURCE, "the saved source");
        let job = fx.node("go.d:T:j1").unwrap();
        assert_eq!((job.current.status, job.stored.plugin_rejected), (Status::Running, false));
    });
    assert!(texts(&records).iter().all(|(p, _)| *p == Priority::Notice), "{records:?}");
}

/// Replaces every number after `_ut":` and `"now":`, which the clock sets.
fn untimed(json: &str) -> String {
    let mut out = String::new();
    let mut rest = json;
    while let Some(at) = ["_ut\":", "\"now\":"].iter().filter_map(|k| rest.find(k).map(|i| i + k.len())).min() {
        out.push_str(&rest[..at]);
        rest = rest[at..].trim_start_matches(|c: char| c.is_ascii_digit());
        out.push('T');
    }
    out.push_str(rest);
    out
}

/// `config tree`: the host's nodes under the path, sorted by path and id, an orphan marked with only `remove`, the
/// sources DynCfg saved hidden from an anonymous caller, the counts that need attention, the agent.
#[test]
fn the_tree_lists_the_hosts_nodes() {
    let ((fx, anonymous, signed, filtered), _) = within(|| {
        let fx = fx();
        fx.add("go.d:x", Type::Single, "get enable disable", SourceType::Stock);
        fx.add("go.d:T", Type::Template, "schema add enable disable", SourceType::Stock);
        fx.call("config go.d:T add j1", Some("{}"), access::ALL);
        let anonymous = fx.call("config tree /collectors", None, access::ANONYMOUS_DATA);
        let signed = fx.call("config tree / go.d:T", None, access::ANONYMOUS_DATA | access::SENSITIVE_DATA);
        let filtered = fx.call("config tree /other", None, access::ALL);
        (fx, anonymous, signed, filtered)
    });
    let agent = format!("\"agent\":{{\"mg\":\"{GUID}\",\"nd\":null,\"nm\":\"dc-host\",\"now\":T}}");
    let access = "\"access\":{\"view\":[\"signed-in\",\"same-space\",\"view-config\"],\"edit\":[\"signed-in\",\
                  \"same-space\",\"commercial\",\"edit-config\"]}";
    let job = |source: &str| {
        format!(
            "\"go.d:T:j1\":{{\"type\":\"job\",\"template\":\"go.d:T\",\"status\":\"orphan\",\"cmds\":[\"remove\"],{access},\
             \"source_type\":\"dyncfg\",\"source\":\"{source}\",\"sync\":false,\"user_disabled\":false,\
             \"restart_required\":false,\"plugin_rejected\":false,\"payload\":{{\"available\":true,\"status\":\"running\",\
             \"source_type\":\"dyncfg\",\"source\":\"{source}\",\"created_ut\":T,\"modified_ut\":T,\
             \"content_type\":\"application/json\",\"content_length\":2}},\"saves\":1,\"created_ut\":T,\"modified_ut\":T}}"
        )
    };
    let template = format!(
        "\"go.d:T\":{{\"type\":\"template\",\"status\":\"accepted\",\"cmds\":[\"schema\",\"add\",\"enable\",\"disable\"],\
         {access},\"source_type\":\"stock\",\"source\":\"type=stock\",\"sync\":false,\"user_disabled\":false,\
         \"restart_required\":false,\"plugin_rejected\":false,\"payload\":{{\"available\":false}},\"saves\":0,\
         \"created_ut\":T,\"modified_ut\":T}}"
    );
    let single = format!(
        "\"go.d:x\":{{\"type\":\"single\",\"status\":\"running\",\"cmds\":[\"get\",\"schema\",\"enable\",\"disable\"],\
         {access},\"source_type\":\"stock\",\"source\":\"type=stock\",\"sync\":false,\"user_disabled\":false,\
         \"restart_required\":false,\"plugin_rejected\":false,\"payload\":{{\"available\":false}},\"saves\":0,\
         \"created_ut\":T,\"modified_ut\":T}}"
    );
    let attention = "\"attention\":{\"degraded\":false,\"restart_required\":0,\"plugin_rejected\":0,\"status_failed\":0,\
                     \"status_incomplete\":0}";
    let hidden = "User details hidden in anonymous mode. Sign in to access configuration details.";
    assert_eq!(
        (anonymous.0, untimed(&anonymous.1)),
        (
            200,
            format!(
                "{{\"version\":1,\"tree\":{{\"/collectors/go.d\":{{{template},{},{single}}}}},{attention},{agent}}}",
                job(hidden)
            )
        )
    );
    assert_eq!(
        untimed(&signed.1),
        format!(
            "{{\"version\":1,\"tree\":{{\"/collectors/go.d\":{{{template},{}}}}},{attention},{agent}}}",
            job(SOURCE)
        )
    );
    assert_eq!(untimed(&filtered.1), format!("{{\"version\":1,\"tree\":{{}},{attention},{agent}}}"));
    assert_eq!(fx.node("go.d:T:j1").unwrap().current.status, Status::Orphan, "sticky");
}

/// The catch-all: an unknown id is C's 404 and ERR; a new job's `test` goes to its template under the template's
/// name; an orphan's `remove` deletes it and its file; an invalid tree id is refused.
#[test]
fn the_catch_all_routes_as_c() {
    let ((fx, unknown, removed, bad_id), records) = within(|| {
        let fx = fx();
        fx.add("go.d:T", Type::Template, "schema add test enable disable", SourceType::Stock);
        fx.plugin.seen();
        let unknown = fx.call("config go.d:nothing get", None, access::ALL);
        assert_eq!(fx.call("config go.d:T:j9 test", Some("{}"), access::ALL), (200, "plugin".into()));
        assert_eq!(fx.plugin.functions(), ["config go.d:T test j9"]);
        fx.call("config go.d:T add j1", Some("{}"), access::ALL);
        let removed = fx.call("config go.d:T:j1 remove", None, access::ALL);
        let bad_id = fx.call("config tree / 'a b'", None, access::ALL);
        (fx, unknown, removed, bad_id)
    });
    assert_eq!(unknown, (404, "{\"status\":404,\"errorMessage\":\"Unknown config id given.\"}".into()));
    assert_eq!(removed, (200, "{\"status\":200,\"message\":\"\"}".into()));
    assert!(fx.node("go.d:T:j1").is_none());
    assert!(!fx.file("go.d:T:j1").exists());
    assert_eq!(bad_id, (400, "{\"status\":400,\"message\":\"invalid id given\"}".into()));
    let errs: Vec<_> = texts(&records).into_iter().filter(|(p, _)| *p == Priority::Err).map(|(_, t)| t).collect();
    assert_eq!(
        errs,
        [
            "DYNCFG: unknown config id 'go.d:nothing' in call: 'config go.d:nothing get'. This can happen if the plugin \
             that registered the dynamic configuration is not running now."
                .to_string(),
            "DYNCFG TREE: function call 'config tree / 'a b'': invalid id given".into(),
        ]
    );
}

/// `dyncfg_del_low_level()`: the method unregistered; a node never saved is gone, a saved one stays.
#[test]
fn a_plugins_delete_keeps_only_saved_nodes() {
    let ((), _) = within(|| {
        let fx = fx();
        fx.add("go.d:x", Type::Single, "get update enable disable", SourceType::Stock);
        fx.add("go.d:y", Type::Single, "get update enable disable", SourceType::Stock);
        fx.call("config go.d:y update", Some("{}"), access::ALL);
        fx.dyncfg.del_low_level(fx.localhost(), b"go.d:x");
        fx.dyncfg.del_low_level(fx.localhost(), b"go.d:y");
        assert!(fx.node("go.d:x").is_none());
        assert_eq!(fx.node("go.d:y").unwrap().stored.saves, 1);
        assert!(!fx.localhost().functions().available(b"config go.d:x"));
        assert!(!fx.localhost().functions().available(b"config go.d:y"));
    });
}
