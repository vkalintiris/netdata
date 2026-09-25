//! The netdata.conf engine, ported from `src/libnetdata/inicfg/`.
//!
//! There is no schema: every typed getter creates the option if it is missing, records the first default it is
//! asked with, and marks it used. The effective configuration is therefore the union of what the code read, and
//! [`Config::generate`] (served as `/netdata.conf`) reproduces the C output byte for byte, including which options
//! are commented out and the `#|` annotations.
//!
//! Names and values are bytes, as in C. The engine logs through `netdata-agent-log` where C does, on the daemon
//! source.

#![forbid(unsafe_code)]

use std::path::Path;

use netdata_agent_log::{Priority, Source, errno_of, nd_log, netdata_log_error};

use netdata_agent_text::c::{c_str, eq_ignore_case, is_space};
use netdata_agent_text::duration::{duration_parse, duration_parse_seconds, duration_to_string};
use netdata_agent_text::parse::{str2ndd, strtoll0, uuid_parse_flexi};
use netdata_agent_text::size::{size_parse, size_to_string};

pub const SECTION_GLOBAL: &str = "global";
pub const SECTION_DIRECTORIES: &str = "directories";
pub const SECTION_LOGS: &str = "logs";
pub const SECTION_ENV_VARS: &str = "environment variables";
pub const SECTION_SQLITE: &str = "sqlite";
pub const SECTION_WEB: &str = "web";
pub const SECTION_WEBRTC: &str = "webrtc";
pub const SECTION_STATSD: &str = "statsd";
pub const SECTION_PLUGINS: &str = "plugins";
pub const SECTION_CLOUD: &str = "cloud";
pub const SECTION_REGISTRY: &str = "registry";
pub const SECTION_HEALTH: &str = "health";
pub const SECTION_STREAM: &str = "stream";
pub const SECTION_ML: &str = "ml";
pub const SECTION_EXPORTING: &str = "exporting:global";
pub const SECTION_PROMETHEUS: &str = "prometheus:exporter";
pub const SECTION_HOST_LABEL: &str = "host labels";
pub const SECTION_PULSE: &str = "pulse";
pub const SECTION_DB: &str = "db";
/// `EXPORTING_CONF`: a file whose name contains it is loaded with the exporting-connector rules.
pub const EXPORTING_CONF: &str = "exporting.conf";

/// `CONFIG_FILE_LINE_MAX`: `fgets()` reads at most this minus one byte per call, so longer lines are split.
const FILE_LINE_MAX: usize = 8192;

/// `CONFIG_BOOLEAN_*` values of the yes/no/auto getter.
pub const BOOLEAN_NO: i32 = 0;
pub const BOOLEAN_YES: i32 = 1;
pub const BOOLEAN_AUTO: i32 = 2;

/// `CONFIG_VALUE_TYPES`, as shown in the `#| datatype:` annotation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ValueType {
    #[default]
    Unknown,
    Text,
    Hostname,
    Username,
    Filename,
    Path,
    SimplePattern,
    Url,
    Enum,
    Bitmap,
    Integer,
    Double,
    Boolean,
    BooleanOndemand,
    DurationInSecs,
    DurationInMs,
    DurationInDaysToSeconds,
    SizeInBytes,
    SizeInMb,
}

impl ValueType {
    /// `CONFIG_VALUE_TYPES_2str()`.
    pub fn name(self) -> &'static str {
        match self {
            ValueType::Unknown => "unknown",
            ValueType::Text => "text",
            ValueType::Hostname => "hostname",
            ValueType::Username => "username",
            ValueType::Filename => "filename",
            ValueType::Path => "path",
            ValueType::SimplePattern => "simple pattern",
            ValueType::Url => "URL",
            ValueType::Enum => "one of keywords",
            ValueType::Bitmap => "any of keywords",
            ValueType::Integer => "number (integer)",
            ValueType::Double => "number (double)",
            ValueType::Boolean => "yes or no",
            ValueType::BooleanOndemand => "yes, no, or auto",
            ValueType::DurationInSecs => "duration (seconds)",
            ValueType::DurationInMs => "duration (ms)",
            ValueType::DurationInDaysToSeconds => "duration (days)",
            ValueType::SizeInBytes => "size (bytes)",
            ValueType::SizeInMb => "size (MiB)",
        }
    }
}

// CONFIG_VALUE_FLAGS
const LOADED: u8 = 1 << 0;
const USED: u8 = 1 << 1;
const CHANGED: u8 = 1 << 2;
const CHECKED: u8 = 1 << 3;
const MIGRATED: u8 = 1 << 4;
const REFORMATTED: u8 = 1 << 5;
const DEFAULT_SET: u8 = 1 << 6;

#[derive(Debug, Clone)]
struct Opt {
    name: Vec<u8>,
    value: Vec<u8>,
    /// The first value the option got, however it got it.
    value_original: Vec<u8>,
    /// The internal default: the first default a getter asked with.
    value_default: Option<Vec<u8>>,
    /// Section and name before the first migration.
    migrated: Option<(Vec<u8>, Vec<u8>)>,
    ty: ValueType,
    flags: u8,
}

impl Opt {
    fn new(name: &[u8], value: &[u8]) -> Self {
        Opt {
            name: name.to_vec(),
            value: value.to_vec(),
            value_original: value.to_vec(),
            value_default: None,
            migrated: None,
            ty: ValueType::Unknown,
            flags: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct Section {
    name: Vec<u8>,
    /// In list order (generation order).
    options: Vec<Opt>,
}

impl Section {
    fn find(&self, name: &[u8]) -> Option<usize> {
        self.options.iter().position(|o| o.name == name)
    }
}

/// The errno a caller's "cannot load" line carries after [`Config::load`] failed: ENOENT for a missing file; any
/// other failure was logged by the load itself, which clears errno as every C log call does.
pub fn load_errno(err: &std::io::Error) -> i32 {
    if err.kind() == std::io::ErrorKind::NotFound {
        errno_of(err)
    } else {
        0
    }
}

/// A reformat callback: returns the reformatted value when it differs.
type Reformat = fn(&[u8]) -> Option<Vec<u8>>;

/// One INI configuration (`struct config`): netdata.conf, stream.conf, cloud.conf, ...
#[derive(Debug, Clone, Default)]
pub struct Config {
    sections: Vec<Section>,
    /// `add_connector_instance()`: (connector, instance) section names from exporting.conf, newest first.
    connector_instances: Vec<(Vec<u8>, Vec<u8>)>,
}

/// `is_valid_connector()`'s list of exporting connector types.
const CONNECTOR_TYPES: [&[u8]; 21] = [
    b"graphite",
    b"graphite:plaintext",
    b"graphite:http",
    b"graphite:https",
    b"json",
    b"json:plaintext",
    b"json:http",
    b"json:https",
    b"opentsdb",
    b"opentsdb:telnet",
    b"opentsdb:http",
    b"opentsdb:https",
    b"prometheus_remote_write",
    b"prometheus_remote_write:http",
    b"prometheus_remote_write:https",
    b"kinesis",
    b"kinesis:plaintext",
    b"pubsub",
    b"pubsub:plaintext",
    b"mongodb",
    b"mongodb:plaintext",
];

/// `CONFIG_MAX_NAME`.
const CONFIG_MAX_NAME: usize = 1024;

/// `is_valid_connector(name, 0)`: the offset of the last `:` when the text before it is a connector type and the
/// whole name is not itself one (a reserved name).
fn valid_connector(name: &[u8]) -> Option<usize> {
    if CONNECTOR_TYPES.contains(&name) {
        return None;
    }
    let separator = name.iter().rposition(|&c| c == b':')?;
    (separator > 0 && CONNECTOR_TYPES.contains(&&name[..separator])).then_some(separator)
}

/// `trim()`: leading and trailing `isspace()` removed; `None` when nothing remains.
fn trim(s: &[u8]) -> Option<&[u8]> {
    let s = c_str(s);
    let start = s.iter().position(|&c| !is_space(c))?;
    let end = s
        .iter()
        .rposition(|&c| !is_space(c))
        .map_or(start, |e| e + 1);
    Some(&s[start..end])
}

/// `inicfg_test_boolean_value()`.
pub fn test_boolean_value(s: &[u8]) -> bool {
    ["yes", "true", "on", "auto", "on demand"]
        .iter()
        .any(|word| eq_ignore_case(s, word.as_bytes()))
}

fn lossy(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

fn duration_seconds_text(value: i64) -> Vec<u8> {
    duration_to_string(value, "s", false)
        .unwrap_or_default()
        .into_bytes()
}

fn duration_ms_text(value: i64) -> Vec<u8> {
    duration_to_string(value, "ms", false)
        .unwrap_or_default()
        .into_bytes()
}

fn size_text(value: u64, unit: &str) -> Vec<u8> {
    size_to_string(value, unit, true)
        .unwrap_or_default()
        .into_bytes()
}

fn reformat_duration_seconds(value: &[u8]) -> Option<Vec<u8>> {
    let parsed = duration_parse_seconds(value)?;
    let text = duration_to_string(i64::from(parsed), "s", false)?;
    (text.as_bytes() != value).then(|| text.into_bytes())
}

fn reformat_duration_ms(value: &[u8]) -> Option<Vec<u8>> {
    let parsed = duration_parse(value, "ms", "ms")?;
    let text = duration_to_string(parsed, "ms", false)?;
    (text.as_bytes() != value).then(|| text.into_bytes())
}

fn reformat_duration_days_to_seconds(value: &[u8]) -> Option<Vec<u8>> {
    let parsed = duration_parse(value, "d", "s")?;
    let text = duration_to_string(parsed, "s", false)?;
    (text.as_bytes() != value).then(|| text.into_bytes())
}

fn reformat_size(value: &[u8], unit: &str) -> Option<Vec<u8>> {
    let parsed = size_parse(value, unit)?;
    let text = size_to_string(parsed, unit, true)?;
    (text.as_bytes() != value).then(|| text.into_bytes())
}

fn reformat_size_bytes(value: &[u8]) -> Option<Vec<u8>> {
    reformat_size(value, "B")
}

fn reformat_size_mb(value: &[u8]) -> Option<Vec<u8>> {
    reformat_size(value, "MiB")
}

/// `snprintf("%0.5f")`, falling back to `"%0.19e"` when that does not fit the 100-byte buffer.
fn double_text(value: f64) -> Vec<u8> {
    if value.is_nan() {
        return if value.is_sign_negative() {
            b"-nan".to_vec()
        } else {
            b"nan".to_vec()
        };
    }
    if value.is_infinite() {
        return if value < 0.0 {
            b"-inf".to_vec()
        } else {
            b"inf".to_vec()
        };
    }
    let fixed = format!("{value:.5}");
    if fixed.len() < 100 {
        return fixed.into_bytes();
    }
    // C writes the exponent with a sign and at least two digits.
    let sci = format!("{value:.19e}");
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let (sign, digits) = match exponent.strip_prefix('-') {
        Some(d) => ('-', d),
        None => ('+', exponent),
    };
    format!("{mantissa}e{sign}{digits:0>2}").into_bytes()
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }

    fn section_index(&self, name: &[u8]) -> Option<usize> {
        self.sections.iter().position(|s| s.name == name)
    }

    fn section_create(&mut self, name: &[u8]) -> usize {
        self.sections.push(Section {
            name: name.to_vec(),
            options: Vec::new(),
        });
        self.sections.len() - 1
    }

    fn section_find_or_create(&mut self, name: &[u8]) -> usize {
        self.section_index(name)
            .unwrap_or_else(|| self.section_create(name))
    }

    /// `inicfg_get_raw_value_of_option()` applied to an existing option.
    fn touch_get(opt: &mut Opt, default: Option<&[u8]>, ty: ValueType, reformat: Option<Reformat>) {
        opt.flags |= USED;
        if ty != ValueType::Unknown {
            opt.ty = ty;
        }
        if opt.flags & (LOADED | CHANGED) != 0 && opt.flags & CHECKED == 0 {
            if opt.flags & REFORMATTED == 0 {
                if let Some(reformatted) = reformat.and_then(|cb| cb(&opt.value)) {
                    opt.value = reformatted;
                    opt.flags |= REFORMATTED;
                }
            }
            if let Some(default) = default {
                if opt.value != default {
                    opt.flags |= CHANGED;
                }
            }
            opt.flags |= CHECKED;
        }
        if opt.flags & DEFAULT_SET == 0 {
            opt.flags |= DEFAULT_SET;
            // string_strdupz("") is NULL: an empty default is no default.
            opt.value_default = default.filter(|d| !d.is_empty()).map(<[u8]>::to_vec);
        }
    }

    /// `inicfg_get_raw_value_of_option_in_section()`: `(section, option)` indices, or `None` when missing and no
    /// default was given.
    fn get_raw_in(
        &mut self,
        section: usize,
        name: &[u8],
        default: Option<&[u8]>,
        ty: ValueType,
        reformat: Option<Reformat>,
    ) -> Option<(usize, usize)> {
        let sect = &mut self.sections[section];
        let index = match sect.find(name) {
            Some(i) => i,
            None => {
                let default = default?;
                sect.options.push(Opt::new(name, default));
                sect.options.len() - 1
            }
        };
        Self::touch_get(&mut sect.options[index], default, ty, reformat);
        Some((section, index))
    }

    /// `inicfg_get_raw_value()`.
    fn get_raw(
        &mut self,
        section: &str,
        name: &str,
        default: Option<&[u8]>,
        ty: ValueType,
        reformat: Option<Reformat>,
    ) -> Option<&[u8]> {
        let sect = match self.section_index(section.as_bytes()) {
            Some(s) => s,
            None => {
                default?;
                self.section_create(section.as_bytes())
            }
        };
        let (s, o) = self.get_raw_in(sect, name.as_bytes(), default, ty, reformat)?;
        Some(&self.sections[s].options[o].value)
    }

    /// `inicfg_set_raw_value()`: returns the option's `(section, option)` indices.
    fn set_raw(
        &mut self,
        section: &str,
        name: &str,
        value: &[u8],
        ty: ValueType,
    ) -> (usize, usize) {
        let s = self.section_find_or_create(section.as_bytes());
        let sect = &mut self.sections[s];
        let o = match sect.find(name.as_bytes()) {
            Some(o) => o,
            None => {
                sect.options.push(Opt::new(name.as_bytes(), value));
                sect.options.len() - 1
            }
        };
        let opt = &mut sect.options[o];
        opt.flags |= USED;
        if opt.ty == ValueType::Unknown {
            opt.ty = ty;
        }
        if opt.value != value {
            opt.flags |= CHANGED;
            opt.value = value.to_vec();
        }
        (s, o)
    }

    /// `inicfg_get()`; `None` only when the option is missing and no default was given.
    pub fn get(&mut self, section: &str, name: &str, default: Option<&str>) -> Option<Vec<u8>> {
        self.get_raw(
            section,
            name,
            default.map(str::as_bytes),
            ValueType::Text,
            None,
        )
        .map(<[u8]>::to_vec)
    }

    /// `inicfg_set()`.
    pub fn set(&mut self, section: &str, name: &str, value: &str) -> Vec<u8> {
        let (s, o) = self.set_raw(section, name, value.as_bytes(), ValueType::Text);
        self.sections[s].options[o].value.clone()
    }

    /// `inicfg_get_filename()`: no reformatting on Linux.
    pub fn get_filename(
        &mut self,
        section: &str,
        name: &str,
        default: Option<&str>,
    ) -> Option<Vec<u8>> {
        self.get_raw(
            section,
            name,
            default.map(str::as_bytes),
            ValueType::Filename,
            None,
        )
        .map(<[u8]>::to_vec)
    }

    /// `inicfg_get_path()`: no reformatting on Linux.
    pub fn get_path(
        &mut self,
        section: &str,
        name: &str,
        default: Option<&str>,
    ) -> Option<Vec<u8>> {
        self.get_raw(
            section,
            name,
            default.map(str::as_bytes),
            ValueType::Path,
            None,
        )
        .map(<[u8]>::to_vec)
    }

    /// `inicfg_get_path_list()`, `inicfg_get_quoted_path_list()` and `inicfg_get_log_path_setting()`: text options
    /// whose reformatting only exists on Windows.
    pub fn get_path_list(
        &mut self,
        section: &str,
        name: &str,
        default: Option<&str>,
    ) -> Option<Vec<u8>> {
        self.get(section, name, default)
    }

    /// `inicfg_get_boolean()`.
    pub fn get_boolean(&mut self, section: &str, name: &str, value: bool) -> bool {
        let default: &[u8] = if value { b"yes" } else { b"no" };
        match self.get_raw(section, name, Some(default), ValueType::Boolean, None) {
            Some(v) => test_boolean_value(v),
            None => value,
        }
    }

    /// `inicfg_get_boolean_ondemand()`: [`BOOLEAN_NO`], [`BOOLEAN_YES`] or [`BOOLEAN_AUTO`]; case-sensitive, and
    /// unknown text returns the default.
    pub fn get_boolean_ondemand(&mut self, section: &str, name: &str, value: i32) -> i32 {
        let default: &[u8] = match value {
            BOOLEAN_AUTO => b"auto",
            BOOLEAN_NO => b"no",
            _ => b"yes",
        };
        let Some(v) = self.get_raw(
            section,
            name,
            Some(default),
            ValueType::BooleanOndemand,
            None,
        ) else {
            return value;
        };
        match v {
            b"yes" | b"true" | b"on" => BOOLEAN_YES,
            b"no" | b"false" | b"off" => BOOLEAN_NO,
            b"auto" | b"on demand" => BOOLEAN_AUTO,
            _ => value,
        }
    }

    /// `inicfg_set_boolean()`.
    pub fn set_boolean(&mut self, section: &str, name: &str, value: bool) -> bool {
        self.set_raw(
            section,
            name,
            if value { b"yes" } else { b"no" },
            ValueType::Boolean,
        );
        value
    }

    fn invalid(&mut self, kind: &str, section: &str, name: &str, value: &[u8]) {
        netdata_log_error!(
            "config option '[{section}].{name} = {}' is configured with an invalid {kind}",
            lossy(value)
        );
    }

    /// `inicfg_get_duration_seconds()`: the absolute value; invalid text is replaced by the default.
    pub fn get_duration_seconds(&mut self, section: &str, name: &str, default: i64) -> i64 {
        let default_text = duration_seconds_text(default);
        let Some(v) = self.get_raw(
            section,
            name,
            Some(&default_text),
            ValueType::DurationInSecs,
            Some(reformat_duration_seconds),
        ) else {
            return default;
        };
        let v = v.to_vec();
        match duration_parse_seconds(&v) {
            Some(parsed) => i64::from(parsed).abs(),
            None => {
                self.invalid("duration", section, name, &v);
                self.set_raw(section, name, &default_text, ValueType::DurationInSecs);
                default
            }
        }
    }

    /// `inicfg_set_duration_seconds()`.
    pub fn set_duration_seconds(&mut self, section: &str, name: &str, value: i64) -> i64 {
        self.set_raw(
            section,
            name,
            &duration_seconds_text(value),
            ValueType::DurationInSecs,
        );
        value
    }

    /// `inicfg_get_duration_ms()`: the absolute value; invalid text is replaced by the default.
    pub fn get_duration_ms(&mut self, section: &str, name: &str, default: u64) -> u64 {
        let default_text = duration_ms_text(default as i64);
        let Some(v) = self.get_raw(
            section,
            name,
            Some(&default_text),
            ValueType::DurationInMs,
            Some(reformat_duration_ms),
        ) else {
            return default;
        };
        let v = v.to_vec();
        match duration_parse(&v, "ms", "ms") {
            Some(parsed) => parsed.unsigned_abs(),
            None => {
                self.invalid("duration", section, name, &v);
                self.set_raw(section, name, &default_text, ValueType::DurationInMs);
                default
            }
        }
    }

    /// `inicfg_set_duration_ms()`.
    pub fn set_duration_ms(&mut self, section: &str, name: &str, value: u64) -> u64 {
        self.set_raw(
            section,
            name,
            &duration_ms_text(value as i64),
            ValueType::DurationInMs,
        );
        value
    }

    /// `inicfg_get_duration_days_to_seconds()`: values without a unit are days; the result is seconds.
    pub fn get_duration_days_to_seconds(
        &mut self,
        section: &str,
        name: &str,
        default_seconds: u32,
    ) -> i64 {
        let default_text = duration_seconds_text(i64::from(default_seconds));
        let Some(v) = self.get_raw(
            section,
            name,
            Some(&default_text),
            ValueType::DurationInDaysToSeconds,
            Some(reformat_duration_days_to_seconds),
        ) else {
            return i64::from(default_seconds);
        };
        let v = v.to_vec();
        match duration_parse(&v, "d", "s") {
            Some(parsed) => parsed.wrapping_abs(),
            None => {
                self.invalid("duration", section, name, &v);
                self.set_raw(
                    section,
                    name,
                    &default_text,
                    ValueType::DurationInDaysToSeconds,
                );
                i64::from(default_seconds)
            }
        }
    }

    /// `inicfg_get_number()`: `strtoll(value, NULL, 0)`, so hex, octal and garbage (0) are accepted.
    pub fn get_number(&mut self, section: &str, name: &str, value: i64) -> i64 {
        let default = value.to_string();
        match self.get_raw(
            section,
            name,
            Some(default.as_bytes()),
            ValueType::Integer,
            None,
        ) {
            Some(v) => strtoll0(v).0,
            None => value,
        }
    }

    /// `inicfg_get_number_range()`: out-of-range values are clamped, logged and written back.
    pub fn get_number_range(
        &mut self,
        section: &str,
        name: &str,
        value: i64,
        min: i64,
        max: i64,
    ) -> i64 {
        let default = value.to_string();
        let Some(v) = self.get_raw(
            section,
            name,
            Some(default.as_bytes()),
            ValueType::Integer,
            None,
        ) else {
            return value;
        };
        let rc = strtoll0(v).0;
        let clamped = rc.clamp(min, max);
        if rc != clamped {
            netdata_log_error!(
                "CONFIG: out of range [{section}].{name} = {rc}. Acceptable values: {min} to {max} inclusive. Setting it to {clamped}"
            );
            self.set_number(section, name, clamped);
        }
        clamped
    }

    /// `inicfg_set_number()`.
    pub fn set_number(&mut self, section: &str, name: &str, value: i64) -> i64 {
        self.set_raw(
            section,
            name,
            value.to_string().as_bytes(),
            ValueType::Integer,
        );
        value
    }

    /// `inicfg_get_double()`: parsed with `str2ndd()`.
    pub fn get_double(&mut self, section: &str, name: &str, value: f64) -> f64 {
        let default = double_text(value);
        match self.get_raw(section, name, Some(&default), ValueType::Double, None) {
            Some(v) => str2ndd(v).0,
            None => value,
        }
    }

    /// `inicfg_set_double()`.
    pub fn set_double(&mut self, section: &str, name: &str, value: f64) -> f64 {
        self.set_raw(section, name, &double_text(value), ValueType::Double);
        value
    }

    fn get_size(
        &mut self,
        section: &str,
        name: &str,
        default: u64,
        unit: &str,
        ty: ValueType,
        reformat: Reformat,
    ) -> u64 {
        let default_text = size_text(default, unit);
        let Some(v) = self.get_raw(section, name, Some(&default_text), ty, Some(reformat)) else {
            return default;
        };
        let v = v.to_vec();
        match size_parse(&v, unit) {
            Some(parsed) => parsed,
            None => {
                self.invalid("size", section, name, &v);
                self.set_raw(section, name, &default_text, ty);
                default
            }
        }
    }

    /// `inicfg_get_size_bytes()`.
    pub fn get_size_bytes(&mut self, section: &str, name: &str, default: u64) -> u64 {
        self.get_size(
            section,
            name,
            default,
            "B",
            ValueType::SizeInBytes,
            reformat_size_bytes,
        )
    }

    /// `inicfg_set_size_bytes()`.
    pub fn set_size_bytes(&mut self, section: &str, name: &str, value: u64) -> u64 {
        self.set_raw(
            section,
            name,
            &size_text(value, "B"),
            ValueType::SizeInBytes,
        );
        value
    }

    /// `inicfg_get_size_mb()`.
    pub fn get_size_mb(&mut self, section: &str, name: &str, default: u64) -> u64 {
        self.get_size(
            section,
            name,
            default,
            "MiB",
            ValueType::SizeInMb,
            reformat_size_mb,
        )
    }

    /// `inicfg_set_size_mb()`.
    pub fn set_size_mb(&mut self, section: &str, name: &str, value: u64) -> u64 {
        self.set_raw(section, name, &size_text(value, "MiB"), ValueType::SizeInMb);
        value
    }

    /// `inicfg_exists()`: does not mark the option used.
    pub fn exists(&self, section: &str, name: &str) -> bool {
        self.section_index(section.as_bytes())
            .is_some_and(|s| self.sections[s].find(name.as_bytes()).is_some())
    }

    /// `inicfg_set_default_raw_value()`: sets the value unless it came from the file.
    pub fn set_default_raw_value(&mut self, section: &str, name: &str, value: &str) {
        let found = self
            .section_index(section.as_bytes())
            .and_then(|s| self.sections[s].find(name.as_bytes()).map(|o| (s, o)));
        let Some((s, o)) = found else {
            self.set_raw(section, name, value.as_bytes(), ValueType::Unknown);
            return;
        };
        let opt = &mut self.sections[s].options[o];
        opt.flags |= USED;
        if opt.flags & LOADED != 0 {
            return;
        }
        if opt.value != value.as_bytes() {
            opt.flags |= CHANGED;
            opt.value = value.as_bytes().to_vec();
        }
    }

    /// `inicfg_foreach_value_in_section()`: options the callback accepts are marked used; returns their count.
    pub fn foreach_value_in_section(
        &mut self,
        section: &str,
        mut cb: impl FnMut(&[u8], &[u8]) -> bool,
    ) -> usize {
        let Some(s) = self.section_index(section.as_bytes()) else {
            return 0;
        };
        let mut used = 0;
        for opt in &mut self.sections[s].options {
            if cb(&opt.name, &opt.value) {
                opt.flags |= USED;
                used += 1;
            }
        }
        used
    }

    /// `inicfg_move()`: renames an option (possibly into another section) if the old one exists and the new one does
    /// not. Returns whether it moved. A moved option keeps its place within the same section; into another section
    /// it goes after the last migrated option found scanning back from the tail, or to the top.
    pub fn move_option(
        &mut self,
        section_old: &str,
        name_old: &str,
        section_new: &str,
        name_new: &str,
    ) -> bool {
        let Some(s_old) = self.section_index(section_old.as_bytes()) else {
            return false;
        };
        let s_new = self.section_find_or_create(section_new.as_bytes());
        let Some(o_old) = self.sections[s_old].find(name_old.as_bytes()) else {
            return false;
        };
        if self.sections[s_new].find(name_new.as_bytes()).is_some() {
            return false;
        }

        let same = s_old == s_new;
        let had_next = o_old + 1 < self.sections[s_old].options.len();
        let old_section_name = self.sections[s_old].name.clone();
        let mut opt = self.sections[s_old].options.remove(o_old);
        if opt.migrated.is_none() {
            opt.migrated = Some((old_section_name, opt.name.clone()));
        }
        opt.name = name_new.as_bytes().to_vec();
        opt.flags |= MIGRATED;

        let options = &mut self.sections[s_new].options;
        if same && had_next {
            // Before its old successor, i.e. where it was.
            options.insert(o_old, opt);
        } else {
            // Scan back from the tail for a migrated option; the head itself is never examined.
            let mut t = options.len();
            let mut found = None;
            while t > 1 {
                t -= 1;
                if options[t].flags & MIGRATED != 0 {
                    found = Some(t);
                    break;
                }
            }
            match found {
                Some(t) => options.insert(t + 1, opt),
                None => options.insert(0, opt),
            }
        }
        true
    }

    /// `inicfg_move_everywhere()`: renames an option within every section that has it.
    pub fn move_everywhere(&mut self, name_old: &str, name_new: &str) -> bool {
        let names: Vec<String> = self.sections.iter().map(|s| lossy(&s.name)).collect();
        let mut moved = false;
        for name in names {
            moved |= self.move_option(&name, name_old, &name, name_new);
        }
        moved
    }

    /// `inicfg_section_destroy_non_loaded()`: removes a section none of whose options came from the file.
    pub fn section_destroy_non_loaded(&mut self, section: &str) {
        let Some(s) = self.section_index(section.as_bytes()) else {
            netdata_log_error!("Could not destroy section '{section}'. Not found.");
            return;
        };
        if self.sections[s]
            .options
            .iter()
            .all(|o| o.flags & LOADED == 0)
        {
            self.sections.remove(s);
        }
    }

    /// `inicfg_section_option_destroy_non_loaded()`.
    pub fn section_option_destroy_non_loaded(&mut self, section: &str, name: &str) {
        let Some(s) = self.section_index(section.as_bytes()) else {
            netdata_log_error!(
                "Could not destroy section option '{section} -> {name}'. The section not found."
            );
            return;
        };
        match self.sections[s].find(name.as_bytes()) {
            Some(o) if self.sections[s].options[o].flags & LOADED != 0 => {}
            Some(o) => {
                self.sections[s].options.remove(o);
            }
            None => {
                netdata_log_error!(
                    "Could not destroy section option '{section} -> {name}'. The option not found."
                );
            }
        }
    }

    /// `stream_conf_needs_dbengine()`: some enabled receiving section other than `[stream]` stores in dbengine.
    /// Reading `enabled` and `db` marks them used, as in C.
    pub fn stream_conf_needs_dbengine(&mut self) -> bool {
        for s in 0..self.sections.len() {
            if self.sections[s].name == SECTION_STREAM.as_bytes() {
                continue;
            }
            let Some((_, o)) = self.get_raw_in(s, b"enabled", None, ValueType::Unknown, None)
            else {
                continue;
            };
            if !test_boolean_value(&self.sections[s].options[o].value) {
                continue;
            }
            if let Some((_, o)) = self.get_raw_in(s, b"db", None, ValueType::Unknown, None) {
                if self.sections[s].options[o].value == b"dbengine" {
                    return true;
                }
            }
        }
        false
    }

    /// `stream_conf_has_api_enabled()`: a section named by a UUID, of type `api` (or untyped), is enabled.
    pub fn stream_conf_has_api_enabled(&self) -> bool {
        self.sections.iter().any(|sect| {
            // uuid_parse() is uuid_parse_flexi() in Netdata (libnetdata/uuid/uuid.h).
            if uuid_parse_flexi(&sect.name).is_none() {
                return false;
            }
            if let Some(o) = sect.find(b"type") {
                if sect.options[o].value != b"api" {
                    return false;
                }
            }
            sect.find(b"enabled")
                .is_some_and(|o| test_boolean_value(&sect.options[o].value))
        })
    }

    /// `inicfg_load()` from a file. Fails with the open error when the file cannot be opened; C logs it here unless
    /// the file is missing, and its callers' own lines then carry the errno only for a missing file (see
    /// [`load_errno`]). A read error ends the file where it happened, as `fgets()` does: a directory opens and loads
    /// nothing.
    pub fn load(
        &mut self,
        path: &Path,
        overwrite_used: bool,
        only_section: Option<&str>,
    ) -> std::io::Result<()> {
        use std::io::Read;
        match std::fs::File::open(path) {
            Ok(mut file) => {
                let mut content = Vec::new();
                let _ = file.read_to_end(&mut content);
                let name = path.to_string_lossy().into_owned();
                self.load_bytes(&content, &name, overwrite_used, only_section);
                Ok(())
            }
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    nd_log!(Source::Daemon, Priority::Info, errno = errno_of(&err);
                        "CONFIG: cannot open file '{}'. Using internal defaults.", path.to_string_lossy());
                }
                Err(err)
            }
        }
    }

    /// The exporting connector instances loaded so far (`add_connector_instance(NULL, NULL)`), newest first.
    pub fn connector_instances(&self) -> &[(Vec<u8>, Vec<u8>)] {
        &self.connector_instances
    }

    /// `inicfg_load()` over the file's bytes; `filename` is used for messages and the exporting rules.
    ///
    /// With `overwrite_used`, values the program already read are replaced (C passes it for `-c`); with
    /// `only_section`, only that section is loaded and its existing options are dropped first. A filename containing
    /// `exporting.conf` turns on the connector rules: sections other than `[exporting:global]` and
    /// `[prometheus:exporter]` must be `<connector type>:<instance>` and are loaded under the instance name.
    pub fn load_bytes(
        &mut self,
        content: &[u8],
        filename: &str,
        overwrite_used: bool,
        only_section: Option<&str>,
    ) {
        let is_exporter_config = filename.contains(EXPORTING_CONF);
        let mut connectors = 0usize;
        let mut working_connector: Vec<u8> = Vec::new();
        let mut working_connector_section: Option<usize> = None;
        let mut global_exporting_section = false;
        let only_section = only_section.map(str::as_bytes);
        let mut section: Option<usize> = None;
        let mut line = 0usize;
        let mut rest = content;
        while !rest.is_empty() {
            // fgets(buffer, CONFIG_FILE_LINE_MAX): up to and including '\n', at most FILE_LINE_MAX - 1 bytes.
            let max = rest.len().min(FILE_LINE_MAX - 1);
            let take = rest[..max]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(max, |nl| nl + 1);
            let chunk = &rest[..take];
            rest = &rest[take..];
            line += 1;

            let Some(s) = trim(chunk) else {
                continue;
            };
            if s[0] == b'#' {
                continue;
            }
            if s[0] == b'[' && s[s.len() - 1] == b']' {
                let mut name = s[1..s.len() - 1].to_vec();
                if is_exporter_config {
                    global_exporting_section =
                        name == b"exporting:global" || name == b"prometheus:exporter";
                    if !global_exporting_section {
                        let Some(separator) = valid_connector(&name) else {
                            // C cut the name at its last ':' while checking, unless it was a reserved name.
                            let shown = match name.iter().rposition(|&c| c == b':') {
                                Some(sep) if !CONNECTOR_TYPES.contains(&name.as_slice()) => {
                                    &name[..sep]
                                }
                                _ => &name[..],
                            };
                            netdata_log_error!(
                                "Section ({}) does not specify a valid connector",
                                lossy(shown)
                            );
                            section = None;
                            continue;
                        };
                        working_connector = name[..separator.min(CONFIG_MAX_NAME)].to_vec();
                        let mut instance = name[separator + 1..].to_vec();
                        if instance.is_empty() {
                            connectors += 1;
                            instance = format!("instance_{connectors}").into_bytes();
                        }
                        working_connector_section = None;
                        let working_instance = &instance[..instance.len().min(CONFIG_MAX_NAME)];
                        if self.section_index(working_instance).is_some() {
                            netdata_log_error!(
                                "Instance ({}) already exists",
                                lossy(working_instance)
                            );
                            section = None;
                            continue;
                        }
                        name = instance;
                    }
                }
                let name = name.as_slice();
                let idx = self.section_find_or_create(name);
                if overwrite_used && only_section == Some(name) {
                    self.sections[idx].options.clear();
                }
                section = Some(idx);
                continue;
            }
            let Some(sect) = section else {
                netdata_log_error!(
                    "CONFIG: ignoring line {line} ('{}') of file '{filename}', it is outside all sections.",
                    lossy(s)
                );
                continue;
            };
            if overwrite_used && only_section.is_some_and(|only| only != self.sections[sect].name) {
                continue;
            }
            let Some(eq) = s.iter().position(|&c| c == b'=') else {
                netdata_log_error!(
                    "CONFIG: ignoring line {line} ('{}') of file '{filename}', there is no = in it.",
                    lossy(s)
                );
                continue;
            };
            let name = trim(&s[..eq]);
            let value = trim(&s[eq + 1..]).unwrap_or(b"");
            let Some(name) = name.filter(|n| n[0] != b'#') else {
                netdata_log_error!(
                    "CONFIG: ignoring line {line} of file '{filename}', name is empty."
                );
                continue;
            };
            let options = &mut self.sections[sect].options;
            let o = match options.iter().position(|o| o.name == name) {
                Some(o) => {
                    let opt = &mut options[o];
                    if opt.flags & USED == 0 || overwrite_used {
                        opt.value = value.to_vec();
                    }
                    o
                }
                None => {
                    options.push(Opt::new(name, value));
                    let o = options.len() - 1;
                    if is_exporter_config
                        && !global_exporting_section
                        && working_connector_section.is_none()
                    {
                        let connector = self.section_find_or_create(&working_connector);
                        working_connector_section = Some(connector);
                        let instance = self.sections[sect].name.clone();
                        let connector = self.sections[connector].name.clone();
                        self.connector_instances.insert(0, (connector, instance));
                    }
                    o
                }
            };
            self.sections[sect].options[o].flags |= LOADED;
        }
    }

    /// `inicfg_generate()`: the text served at `/netdata.conf`. For netdata.conf a `[host labels]` example is added
    /// first when the section is missing (this mutates the config, as in C).
    pub fn generate(&mut self, only_changed: bool, netdata_conf: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 * 1024);
        if netdata_conf && self.section_index(SECTION_HOST_LABEL.as_bytes()).is_none() {
            self.section_create(SECTION_HOST_LABEL.as_bytes());
            self.get_raw(
                SECTION_HOST_LABEL,
                "name",
                Some(b"value"),
                ValueType::Text,
                None,
            );
        }
        if netdata_conf {
            out.extend_from_slice(
                b"# netdata configuration\n\
                  #\n\
                  # You can download the latest version of this file, using:\n\
                  #\n\
                  #  wget -O /etc/netdata/netdata.conf http://localhost:19999/netdata.conf\n\
                  # or\n\
                  #  curl -o /etc/netdata/netdata.conf http://localhost:19999/netdata.conf\n\
                  #\n\
                  # You can uncomment and change any of the options below.\n\
                  # The value shown in the commented settings, is the default value.\n\
                  #\n\
                  \n# global netdata configuration\n",
            );
        }
        for pass in 0..=17 {
            for sect in &self.sections {
                if section_priority(&sect.name) != pass {
                    continue;
                }
                write_section(&mut out, sect, only_changed);
            }
        }
        out
    }
}

/// The order `inicfg_generate()` writes sections in; unknown sections are 12.
fn section_priority(name: &[u8]) -> i32 {
    let known: [(&str, i32); 16] = [
        (SECTION_GLOBAL, 0),
        (SECTION_DB, 1),
        (SECTION_DIRECTORIES, 2),
        (SECTION_LOGS, 3),
        (SECTION_ENV_VARS, 4),
        (SECTION_HOST_LABEL, 5),
        (SECTION_SQLITE, 6),
        (SECTION_CLOUD, 7),
        (SECTION_ML, 8),
        (SECTION_HEALTH, 9),
        (SECTION_WEB, 10),
        (SECTION_WEBRTC, 11),
        (SECTION_REGISTRY, 13),
        (SECTION_PULSE, 14),
        (SECTION_PLUGINS, 15),
        (SECTION_STATSD, 16),
    ];
    if let Some((_, p)) = known.iter().find(|(n, _)| n.as_bytes() == name) {
        return *p;
    }
    if name.starts_with(b"plugin:") { 17 } else { 12 }
}

fn write_section(out: &mut Vec<u8>, sect: &Section, only_changed: bool) {
    let count = sect.options.len();
    let used = sect.options.iter().filter(|o| o.flags & USED != 0).count();
    let loaded = sect
        .options
        .iter()
        .filter(|o| o.flags & LOADED != 0)
        .count();
    let changed = sect
        .options
        .iter()
        .filter(|o| o.flags & CHANGED != 0)
        .count();
    if count == 0 || (only_changed && changed == 0 && loaded == 0) {
        return;
    }
    let used = used > 0;
    if !used {
        out.extend_from_slice(b"\n# section '");
        out.extend_from_slice(&sect.name);
        out.extend_from_slice(b"' is not used.");
    }
    out.extend_from_slice(b"\n[");
    out.extend_from_slice(&sect.name);
    out.extend_from_slice(b"]\n");

    let mut last_had_comments = false;
    for (options_added, opt) in sect.options.iter().enumerate() {
        let unused = used && opt.flags & USED == 0;
        let migrated = used && opt.flags & MIGRATED != 0;
        let reformatted = used && opt.flags & REFORMATTED != 0;
        let show_default =
            used && opt.flags & (LOADED | CHANGED) != 0 && opt.value_default.is_some();

        if unused || migrated || reformatted || show_default {
            if options_added > 0 {
                out.push(b'\n');
            }
            out.extend_from_slice(b"\t#| >>> [");
            out.extend_from_slice(&sect.name);
            out.extend_from_slice(b"].");
            out.extend_from_slice(&opt.name);
            out.extend_from_slice(b" <<<\n");
            last_had_comments = true;
        } else if last_had_comments {
            out.push(b'\n');
            last_had_comments = false;
        }

        if unused {
            out.extend_from_slice(b"\t#| found in the config file, but is not used\n");
        }
        let (m_section, m_name): (&[u8], &[u8]) = opt
            .migrated
            .as_ref()
            .map_or((b"", b""), |(s, n)| (s.as_slice(), n.as_slice()));
        if migrated && reformatted {
            out.extend_from_slice(b"\t#| migrated from: [");
            out.extend_from_slice(m_section);
            out.extend_from_slice(b"].");
            out.extend_from_slice(m_name);
            out.extend_from_slice(b" = ");
            out.extend_from_slice(&opt.value_original);
            out.push(b'\n');
        } else {
            if migrated {
                out.extend_from_slice(b"\t#| migrated from: [");
                out.extend_from_slice(m_section);
                out.extend_from_slice(b"].");
                out.extend_from_slice(m_name);
                out.push(b'\n');
            }
            if reformatted {
                out.extend_from_slice(b"\t#| reformatted from: ");
                out.extend_from_slice(&opt.value_original);
                out.push(b'\n');
            }
        }
        if show_default {
            out.extend_from_slice(b"\t#| datatype: ");
            out.extend_from_slice(opt.ty.name().as_bytes());
            out.extend_from_slice(b", default value: ");
            out.extend_from_slice(opt.value_default.as_deref().unwrap_or(b""));
            out.push(b'\n');
        }
        let commented =
            opt.flags & LOADED == 0 && opt.flags & CHANGED == 0 && opt.flags & USED != 0;
        out.push(b'\t');
        if commented {
            out.extend_from_slice(b"# ");
        }
        out.extend_from_slice(&opt.name);
        out.extend_from_slice(b" = ");
        out.extend_from_slice(&opt.value);
        out.push(b'\n');
    }
}
