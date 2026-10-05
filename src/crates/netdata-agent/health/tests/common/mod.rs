//! Shared by the vector tests: the reader of `netdata-agent-text` (the vectors use its field encoding).

#![allow(dead_code, unused_imports)]

#[path = "../../../text/tests/common/mod.rs"]
mod reader;

pub use reader::{Row, check, rows, unescape};

/// A string field: C's NULL is written as a lone NUL byte.
pub fn nullable(row: &Row, i: usize) -> Option<&[u8]> {
    (row.bytes(i) != [0]).then(|| row.bytes(i))
}

/// An `f32` from its bits in a field.
pub fn float(row: &Row, i: usize) -> f32 {
    f32::from_bits(u32::from_str_radix(row.str(i), 16).expect("hex bits"))
}

// ------------------------------------------------------------------------------------------------
// shared by the corpus tests: the items of rules.tsv, C's records, a rule as its row

use netdata_agent_eval::Expression;
use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::json::prototype_to_json;
use netdata_agent_health::prototype::Rule;
use netdata_agent_log::Captured;

/// Fields as the vectors hold them, shown as text in a failure's message. Comparisons are made on the bytes.
pub fn show(fields: &[Vec<u8>]) -> Vec<String> {
    fields.iter().map(|field| String::from_utf8_lossy(field).into_owned()).collect()
}

/// C's NULL string in a vector.
pub fn text(value: &Option<Vec<u8>>) -> Vec<u8> {
    value.clone().unwrap_or_else(|| vec![0])
}

pub fn number(n: impl ToString) -> Vec<u8> {
    n.to_string().into_bytes()
}

pub fn hex(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().flat_map(|b| format!("{b:02x}").into_bytes()).collect()
}

/// An expression's source and parsed_as, NULL for both when absent.
pub fn expression(e: &Option<Expression>) -> [Vec<u8>; 2] {
    match e {
        Some(e) => [e.source().to_vec(), e.parsed_as().to_vec()],
        None => [vec![0], vec![0]],
    }
}

/// A rule as its `rule` row, from the name on: what C's dumper writes when the rule is stored.
pub fn rule_fields(rule: &Rule) -> Vec<Vec<u8>> {
    let (am, ac) = (&rule.r#match, &rule.config);
    let mut fields = vec![
        text(&ac.name),
        number(u8::from(am.is_template)),
        number(u8::from(am.enabled)),
        text(&am.on),
        text(&am.host_labels),
        text(&am.chart_labels),
        text(&ac.exec),
        text(&ac.recipient),
        text(&ac.classification),
        text(&ac.component),
        text(&ac.r#type),
        number(ac.source_type as u8),
        text(&ac.source),
        text(&ac.units),
        text(&ac.summary),
        text(&ac.info),
        number(ac.update_every),
        number(ac.alert_action_options),
        text(&ac.dimensions),
        ac.time_group_name().as_bytes().to_vec(),
        number(ac.time_group_condition as u8),
        format!("{:016x}", ac.time_group_value.to_bits()).into_bytes(),
        number(ac.dims_group as u8),
        number(ac.data_source as u8),
        number(ac.before),
        number(ac.after),
        format!("{:08x}", ac.options as u32).into_bytes(),
    ];
    fields.extend(expression(&ac.calculation));
    fields.extend(expression(&ac.warning));
    fields.extend(expression(&ac.critical));
    fields.extend([
        number(ac.delay_up_duration),
        number(ac.delay_down_duration),
        number(ac.delay_max_duration),
        format!("{:08x}", ac.delay_multiplier.to_bits()).into_bytes(),
        number(u8::from(ac.has_custom_repeat_config)),
        number(ac.warn_repeat_every),
        number(ac.crit_repeat_every),
        hex(&ac.hash_id),
        prototype_to_json(ac.name.as_deref().unwrap_or(b""), &[rule], true),
    ]);
    fields
}

/// What C's logfmt printed of a message, back as the message's bytes: its escapes are JSON's.
pub fn unescape_logfmt(printed: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(printed.len());
    let mut bytes = printed.iter().copied();
    while let Some(c) = bytes.next() {
        if c != b'\\' {
            out.push(c);
            continue;
        }
        match bytes.next() {
            Some(b'n') => out.push(b'\n'),
            Some(b't') => out.push(b'\t'),
            Some(b'r') => out.push(b'\r'),
            Some(b'b') => out.push(8),
            Some(b'f') => out.push(12),
            Some(b'u') => {
                let code: Vec<u8> = bytes.by_ref().take(4).collect();
                let code = u32::from_str_radix(std::str::from_utf8(&code).expect("hex"), 16).expect("hex");
                let c = char::from_u32(code).expect("a character");
                out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// One of C's records: level, errno number (0 for none), the message's bytes.
pub type CRecord = (String, i32, Vec<u8>);

/// A `records.tsv`-shaped triple of fields (level, errno, message as printed) as a record.
pub fn c_record(level: &str, errno: &str, printed: &[u8]) -> CRecord {
    let errno = match errno {
        "-" => 0,
        text => text.split(',').next().and_then(|n| n.parse().ok()).expect("an errno number"),
    };
    (level.to_owned(), errno, unescape_logfmt(printed))
}

/// Whether the captured records are C's. A record of Rust is text, so where C's message is not UTF-8 (a byte of a
/// health file written as it is) Rust's holds U+FFFD for each such byte: those compare after the same replacement,
/// and are counted.
pub fn same_records(expected: &[CRecord], actual: &[Captured], not_utf8: &mut usize) -> bool {
    use netdata_agent_log::Priority;
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|((level, errno, message), record)| {
            // the first name of each priority in C's `nd_log_priorities[]`
            let actual_level = match record.priority {
                Priority::Emerg => "emergency",
                Priority::Alert => "alert",
                Priority::Crit => "critical",
                Priority::Err => "error",
                Priority::Warning => "warning",
                Priority::Notice => "notice",
                Priority::Info => "info",
                Priority::Debug => "debug",
            };
            let actual_message = record.message.as_deref().unwrap_or("");
            let same_message = match std::str::from_utf8(message) {
                Ok(message) => message == actual_message,
                Err(_) => {
                    *not_utf8 += 1;
                    String::from_utf8_lossy(message) == actual_message
                }
            };
            level == actual_level && *errno == record.errno && same_message
        })
}

/// The records as text, for a failure's message.
pub fn show_records(expected: &[CRecord], actual: &[Captured]) -> String {
    let first = expected
        .iter()
        .zip(actual)
        .position(|(e, a)| !same_records(std::slice::from_ref(e), std::slice::from_ref(a), &mut 0))
        .unwrap_or(expected.len().min(actual.len()));
    format!(
        "{} against {}, first at record {first}:\n  C    {:?}\n  Rust {:?}",
        expected.len(),
        actual.len(),
        expected.get(first).map(|(level, errno, message)| (level, errno, String::from_utf8_lossy(message))),
        actual.get(first).map(|record| (record.priority, record.errno, record.message.clone()))
    )
}

/// One corpus item of `rules.tsv`: its rows by kind.
pub struct Item {
    pub name: Vec<u8>,
    pub rules: Vec<Row>,
    pub files: Vec<Row>,
    pub entries: Vec<Row>,
}

pub fn items() -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    for row in rows("rules.tsv") {
        if items.last().is_none_or(|item| item.name != row.bytes(0)) {
            items.push(Item { name: row.bytes(0).to_vec(), rules: Vec::new(), files: Vec::new(), entries: Vec::new() });
        }
        let item = items.last_mut().expect("just pushed");
        match row.str(1) {
            "rule" => item.rules.push(row),
            "file" => item.files.push(row),
            "entry" => item.entries.push(row),
            other => panic!("rules.tsv:{}: kind {other}", row.line),
        }
    }
    items
}

/// The oracle's `[health]` values (`gen-health-vectors.c` `main()`).
pub fn oracle_config() -> HealthConfig {
    HealthConfig {
        default_exec: b"/oracle/plugins.d/alarm-notify.sh".to_vec(),
        default_recipient: b"root".to_vec(),
        ..HealthConfig::default()
    }
}

// ------------------------------------------------------------------------------------------------
// shared by the DynCfg tests: the generator's model of the configuration core

use std::sync::Arc;

use netdata_agent_dyncfg::model::{Cmds, SourceType, Status, Type};
use netdata_agent_health::dyncfg::{Cloud, Ctx, HostIndex, NodeSpec, Nodes, TEMPLATE_ID};
use netdata_agent_health::pass::Env;
use netdata_agent_health::{Clock, Health};
use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_rrd::host::Host;

/// The cases of a list of `tests/corpus/dyncfg/`: every line that is no comment, decoded; an empty line is the
/// empty text.
pub fn dyncfg_cases(name: &str) -> Vec<Vec<u8>> {
    let path: std::path::PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "corpus", "dyncfg", name].iter().collect();
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let data = data.strip_suffix(b"\n").unwrap_or(&data);
    data.split(|&b| b == b'\n').filter(|line| line.first() != Some(&b'#')).map(<[u8]>::to_vec).collect()
}

/// The one host of a scenario, which is localhost.
pub struct OneHost(pub Arc<Host>);

impl HostIndex for OneHost {
    fn all(&self) -> Vec<Arc<Host>> {
        vec![Arc::clone(&self.0)]
    }

    fn localhost(&self) -> Arc<Host> {
        Arc::clone(&self.0)
    }
}

/// A UUID's text, as C prints a hash in a call.
pub fn uuid_text(id: &[u8; 16]) -> Vec<u8> {
    let hex: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]).into_bytes()
}

/// The twin of the generator's stubs of the DynCfg core and of the Cloud's copy of a rule
/// (`tests/oracle/health-loop-stubs.c`): every call health makes is recorded with its fields, and a registration
/// is answered as the model of the core there answers it: the template's with an `add` per saved job, a job's with
/// `disable` (the user disabled it or the template, or it is registered disabled) or `enable`, and then, for a
/// job that is no DynCfg one and has a saved payload, with an `update`. Where that model is not the core at a
/// second registration is said in the stubs' header; the daemon's units run a reload on the real core.
pub struct Core<'a> {
    pub health: &'a Health,
    pub hosts: &'a dyn HostIndex,
    pub env: &'a dyn Env,
    pub clock: Clock<'a>,
    /// The jobs a saved file has: name and payload, in the core's order.
    pub saved: &'a [(Vec<u8>, Vec<u8>)],
    /// The ids a saved file says the user disabled.
    pub user_disabled: &'a [Vec<u8>],
    /// Whether the Cloud has a rule's hash.
    pub cloud_has: bool,
    /// Where a call goes, with its fields: the list the caller keeps its other calls in, so that their order shows.
    pub record: &'a dyn Fn(Vec<Vec<u8>>),
}

impl Core<'_> {
    pub fn ctx(&self) -> Ctx<'_> {
        Ctx { nodes: self, cloud: self, hosts: self.hosts, env: self.env, clock: self.clock }
    }

    fn call(&self, fields: &[&[u8]]) {
        (self.record)(fields.iter().map(|field| field.to_vec()).collect());
    }

    fn echo(&self, id: &[u8], cmd: Cmds, name: Option<&[u8]>, payload: Option<&[u8]>) {
        let cmd_name = cmd.name_one().expect("one command").as_bytes();
        self.call(&[b"echo", id, cmd_name, name.unwrap_or(b"-"), payload.unwrap_or(b"-")]);
        let mut reply = Reply::new(ContentType::TextPlain);
        let code = self.health.dyncfg_callback(&self.ctx(), &mut reply, id, cmd, name, payload);
        self.call(&[b"echoed", id, cmd_name, code.to_string().as_bytes(), &reply.body]);
    }
}

impl Nodes for Core<'_> {
    fn add(&self, node: &NodeSpec<'_>) -> bool {
        let mut cmds = Vec::new();
        node.cmds.write_joined(&mut cmds);
        self.call(&[
            b"dyncfg_add",
            node.id,
            node.path,
            node.kind.name().as_bytes(),
            node.status.name().as_bytes(),
            node.source_type.name().as_bytes(),
            node.source,
            &cmds,
        ]);

        if node.kind == Type::Template {
            for (name, payload) in self.saved {
                self.echo(node.id, Cmds::ADD, Some(name), Some(payload));
            }
            return true;
        }
        let user_disabled = |id: &[u8]| self.user_disabled.iter().any(|disabled| disabled == id);
        let disable = user_disabled(node.id) || node.status == Status::Disabled || user_disabled(TEMPLATE_ID);
        self.echo(node.id, if disable { Cmds::DISABLE } else { Cmds::ENABLE }, None, None);
        if node.source_type != SourceType::Dyncfg
            && let Some(name) = node.id.strip_prefix(TEMPLATE_ID).and_then(|rest| rest.strip_prefix(b":"))
        {
            for (_, payload) in self.saved.iter().filter(|(saved, _)| saved == name) {
                self.echo(node.id, Cmds::UPDATE, None, Some(payload));
            }
        }
        true
    }

    fn del(&self, id: &[u8]) {
        self.call(&[b"dyncfg_del", id]);
    }

    fn status(&self, id: &[u8], status: Status) {
        self.call(&[b"dyncfg_status", id, status.name().as_bytes()]);
    }
}

impl Cloud for Core<'_> {
    fn has(&self, hash: &[u8; 16]) -> bool {
        self.call(&[b"alert_hash_has_transitioned", &uuid_text(hash), if self.cloud_has { b"1" } else { b"0" }]);
        self.cloud_has
    }

    fn send_configuration(&self, hash: &[u8; 16]) {
        self.call(&[b"aclk_send_alert_configuration", &uuid_text(hash)]);
    }
}

/// The fields of an `answer`: the code, the content type, the cache word, the expiry as seconds after `now`
/// (`-` for none), the body.
pub fn answer_fields(code: u16, reply: &Reply, now: i64) -> Vec<Vec<u8>> {
    let content_type = match reply.content_type {
        ContentType::ApplicationJson => "application/json",
        ContentType::TextPlain => "text/plain",
        other => panic!("an answer of {other:?}"),
    };
    let expiry = if reply.expires == 0 { "-".to_owned() } else { (reply.expires - now).to_string() };
    vec![
        number(code),
        content_type.as_bytes().to_vec(),
        if reply.cacheable { b"c".to_vec() } else { b"n".to_vec() },
        expiry.into_bytes(),
        reply.body.clone(),
    ]
}
