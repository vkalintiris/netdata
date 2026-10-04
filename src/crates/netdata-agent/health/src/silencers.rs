//! The silencers (`src/health/health_silencers.c`): one state for the whole agent, which says for an alert whether
//! its health checks are disabled or its notifications silenced. It is read from a JSON file at health's start,
//! changed by the requests of `/api/v1/manage/health`, and written back after a request.
//!
//! The state is `all`, one type (none, DISABLE or SILENCE) and a list of selectors. The type is the state's, not a
//! selector's: selectors only say which alerts, and without a type they have no effect.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use netdata_agent_ingest::jsonc::{Walked, json_parse};
use netdata_agent_log::{Priority, Source, errno_of, nd_log, netdata_log_error_errno, netdata_log_info, strerror};
use netdata_agent_text::c::{c_str, set_errno, strsep_skip};
use netdata_agent_text::simple_pattern::{Separators, SimplePattern, SimplePatternMode};

use crate::alert::run_flags;
use crate::keywords::lossy;

/// `HEALTH_SILENCERS_MAX_FILE_LEN`: a file of this size or more is not read, though a request writes it.
const MAX_FILE_LEN: u64 = 10000;

const MSG_AUTHERROR: &[u8] = b"Auth Error\n";
const MSG_SILENCEALL: &[u8] = b"All alarm notifications are silenced\n";
const MSG_DISABLEALL: &[u8] = b"All health checks are disabled\n";
const MSG_RESET: &[u8] = b"All health checks and notifications are enabled\n";
const MSG_DISABLE: &[u8] = b"Health checks disabled for alarms matching the selectors\n";
const MSG_SILENCE: &[u8] = b"Alarm notifications silenced for alarms matching the selectors\n";
const MSG_ADDED: &[u8] = b"Alarm selector added\n";
const MSG_STYPEWARNING: &[u8] =
    b"WARNING: Added alarm selector to silence/disable alarms without a SILENCE or DISABLE command.\n";
const MSG_NOSELECTORWARNING: &[u8] =
    b"WARNING: SILENCE or DISABLE command is ineffective without defining any alarm selectors.\n";

/// `SILENCE_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceType {
    None,
    DisableAlarms,
    SilenceNotifications,
}

impl SilenceType {
    /// As the JSON text names it.
    pub fn name(self) -> &'static str {
        match self {
            SilenceType::None => "None",
            SilenceType::DisableAlarms => "DISABLE",
            SilenceType::SilenceNotifications => "SILENCE",
        }
    }

    /// The type a `type` string of the file sets; any other text, `None` too, leaves the type as it is.
    fn of_file(text: &[u8]) -> Option<SilenceType> {
        match text {
            b"SILENCE" => Some(SilenceType::SilenceNotifications),
            b"DISABLE" => Some(SilenceType::DisableAlarms),
            _ => None,
        }
    }
}

/// One criterion of a selector: its text, and the text's pattern. A text with no word in it (spaces, a lone `!`)
/// has no pattern, as C's is NULL then: the criterion is printed and tests nothing.
#[derive(Default)]
struct Criterion {
    text: Option<Vec<u8>>,
    pattern: Option<SimplePattern>,
}

impl Criterion {
    /// A repeated key: the last value stands.
    fn set(&mut self, value: &[u8]) {
        let pattern = SimplePattern::new(value, Separators::Whitespace, SimplePatternMode::Exact, true);
        self.pattern = (!pattern.is_empty()).then_some(pattern);
        self.text = Some(value.to_vec());
    }
}

/// `SILENCER`: a selector.
#[derive(Default)]
struct Silencer {
    alarms: Criterion,
    charts: Criterion,
    contexts: Criterion,
    hosts: Criterion,
}

impl Silencer {
    /// `health_silencers_addparam()` for a selector in hand: `alarm`, `chart`, `context` and `hosts`, in any letter
    /// case, set their criterion; any other key sets nothing.
    fn set(&mut self, key: &[u8], value: &[u8]) {
        let criterion = if key.eq_ignore_ascii_case(b"alarm") {
            &mut self.alarms
        } else if key.eq_ignore_ascii_case(b"chart") {
            &mut self.charts
        } else if key.eq_ignore_ascii_case(b"context") {
            &mut self.contexts
        } else if key.eq_ignore_ascii_case(b"hosts") {
            &mut self.hosts
        } else {
            return;
        };
        criterion.set(value);
    }

    /// Whether a request's key makes the request's selector when there is none yet: the four criteria and
    /// `template`, which sets nothing.
    fn is_key(key: &[u8]) -> bool {
        [&b"alarm"[..], b"template", b"chart", b"context", b"hosts"].iter().any(|name| key.eq_ignore_ascii_case(name))
    }
}

/// What an alert is matched by; the chart's context is asked for only by a selector that tests it.
pub struct Subject<'a> {
    /// The rule's name.
    pub name: &'a [u8],
    /// The chart's id.
    pub chart: &'a [u8],
    /// Whether a pattern matches the chart's context; false for an alert without a chart.
    pub context: &'a dyn Fn(&SimplePattern) -> bool,
    /// The host's hostname (not its registry hostname).
    pub hostname: &'a [u8],
}

struct State {
    all_alarms: bool,
    stype: SilenceType,
    /// The front is C's list head: a new selector goes there.
    silencers: VecDeque<Silencer>,
}

impl State {
    /// `health_silencers_check_silenced_unsafe()`: the state's type when a selector matches, each present pattern
    /// positively: the alarm, the context, the host, the chart, in that order.
    fn check(&self, subject: &Subject<'_>) -> SilenceType {
        let matches = |criterion: &Criterion, text: &[u8]| criterion.pattern.as_ref().is_none_or(|p| p.matches(text));
        let selected = |s: &Silencer| {
            matches(&s.alarms, subject.name)
                && s.contexts.pattern.as_ref().is_none_or(|p| (subject.context)(p))
                && matches(&s.hosts, subject.hostname)
                && matches(&s.charts, subject.chart)
        };
        if self.silencers.iter().any(selected) { self.stype } else { SilenceType::None }
    }

    /// `health_silencers2json_unsafe()`. The values are written as they are: a quote or a backslash in one makes
    /// a text that is no JSON, as C's.
    fn to_json(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"{\n\t\"all\": ");
        out.extend_from_slice(if self.all_alarms { b"true" } else { b"false" });
        out.extend_from_slice(b",\n\t\"type\": \"");
        out.extend_from_slice(self.stype.name().as_bytes());
        out.extend_from_slice(b"\",\n\t\"silencers\": [");
        for (i, silencer) in self.silencers.iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.extend_from_slice(b"\n\t\t{");
            let members = [
                ("alarm", &silencer.alarms),
                ("chart", &silencer.charts),
                ("context", &silencer.contexts),
                ("hosts", &silencer.hosts),
            ];
            let mut has_previous = false;
            for (name, criterion) in members {
                let Some(text) = &criterion.text else { continue };
                if has_previous {
                    out.push(b',');
                }
                out.extend_from_slice(b"\n\t\t\t\"");
                out.extend_from_slice(name.as_bytes());
                out.extend_from_slice(b"\": \"");
                out.extend_from_slice(text);
                out.push(b'"');
                has_previous = true;
            }
            out.extend_from_slice(b"\n\t\t}");
        }
        if !self.silencers.is_empty() {
            out.extend_from_slice(b"\n\t");
        }
        out.extend_from_slice(b"]\n}\n");
    }
}

/// A request's reply.
#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    pub code: u16,
    /// The content type is `application/json` (a request with `cmd=LIST`), else `text/plain`.
    pub json: bool,
    pub body: Vec<u8>,
}

/// `silencers` with its lock, and `[health] silencers file`.
pub struct Silencers {
    /// A leaf lock: only pattern matching, and a look at a chart's context, happen while it is held.
    state: RwLock<State>,
    filename: Vec<u8>,
}

impl Silencers {
    /// `health_initialize_global_silencers()`: the empty state.
    pub fn new(filename: Vec<u8>) -> Silencers {
        let state = State { all_alarms: false, stype: SilenceType::None, silencers: VecDeque::new() };
        Silencers { state: RwLock::new(state), filename }
    }

    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }

    fn path(&self) -> &std::path::Path {
        std::path::Path::new(std::ffi::OsStr::from_bytes(&self.filename))
    }

    /// `health_silencers_init()`: the file's read at health's start. It adds to the state and clears nothing: a
    /// `type` of `SILENCE` or `DISABLE` sets the type wherever the walk meets it, any boolean sets `all`, and every
    /// element of every array is a selector, which goes to the list's head: the file's order is reversed.
    pub fn init(&self) {
        let name = lossy(&self.filename);
        let file = match std::fs::File::open(self.path()) {
            Ok(file) => file,
            Err(e) => {
                // C's record carries the errno the failed fopen() left, as every record of a thread carries the
                // thread's; a record the log level filters out leaves it for the next one
                set_errno(errno_of(&e));
                if !netdata_agent_log::filtered(Source::Daemon, Priority::Info) {
                    nd_log!(
                        Source::Daemon,
                        Priority::Info,
                        errno = netdata_agent_log::take_errno();
                        "Cannot open the file {name}, so Netdata will work with the default health configuration."
                    );
                }
                return;
            }
        };
        // C's ftell() at the file's end
        let length = file.metadata().map_or(0, |metadata| metadata.len());
        if length == 0 || length >= MAX_FILE_LEN {
            netdata_log_error_errno!(
                "Health silencers file {name} has the size {length} that is out of range[ 1 , {MAX_FILE_LEN} ]. \
                 Aborting read."
            );
            return;
        }
        let mut text = Vec::with_capacity(length as usize);
        let read = (&file).take(length).read_to_end(&mut text);
        if read.ok() != Some(length as usize) {
            netdata_log_error_errno!("Cannot read the data from health silencers file {name}");
            return;
        }

        // the selectors in the order the walk makes them; the record of a text that is refused is written by the
        // parse, and what the walk gives is applied under the lock, with nothing else done there
        let mut made: Vec<Silencer> = Vec::new();
        // the types and the booleans, in the walk's order: the last of each stands
        let (mut stype, mut all_alarms) = (None, None);
        json_parse(&text, &mut |walked| match walked {
            Walked::Element => made.push(Silencer::default()),
            Walked::ElementString { name: b"type", value, .. } | Walked::String { name: b"type", value } => {
                stype = SilenceType::of_file(value).or(stype);
            }
            Walked::ElementString { element, name, value } => made[element].set(name, value),
            Walked::String { .. } => {}
            Walked::Boolean(all) => all_alarms = Some(all),
        });
        {
            let mut state = self.write();
            state.stype = stype.unwrap_or(state.stype);
            state.all_alarms = all_alarms.unwrap_or(state.all_alarms);
            for silencer in made {
                state.silencers.push_front(silencer);
            }
        }
        netdata_log_info!("Parsed health silencers file {name}");
    }

    /// `health_silencers2json()`: the state as `cmd=LIST` and the file show it.
    pub fn to_json(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.read().to_json(&mut out);
        out
    }

    /// `web_client_api_request_v1_mgmt_health()`: a request of `/api/v1/manage/health`. `token` is the request's
    /// `X-Auth-Token`, `key` the management key, `query` the decoded query string. An authorized request without
    /// `cmd=LIST` writes the file, whether it changed anything or not; one with `cmd=LIST` never does.
    pub fn request(&self, token: Option<&[u8]>, key: &[u8], query: &[u8]) -> Reply {
        if token.map(c_str) != Some(c_str(key)) {
            return Reply { code: 403, json: false, body: MSG_AUTHERROR.to_vec() };
        }

        let (mut body, mut json, mut save) = (Vec::new(), false, true);
        let file = {
            let mut state = self.write();
            // the request's one selector, made by its first selector key
            let mut silencer: Option<Silencer> = None;
            let mut rest = Some(c_str(query));
            while rest.is_some() {
                let mut value = Some(strsep_skip(&mut rest, b"&"));
                let name = strsep_skip(&mut value, b"=");
                let Some(value) = value.filter(|value| !name.is_empty() && !value.is_empty()) else { continue };
                if name != b"cmd" {
                    if silencer.is_none() && Silencer::is_key(name) {
                        silencer = Some(Silencer::default());
                    }
                    if let Some(silencer) = &mut silencer {
                        silencer.set(name, value);
                    }
                    continue;
                }
                let (all, stype, text) = match value {
                    b"SILENCE ALL" => (Some(true), SilenceType::SilenceNotifications, MSG_SILENCEALL),
                    b"DISABLE ALL" => (Some(true), SilenceType::DisableAlarms, MSG_DISABLEALL),
                    b"SILENCE" => (None, SilenceType::SilenceNotifications, MSG_SILENCE),
                    b"DISABLE" => (None, SilenceType::DisableAlarms, MSG_DISABLE),
                    b"RESET" => {
                        state.silencers.clear();
                        (Some(false), SilenceType::None, MSG_RESET)
                    }
                    b"LIST" => {
                        json = true;
                        save = false;
                        state.to_json(&mut body);
                        continue;
                    }
                    _ => continue,
                };
                if let Some(all) = all {
                    state.all_alarms = all;
                }
                state.stype = stype;
                body.extend_from_slice(text);
            }

            if let Some(silencer) = silencer {
                state.silencers.push_front(silencer);
                body.extend_from_slice(MSG_ADDED);
                if state.stype == SilenceType::None {
                    body.extend_from_slice(MSG_STYPEWARNING);
                }
            }
            if state.stype != SilenceType::None && !state.all_alarms && state.silencers.is_empty() {
                body.extend_from_slice(MSG_NOSELECTORWARNING);
            }
            save.then(|| {
                let mut file = Vec::new();
                state.to_json(&mut file);
                file
            })
        };
        if let Some(file) = file {
            self.to_file(&file);
        }
        Reply { code: 200, json, body }
    }

    /// `health_silencers2file()`: after the lock, not atomically, a link followed; two requests are not serialized
    /// against each other. A write that fails once the file is open leaves no record.
    fn to_file(&self, text: &[u8]) {
        let name = lossy(&self.filename);
        match std::fs::File::create(self.path()) {
            Ok(mut file) => {
                if file.write_all(text).is_ok() {
                    netdata_log_info!("Silencer changes written to {name}");
                }
            }
            Err(e) => {
                // the record names the error and carries it
                let errno = errno_of(&e);
                set_errno(errno);
                netdata_log_error_errno!("Silencer changes could not be written to {name}. Error {}", strerror(errno));
            }
        }
    }

    /// `health_silencers_check_silenced()`.
    pub fn check(&self, subject: &Subject<'_>) -> SilenceType {
        self.read().check(subject)
    }

    /// `health_silencers_update_disabled_silenced()` without its record: the alert's run flags with DISABLED and
    /// SILENCED as the state says now: the state's type when `all` is on, else what the selectors answer.
    pub fn update(&self, subject: &Subject<'_>, flags: u32) -> u32 {
        let stype = {
            let state = self.read();
            if state.all_alarms { state.stype } else { state.check(subject) }
        };
        let cleared = flags & !(run_flags::DISABLED | run_flags::SILENCED);
        match stype {
            SilenceType::DisableAlarms => cleared | run_flags::DISABLED,
            SilenceType::SilenceNotifications => cleared | run_flags::SILENCED,
            SilenceType::None => cleared,
        }
    }

    /// `health_silencers_all_alarms_disabled()`.
    pub fn all_alarms_disabled(&self) -> bool {
        let state = self.read();
        state.all_alarms && state.stype == SilenceType::DisableAlarms
    }
}

/// The record `health_silencers_update_disabled_silenced()` writes when an alert's flags changed.
pub fn changed_record(hostname: &[u8], alert: &[u8], before: u32, after: u32) {
    let flag = |flags: u32, flag: u32| if flags & flag != 0 { "true" } else { "false" };
    netdata_log_info!(
        "Alarm silencing changed for host '{}' alarm '{}': Disabled {}->{} Silenced {}->{}",
        lossy(hostname),
        lossy(alert),
        flag(before, run_flags::DISABLED),
        flag(after, run_flags::DISABLED),
        flag(before, run_flags::SILENCED),
        flag(after, run_flags::SILENCED)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `health_silencers_all_alarms_disabled()`: `all` with the type DISABLE, and nothing else.
    #[test]
    fn all_alarms_are_disabled_by_all_with_the_type_disable() {
        let dir = tempfile::tempdir().expect("a directory");
        let silencers = Silencers::new(dir.path().join("silencers.json").into_os_string().into_encoded_bytes());
        let after = |query: &[u8]| {
            let (reply, _records) = netdata_agent_log::capture(|| silencers.request(Some(b"key"), b"key", query));
            assert_eq!(reply.code, 200);
            silencers.all_alarms_disabled()
        };
        assert!(!silencers.all_alarms_disabled());
        assert!(!after(b"cmd=SILENCE ALL"));
        assert!(after(b"cmd=DISABLE ALL"));
        // `all` stays on when the type alone changes
        assert!(!after(b"cmd=SILENCE"));
        assert!(after(b"cmd=DISABLE"));
        assert!(!after(b"cmd=RESET"));
        // a selector is not `all`
        assert!(!after(b"cmd=DISABLE&alarm=*"));
    }
}
