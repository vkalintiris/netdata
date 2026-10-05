//! Health's DynCfg nodes (`health_dyncfg.c`): the payload of an alert as the dashboard sends it, the `health.d`
//! text it is shown as, and what health answers when DynCfg calls it for its template or for an alert's job.
//!
//! A payload is read with json-c's coercions ([`netdata_agent_ingest::jsonc`]). What json-c reads and a strict
//! reader refuses (a comment, single quotes, a trailing comma, `TRUE`) is refused here too, with json-c's text for
//! the nearest refusal (D46.1).
//!
//! Health reaches the daemon through [`Ctx`], handed in with every call: the DynCfg core ([`Nodes`]), the Cloud's
//! copy of a rule ([`Cloud`]), the hosts ([`HostIndex`]) and the pass's environment. A registration runs the core's
//! first echo into [`Health::dyncfg_callback`] on the same thread, so no lock of the rules' store is held across a
//! call of [`Nodes`].

use std::sync::Arc;
use std::sync::atomic::Ordering;

use netdata_agent_dyncfg::model::{
    Cmds, RESP_ACCEPTED, RESP_ACCEPTED_DISABLED, SourceType, Status, Type, default_response,
};
use netdata_agent_eval::{ERROR_REMAINING_GARBAGE, Expression, strerror};
use netdata_agent_ingest::jsonc::{self, Presence};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_query::tables::{TimeGrouping, options, options_parse_one, options_to_text};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_text::c::c_str;
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};
use serde_json::{Map, Value};

use crate::expr::parse_logged;
use crate::json::prototype_to_json;
use crate::keywords::lossy;
use crate::matching::{ChartKey, prototype_rules_for_chart};
use crate::pass::Env;
use crate::prototype::{AlertConfig, AlertMatch, Prototype, Rule};
use crate::tables::{
    ACTION_OPTION_NO_CLEAR_NOTIFICATION_NAME, DataSource, DimsGrouping, GroupCondition, OPTIONS_DATA_SOURCES,
    OPTIONS_DIMS_AGGREGATION, action_options_parse_one,
};
use crate::{Clock, Health};

/// `DYNCFG_HEALTH_ALERT_PROTOTYPE_PREFIX`: the template's id; a job's is this, a colon and the alert's name.
pub const TEMPLATE_ID: &[u8] = b"health:alert:prototype";
/// The path of the template and of every job.
pub const PATH: &[u8] = b"/health/alerts/prototypes";

/// A node as health registers it (`struct dyncfg_add_inline_spec`, without the host, which is localhost, the
/// accesses, which are the core's defaults, and the callback, which is [`Health::dyncfg_callback`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSpec<'a> {
    pub id: &'a [u8],
    pub path: &'a [u8],
    pub kind: Type,
    pub status: Status,
    pub source_type: SourceType,
    pub source: &'a [u8],
    pub cmds: Cmds,
}

/// What health asks of the DynCfg core (`dyncfg_add()`, `dyncfg_del()`, `dyncfg_status()` of `dyncfg-inline.c`).
/// `add` may call [`Health::dyncfg_callback`] before it returns: the core echoes a job's `enable` or `disable`,
/// and a template's saved jobs, at once.
pub trait Nodes {
    fn add(&self, node: &NodeSpec<'_>) -> bool;
    fn del(&self, id: &[u8]);
    fn status(&self, id: &[u8], status: Status);
}

/// The Cloud's copy of a rule (`sqlite_aclk_alert.c`).
pub trait Cloud {
    /// `alert_hash_has_transitioned()`: whether the table `alert_hash_cloud` has the rule's hash.
    fn has(&self, hash: &[u8; 16]) -> bool;
    /// `aclk_send_alert_configuration()`.
    fn send_configuration(&self, hash: &[u8; 16]);
}

/// The hosts a DynCfg change reaches (`rrdhost_root_index`), and the one whose default command the `userconfig`
/// text compares with (`localhost`).
pub trait HostIndex {
    fn all(&self) -> Vec<Arc<Host>>;
    fn localhost(&self) -> Arc<Host>;
}

impl HostIndex for Hosts {
    fn all(&self) -> Vec<Arc<Host>> {
        Hosts::all(self)
    }

    fn localhost(&self) -> Arc<Host> {
        Arc::clone(Hosts::localhost(self))
    }
}

/// What a DynCfg call of health works with: the daemon's, or a test's.
pub struct Ctx<'a> {
    pub nodes: &'a dyn Nodes,
    pub cloud: &'a dyn Cloud,
    pub hosts: &'a dyn HostIndex,
    pub env: &'a dyn Env,
    pub clock: Clock<'a>,
}

type Members = Map<String, Value>;

fn fail<T>(error: &mut String, text: String) -> Option<T> {
    error.push_str(&text);
    None
}

fn bytes(text: Option<String>) -> Option<Vec<u8>> {
    text.map(String::into_bytes)
}

/// `JSONC_PARSE_SUBOBJECT_CB`'s path for the member: C joins with a dot unless the path is empty.
fn sub(path: &str, member: &str) -> String {
    if path.is_empty() { member.to_owned() } else { format!("{path}.{member}") }
}

/// `parse_bounded_int64()`.
fn bounded(
    obj: &Members,
    path: &str,
    member: &str,
    (minimum, maximum): (i64, i64),
    presence: Presence,
    error: &mut String,
) -> Option<i64> {
    let value = jsonc::int64(obj, path, member, presence, error)?;
    if value < minimum || value > maximum {
        return fail(error, format!("value for '{path}.{member}' is outside range [{minimum}, {maximum}]"));
    }
    Some(value)
}

const INT: (i64, i64) = (i32::MIN as i64, i32::MAX as i64);

/// `JSONC_PARSE_TXT2EXPRESSION_OR_ERROR_AND_RETURN`: an empty text and `*` are no expression; one that does not
/// parse fails in both modes, after the evaluator's own record.
fn expression(
    obj: &Members,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<Option<Expression>> {
    match obj.get(member) {
        Some(Value::String(text)) => {
            let text = c_str(text.as_bytes());
            if text.is_empty() || text == b"*" {
                return Some(None);
            }
            match parse_logged(text) {
                Ok(parsed) => Some(Some(parsed)),
                Err(e) => {
                    let failed_at = e.failed_at().unwrap_or(0).min(text.len());
                    fail(
                        error,
                        format!(
                            "expression '{path}.{member}' has a non-parseable expression '{}': {} at '{}'",
                            lossy(text),
                            strerror(e.code().unwrap_or(ERROR_REMAINING_GARBAGE)),
                            lossy(&text[failed_at..])
                        ),
                    )
                }
            }
        }
        Some(_) if presence == Presence::Required => {
            fail(error, format!("invalid type for '{path}.{member}' expression"))
        }
        None if presence == Presence::Required => fail(error, format!("missing '{path}.{member}' expression")),
        _ => Some(None),
    }
}

/// `parse_match()`.
fn parse_match(obj: &Members, path: &str, m: &mut AlertMatch, presence: Presence, error: &mut String) -> Option<()> {
    let on = jsonc::txt(obj, path, "on", presence, error)?;
    // an empty text is no text, and a rule without one would match everything
    if on.is_none() && presence == Presence::Required {
        return fail(error, format!("member '{path}.on' cannot be empty"));
    }
    m.on = bytes(on);
    m.host_labels = bytes(jsonc::pattern(obj, path, "host_labels", presence, error)?);
    m.chart_labels = bytes(jsonc::pattern(obj, path, "instance_labels", presence, error)?);
    Some(())
}

/// `parse_config_value_database_lookup()`.
fn parse_lookup(obj: &Members, path: &str, ac: &mut AlertConfig, presence: Presence, error: &mut String) -> Option<()> {
    let after = bounded(obj, path, "after", INT, presence, error)?;
    let before = bounded(obj, path, "before", INT, presence, error)?;
    ac.after = after as i32;
    ac.before = before as i32;

    if let Some(name) = jsonc::enum_text(obj, path, "time_group", presence, error)? {
        ac.time_group = Some(TimeGrouping::parse(name));
    }
    if let Some(name) = jsonc::enum_text(obj, path, "dims_group", presence, error)? {
        ac.dims_group = DimsGrouping::parse(name);
    }
    if let Some(name) = jsonc::enum_text(obj, path, "data_source", presence, error)? {
        ac.data_source = DataSource::parse(name);
    }

    let reads_value = match ac.time_group {
        Some(TimeGrouping::Countif) => {
            if let Some(name) = jsonc::enum_text(obj, path, "time_group_condition", presence, error)? {
                ac.time_group_condition = GroupCondition::parse(name);
            }
            true
        }
        Some(TimeGrouping::TrimmedMean | TimeGrouping::TrimmedMedian | TimeGrouping::Percentile) => true,
        _ => false,
    };
    if reads_value && let Some(value) = jsonc::double(obj, path, "time_group_value", presence, error)? {
        ac.time_group_value = value;
    }

    jsonc::bitmap_into(obj, path, "options", options_parse_one, presence, error, &mut ac.options)?;
    ac.dimensions = bytes(jsonc::txt(obj, path, "dimensions", presence, error)?);
    Some(())
}

/// `parse_config_value()`: the calculation and the units are optional in both modes.
fn parse_value(obj: &Members, path: &str, ac: &mut AlertConfig, presence: Presence, error: &mut String) -> Option<()> {
    if let Some(lookup) = jsonc::object(obj, path, "database_lookup", presence, error)? {
        parse_lookup(lookup, &sub(path, "database_lookup"), ac, presence, error)?;
    }
    if let Some(parsed) = expression(obj, path, "calculation", Presence::Optional, error)? {
        ac.calculation = Some(parsed);
    }
    ac.units = bytes(jsonc::txt(obj, path, "units", Presence::Optional, error)?);

    let update_every = jsonc::int64(obj, path, "update_every", presence, error)?;
    if update_every < 0 {
        return fail(error, format!("negative value for '{path}.update_every'"));
    }
    if update_every > i64::from(i32::MAX) {
        return fail(error, format!("value for '{path}.update_every' exceeds maximum {}", i32::MAX));
    }
    ac.update_every = update_every as i32;
    Some(())
}

/// `parse_config_conditions()`.
fn parse_conditions(
    obj: &Members,
    path: &str,
    ac: &mut AlertConfig,
    presence: Presence,
    error: &mut String,
) -> Option<()> {
    if let Some(parsed) = expression(obj, path, "warning_condition", presence, error)? {
        ac.warning = Some(parsed);
    }
    if let Some(parsed) = expression(obj, path, "critical_condition", presence, error)? {
        ac.critical = Some(parsed);
    }
    Some(())
}

/// `parse_config_action_delay()`. A multiplier written as a JSON integer is converted from the integer, not from
/// its double.
fn parse_delay(obj: &Members, path: &str, ac: &mut AlertConfig, presence: Presence, error: &mut String) -> Option<()> {
    let up = bounded(obj, path, "up", INT, presence, error)?;
    let down = bounded(obj, path, "down", INT, presence, error)?;
    let max = bounded(obj, path, "max", INT, presence, error)?;
    ac.delay_up_duration = up as i32;
    ac.delay_down_duration = down as i32;
    ac.delay_max_duration = max as i32;

    let integer = jsonc::int_member(obj, "multiplier");
    let multiplier =
        jsonc::double(obj, path, "multiplier", presence, error)?.unwrap_or(f64::from(ac.delay_multiplier));
    if !multiplier.is_finite() || multiplier < -f64::from(f32::MAX) || multiplier > f64::from(f32::MAX) {
        return fail(error, format!("non-finite or out-of-range value for '{path}.multiplier'"));
    }
    ac.delay_multiplier = match integer {
        Some(integer) => integer as f32,
        None => multiplier as f32,
    };
    Some(())
}

/// `parse_config_action_repeat()`.
fn parse_repeat(obj: &Members, path: &str, ac: &mut AlertConfig, presence: Presence, error: &mut String) -> Option<()> {
    ac.has_custom_repeat_config = jsonc::boolean(obj, path, "enabled", presence, error)?;
    let range = (0, i64::from(i32::MAX));
    let warning = bounded(obj, path, "warning", range, presence, error)?;
    let critical = bounded(obj, path, "critical", range, presence, error)?;
    ac.warn_repeat_every = warning as u32;
    ac.crit_repeat_every = critical as u32;
    Some(())
}

/// `parse_config_action()`.
fn parse_action(obj: &Members, path: &str, ac: &mut AlertConfig, presence: Presence, error: &mut String) -> Option<()> {
    jsonc::bitmap_into(obj, path, "options", action_options_parse_one, presence, error, &mut ac.alert_action_options)?;
    ac.exec = bytes(jsonc::txt(obj, path, "execute", presence, error)?);
    ac.recipient = bytes(jsonc::txt(obj, path, "recipient", presence, error)?);
    if let Some(delay) = jsonc::object(obj, path, "delay", presence, error)? {
        parse_delay(delay, &sub(path, "delay"), ac, presence, error)?;
    }
    if let Some(repeat) = jsonc::object(obj, path, "repeat", presence, error)? {
        parse_repeat(repeat, &sub(path, "repeat"), ac, presence, error)?;
    }
    Some(())
}

/// `parse_config()`: the five texts, the conditions and the action are optional in both modes, and so is every
/// member of the last two.
fn parse_config(obj: &Members, path: &str, rule: &mut Rule, presence: Presence, error: &mut String) -> Option<()> {
    use Presence::Optional;
    let ac = &mut rule.config;
    ac.summary = bytes(jsonc::txt(obj, path, "summary", Optional, error)?);
    ac.info = bytes(jsonc::txt(obj, path, "info", Optional, error)?);
    ac.r#type = bytes(jsonc::txt(obj, path, "type", Optional, error)?);
    ac.component = bytes(jsonc::txt(obj, path, "component", Optional, error)?);
    ac.classification = bytes(jsonc::txt(obj, path, "classification", Optional, error)?);

    if let Some(value) = jsonc::object(obj, path, "value", presence, error)? {
        parse_value(value, &sub(path, "value"), ac, presence, error)?;
    }
    if let Some(conditions) = jsonc::object(obj, path, "conditions", Optional, error)? {
        parse_conditions(conditions, &sub(path, "conditions"), ac, Optional, error)?;
    }
    if let Some(action) = jsonc::object(obj, path, "action", Optional, error)? {
        parse_action(action, &sub(path, "action"), ac, Optional, error)?;
    }
    if let Some(m) = jsonc::object(obj, path, "match", presence, error)? {
        parse_match(m, &sub(path, "match"), &mut rule.r#match, presence, error)?;
    }
    Some(())
}

/// `parse_prototype()`: the document's name, and a rule per item of `rules`. C fills its first rule in place, so a
/// document with no item still gives one rule, an empty one.
fn parse_prototype(
    obj: &Members,
    name: Option<&[u8]>,
    presence: Presence,
    error: &mut String,
) -> Option<(Option<Vec<u8>>, Vec<Rule>)> {
    let version = jsonc::uint64(obj, "", "format_version", presence, error)?;
    if version != 1 {
        return fail(error, "unsupported document version".to_owned());
    }

    let unnamed = name.is_none_or(<[u8]>::is_empty);
    let name_presence = if unnamed { presence } else { Presence::Optional };
    let payload_name = bytes(jsonc::txt(obj, "", "name", name_presence, error)?);

    let Some(rules) = obj.get("rules") else {
        return fail(error, "the rules array is missing".to_owned());
    };
    let Value::Array(items) = rules else {
        return fail(error, "member 'rules' is not an array".to_owned());
    };

    let none = Members::new();
    let mut rules = Vec::with_capacity(items.len().max(1));
    for item in items {
        // an item that is no object has no member
        let item = item.as_object().unwrap_or(&none);
        let mut rule = Rule::default();
        rule.r#match.enabled = jsonc::boolean(item, "", "enabled", presence, error)?;

        let kind = jsonc::chars(item, "", "type", 32, presence, error)?;
        rule.r#match.is_template = match kind.as_slice() {
            b"template" => true,
            b"instance" => false,
            other => {
                return fail(
                    error,
                    format!("type is '{}', but it can only be 'instance' or 'template'", lossy(other)),
                );
            }
        };

        if let Some(config) = jsonc::object(item, "", "config", presence, error)? {
            parse_config(config, "config", &mut rule, presence, error)?;
        }
        rules.push(rule);
    }
    if rules.is_empty() {
        rules.push(Rule::default());
    }
    Some((payload_name, rules))
}

/// `data_source_to_rrdr_options()` and `dims_grouping_to_rrdr_options()`: the two names are written into the
/// lookup's options, in place of whatever the `options` array said of those bits.
fn names_to_options(ac: &mut AlertConfig) {
    ac.options &= !OPTIONS_DATA_SOURCES;
    ac.options |= match ac.data_source {
        DataSource::Samples => 0,
        DataSource::Percentages => options::PERCENTAGE,
        DataSource::Anomalies => options::ANOMALY_BIT,
    };
    ac.options &= !OPTIONS_DIMS_AGGREGATION;
    ac.options |= match ac.dims_group {
        DimsGrouping::Sum => 0,
        DimsGrouping::Average => options::DIMS_AVERAGE,
        DimsGrouping::Min => options::DIMS_MIN,
        DimsGrouping::Max => options::DIMS_MAX,
        DimsGrouping::Min2Max => options::DIMS_MIN2MAX,
    };
}

/// `health_prototype_payload_parse()`: the rules of one alert from a payload, or why there are none. `name` is
/// the name the call gives, and it always replaces the document's. `presence` is the mode: an `add` and an
/// `update` require what a `userconfig` only reads when it is there.
///
/// The error text grows as C's buffer does: the text of an option name that is none is no failure, and stands in
/// front of a later one.
pub fn payload_parse(payload: &[u8], name: Option<&[u8]>, presence: Presence) -> Result<Vec<Rule>, String> {
    let root = jsonc::tokener_parse_ex(payload).map_err(|text| format!("failed to parse json payload: {text}"))?;
    let none = Members::new();
    let members = root.as_object().unwrap_or(&none);

    let mut error = String::new();
    let Some((payload_name, mut rules)) = parse_prototype(members, name, presence, &mut error) else {
        return Err(error);
    };
    // an empty text is C's NULL string
    let name = match name {
        Some(name) if !name.is_empty() => Some(name.to_vec()),
        _ => payload_name,
    };

    for (index, rule) in rules.iter_mut().enumerate() {
        rule.config.name.clone_from(&name);
        if !rule.config.has_db_lookup() && rule.config.calculation.is_none() && presence == Presence::Required {
            error.push_str(&format!("Item {index} has neither database lookup nor calculation"));
            return Err(error);
        }
        names_to_options(&mut rule.config);
    }
    Ok(rules)
}

/// `dyncfg_user_config_print_duration()`: hours when the seconds divide into them, else minutes, else seconds.
fn duration(seconds: i32) -> String {
    if seconds % 3600 == 0 {
        format!("{}h", seconds / 3600)
    } else if seconds % 60 == 0 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

/// C's `%0.2f`.
fn fixed2(value: f64) -> String {
    if value.is_nan() {
        if value.is_sign_negative() { "-nan" } else { "nan" }.to_owned()
    } else {
        format!("{value:.2}")
    }
}

/// `dyncfg_health_prototype_to_conf()`: the rules as a `health.d` text, under `name`. A rule's command is left
/// out when it is localhost's default one, which is no text until localhost's first health pass.
pub fn prototype_to_conf(rules: &[Rule], name: &[u8], localhost_default_exec: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        let (am, ac) = (&rule.r#match, &rule.config);
        if index > 0 {
            out.push(b'\n');
        }
        line(&mut out, if am.is_template { "template" } else { "alarm" }, b" ", name);
        line(&mut out, "on", b" ", am.on.as_deref().unwrap_or(b""));
        for (label, value) in [
            ("class", &ac.classification),
            ("type", &ac.r#type),
            ("component", &ac.component),
            ("host labels", &am.host_labels),
            ("chart labels", &am.chart_labels),
        ] {
            if let Some(value) = value {
                line(&mut out, label, b" ", value);
            }
        }

        if ac.after != 0 {
            let mut lookup = ac.time_group.map_or("unknown", TimeGrouping::name).to_owned();
            match ac.time_group {
                Some(TimeGrouping::Percentile | TimeGrouping::TrimmedMean | TimeGrouping::TrimmedMedian) => {
                    lookup.push_str(&format!("({})", fixed2(ac.time_group_value)));
                }
                Some(TimeGrouping::Countif) => {
                    lookup.push_str(&format!("({}{})", ac.time_group_condition.name(), fixed2(ac.time_group_value)));
                }
                _ => {}
            }
            lookup.push_str(&format!(" {}", duration(ac.after)));
            if ac.before != 0 {
                lookup.push_str(&format!(" at {}", duration(ac.before)));
            }
            if ac.options != 0 {
                lookup.push_str(&format!(" {}", options_to_text(ac.options)));
            }
            let mut lookup = lookup.into_bytes();
            if let Some(dimensions) = &ac.dimensions {
                lookup.extend_from_slice(b" of ");
                lookup.extend_from_slice(dimensions);
            }
            line(&mut out, "lookup", b" ", &lookup);
        }

        if let Some(calculation) = &ac.calculation {
            line(&mut out, "calc", b" ", calculation.source());
        }
        if let Some(units) = &ac.units {
            line(&mut out, "units", b" ", units);
        }
        if ac.update_every != 0 {
            line(&mut out, "every", b" ", duration(ac.update_every).as_bytes());
        }
        if let Some(warning) = &ac.warning {
            line(&mut out, "warn", b" ", warning.source());
        }
        if let Some(critical) = &ac.critical {
            line(&mut out, "crit", b" ", critical.source());
        }

        // these three labels' colons have no space after them: the words bring their own, the options none
        if ac.delay_up_duration != 0 || ac.delay_down_duration != 0 {
            let mut delay = String::new();
            if ac.delay_up_duration != 0 {
                delay.push_str(&format!(" up {}", duration(ac.delay_up_duration)));
            }
            if ac.delay_down_duration != 0 {
                delay.push_str(&format!(" down {}", duration(ac.delay_down_duration)));
            }
            if ac.delay_multiplier != 0.0 {
                delay.push_str(&format!(" multiplier {}", fixed2(f64::from(ac.delay_multiplier))));
            }
            if ac.delay_max_duration != 0 {
                delay.push_str(&format!(" max {}", duration(ac.delay_max_duration)));
            }
            line(&mut out, "delay", b"", delay.as_bytes());
        }
        if ac.alert_action_options != 0 {
            line(&mut out, "options", b"", ACTION_OPTION_NO_CLEAR_NOTIFICATION_NAME.as_bytes());
        }
        if ac.has_custom_repeat_config {
            let repeat = if ac.crit_repeat_every == 0 && ac.warn_repeat_every == 0 {
                " off".to_owned()
            } else {
                format!(
                    " warning {} critical {}",
                    duration(ac.warn_repeat_every as i32),
                    duration(ac.crit_repeat_every as i32)
                )
            };
            line(&mut out, "repeat", b"", repeat.as_bytes());
        }

        if let Some(summary) = &ac.summary {
            line(&mut out, "summary", b" ", summary);
        }
        if let Some(info) = &ac.info {
            line(&mut out, "info", b" ", info);
        }
        if let Some(exec) = ac.exec.as_deref().filter(|exec| *exec != localhost_default_exec) {
            line(&mut out, "exec", b" ", exec);
        }
        if let Some(recipient) = &ac.recipient {
            line(&mut out, "to", b" ", recipient);
        }
    }
    out
}

/// One line of the text: the label right-aligned in 13 columns (C's `%13s:`), what follows its colon, the value.
fn line(out: &mut Vec<u8>, label: &str, after_colon: &[u8], value: &[u8]) {
    out.extend_from_slice(format!("{label:>13}:").as_bytes());
    out.extend_from_slice(after_colon);
    out.extend_from_slice(value);
    out.push(b'\n');
}

/// `health_dyncfg_alert_prototype_id_strdupz()`.
fn job_id(name: &[u8]) -> Vec<u8> {
    [TEMPLATE_ID, b":", name].concat()
}

/// `health_dyncfg_reject()`: the record names the command and the alert alone. The parser's reason goes to the
/// caller only: a payload's texts may hold what no log should.
fn reject(reply: &mut Reply, cmd: &str, alert: &[u8], reason: &str) -> u16 {
    nd_log!(Source::Daemon, Priority::Err, "HEALTH DYNCFG: rejected '{cmd}' of alert prototype '{}'", lossy(alert));
    default_response(reply, 400, reason)
}

impl Health {
    /// `dyncfg_health_cb()`: what health answers to a command on its template or on an alert's job. The id's first
    /// three words name health's nodes; a fourth is the alert, and what follows it is not read.
    pub fn dyncfg_callback(
        &self,
        ctx: &Ctx<'_>,
        reply: &mut Reply,
        id: &[u8],
        cmd: Cmds,
        name: Option<&[u8]>,
        payload: Option<&[u8]>,
    ) -> u16 {
        let words = quoted_strings_splitter(id, 100, Separators::DyncfgId);
        let word = |index: usize| words.get(index).map(Vec::as_slice).filter(|word| !word.is_empty());
        for (index, expected, ordinal) in [(0, "health", "first"), (1, "alert", "second"), (2, "prototype", "third")] {
            if word(index) != Some(expected.as_bytes()) {
                return default_response(reply, 400, &format!("{ordinal} component of id is not '{expected}'"));
            }
        }
        match word(3) {
            None => self.dyncfg_template_action(ctx, reply, cmd, name, payload),
            Some(alert) => self.dyncfg_job_action(ctx, reply, cmd, payload, alert),
        }
    }

    /// `dyncfg_health_prototype_template_action()`.
    fn dyncfg_template_action(
        &self,
        ctx: &Ctx<'_>,
        reply: &mut Reply,
        cmd: Cmds,
        add_name: Option<&[u8]>,
        payload: Option<&[u8]>,
    ) -> u16 {
        let name = add_name.unwrap_or(b"");
        match cmd {
            Cmds::ADD => {
                let mut rules = match payload_parse(payload.unwrap_or(b""), add_name, Presence::Required) {
                    Ok(rules) => rules,
                    Err(reason) => return reject(reply, "add", name, &reason),
                };
                rules[0].config.source_type = SourceType::Dyncfg;
                if let Err(reason) = self.add_from_dyncfg(rules, ctx.cloud) {
                    return default_response(reply, 400, reason);
                }
                if self.prototypes().get(name).is_none() {
                    return default_response(reply, 500, "added prototype is not found");
                }
                self.apply_prototype_to_all_hosts(ctx, name);
                self.dyncfg_register_prototype(ctx, name);
                // read after the registration: the job's first echo may have disabled the name
                let enabled = self.prototypes().get(name).is_some_and(Prototype::enabled);
                default_response(reply, if enabled { RESP_ACCEPTED } else { RESP_ACCEPTED_DISABLED }, "accepted")
            }
            Cmds::USERCONFIG => self.dyncfg_userconfig(ctx, reply, payload, name),
            Cmds::SCHEMA => default_response(reply, 501, "schema not implemented yet for prototype templates"),
            Cmds::TEST => default_response(reply, 501, "test not implemented yet for prototype templates"),
            Cmds::REMOVE | Cmds::RESTART | Cmds::DISABLE | Cmds::ENABLE | Cmds::UPDATE | Cmds::GET => {
                default_response(reply, 400, "action given is not supported for prototype templates")
            }
            Cmds::NONE => default_response(reply, 400, "invalid action received for prototype templates"),
            // no single command: C's switch takes no case and returns its first value
            _ => 500,
        }
    }

    /// `dyncfg_health_prototype_job_action()`.
    fn dyncfg_job_action(
        &self,
        ctx: &Ctx<'_>,
        reply: &mut Reply,
        cmd: Cmds,
        payload: Option<&[u8]>,
        alert: &[u8],
    ) -> u16 {
        let Some((enabled, any_rule_enabled)) = self.prototypes().get(alert).map(|prototype| {
            (prototype.enabled(), prototype.rules().iter().any(|rule| rule.r#match.enabled))
        }) else {
            return default_response(reply, 404, "no alert prototype is available by the name given");
        };
        let id = job_id(alert);

        match cmd {
            Cmds::SCHEMA => default_response(reply, 501, "schema not implemented yet"),
            Cmds::GET => {
                let prototypes = self.prototypes();
                let rules: Vec<&Rule> = prototypes.get(alert).map(|p| p.rules().iter().collect()).unwrap_or_default();
                reply.body = prototype_to_json(alert, &rules, false);
                reply.content_type = ContentType::ApplicationJson;
                reply.expires = 0;
                reply.cacheable = false;
                200
            }
            Cmds::DISABLE if enabled => {
                self.dyncfg_set_enabled(alert, false);
                self.apply_prototype_to_all_hosts(ctx, alert);
                ctx.nodes.status(&id, Status::Disabled);
                default_response(reply, 200, "disabled")
            }
            Cmds::DISABLE => default_response(reply, 200, "already disabled"),
            Cmds::ENABLE if enabled => default_response(reply, 200, "already enabled"),
            Cmds::ENABLE if !any_rule_enabled => default_response(
                reply,
                400,
                "all rules in this alert are disabled, so enabling the alert has no effect",
            ),
            Cmds::ENABLE => {
                self.dyncfg_set_enabled(alert, true);
                self.apply_prototype_to_all_hosts(ctx, alert);
                ctx.nodes.status(&id, Status::Accepted);
                default_response(reply, RESP_ACCEPTED, "enabled")
            }
            Cmds::UPDATE => {
                let mut rules = match payload_parse(payload.unwrap_or(b""), Some(alert), Presence::Required) {
                    Ok(rules) => rules,
                    Err(reason) => return reject(reply, "update", alert, &reason),
                };
                rules[0].config.source_type = SourceType::Dyncfg;
                if let Err(reason) = self.add_from_dyncfg(rules, ctx.cloud) {
                    return default_response(reply, 400, reason);
                }
                self.apply_prototype_to_all_hosts(ctx, alert);
                // the job is not registered again
                let enabled = self.prototypes().get(alert).is_some_and(Prototype::enabled);
                default_response(reply, if enabled { RESP_ACCEPTED } else { RESP_ACCEPTED_DISABLED }, "updated")
            }
            Cmds::USERCONFIG => self.dyncfg_userconfig(ctx, reply, payload, alert),
            Cmds::REMOVE => {
                self.remove_alerts_of_prototype(ctx, alert);
                self.prototypes.write().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(alert);
                let code = default_response(reply, 200, "deleted");
                ctx.nodes.del(&id);
                code
            }
            Cmds::TEST | Cmds::ADD | Cmds::RESTART => {
                default_response(reply, 400, "action given is not supported for the prototype job")
            }
            Cmds::NONE => default_response(reply, 400, "invalid action received"),
            _ => 500,
        }
    }

    /// A `userconfig` of the template or of a job: the payload as a `health.d` text under `name`, with nothing
    /// required of it; a payload that is refused is no record.
    fn dyncfg_userconfig(&self, ctx: &Ctx<'_>, reply: &mut Reply, payload: Option<&[u8]>, name: &[u8]) -> u16 {
        match payload_parse(payload.unwrap_or(b""), Some(name), Presence::Optional) {
            Err(reason) => default_response(reply, 400, &reason),
            Ok(rules) => {
                let localhost = ctx.hosts.localhost();
                let (default_exec, _) = self.host_defaults(&localhost);
                reply.body = prototype_to_conf(&rules, name, default_exec);
                reply.content_type = ContentType::TextPlain;
                reply.expires = (ctx.clock)();
                reply.cacheable = false;
                200
            }
        }
    }

    fn dyncfg_set_enabled(&self, name: &[u8], enabled: bool) {
        self.prototypes.write().unwrap_or_else(|poisoned| poisoned.into_inner()).set_enabled(name, enabled);
    }

    /// `health_dyncfg_register_prototype()`: the name's job, with what its first rule says of where it comes from;
    /// only a DynCfg rule's job can be removed.
    fn dyncfg_register_prototype(&self, ctx: &Ctx<'_>, name: &[u8]) {
        // copied out: the registration's echo comes back into the store
        let Some((enabled, source_type, source)) = self.prototypes().get(name).and_then(|prototype| {
            let first = prototype.rules().first()?;
            Some((prototype.enabled(), first.config.source_type, first.config.source.clone()))
        }) else {
            return;
        };
        let mut cmds = Cmds::SCHEMA | Cmds::GET | Cmds::ENABLE | Cmds::DISABLE | Cmds::UPDATE | Cmds::USERCONFIG;
        if source_type == SourceType::Dyncfg {
            cmds = cmds | Cmds::REMOVE;
        }
        ctx.nodes.add(&NodeSpec {
            id: &job_id(name),
            path: PATH,
            kind: Type::Job,
            status: if enabled { Status::Accepted } else { Status::Disabled },
            source_type,
            source: source.as_deref().unwrap_or(b""),
            cmds,
        });
    }

    /// `health_dyncfg_register_all_prototypes()` as `health_reload_prototypes()` runs it: the template, whose
    /// registration brings the saved jobs back through the callback, then the job of every name whose first rule
    /// is no DynCfg rule, in the store's order. Rules added meanwhile are not pushed to the Cloud.
    pub fn dyncfg_register_all(&self, ctx: &Ctx<'_>) {
        self.registering.store(true, Ordering::Relaxed);
        ctx.nodes.add(&NodeSpec {
            id: TEMPLATE_ID,
            path: PATH,
            kind: Type::Template,
            status: Status::Accepted,
            source_type: SourceType::Internal,
            source: b"internal",
            cmds: Cmds::SCHEMA | Cmds::ADD | Cmds::ENABLE | Cmds::DISABLE | Cmds::USERCONFIG,
        });
        let names: Vec<Vec<u8>> = self.prototypes().iter().map(|(name, _)| name.to_vec()).collect();
        for name in names {
            let from_file = self
                .prototypes()
                .get(&name)
                .and_then(|prototype| prototype.rules().first().map(|rule| rule.config.source_type != SourceType::Dyncfg));
            if from_file == Some(true) {
                self.dyncfg_register_prototype(ctx, &name);
            }
        }
        self.registering.store(false, Ordering::Relaxed);
    }

    /// `health_dyncfg_unregister_all_prototypes()`: every name's job, then the template.
    pub fn dyncfg_unregister_all(&self, ctx: &Ctx<'_>) {
        let names: Vec<Vec<u8>> = self.prototypes().iter().map(|(name, _)| name.to_vec()).collect();
        for name in names {
            ctx.nodes.del(&job_id(&name));
        }
        ctx.nodes.del(TEMPLATE_ID);
    }

    /// The hosts a DynCfg change is applied to: those whose health is enabled and ran once.
    fn dyncfg_hosts(&self, ctx: &Ctx<'_>) -> Vec<(Arc<Host>, Arc<crate::alerts::HostAlerts>)> {
        let mut hosts = Vec::new();
        for host in ctx.hosts.all() {
            if !host.info().health_enabled {
                continue;
            }
            if let Some(alerts) = self.host(&host).filter(|alerts| alerts.is_initialized()) {
                hosts.push((host, alerts));
            }
        }
        hosts
    }

    /// `health_prototype_apply_to_all_hosts()`: on every such host the alerts of the name go, and, when the name
    /// is enabled, every chart gets what the name's rules give it, on the caller's thread.
    pub fn apply_prototype_to_all_hosts(&self, ctx: &Ctx<'_>, name: &[u8]) {
        for (host, alerts) in self.dyncfg_hosts(ctx) {
            alerts.unlink_named(name, ctx.env, ctx.clock);

            let host_labels = host.labels();
            let prototypes = self.prototypes();
            let Some(prototype) = prototypes.get(name) else {
                continue;
            };
            let enabled_alerts = &self.config().enabled_alerts;
            for chart in host.charts().all() {
                let meta = chart.meta();
                let key = ChartKey {
                    id: chart.id().as_bytes(),
                    name: meta.name.as_deref().unwrap_or(chart.id()).as_bytes(),
                    context: meta.context.as_bytes(),
                    labels: Some(&meta.labels),
                };
                for (_, rule) in prototype_rules_for_chart(prototype, enabled_alerts, Some(&host_labels), &key) {
                    alerts.add(&chart, rule, ctx.env, ctx.clock);
                }
            }
        }
    }

    /// `dyncfg_health_remove_all_rrdcalc_of_prototype()`: the alerts of the name go on every such host. How many.
    pub fn remove_alerts_of_prototype(&self, ctx: &Ctx<'_>, name: &[u8]) -> usize {
        self.dyncfg_hosts(ctx).iter().map(|(_, alerts)| alerts.unlink_named(name, ctx.env, ctx.clock)).sum()
    }
}
