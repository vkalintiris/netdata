//! The netdatacli commands (`src/daemon/commands.c`): the table, the request parser, the reply framing, the locks and
//! the init gating. The socket and its threads are in `command_server`.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, Weak};

use netdata_agent_inicfg::Config;
use netdata_agent_log::{netdata_log_error, netdata_log_info};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::labels;
use netdata_agent_rrd::mode::DbMode;

use crate::metasync::MetaQueue;
use crate::{build, cloud_proxy, conf, host_labels, meta_store, server, shutdown};

/// `MAX_COMMAND_LENGTH`: a request keeps at most one byte less.
pub const MAX_COMMAND_LENGTH: usize = 8192;

/// `cmd_status_t`.
pub type Status = u32;
pub const SUCCESS: Status = 0;
pub const FAILURE: Status = 1;

/// `cmd_type_t`: which locks a command takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The write lock: waits for every locked command.
    Exclusive,
    /// The read lock and the command's own mutex.
    Orthogonal,
    /// The read lock.
    Concurrent,
    /// No lock.
    HighPriority,
}

/// `cmd_init_status_t`.
pub const OFF: u8 = 0;
pub const INIT: u8 = 1;
pub const FULL: u8 = 2;

struct Command {
    name: &'static str,
    params: &'static str,
    help: &'static str,
    kind: Kind,
    /// The init status the command needs.
    init: u8,
}

pub const HELP: usize = 0;
pub const RELOAD_HEALTH: usize = 1;
pub const REOPEN_LOGS: usize = 2;
pub const EXIT: usize = 3;
pub const FATAL: usize = 4;
pub const RELOAD_CLAIMING_STATE: usize = 5;
pub const RELOAD_LABELS: usize = 6;
pub const READ_CONFIG: usize = 7;
pub const WRITE_CONFIG: usize = 8;
pub const PING: usize = 9;
pub const ACLK_STATE: usize = 10;
pub const VERSION: usize = 11;
pub const DUMPCONFIG: usize = 12;
pub const MARK_STALE_NODES_EPHEMERAL: usize = 13;
pub const REMOVE_STALE_NODE: usize = 14;
pub const UPDATE_NODE_INFO: usize = 15;

const STALE_PARAMS: &str = "<node_id | machine_guid | hostname | ALL_NODES>";

/// `command_info_array`, in C's order: the parser takes the first name that prefixes the request.
static COMMANDS: [Command; 16] = [
    Command {
        name: "help",
        params: "",
        help: "Show this help menu.",
        kind: Kind::HighPriority,
        init: INIT,
    },
    Command {
        name: "reload-health",
        params: "",
        help: "Reload health configuration.",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "reopen-logs",
        params: "",
        help: "Close and reopen log files.",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "shutdown-agent",
        params: "",
        help: "Cleanup and exit the netdata agent.",
        kind: Kind::Exclusive,
        init: FULL,
    },
    Command {
        name: "fatal-agent",
        params: "",
        help: "Log the state and halt the netdata agent.",
        kind: Kind::HighPriority,
        init: FULL,
    },
    Command {
        name: "reload-claiming-state",
        params: "",
        help: "Reload agent claiming state from disk.",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "reload-labels",
        params: "",
        help: "Reload all localhost labels.",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "read-config",
        params: "",
        help: "",
        kind: Kind::Concurrent,
        init: FULL,
    },
    Command {
        name: "write-config",
        params: "",
        help: "",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "ping",
        params: "",
        help: "Return with 'pong'; exit 0 when ready, 1 while initializing, 255 if the agent cannot be contacted.",
        kind: Kind::Orthogonal,
        init: INIT,
    },
    Command {
        name: "aclk-state",
        params: "[json]",
        help: "Returns current state of ACLK and Netdata Cloud connection. (optionally in json).",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "version",
        params: "",
        help: "Returns the netdata version.",
        kind: Kind::Orthogonal,
        init: INIT,
    },
    Command {
        name: "dumpconfig",
        params: "",
        help: "Returns the current netdata.conf on stdout.",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "mark-stale-nodes-ephemeral",
        params: STALE_PARAMS,
        help: "Marks one or all disconnected nodes as ephemeral, while keeping their retention\n      available for \
               queries on both this Netdata Agent dashboard and Netdata Cloud",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "remove-stale-node",
        params: STALE_PARAMS,
        help: "Marks one or all disconnected nodes as ephemeral, and removes them\n      so that they are no longer \
               available for queries, from both this\n      Netdata Agent dashboard and Netdata Cloud.",
        kind: Kind::Orthogonal,
        init: FULL,
    },
    Command {
        name: "update-node-info",
        params: "",
        help: "Schedules an node update message for localhost to Netdata Cloud.",
        kind: Kind::Orthogonal,
        init: FULL,
    },
];

/// `isspace()` in the C locale.
fn is_space(b: u8) -> bool {
    b == b' ' || (0x09..=0x0d).contains(&b)
}

/// `parse_commands()`: the request as a C string (up to its first NUL); leading whitespace skipped, the first command
/// whose name prefixes the rest, and its arguments without surrounding whitespace. `None` is an illegal command.
pub fn parse(request: &[u8]) -> Option<(usize, &[u8])> {
    let request = request
        .iter()
        .position(|&b| b == 0)
        .map_or(request, |nul| &request[..nul]);
    let start = request
        .iter()
        .position(|&b| !is_space(b))
        .unwrap_or(request.len());
    let request = &request[start..];
    let idx = COMMANDS
        .iter()
        .position(|c| request.starts_with(c.name.as_bytes()))?;
    let args = &request[COMMANDS[idx].name.len()..];
    let args = &args[args
        .iter()
        .position(|&b| !is_space(b))
        .unwrap_or(args.len())..];
    let end = args
        .iter()
        .rposition(|&b| !is_space(b))
        .map_or(0, |last| last + 1);
    Some((idx, &args[..end]))
}

/// The reply to an illegal request.
pub const ILLEGAL: &str = "Illegal command. Please type \"help\" for instructions.";

/// `send_command_reply()`: `X<status>\0`, then the message after `O` (stdout) or `E` (stderr). `ping` always answers
/// on stdout; `idx` is `None` for an illegal request. The message ends at its first NUL, as C's strings do.
pub fn reply(idx: Option<usize>, status: Status, message: Option<&[u8]>) -> Vec<u8> {
    let mut out = format!("X{status}\0").into_bytes();
    if let Some(message) = message {
        out.push(if idx == Some(PING) || status == SUCCESS {
            b'O'
        } else {
            b'E'
        });
        let len = message
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(message.len());
        out.extend_from_slice(&message[..len]);
    }
    out
}

/// `command_server_initialized`.
static STATUS: AtomicU8 = AtomicU8::new(OFF);

pub fn status() -> u8 {
    STATUS.load(Ordering::Acquire)
}

pub fn set_status(status: u8) {
    STATUS.store(status, Ordering::Release);
}

/// `netdata_ready`: set once startup completed; `ping` answers 1 until then.
static READY: AtomicBool = AtomicBool::new(false);

pub fn set_ready() {
    READY.store(true, Ordering::Release);
}

/// What the FULL commands work on, set before the command server goes FULL.
pub struct Ctx {
    /// netdata.conf and the hosts.
    pub shared: Arc<server::Shared>,
    /// `cloud_config`, and the file `reload-claiming-state` reloads it from.
    pub cloud: Mutex<Config>,
    pub cloud_conf_file: String,
    /// Where `reload-labels` finds the Kubernetes labels script.
    pub plugins_dir: String,
    /// `db_meta`, which the stale-node commands read and write; weak, so that the exit can close it.
    pub meta: Weak<MetaDb>,
    /// METASYNC's queue, which takes the dimensions of a removed host.
    pub metaqueue: MetaQueue,
}

static CTX: OnceLock<Ctx> = OnceLock::new();

pub fn set_context(ctx: Ctx) {
    let _ = CTX.set(ctx);
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `exclusive_rwlock` and `command_lock_array`, taken in that order.
static EXCLUSIVE: RwLock<()> = RwLock::new(());
static LOCKS: [Mutex<()>; 16] = [const { Mutex::new(()) }; 16];

/// `execute_command()`: the command under its locks, or `Agent is initializing` (success) when the command server is
/// not yet at the status the command needs.
pub fn execute(idx: usize, args: &[u8]) -> (Status, Option<Vec<u8>>) {
    let command = &COMMANDS[idx];
    let _write;
    let _read;
    let _own;
    match command.kind {
        Kind::Exclusive => _write = EXCLUSIVE.write().unwrap_or_else(PoisonError::into_inner),
        Kind::Orthogonal => {
            _read = EXCLUSIVE.read().unwrap_or_else(PoisonError::into_inner);
            _own = lock(&LOCKS[idx]);
        }
        Kind::Concurrent => _read = EXCLUSIVE.read().unwrap_or_else(PoisonError::into_inner),
        Kind::HighPriority => {}
    }
    if status() >= command.init {
        run(idx, args)
    } else {
        (SUCCESS, Some(b"Agent is initializing".to_vec()))
    }
}

fn run(idx: usize, args: &[u8]) -> (Status, Option<Vec<u8>>) {
    match idx {
        HELP => (SUCCESS, Some(help().into_bytes())),
        EXIT => {
            netdata_agent_log::limits_unlimited();
            netdata_log_info!("COMMAND: Cleaning up to exit.");
            shutdown::exit_gracefully("cmd-exit");
            std::process::exit(0);
        }
        FATAL => netdata_agent_log::fatal!("COMMAND: netdata now exits."),
        RELOAD_CLAIMING_STATE => {
            // claiming is not ported: cloud.conf is reloaded (its CLAIM: records are C's alone) and the agent is
            // unclaimed
            if let Some(ctx) = CTX.get() {
                netdata_agent_log::limits_unlimited();
                conf::load_cloud_conf(&mut lock(&ctx.cloud), &ctx.cloud_conf_file, true);
                netdata_agent_log::limits_reset();
            }
            (
                SUCCESS,
                Some(
                    b"Netdata Agent is not claimed to Netdata Cloud: Agent is not claimed yet"
                        .to_vec(),
                ),
            )
        }
        RELOAD_LABELS => reload_labels(),
        ACLK_STATE => {
            netdata_log_info!("COMMAND: Reopening aclk/cloud state.");
            let Some(ctx) = CTX.get() else {
                return (FAILURE, None);
            };
            if args.windows(4).any(|w| w == b"json") {
                (SUCCESS, Some(aclk_state_json(ctx)))
            } else {
                (SUCCESS, Some(ACLK_STATE_OFFLINE.as_bytes().to_vec()))
            }
        }
        MARK_STALE_NODES_EPHEMERAL => remove_stale_node(args, false),
        REMOVE_STALE_NODE => remove_stale_node(args, true),
        UPDATE_NODE_INFO => (
            SUCCESS,
            Some(b"Agent is not connected to Netdata Cloud".to_vec()),
        ),
        RELOAD_HEALTH => {
            // health is not ported: only its record
            netdata_agent_log::limits_unlimited();
            netdata_log_info!("COMMAND: Reloading HEALTH configuration.");
            netdata_agent_log::limits_reset();
            (SUCCESS, None)
        }
        REOPEN_LOGS => {
            netdata_agent_log::limits_unlimited();
            netdata_agent_log::reopen_log_files(true);
            netdata_agent_log::limits_reset();
            (SUCCESS, None)
        }
        READ_CONFIG => read_config(args),
        WRITE_CONFIG => write_config(args),
        PING => (
            if READY.load(Ordering::Acquire) {
                SUCCESS
            } else {
                FAILURE
            },
            Some(b"pong".to_vec()),
        ),
        VERSION => (
            SUCCESS,
            Some(format!("netdata {}", build::NETDATA_VERSION).into_bytes()),
        ),
        DUMPCONFIG => match CTX.get() {
            Some(ctx) => (SUCCESS, Some(ctx.shared.conf().generate(false, true))),
            None => (FAILURE, None),
        },
        _ => (FAILURE, None),
    }
}

/// `cmd_help_execute()`: every command with a help text.
fn help() -> String {
    let mut out = String::from("The commands are:\n\n");
    for c in COMMANDS.iter().filter(|c| !c.help.is_empty()) {
        out.push_str("  ");
        out.push_str(c.name);
        if !c.params.is_empty() {
            out.push(' ');
            out.push_str(c.params);
        }
        out.push_str("\n      ");
        out.push_str(c.help);
        out.push_str("\n\n");
    }
    out
}

/// `cmd_reload_labels_execute()`: the localhost labels reloaded, then listed.
fn reload_labels() -> (Status, Option<Vec<u8>>) {
    netdata_log_info!("COMMAND: reloading host labels.");
    let Some(ctx) = CTX.get() else {
        return (FAILURE, None);
    };
    let hosts = &ctx.shared.hosts;
    {
        let mut netdata = ctx.shared.conf();
        let mut cloud = lock(&ctx.cloud);
        host_labels::reload(&mut netdata, &mut cloud, &ctx.plugins_dir, hosts);
    }
    // rrdlabels_log_to_buffer()
    let mut out = Vec::new();
    for label in hosts.localhost().labels().iter() {
        out.extend_from_slice(b"Label: ");
        out.extend_from_slice(&label.name);
        out.extend_from_slice(b": \"");
        out.extend_from_slice(&label.value);
        out.extend_from_slice(b"\" (unknown)\n");
    }
    (SUCCESS, Some(out))
}

/// `remove_ephemeral_host()`: an offline child marked ephemeral (its `_is_ephemeral` label too, stored at once), and
/// with `unregister` its node unregistered and the host freed, its dimensions queued for deletion. Positive when
/// changed, 0 otherwise, negative when busy; the texts go to `out` (errors only if `report`). The cloud and pulse are
/// not ported: their updates are left out.
fn remove_ephemeral_host(out: &mut Vec<u8>, host: &Host, report: bool, unregister: bool) -> i32 {
    let ctx = CTX.get().expect("the command server is FULL");
    let name = |what: &str| {
        format!(
            "Node '{}' (machine guid: {}) {what}",
            host.hostname(),
            host.machine_guid()
        )
        .into_bytes()
    };
    if host.is_localhost() {
        if report {
            out.extend(name("is our localhost - not changing it"));
        }
        return 0;
    }
    // the context load still uses the host
    if unregister && host.is_pending_context_load() {
        if report {
            out.extend(name("is busy loading contexts - try again"));
        }
        return -1;
    }
    // the metadata writer stores the host under the read side of this lock; freeing it takes the write side
    let (read, mut write) = if unregister {
        (None, host.metadata_try_write())
    } else {
        (host.metadata_try_read(), None)
    };
    if read.is_none() && write.is_none() {
        if report {
            out.extend(name("is busy - try again"));
        }
        return -1;
    }
    if host.is_online() {
        if report {
            out.extend(name("is online - not changing it"));
        }
        return 0;
    }
    let mut marked = !host.is_ephemeral();
    host.set_ephemeral(true);
    marked |= host.update_labels(|l| {
        l.add_changed(b"_is_ephemeral", b"true", labels::SRC_CONFIG)
            .unwrap_or(false)
    });
    let id = meta_store::host_id(host);
    let meta = ctx.meta.upgrade();
    // sql_set_host_label()
    match (&meta, &id) {
        (Some(meta), Some(id)) => {
            let _ = meta.set_host_label(id, "_is_ephemeral", "true");
        }
        (None, _) => meta_store::no_database("sql_set_host_label"),
        (Some(_), None) => {}
    }
    if unregister {
        // unregister_node(): ACLKSYNC is not ported, so its statements run here
        if let (Some(meta), Some(id)) = (&meta, &id) {
            meta.unregister_node(id);
        }
        host.set_node_id([0; 16]);
        out.extend(name("has been unregistered"));
        // rrdhost_free___consume_metadata_lifetime_writelock(): the freed dimensions of ram, alloc and none charts
        // leave no data behind
        if let Some(freed) = write.as_deref_mut() {
            *freed = true;
        }
        ctx.shared.hosts.remove(host.machine_guid());
        for chart in host.charts().all() {
            if matches!(chart.mode(), DbMode::Ram | DbMode::Alloc | DbMode::None) {
                for dim in chart.dims() {
                    ctx.metaqueue.delete_dimension(*dim.uuid());
                }
            }
        }
        return 1;
    }
    if marked {
        out.extend(name("has been marked ephemeral"));
        return 1;
    }
    if report {
        out.extend(name("is already ephemeral - not changing it"));
    }
    0
}

/// `cmd_remove_stale_node_internal()`: a machine GUID or node ID names one host in memory; otherwise every stored host
/// with that hostname, or all of them for `ALL_NODES`, in the `host` table's order (a stored host no longer in memory
/// is skipped).
fn remove_stale_node(args: &[u8], unregister: bool) -> (Status, Option<Vec<u8>>) {
    let Some(ctx) = CTX.get() else {
        return (FAILURE, None);
    };
    let mut out = Vec::new();
    if args.is_empty() {
        return (
            SUCCESS,
            Some(b"Please specify a machine or node UUID or hostname".to_vec()),
        );
    }
    let arg = String::from_utf8_lossy(args);
    let hosts = &ctx.shared.hosts;
    let named = hosts.find_by_guid(&arg).or_else(|| {
        uuid::Uuid::parse_str(&arg)
            .ok()
            .and_then(|id| hosts.find_by_node_id(id.as_bytes()))
    });
    if let Some(host) = named {
        remove_ephemeral_host(&mut out, &host, true, unregister);
        return (SUCCESS, Some(out));
    }
    let report = arg != "ALL_NODES";
    let guids = match &ctx.meta.upgrade() {
        Some(meta) => meta.hosts_named(&arg),
        None => {
            meta_store::no_database("cmd_remove_stale_node_internal");
            None
        }
    };
    let Some(guids) = guids else {
        return (
            SUCCESS,
            Some(b"Failed to prepare database statement to check for stale nodes".to_vec()),
        );
    };
    let (mut changed, mut busy) = (0, 0);
    for host in guids.iter().filter_map(|guid| hosts.find_by_guid(guid)) {
        let rc = remove_ephemeral_host(&mut out, &host, report, unregister);
        if rc > 0 {
            changed += rc;
            out.push(b'\n');
        } else if rc < 0 {
            busy += 1;
            if report {
                out.push(b'\n');
            }
        }
    }
    if changed == 0 && busy == 0 && out.is_empty() {
        out = if report {
            format!("No match for \"{arg}\"").into_bytes()
        } else {
            b"No stale nodes found".to_vec()
        };
    } else if busy > 0 && !report {
        let (s, verb) = if busy == 1 { ("", "is") } else { ("s", "are") };
        out.extend(format!("{busy} node{s} {verb} busy - try again").into_bytes());
    }
    (SUCCESS, Some(out))
}

/// `aclk_state()` of an agent that is not claimed and not connected: ACLK is not ported.
const ACLK_STATE_OFFLINE: &str = "ACLK Available: Yes\nACLK Version: 2\nProtocols Supported: Protobuf\nProtocol Used: \
                                  Protobuf\nMQTT Version: 5\nClaimed: No\nOnline: No\nReconnect count: 0\nBanned By \
                                  Cloud: No\n\nMQTT Messages Dropped Without a PUBACK (since start): 0\n";

/// A json-c string (`json_escape_str()`): `/` is escaped too, other control bytes as lower-case `\u00xx`.
fn jsonc_string(out: &mut Vec<u8>, text: &[u8]) {
    out.push(b'"');
    for &ch in text {
        match ch {
            0x08 => out.extend_from_slice(b"\\b"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x0c => out.extend_from_slice(b"\\f"),
            b'"' | b'\\' | b'/' => out.extend_from_slice(&[b'\\', ch]),
            0..0x20 => out.extend_from_slice(format!("\\u{ch:04x}").as_bytes()),
            _ => out.push(ch),
        }
    }
    out.push(b'"');
}

/// `aclk_state_json()` of an agent that is not claimed and not connected, with its node instances. Every host has
/// ACLK's alert sync state, all zeros without claiming and health.
fn aclk_state_json(ctx: &Ctx) -> Vec<u8> {
    let (url, proxy) = {
        let mut netdata = ctx.shared.conf();
        let mut cloud = lock(&ctx.cloud);
        let url = cloud.get(netdata_agent_inicfg::SECTION_GLOBAL, "url", None);
        (url, cloud_proxy::full_display(&mut netdata, &mut cloud))
    };
    let mut out = Vec::new();
    out.extend_from_slice(
        b"{\"aclk-available\":true,\"aclk-version\":2,\"protocols-supported\":[\"Protobuf\"],\"agent-claimed\":false,\
          \"claimed-id\":null,\"cloud-url\":",
    );
    match url {
        Some(url) => jsonc_string(&mut out, &url),
        None => out.extend_from_slice(b"null"),
    }
    out.extend_from_slice(b",\"aclk_proxy\":");
    jsonc_string(&mut out, proxy.as_bytes());
    out.extend_from_slice(
        b",\"publish_latency_us\":0,\"online\":false,\"used-cloud-protocol\":\"Protobuf\",\"mqtt-version\":5,\
          \"received-app-layer-msgs\":0,\"received-mqtt-pubacks\":0,\"pending-mqtt-pubacks\":0,\
          \"timed-out-mqtt-pubacks-since-start\":0,\"server-receive-maximum\":0,\"reconnect-count\":0,\
          \"last-connect-time-utc\":null,\"last-connect-time-puback-utc\":null,\"last-disconnect-time-utc\":null,\
          \"next-connection-attempt-utc\":null,\"last-backoff-value\":null,\"banned-by-cloud\":false,\
          \"node-instances\":[",
    );
    for (i, host) in ctx.shared.hosts.all().iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        let uuid = |id: [u8; 16]| uuid::Uuid::from_bytes(id).hyphenated().to_string();
        out.extend_from_slice(b"{\"hostname\":");
        jsonc_string(&mut out, host.hostname().as_bytes());
        out.extend_from_slice(b",\"mguid\":");
        jsonc_string(&mut out, host.machine_guid().as_bytes());
        out.extend_from_slice(b",\"claimed_id\":");
        // the localhost's own claim: not claimed
        match host.claim_id().filter(|_| !host.is_localhost()) {
            Some(id) => jsonc_string(&mut out, uuid(id).as_bytes()),
            None => out.extend_from_slice(b"null"),
        }
        out.extend_from_slice(b",\"node-id\":");
        match host.node_id() {
            id if id == [0; 16] => out.extend_from_slice(b"null"),
            id => jsonc_string(&mut out, uuid(id).as_bytes()),
        }
        let online = host.is_localhost() || host.receiver().is_some();
        out.extend_from_slice(
            format!(
                ",\"streaming-hops\":{},\"relationship\":\"{}\",\"streaming-online\":{online},\"alert-sync-status\":\
                 {{\"updates\":0,\"checkpoint-count\":0,\"alert-count\":0,\"alert-snapshot-count\":0,\"alert-version\":0}}}}",
                host.ingestion_hops(),
                if host.is_localhost() { "self" } else { "child" },
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(b"]}");
    out
}

/// Splits `args` at its first `n` `|` separators: `None` when there are fewer. The last part keeps any later `|`.
fn split_config_args(args: &[u8], n: usize) -> Option<Vec<String>> {
    let parts: Vec<&[u8]> = args.splitn(n + 1, |&b| b == b'|').collect();
    (parts.len() == n + 1).then(|| {
        parts
            .into_iter()
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect()
    })
}

/// The configuration `conf_file` names: `cloud` is cloud.conf, anything else netdata.conf.
fn with_config<R>(ctx: &Ctx, conf_file: &str, f: impl FnOnce(&mut Config) -> R) -> R {
    if conf_file == "cloud" {
        f(&mut lock(&ctx.cloud))
    } else {
        f(&mut ctx.shared.conf())
    }
}

/// `cmd_read_config_execute()`: `<file>|<section>|<key>`.
fn read_config(args: &[u8]) -> (Status, Option<Vec<u8>>) {
    let (Some(parts), Some(ctx)) = (split_config_args(args, 2), CTX.get()) else {
        return (FAILURE, None);
    };
    let [conf_file, section, key] = &parts[..] else {
        return (FAILURE, None);
    };
    match with_config(ctx, conf_file, |c| c.get(section, key, None)) {
        Some(value) => (SUCCESS, Some(value)),
        None => {
            netdata_log_error!(
                "Cannot execute read-config conf_file={conf_file} section={section} / key={key} because no value set"
            );
            (FAILURE, None)
        }
    }
}

/// `cmd_write_config_execute()`: `<file>|<section>|<key>|<value>`, in memory only.
fn write_config(args: &[u8]) -> (Status, Option<Vec<u8>>) {
    netdata_log_info!("write-config {}", String::from_utf8_lossy(args));
    let (Some(parts), Some(ctx)) = (split_config_args(args, 3), CTX.get()) else {
        return (FAILURE, None);
    };
    let [conf_file, section, key, value] = &parts[..] else {
        return (FAILURE, None);
    };
    with_config(ctx, conf_file, |c| c.set(section, key, value));
    netdata_log_info!(
        "write-config conf_file={conf_file} section={section} key={key} value={value}"
    );
    (SUCCESS, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Request, and the command and arguments it parses to.
    type ParseCase = (Vec<u8>, Option<(usize, &'static [u8])>);
    /// Command, status, message, and the reply's bytes.
    type ReplyCase = (Option<usize>, Status, Option<&'static [u8]>, &'static [u8]);
    /// Arguments, separators wanted, and the parts.
    type SplitCase = (&'static [u8], usize, Option<Vec<&'static str>>);

    #[test]
    fn requests_parse_as_cs_prefix_match() {
        let spaces = |n| " ".repeat(n);
        let cases: HashMap<&str, ParseCase> = HashMap::from([
            ("bare", (b"ping".to_vec(), Some((PING, &b""[..])))),
            (
                "client's trailing space",
                (b"ping ".to_vec(), Some((PING, &b""[..]))),
            ),
            (
                "surrounding whitespace",
                (b"\t\x0bping\n".to_vec(), Some((PING, &b""[..]))),
            ),
            ("prefix", (b"pingpong".to_vec(), Some((PING, &b"pong"[..])))),
            (
                "help prefix",
                (b"helpme".to_vec(), Some((HELP, &b"me"[..]))),
            ),
            (
                "nul ends it",
                (b"ping\0junk".to_vec(), Some((PING, &b""[..]))),
            ),
            ("empty", (Vec::new(), None)),
            ("upper case", (b"PING".to_vec(), None)),
            (
                "args trimmed",
                (
                    b"read-config a|b|c  \n".to_vec(),
                    Some((READ_CONFIG, &b"a|b|c"[..])),
                ),
            ),
            (
                "args inner space kept",
                (
                    b"write-config a|b|c|d e ".to_vec(),
                    Some((WRITE_CONFIG, &b"a|b|c|d e"[..])),
                ),
            ),
            (
                "one-byte arg",
                (b"ping x".to_vec(), Some((PING, &b"x"[..]))),
            ),
            (
                "long",
                (
                    format!("{}ping", spaces(8187)).into_bytes(),
                    Some((PING, &b""[..])),
                ),
            ),
            (
                "first match wins",
                (b"reload-healthx".to_vec(), Some((RELOAD_HEALTH, &b"x"[..]))),
            ),
        ]);
        for (name, (request, want)) in cases {
            assert_eq!(parse(&request), want, "{name}");
        }
        for (i, c) in COMMANDS.iter().enumerate() {
            assert_eq!(parse(c.name.as_bytes()), Some((i, &b""[..])), "{}", c.name);
        }
    }

    #[test]
    fn replies_are_cs_frames() {
        let cases: HashMap<&str, ReplyCase> = HashMap::from([
            (
                "pong",
                (Some(PING), SUCCESS, Some(&b"pong"[..]), &b"X0\0Opong"[..]),
            ),
            (
                "ping while initializing",
                (Some(PING), FAILURE, Some(b"pong"), b"X1\0Opong"),
            ),
            (
                "failure without message",
                (Some(READ_CONFIG), FAILURE, None, b"X1\0"),
            ),
            (
                "empty message",
                (Some(READ_CONFIG), SUCCESS, Some(b""), b"X0\0O"),
            ),
            ("illegal", (None, FAILURE, Some(b"bad"), b"X1\0Ebad")),
            (
                "nul ends the message",
                (Some(VERSION), SUCCESS, Some(b"a\0b"), b"X0\0Oa"),
            ),
        ]);
        for (name, (idx, status, message, want)) in cases {
            assert_eq!(reply(idx, status, message), want, "{name}");
        }
    }

    #[test]
    fn the_help_text_is_cs() {
        let help = help();
        assert_eq!(help.len(), 1332);
        assert!(help.starts_with(
            "The commands are:\n\n  help\n      Show this help menu.\n\n  reload-health\n"
        ));
        assert!(help.contains(
            "\n  aclk-state [json]\n      Returns current state of ACLK and Netdata Cloud connection. (optionally in \
             json).\n\n"
        ));
        assert!(!help.contains("read-config") && !help.contains("write-config"));
        assert!(help.ends_with(
            "  update-node-info\n      Schedules an node update message for localhost to Netdata Cloud.\n\n"
        ));
    }

    #[test]
    fn config_args_split_at_the_first_separators() {
        let cases: HashMap<&str, SplitCase> = HashMap::from([
            ("none", (&b""[..], 2, None)),
            ("one", (&b"netdata|global"[..], 2, None)),
            (
                "two",
                (
                    &b"netdata|global|hostname"[..],
                    2,
                    Some(vec!["netdata", "global", "hostname"]),
                ),
            ),
            (
                "key keeps later bars",
                (
                    &b"cloud|global|a|b"[..],
                    2,
                    Some(vec!["cloud", "global", "a|b"]),
                ),
            ),
            (
                "value keeps later bars",
                (&b"a|b|c|d|e"[..], 3, Some(vec!["a", "b", "c", "d|e"])),
            ),
            ("empty parts", (&b"||"[..], 2, Some(vec!["", "", ""]))),
        ]);
        for (name, (args, n, want)) in cases {
            let want = want.map(|v| v.into_iter().map(String::from).collect::<Vec<_>>());
            assert_eq!(split_config_args(args, n), want, "{name}");
        }
    }
}
