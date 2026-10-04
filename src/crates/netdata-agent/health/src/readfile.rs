//! The `health.d` file reader (`health_config.c` `health_readfile()`).
//!
//! A file is a list of entities, each started by an `alarm:` or `template:` line and ended by the next one or the
//! end of the file. Every other line sets one member of the entity in progress. Every record this code writes is an
//! error record with C's text; a clean file logs nothing.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;

use netdata_agent_dyncfg::model::SourceType;
use netdata_agent_eval::{Expression, strerror};
use netdata_agent_log::netdata_log_error_errno;
use netdata_agent_query::tables::options;
use netdata_agent_text::c::{c_str, is_space, set_errno, trim_all};
use netdata_agent_text::parse::str2ndd;
use netdata_agent_text::sanitize::rrdvar_fix_name;

use crate::Health;
use crate::expr::parse_logged;
use crate::keywords::{lossy, parse_db_lookup, parse_delay, parse_options, parse_repeat, parse_update_every};
use crate::prototype::Rule;
use crate::tables::{DataSource, DimsGrouping};

/// `HEALTH_CONF_MAX_LINE`: the size of C's line buffer, without its terminator.
const MAX_LINE: usize = 4096;
/// `FILENAME_MAX`: a rule's source text is cut to one byte less.
const FILENAME_MAX: usize = 4096;

/// glibc's `fgets(buffer, size, fp)` over the file's bytes: the bytes up to and including a newline, `size - 1` of
/// them at most; `None` at the end of the file. A size of 1 gives an empty read without touching the file.
fn fgets<'a>(data: &'a [u8], pos: &mut usize, size: usize) -> Option<&'a [u8]> {
    if size == 1 {
        return Some(&[]);
    }
    let rest = &data[*pos..];
    if rest.is_empty() {
        return None;
    }
    let max = rest.len().min(size - 1);
    let len = rest[..max].iter().position(|&c| c == b'\n').map_or(max, |newline| newline + 1);
    *pos += len;
    Some(&rest[..len])
}

/// The entity in progress: its rule, and the two numbers `green` and `red` name in its expressions.
struct Entity {
    rule: Rule,
    green: f64,
    red: f64,
}

impl Entity {
    /// `string2str(ac->name)`.
    fn name(&self) -> std::borrow::Cow<'_, str> {
        lossy(self.rule.config.name.as_deref().unwrap_or(b""))
    }
}

/// `strip_quotes()`: every quote becomes a space; nothing trims afterwards.
fn strip_quotes(value: &[u8]) -> Vec<u8> {
    value.iter().map(|&c| if c == b'\'' || c == b'"' { b' ' } else { c }).collect()
}

/// `replace_green_red()`, then `health_add_file_prototype()`: the entity is complete.
fn finalize(health: &Health, entity: Entity) {
    let Entity { mut rule, green, red } = entity;
    let ac = &mut rule.config;

    // lookup_data_source_from_rrdr_options(), dims_grouping_from_rrdr_options()
    ac.data_source = if ac.options & options::PERCENTAGE != 0 {
        DataSource::Percentages
    } else if ac.options & options::ANOMALY_BIT != 0 {
        DataSource::Anomalies
    } else {
        DataSource::Samples
    };
    ac.dims_group = if ac.options & options::DIMS_MIN != 0 {
        DimsGrouping::Min
    } else if ac.options & options::DIMS_MAX != 0 {
        DimsGrouping::Max
    } else if ac.options & options::DIMS_MIN2MAX != 0 {
        DimsGrouping::Min2Max
    } else if ac.options & options::DIMS_AVERAGE != 0 {
        DimsGrouping::Average
    } else {
        DimsGrouping::Sum
    };

    for (name, value) in [(&b"green"[..], green), (&b"red"[..], red)] {
        if value.is_nan() {
            continue;
        }
        for expression in [&mut ac.calculation, &mut ac.warning, &mut ac.critical].into_iter().flatten() {
            expression.hardcode_variable(name, value);
        }
    }

    // a rule that cannot stand is logged and dropped; the others of its name stay
    let _ = health.add(vec![rule]);
}

/// A text member (`PARSE_HEALTH_CONFIG_LINE_STRING`): a repeated key replaces the value and says so when it differs.
fn set_string(member: &mut Option<Vec<u8>>, value: &[u8], line: usize, filename: &[u8], name: &str, key: &[u8]) {
    if let Some(old) = member {
        if old.as_slice() != value {
            netdata_log_error_errno!(
                "Health configuration at line {line} of file '{}' for alarm '{name}' has key '{}' twice, once with value '{}' and later with value '{}'. Using ('{}').",
                lossy(filename),
                lossy(key),
                lossy(old),
                lossy(value),
                lossy(value)
            );
        }
    }
    *member = Some(value.to_vec());
}

/// A label member (`PARSE_HEALTH_CONFIG_LINE_PATTERN_APPEND`): `*` changes nothing, `!*` disables the rule, and
/// anything else goes in front of what the member holds, as `LABEL=VALUE` for the keys that name a label. True
/// when the rule is to be disabled.
fn append_pattern(
    member: &mut Option<Vec<u8>>,
    label: Option<&[u8]>,
    value: &[u8],
    line: usize,
    filename: &[u8],
    name: &str,
    key: &[u8],
) -> bool {
    if value == b"*" {
        return false;
    }
    if value == b"!* *" || value == b"!*" {
        return true;
    }
    if label.is_none() && !value.contains(&b'=') {
        netdata_log_error_errno!(
            "Health configuration at line {line} of file '{}' for alarm '{name}' has key '{}' with value '{}' that does not match label=pattern. Ignoring it.",
            lossy(filename),
            lossy(key),
            lossy(value)
        );
        return false;
    }

    let mut text = Vec::new();
    if let Some(label) = label {
        text.extend_from_slice(label);
        text.push(b'=');
    }
    text.extend_from_slice(value);
    if let Some(old) = member {
        text.push(b' ');
        text.extend_from_slice(old);
    }
    *member = Some(text);
    false
}

/// `health_readfile()`: false when the file cannot be read.
pub fn health_readfile(health: &Health, filename: &[u8], stock: bool) -> bool {
    let mut data = Vec::new();
    match File::open(OsStr::from_bytes(filename)) {
        Ok(mut opened) => {
            // A failed read ends the file there, as a failed fgets() does (a directory opens and then fails to
            // read), and leaves its errno for the next record.
            if let Err(error) = opened.read_to_end(&mut data) {
                set_errno(error.raw_os_error().unwrap_or(0));
            }
        }
        Err(error) => {
            set_errno(error.raw_os_error().unwrap_or(0));
            netdata_log_error_errno!("Health configuration cannot read file '{}'.", lossy(filename));
            return false;
        }
    }
    let file = lossy(filename);

    let mut entity: Option<Entity> = None;

    // C's buffer as a C string: what earlier reads left for a continuation, then this read
    let mut buffer: Vec<u8> = Vec::with_capacity(MAX_LINE + 1);
    let mut pos = 0;
    let mut line = 0usize;
    let mut append = 0usize;
    loop {
        let read = fgets(&data, &mut pos, MAX_LINE - append);
        if read.is_none() && append == 0 {
            break;
        }
        let stop_appending = read.is_none();
        line += 1;

        // a NUL in the file ends the read's text
        buffer.truncate(append);
        buffer.extend_from_slice(c_str(read.unwrap_or(&[])));

        // trim(buffer): from the buffer's start every time, so a comment mark counts only at the start of the whole
        // text and a `#` line inside a continuation is plain text
        let start = buffer.iter().position(|&c| !is_space(c));
        let Some(start) = start else {
            if append > 0 {
                // A pending continuation that holds only whitespace (a line that is just a backslash): C's buffer
                // now starts with a NUL that no later read overwrites, so nothing after it is seen, and at the
                // end of the file C spins on it for ever. The reading ends here instead.
                break;
            }
            continue;
        };
        let end = buffer.iter().rposition(|&c| !is_space(c)).map_or(start, |last| last + 1);
        buffer.truncate(end);
        if buffer[start] == b'#' {
            // never at the end of the file: what a pending continuation keeps is no comment (a first line that is
            // one does not continue), and the end adds nothing to it
            continue;
        }

        if !stop_appending && buffer[end - 1] == b'\\' {
            buffer[end - 1] = b' ';
            append = end;
            if append < MAX_LINE {
                continue;
            }
            // not reached, in C either: a read holds at most 4,095 bytes
            netdata_log_error_errno!("Health configuration has too long multi-line at line {line} of file '{file}'.");
        }
        append = 0;

        let s = &buffer[start..end];
        let Some(colon) = s.iter().position(|&c| c == b':') else {
            netdata_log_error_errno!(
                "Health configuration has invalid line {line} of file '{file}'. It does not contain a ':'. Ignoring it."
            );
            continue;
        };
        let key = trim_all(&s[..colon]);
        let value = trim_all(&s[colon + 1..]);
        if key.is_empty() {
            netdata_log_error_errno!("Health configuration has invalid line {line} of file '{file}'. Keyword is empty. Ignoring it.");
            continue;
        }
        if value.is_empty() {
            netdata_log_error_errno!("Health configuration has invalid line {line} of file '{file}'. value is empty. Ignoring it.");
            continue;
        }
        let (key, value) = (key.as_slice(), value.as_slice());
        let is = |name: &[u8]| key.eq_ignore_ascii_case(name);

        if is(b"alarm") || is(b"template") {
            if let Some(entity) = entity.take() {
                finalize(health, entity);
            }

            let (name, renamed) = rrdvar_fix_name(value);
            if renamed {
                netdata_log_error_errno!("Health configuration renamed alarm '{}' to '{}'", lossy(value), lossy(&name));
            }

            let mut rule = Rule::default();
            rule.config.name = (!name.is_empty()).then_some(name);
            rule.r#match.enabled = true;
            rule.r#match.is_template = is(b"template");
            let mut source = format!("line={line},file=").into_bytes();
            source.extend_from_slice(filename);
            source.truncate(FILENAME_MAX - 1);
            rule.config.source = Some(source);
            rule.config.source_type = if stock { SourceType::Stock } else { SourceType::User };
            rule.config.delay_multiplier = 1.0;
            rule.config.warn_repeat_every = health.config().default_warn_repeat_every;
            rule.config.crit_repeat_every = health.config().default_crit_repeat_every;
            entity = Some(Entity { rule, green: f64::NAN, red: f64::NAN });
            continue;
        }

        let Some(entity) = entity.as_mut() else {
            netdata_log_error_errno!(
                "Health configuration at line {line} of file '{file}' has unknown key '{}'. Expected either 'alarm' or 'template'.",
                lossy(key)
            );
            continue;
        };
        let name = entity.name().into_owned();
        let (am, ac) = (&mut entity.rule.r#match, &mut entity.rule.config);

        if is(b"on") {
            set_string(&mut am.on, value, line, filename, &name, key);
        } else if is(b"os") {
            am.enabled &= !append_pattern(&mut am.host_labels, Some(b"_os"), value, line, filename, &name, key);
        } else if is(b"hosts") {
            am.enabled &= !append_pattern(&mut am.host_labels, Some(b"_hostname"), value, line, filename, &name, key);
        } else if is(b"host labels") {
            am.enabled &= !append_pattern(&mut am.host_labels, None, value, line, filename, &name, key);
        } else if is(b"plugin") {
            am.enabled &= !append_pattern(&mut am.chart_labels, Some(b"_collect_plugin"), value, line, filename, &name, key);
        } else if is(b"module") {
            am.enabled &= !append_pattern(&mut am.chart_labels, Some(b"_collect_module"), value, line, filename, &name, key);
        } else if is(b"chart labels") {
            am.enabled &= !append_pattern(&mut am.chart_labels, None, value, line, filename, &name, key);
        } else if is(b"class") {
            set_string(&mut ac.classification, &strip_quotes(value), line, filename, &name, key);
        } else if is(b"component") {
            set_string(&mut ac.component, &strip_quotes(value), line, filename, &name, key);
        } else if is(b"type") {
            set_string(&mut ac.r#type, &strip_quotes(value), line, filename, &name, key);
        } else if is(b"lookup") {
            // its result is not looked at
            parse_db_lookup(line, filename, value, ac);
        } else if is(b"every") {
            match parse_update_every(value) {
                Some(seconds) => ac.update_every = seconds,
                None => netdata_log_error_errno!(
                    "Health configuration at line {line} of file '{file}' for alarm '{name}' at key '{}' cannot parse duration: '{}'.",
                    lossy(key),
                    lossy(value)
                ),
            }
        } else if is(b"green") || is(b"red") {
            let (number, used) = str2ndd(value);
            if used < value.len() {
                netdata_log_error_errno!(
                    "Health configuration at line {line} of file '{file}' for alarm '{name}' at key '{}' leaves this string unmatched: '{}'.",
                    lossy(key),
                    lossy(&value[used..])
                );
            }
            if is(b"green") {
                entity.green = number;
            } else {
                entity.red = number;
            }
        } else if is(b"calc") || is(b"warn") || is(b"crit") {
            let parsed: Option<Expression> = match parse_logged(value) {
                Ok(expression) => Some(expression),
                Err(error) => {
                    netdata_log_error_errno!(
                        "Health configuration at line {line} of file '{file}' for alarm '{name}' at key '{}' has non-parseable expression '{}': {} at '{}'",
                        lossy(key),
                        lossy(value),
                        strerror(error.code().unwrap_or(0)),
                        lossy(&value[error.failed_at().unwrap_or(0).min(value.len())..])
                    );
                    am.enabled = false;
                    None
                }
            };
            if is(b"calc") {
                ac.calculation = parsed;
            } else if is(b"warn") {
                ac.warning = parsed;
            } else {
                ac.critical = parsed;
            }
        } else if is(b"exec") {
            set_string(&mut ac.exec, &strip_quotes(value), line, filename, &name, key);
        } else if is(b"to") {
            set_string(&mut ac.recipient, &strip_quotes(value), line, filename, &name, key);
        } else if is(b"units") {
            set_string(&mut ac.units, &strip_quotes(value), line, filename, &name, key);
        } else if is(b"summary") {
            set_string(&mut ac.summary, &strip_quotes(value), line, filename, &name, key);
        } else if is(b"info") {
            set_string(&mut ac.info, &strip_quotes(value), line, filename, &name, key);
        } else if is(b"delay") {
            parse_delay(
                line,
                filename,
                value,
                &mut ac.delay_up_duration,
                &mut ac.delay_down_duration,
                &mut ac.delay_max_duration,
                &mut ac.delay_multiplier,
            );
        } else if is(b"options") {
            ac.alert_action_options |= parse_options(value);
        } else if is(b"repeat") {
            parse_repeat(line, filename, value, &mut ac.warn_repeat_every, &mut ac.crit_repeat_every);
            ac.has_custom_repeat_config = true;
        } else if key != b"families" && key != b"charts" {
            netdata_log_error_errno!(
                "Health configuration at line {line} of file '{file}' for alarm/template '{name}' has unknown key '{}'.",
                lossy(key)
            );
        }
    }

    if let Some(entity) = entity {
        finalize(health, entity);
    }
    true
}

#[cfg(test)]
mod tests {
    use netdata_agent_log::capture;

    use super::*;
    use crate::config::HealthConfig;

    const FIRST: &str = "template: first\non: system.cpu\nevery: 10s\ncalc: 1\n";
    const SECOND: &str = "template: second\non: system.cpu\nevery: 10s\ncalc: 1\n";

    /// Reads `content` from a file: the return, the names stored, the number of records.
    fn read(content: &str) -> (bool, Vec<String>, usize) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("test.conf");
        std::fs::write(&path, content).expect("the file");
        let health = Health::init(HealthConfig::default(), Box::new(|_| {}), false);
        let (read, records) = capture(|| health_readfile(&health, path.as_os_str().as_bytes(), false));
        let names = health.prototypes().iter().map(|(name, _)| String::from_utf8_lossy(name).into_owned()).collect();
        (read, names, records.len())
    }

    /// C never returns from such a file: after a line that is only a backslash, a blank line (or the end of the
    /// file) leaves its buffer empty with the continuation pending, and it reads on without seeing anything, for
    /// ever once the file ends. Here the file ends at that point: the rule in progress is kept.
    #[test]
    fn a_lone_backslash_before_a_blank_line_ends_the_file() {
        let both = (true, vec!["first".to_owned(), "second".to_owned()], 0);
        let first_only = (true, vec!["first".to_owned()], 0);
        assert_eq!(read(&format!("{FIRST}\n{SECOND}")), both);
        assert_eq!(read(&format!("{FIRST}\\\n\n{SECOND}")), first_only);
        assert_eq!(read(&format!("{FIRST}\\\n  \t\n{SECOND}")), first_only);
        assert_eq!(read(&format!("{FIRST}\\\n")), first_only);
        assert_eq!(read(&format!("{FIRST}\\")), first_only);
        // comment lines after it do not help: nothing but text ends the pending continuation
        assert_eq!(read(&format!("{FIRST}\\\n# a comment\n# another\n")), first_only);
        assert_eq!(read(&format!("{FIRST}\\\n# a comment\n\n{SECOND}")), first_only);
        // a line of 4,096 bytes that ends in a backslash: a read takes 4,095, and the backslash is a line alone
        let long = format!(" info: {}\\\n", "A".repeat(4095 - " info: ".len()));
        assert_eq!(long.len(), 4097);
        assert_eq!(read(&format!("{FIRST}{long}")), first_only);
        assert_eq!(read(&format!("{FIRST}{long}\n{SECOND}")), first_only);
        // one byte shorter and the backslash is the line's own: an ordinary continuation
        let fits = format!(" info: {}\\\n units: u\n\n", "A".repeat(4094 - " info: ".len()));
        assert_eq!(read(&format!("{FIRST}{fits}{SECOND}")), both);
        // followed by text, the backslash is an ordinary continuation
        assert_eq!(read(&format!("{FIRST}\\\ninfo: x\n{SECOND}")), both);
    }
}
