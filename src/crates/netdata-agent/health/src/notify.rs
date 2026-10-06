//! Notifications (`health_notifications.c`): whether an entry of the alert log is notified, the command line of
//! the notification script, its start, and the wait for its end.
//!
//! Nothing here holds a lock of health across a question to the table, a spawn or a wait: the caller hands in a
//! copy of the entry and puts what comes back on the live one.

use std::sync::{Arc, Weak};

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};
use netdata_agent_rrd::host::Host;
use netdata_agent_text::c::{at, c_str, is_cntrl};
use netdata_agent_text::print::{print_fixed, print_int64, print_uint64, uuid_lower_text};

use crate::alert::{Alert, Status};
use crate::alerts::HostAlerts;
use crate::entry::{Entry, entry_flags};
use crate::pass::Env;
use crate::sql::edit_command_from_source;
use crate::{Clock, Health};

/// What `health_send_notification()` decides about an entry before it prepares a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The new status is below CLEAR: an internal status is never notified.
    Internal,
    /// A status up to CLEAR of an alert with `no-clear-notification`.
    NoClear,
    /// The alarm's last executed event had this status, and the entry is no repeat.
    Again,
    /// CLEAR of an alarm no notification was executed for.
    FirstClear,
    /// The entry is silenced.
    Silenced,
    Send,
}

/// `health_send_notification()`'s decision for an entry with these flags and statuses. `last_executed` asks the
/// table for the status of the alarm's last executed event (`None`: there is none, or the question failed); C
/// asks it for every entry that got this far unless the alert has `no-clear-notification`.
pub fn decide(flags: u32, new: Status, old: Status, last_executed: impl FnOnce() -> Option<i32>) -> Decision {
    if (new as i32) < Status::Clear as i32 {
        return Decision::Internal;
    }
    let no_clear = flags & entry_flags::NO_CLEAR_NOTIFICATION != 0;
    if new == Status::Clear && no_clear {
        return Decision::NoClear;
    }
    if !no_clear {
        match last_executed() {
            // the same status is not notified twice in a row, but for a repeat
            Some(status) if status == new as i32 && flags & entry_flags::IS_REPEATING == 0 => return Decision::Again,
            Some(_) => {}
            // an alarm that never notified does not start with a CLEAR, unless it was raised before this pass's
            // repeat made its entry
            None if new == Status::Clear
                && (flags & entry_flags::RUN_ONCE == 0 || (old as i32) < Status::Raised as i32) =>
            {
                return Decision::FirstClear;
            }
            None => {}
        }
    }
    if flags & entry_flags::SILENCED != 0 {
        return Decision::Silenced;
    }
    Decision::Send
}

/// What `prepare_command()` gives the sanitizer for one argument: a buffer of 8192 bytes, less its terminator.
const ARGUMENT_MAX: usize = 8191;

/// `sanitize_command_argument_string()`: `src` as it may stand between single quotes in a shell command, appended
/// to `dst`. Leading dashes go; a control character and a `$` become `_`; a single quote and a backtick become
/// `'\''`. False when the result does not fit the C buffer: C then gives up the whole command, and what was
/// appended is of no use.
pub fn sanitize_command_argument(dst: &mut Vec<u8>, src: &[u8]) -> bool {
    let src = c_str(src);
    let mut start = 0;
    while at(src, start) == b'-' {
        start += 1;
    }
    let mut left = ARGUMENT_MAX;
    for &c in &src[start..] {
        if left < 1 {
            return false;
        }
        if is_cntrl(c) || c == b'$' {
            dst.push(b'_');
            left -= 1;
        } else if c == b'\'' || c == b'`' {
            if left < 4 {
                return false;
            }
            dst.extend_from_slice(b"'\\''");
            left -= 4;
        } else {
            dst.push(c);
            left -= 1;
        }
    }
    // the terminator needs a byte too
    left != 0
}

/// `nd_duration_to_uint32_saturating()`.
pub(crate) fn duration_to_u32(duration: i64) -> u32 {
    duration.clamp(0, i64::from(u32::MAX)) as u32
}

/// `prepare_command()`'s arguments after `exec`, in the command's order: the 34 words the notification script is
/// called with (its `$0` is the first).
pub(crate) struct Command<'a> {
    pub exec: &'a [u8],
    pub recipient: &'a [u8],
    pub registry_hostname: &'a [u8],
    pub unique_id: u32,
    pub alarm_id: u32,
    pub alarm_event_id: u32,
    /// C passes the entry's time through a 32-bit parameter.
    pub when: u32,
    pub name: &'a [u8],
    pub chart: &'a [u8],
    pub new_status: &'a str,
    pub old_status: &'a str,
    pub new_value: f64,
    pub old_value: f64,
    pub source: &'a [u8],
    pub duration: u32,
    pub non_clear_duration: u32,
    pub units: &'a [u8],
    pub info: &'a [u8],
    pub new_value_string: &'a [u8],
    pub old_value_string: &'a [u8],
    /// The alert's own warning or critical expression, and that expression's error text.
    pub expression: &'a [u8],
    pub expression_error: &'a [u8],
    /// How many other alerts of the host warn and are critical, and their lists.
    pub n_warn: i32,
    pub n_crit: i32,
    pub warn_alarms: &'a [u8],
    pub crit_alarms: &'a [u8],
    pub classification: &'a [u8],
    pub edit_command: &'a [u8],
    pub machine_guid: &'a [u8],
    /// The transition id, as lower case text.
    pub transition_id: &'a [u8],
    pub summary: &'a [u8],
    pub context: &'a [u8],
    pub component: &'a [u8],
    pub r#type: &'a [u8],
}

/// `prepare_command()`: `exec '<a0>' '<a1>' ...`, one shell word per argument. `None` when a text does not fit its
/// argument (`sanitize_command_argument()`): C then runs nothing.
pub(crate) fn prepare_command(c: &Command<'_>) -> Option<Vec<u8>> {
    let mut wb = Vec::with_capacity(1024);
    wb.extend_from_slice(b"exec");
    let text = |wb: &mut Vec<u8>, value: &[u8]| -> Option<()> {
        wb.extend_from_slice(b" '");
        sanitize_command_argument(wb, value).then_some(())?;
        wb.push(b'\'');
        Some(())
    };
    let unsigned = |wb: &mut Vec<u8>, value: u32| {
        wb.extend_from_slice(b" '");
        print_uint64(wb, u64::from(value));
        wb.push(b'\'');
    };
    let signed = |wb: &mut Vec<u8>, value: i32| {
        wb.extend_from_slice(b" '");
        print_int64(wb, i64::from(value));
        wb.push(b'\'');
    };
    // NETDATA_DOUBLE_FORMAT_ZERO
    let value = |wb: &mut Vec<u8>, value: f64| {
        wb.extend_from_slice(b" '");
        print_fixed(wb, value, 0);
        wb.push(b'\'');
    };

    text(&mut wb, c.exec)?;
    text(&mut wb, c.recipient)?;
    text(&mut wb, c.registry_hostname)?;
    unsigned(&mut wb, c.unique_id);
    unsigned(&mut wb, c.alarm_id);
    unsigned(&mut wb, c.alarm_event_id);
    unsigned(&mut wb, c.when);
    text(&mut wb, c.name)?;
    text(&mut wb, c.chart)?;
    text(&mut wb, c.new_status.as_bytes())?;
    text(&mut wb, c.old_status.as_bytes())?;
    value(&mut wb, c.new_value);
    value(&mut wb, c.old_value);
    text(&mut wb, c.source)?;
    unsigned(&mut wb, c.duration);
    unsigned(&mut wb, c.non_clear_duration);
    text(&mut wb, c.units)?;
    text(&mut wb, c.info)?;
    text(&mut wb, c.new_value_string)?;
    text(&mut wb, c.old_value_string)?;
    text(&mut wb, c.expression)?;
    text(&mut wb, c.expression_error)?;
    signed(&mut wb, c.n_warn);
    signed(&mut wb, c.n_crit);
    text(&mut wb, c.warn_alarms)?;
    text(&mut wb, c.crit_alarms)?;
    text(&mut wb, c.classification)?;
    text(&mut wb, c.edit_command)?;
    text(&mut wb, c.machine_guid)?;
    text(&mut wb, c.transition_id)?;
    text(&mut wb, c.summary)?;
    text(&mut wb, c.context)?;
    text(&mut wb, c.component)?;
    text(&mut wb, c.r#type)?;
    Some(wb)
}

/// A notification's command once it is started (C's `POPEN_INSTANCE`, as health uses it). The queue of running
/// notifications is shared with the thread that cleans a host up.
pub trait Execution: Send {
    /// `spawn_popen_pid()`.
    fn pid(&self) -> i32;
    /// `spawn_popen_timedwait()`: waits up to `timeout_ms` for the command's end.
    fn timedwait(self: Box<Self>, timeout_ms: i32) -> Waiting;
    /// `spawn_popen_kill()`: the command is killed and reaped.
    fn kill(self: Box<Self>, timeout_ms: i32) -> i32;
}

/// How a slice of the wait ended (`SPAWN_TIMEDWAIT_RESULT`).
pub enum Waiting {
    /// The command ended: its code, as `spawn_popen_wait()` gives it.
    Exited(i32),
    /// It still runs, with the errno the wait left (`ETIMEDOUT`, or `ECANCELED` when the thread is cancelled).
    Running(Box<dyn Execution>, i32),
    /// The wait itself broke: the command's state is unknown.
    Error(Box<dyn Execution>),
}

/// A started notification of a logged entry, until HEALTH waited for it (an entry on C's list
/// `alarm_notifications_in_progress`).
pub struct Executing {
    pub(crate) alerts: Weak<HostAlerts>,
    pub(crate) unique_id: u32,
    pub(crate) name: Vec<u8>,
    pub(crate) execution: Box<dyn Execution>,
}

/// `HEALTH_NOTIFICATION_WAIT_SLICE_MS`: how often the wait looks at the service and at its deadline.
const WAIT_SLICE_MS: i32 = 1000;

/// `health_alarm_wait_for_execution()` for an entry whose command runs: its exit code; 128 after a kill, which
/// comes when the wait breaks, when the service stops, or when `timeout_s` (0: none) is over, counted on the
/// monotonic clock from now.
pub(crate) fn wait_for_execution(name: &[u8], mut execution: Box<dyn Execution>, timeout_s: i32, env: &dyn Env) -> i32 {
    // C's arithmetic is unsigned
    let deadline = env.monotonic_usec().wrapping_add((i64::from(timeout_s) as u64).wrapping_mul(1_000_000));
    loop {
        let pid = execution.pid();
        let (running, broken, errno) = match execution.timedwait(WAIT_SLICE_MS) {
            Waiting::Exited(code) => return code,
            Waiting::Running(running, errno) => (running, false, errno),
            Waiting::Error(running) => (running, true, 0),
        };
        // a wait that broke is never looped on; the service is looked at every slice, so that a slow command
        // cannot hold the agent's exit
        let deadline_reached = timeout_s > 0 && env.monotonic_usec() >= deadline;
        if broken || !env.service_running() || deadline_reached {
            let why = match broken {
                true => "could not be waited for (status channel error)",
                false => "is still running past its execution timeout",
            };
            let name = String::from_utf8_lossy(name);
            nd_log!(Source::Daemon, Priority::Err, errno = errno;
                "HEALTH: alert notification '{name}' (pid {pid}) {why} - killing it");
            running.kill(0);
            return 128;
        }
        execution = running;
    }
}

/// `health_alarm_wait_for_execution()` for an entry no command runs for (a repeat that was not sent, or whose
/// spawn failed): C's record, and its code.
pub(crate) fn wait_without_execution() -> i32 {
    nd_log!(
        Source::Daemon,
        Priority::Err,
        "attempted to wait for the execution of alert that has not an execution in progress"
    );
    128
}

/// `struct health_raised_summary`: the host's alerts on collected charts, newest status change first, for the
/// "other alerts raised" arguments of a notification. Built at most once per pass, when a notification first
/// needs it.
#[derive(Default)]
pub(crate) struct RaisedSummary {
    alerts: Option<Vec<Arc<Alert>>>,
}

impl RaisedSummary {
    pub(crate) fn is_built(&self) -> bool {
        self.alerts.is_some()
    }

    /// `alerts_raised_summary_populate()`. It takes the store's lock for the list and each alert's own for its
    /// change time: the caller holds neither.
    pub(crate) fn build(&mut self, alerts: &HostAlerts, env: &dyn Env) {
        if self.alerts.is_some() {
            return;
        }
        let mut collected: Vec<(i64, Arc<Alert>)> = alerts
            .alerts()
            .into_iter()
            .filter(|alert| env.facts(&alert.chart).last_collected_s != 0)
            .map(|alert| {
                let changed = alert.run().last_status_change;
                (changed, alert)
            })
            .collect();
        // C's qsort() by the change time, newest first; alerts of the same second keep the host's order
        collected.sort_by_key(|(changed, _)| std::cmp::Reverse(*changed));
        self.alerts = Some(collected.into_iter().map(|(_, alert)| alert).collect());
    }

    /// `health_raised_summary_entries()`: how many alerts other than the entry's have that status now, and their
    /// `name=<change time>` list.
    fn entries(&self, alarm_id: u32, status: Status) -> (i32, Vec<u8>) {
        let (mut count, mut list) = (0, Vec::new());
        for alert in self.alerts.iter().flatten() {
            let run = alert.run();
            if run.status != status || alert.id == alarm_id {
                continue;
            }
            count += 1;
            if !list.is_empty() {
                list.push(b',');
            }
            list.extend_from_slice(alert.name());
            list.push(b'=');
            print_int64(&mut list, run.last_status_change);
        }
        (count, list)
    }

    /// `health_raised_summary_my_expression_source()` and `..._error()`: the entry's own alert's critical
    /// expression when it is critical now, else its warning one: the source text and the text of its last
    /// evaluation's lookups. Empty for an alert that is not in the summary, and for an expression it lacks.
    fn my_expression(&self, alarm_id: u32) -> (Vec<u8>, Vec<u8>) {
        let Some(alert) = self.alerts.iter().flatten().find(|alert| alert.id == alarm_id) else {
            return (Vec::new(), Vec::new());
        };
        let critical = alert.run().status == Status::Critical;
        let expressions = alert.expressions();
        let expression = if critical { &expressions.critical } else { &expressions.warning };
        match expression {
            Some(expression) => (expression.source().to_vec(), expression.error_msg().to_vec()),
            None => (Vec::new(), Vec::new()),
        }
    }
}

/// `string2str()` of a text the entry may lack.
fn or_empty(text: &Option<Vec<u8>>) -> &[u8] {
    text.as_deref().unwrap_or(b"")
}

/// What `health_send_notification()` did, for the caller to put on the entry.
pub(crate) struct Sent {
    /// `EXEC_RUN`, and `EXEC_IN_PROGRESS` when the command started.
    pub flags: u32,
    /// The wall clock when a command was prepared.
    pub exec_run_timestamp: Option<i64>,
    pub execution: Option<Box<dyn Execution>>,
    /// C saves the entry on every path but one: a command that could not be prepared.
    pub save: bool,
}

/// `health_send_notification()` after its mark (the caller sets PROCESSED first, as C does): the decision, with
/// the table asked where C asks it; then, for an entry to notify, the command, one read of the clock and the
/// spawn. `entry` is a copy of the entry, or a repeat's own.
pub(crate) fn send(
    host: &Host,
    alerts: &HostAlerts,
    entry: &Entry,
    summary: &mut RaisedSummary,
    health: &Health,
    env: &dyn Env,
    clock: Clock,
) -> Sent {
    let skipped = Sent { flags: 0, exec_run_timestamp: None, execution: None, save: true };
    let hostname = host.hostname();
    let (chart, name) = (String::from_utf8_lossy(&entry.chart), entry.name.as_deref().unwrap_or(b""));
    let (shown, status) = (String::from_utf8_lossy(name), entry.new_status.name());
    let last_executed = || env.last_executed_event(host, entry.alarm_id, entry.unique_id);
    match decide(entry.flags, entry.new_status, entry.old_status, last_executed) {
        Decision::Send => {}
        Decision::Internal | Decision::FirstClear => return skipped,
        Decision::NoClear => {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "[{hostname}]: Health not sending notification for alarm '{chart}.{shown}' status {status} (it has \
                 no-clear-notification enabled)"
            );
            return skipped;
        }
        Decision::Again => {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "[{hostname}]: Health not sending again notification for alarm '{chart}.{shown}' status {status}"
            );
            return skipped;
        }
        Decision::Silenced => {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "[{hostname}]: Health not sending notification for alarm '{chart}.{shown}' status {status} (command \
                 API has disabled notifications)"
            );
            return skipped;
        }
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "[{hostname}]: Sending notification for alarm '{chart}.{shown}' status {status}."
    );

    let (default_exec, default_recipient) = health.host_defaults(host);
    let (user_config_dir, registry_hostname) = env.edit_context();
    let edit_command = match &entry.source {
        Some(source) => edit_command_from_source(source, &user_config_dir, &registry_hostname),
        None => b"UNKNOWN=0=UNKNOWN".to_vec(),
    };
    // past every skip, so that a pass without a notification never builds it
    summary.build(alerts, env);
    let (n_warn, warn_alarms) = summary.entries(entry.alarm_id, Status::Warning);
    let (n_crit, crit_alarms) = summary.entries(entry.alarm_id, Status::Critical);
    let (expression, expression_error) = summary.my_expression(entry.alarm_id);
    let (transition_id, transition_len) = uuid_lower_text(&entry.transition_id, false);
    let info = host.info();
    let raised = entry.new_status as i32 >= Status::Warning as i32;
    let raised_repeat = entry.flags & entry_flags::IS_REPEATING != 0 && raised;
    let use_summary = health.config().use_summary_for_notifications;
    let command = prepare_command(&Command {
        exec: entry.exec.as_deref().unwrap_or(default_exec),
        recipient: entry.recipient.as_deref().unwrap_or(default_recipient),
        registry_hostname: info.registry_hostname.as_bytes(),
        unique_id: entry.unique_id,
        alarm_id: entry.alarm_id,
        alarm_event_id: entry.alarm_event_id,
        when: entry.when as u32,
        name,
        // C's NULL chart: a row whose chart is the empty text loads as none (`string_strdupz("")`)
        chart: if entry.chart.is_empty() { b"NOCHART" } else { &entry.chart },
        new_status: status,
        old_status: entry.old_status.name(),
        new_value: entry.new_value,
        old_value: entry.old_value,
        source: entry.source.as_deref().unwrap_or(b"UNKNOWN"),
        duration: duration_to_u32(entry.duration),
        non_clear_duration: duration_to_u32(if raised_repeat { entry.duration } else { entry.non_clear_duration }),
        units: or_empty(&entry.units),
        info: or_empty(&entry.info),
        new_value_string: &entry.new_value_string,
        old_value_string: &entry.old_value_string,
        expression: &expression,
        expression_error: &expression_error,
        n_warn,
        n_crit,
        warn_alarms: &warn_alarms,
        crit_alarms: &crit_alarms,
        classification: entry.classification.as_deref().unwrap_or(b"Unknown"),
        edit_command: &edit_command,
        machine_guid: host.machine_guid().as_bytes(),
        transition_id: &transition_id[..transition_len],
        summary: match &entry.summary {
            Some(summary) if use_summary => summary.as_slice(),
            _ => name,
        },
        context: &entry.chart_context,
        component: or_empty(&entry.component),
        r#type: or_empty(&entry.r#type),
    });
    let Some(command) = command else {
        netdata_log_error!("Failed to format command arguments");
        return Sent { save: false, ..skipped };
    };

    let exec_run_timestamp = Some(clock());
    let execution = env.exec(&command);
    let flags = match execution {
        Some(_) => entry_flags::EXEC_RUN | entry_flags::EXEC_IN_PROGRESS,
        None => {
            netdata_log_error!("Failed to execute alarm notification");
            entry_flags::EXEC_RUN
        }
    };
    Sent { flags, exec_run_timestamp, execution, save: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_is_made_safe_between_single_quotes() {
        let sanitized = |src: &[u8]| {
            let mut dst = Vec::new();
            sanitize_command_argument(&mut dst, src).then_some(dst)
        };
        assert_eq!(sanitized(b"--it's `$HOME`\t-x").unwrap(), b"it'\\''s '\\''_HOME'\\''_-x");
        assert_eq!(sanitized(b"---").unwrap(), b"");
        // 8,190 bytes fit, 8,191 leave no room for the terminator; a quote needs four bytes and then one more
        assert!(sanitized(&[b'x'; 8190]).is_some());
        assert!(sanitized(&[b'x'; 8191]).is_none());
        let quote_after = |plain: usize| [vec![b'x'; plain], b"'".to_vec()].concat();
        assert!(sanitized(&quote_after(8186)).is_some());
        assert!(sanitized(&quote_after(8187)).is_none());
    }

    use crate::pass::Pass;
    use crate::testing::{Scripted, chart, health_with, host_of, rule_text};

    const NOW: i64 = 1_700_000_000;

    /// The wait's loop, in C's order: the deadline's clock is read once before the first slice; after a slice that
    /// ends with the command running the clock is read (only with a timeout), then the service is looked at.
    #[test]
    fn a_wait_reads_the_deadline_s_clock_then_looks_at_the_service() {
        let env = Scripted { command: Some(usize::MAX), ..Scripted::default() };
        let command = env.exec(b"x").expect("a command");
        let (code, records) = netdata_agent_log::capture(|| wait_for_execution(b"a", command, 2, &env));
        assert_eq!(code, 128);
        let slice = ["timedwait 100", "monotonic", "look"];
        assert_eq!(env.take_trace(), [&["spawn 100", "monotonic"][..], &slice, &slice, &["kill 100"]].concat());
        let messages: Vec<(Option<String>, i32)> = records.into_iter().map(|r| (r.message, r.errno)).collect();
        let killed = "HEALTH: alert notification 'a' (pid 100) is still running past its execution timeout - \
                      killing it";
        assert_eq!(messages, [(Some(killed.to_owned()), 110)]);

        // without a timeout the clock is read once, and a command that ends is not killed
        let env = Scripted { command: Some(2), ..Scripted::default() };
        let command = env.exec(b"x").expect("a command");
        assert_eq!(wait_for_execution(b"a", command, 0, &env), 0);
        let slice = ["timedwait 100", "look"];
        assert_eq!(env.take_trace(), [&["spawn 100", "monotonic"][..], &slice, &slice, &["timedwait 100"]].concat());

        // a service that stops ends the wait at the slice's end, whatever the deadline
        let env = Scripted { command: Some(usize::MAX), ..Scripted::default() };
        env.running_for.set(Some(1));
        let command = env.exec(b"x").expect("a command");
        assert_eq!(netdata_agent_log::capture(|| wait_for_execution(b"a", command, 0, &env)).0, 128);
        let trace = env.take_trace();
        assert_eq!(trace[trace.len() - 3..], ["timedwait 100", "look", "kill 100"]);
    }

    /// A world in which alerts are evaluated and saved, and a notification's command ends at its first slice.
    fn raising() -> Scripted {
        Scripted { collected: Some(NOW), saves: true, table: Some(Vec::new()), command: Some(0), ..Scripted::default() }
    }

    /// Two hosts whose alert rises: `health` after their passes, with the hosts and the scripted world.
    fn two_raised(env: &Scripted) -> (Arc<Health>, [Arc<Host>; 2]) {
        let health = health_with(&rule_text("template", "a", "t.ctx", &["warn: $this > 0"]));
        let hosts = [
            host_of("11111111-2222-4333-8444-555555555555", &[]),
            host_of("22222222-2222-4333-8444-555555555555", &[]),
        ];
        for host in &hosts {
            chart(host, "t.c", None, "t.ctx", &[]);
        }
        for now in [NOW, NOW + 10, NOW + 20] {
            for host in &hosts {
                let mut next_run = now + 100;
                let pass = Pass { now, apply_hibernation_delay: false, next_run: &mut next_run, gate: &|| true };
                health.host_pass(host, pass, env, &|| now, &|| true);
            }
        }
        (health, hosts)
    }

    /// The entry of a host's log that was notified: its flags and its command's code.
    fn notified(health: &Health, host: &Host) -> (u32, i32) {
        let entries = health.host(host).expect("the host's alerts").log_entries();
        let entry = entries.iter().find(|entry| entry.flags & entry_flags::EXEC_RUN != 0).expect("a notified entry");
        (entry.flags & (entry_flags::EXEC_IN_PROGRESS | entry_flags::EXEC_FAILED), entry.exec_code)
    }

    /// The notifications of an iteration are waited for in the order they started, whatever their host; each
    /// wait is preceded by a look at the service, and its entry then loses the in-progress mark.
    #[test]
    fn the_running_notifications_are_waited_for_oldest_first() {
        let env = raising();
        let (health, hosts) = two_raised(&env);
        assert_eq!(health.executing().iter().map(|item| item.execution.pid()).collect::<Vec<i32>>(), [100, 101]);
        assert_eq!(notified(&health, &hosts[0]), (entry_flags::EXEC_IN_PROGRESS, 0));
        env.take_trace();

        health.wait_for_notifications(&env);
        assert_eq!(env.take_trace(), ["look", "monotonic", "timedwait 100", "look", "monotonic", "timedwait 101"]);
        assert!(health.executing().is_empty());
        assert_eq!((notified(&health, &hosts[0]), notified(&health, &hosts[1])), ((0, 0), (0, 0)));
    }

    /// A service that stops leaves the notifications it has not come to running, with their marks.
    #[test]
    fn a_stopping_service_leaves_the_rest_of_the_notifications() {
        let env = raising();
        let (health, hosts) = two_raised(&env);
        env.take_trace();
        env.running_for.set(Some(1));
        health.wait_for_notifications(&env);
        assert_eq!(env.take_trace(), ["look", "monotonic", "timedwait 100", "look"]);
        assert_eq!(health.executing().iter().map(|item| item.execution.pid()).collect::<Vec<i32>>(), [101]);
        assert_eq!(notified(&health, &hosts[1]), (entry_flags::EXEC_IN_PROGRESS, 0));
    }

    /// A host's cleanup kills its running notifications and takes them off the queue; another host's stay. The
    /// wait that follows writes nothing on the entries that are gone.
    #[test]
    fn a_host_s_cleanup_kills_its_notifications() {
        let env = raising();
        let (health, hosts) = two_raised(&env);
        env.take_trace();
        health.host_charts_flushed(&hosts[0]);
        assert_eq!(env.take_trace(), ["kill 100"]);
        assert_eq!(health.executing().iter().map(|item| item.execution.pid()).collect::<Vec<i32>>(), [101]);

        // an entry that leaves its log while its command runs has nothing to write on
        health.host(&hosts[1]).expect("the host's alerts").charts_flushed();
        health.wait_for_notifications(&env);
        assert_eq!(env.take_trace(), ["look", "monotonic", "timedwait 101"]);
        assert!(health.executing().is_empty());
    }

    /// The summary of raised alerts: the alerts of collected charts; the lists leave the entry's own alert out and
    /// keep the host's order for alerts that changed in the same second; the entry's alert gives its critical
    /// expression when it is critical, else its warning one.
    #[test]
    fn the_raised_summary_lists_the_other_alerts() {
        let rules = [
            rule_text("template", "a", "t.ctx", &["warn: $this > 0"]),
            rule_text("template", "b", "t.ctx", &["warn: $this > 0"]),
            rule_text("template", "c", "t.ctx", &["crit: $this > 0"]),
        ];
        let health = health_with(&rules.concat());
        let host = host_of("11111111-2222-4333-8444-555555555555", &[]);
        chart(&host, "t.c", None, "t.ctx", &[]);
        let env = raising();
        for now in [NOW, NOW + 10, NOW + 20] {
            let mut next_run = now + 100;
            let pass = Pass { now, apply_hibernation_delay: false, next_run: &mut next_run, gate: &|| true };
            health.host_pass(&host, pass, &env, &|| now, &|| true);
        }
        let alerts = health.host(&host).expect("the host's alerts");
        let of = |name: &[u8]| alerts.alerts().into_iter().find(|alert| alert.name() == name).expect("the alert");
        let (a, b, c) = (of(b"a"), of(b"b"), of(b"c"));
        let changed = |alert: &Alert| alert.run().last_status_change;
        assert_eq!((a.run().status, c.run().status), (Status::Warning, Status::Critical));
        assert_eq!(changed(&a), changed(&b), "both rose in the same pass");

        let mut summary = RaisedSummary::default();
        assert!(!summary.is_built());
        summary.build(&alerts, &env);
        assert!(summary.is_built());
        let list = |text: String| text.into_bytes();
        assert_eq!(summary.entries(0, Status::Warning), (2, list(format!("a={},b={}", changed(&a), changed(&b)))));
        assert_eq!(summary.entries(a.id, Status::Warning), (1, list(format!("b={}", changed(&b)))));
        assert_eq!(summary.entries(a.id, Status::Critical), (1, list(format!("c={}", changed(&c)))));
        assert_eq!(summary.entries(c.id, Status::Critical), (0, Vec::new()));
        assert_eq!(summary.my_expression(c.id).0, b"$this > 0");
        assert_eq!(summary.my_expression(a.id).0, b"$this > 0");
        assert_eq!(summary.my_expression(0), (Vec::new(), Vec::new()), "an alert the summary does not have");

        // the alerts of a chart that was never collected are in no list
        let idle = Scripted::default();
        let mut summary = RaisedSummary::default();
        summary.build(&alerts, &idle);
        assert_eq!(summary.entries(0, Status::Warning), (0, Vec::new()));
        assert_eq!(summary.my_expression(a.id), (Vec::new(), Vec::new()));
    }

    /// The words of a command, as the shell would hand them to the script.
    fn words(command: &[u8]) -> Vec<String> {
        let text = String::from_utf8_lossy(command).into_owned();
        let arguments = text.strip_prefix("exec '").and_then(|rest| rest.strip_suffix('\'')).expect("a command");
        arguments.split("' '").map(str::to_owned).collect()
    }

    /// What goes where in a command: the alert's host gives its registry hostname and its GUID; the edit command
    /// is made of the user configuration directory and localhost's registry hostname; an entry without a source
    /// or a class gets C's two spellings of "unknown".
    #[test]
    fn a_command_s_words_come_from_the_alert_s_host_and_from_localhost() {
        let env = raising();
        let (health, hosts) = two_raised(&env);
        let commands: Vec<Vec<String>> = env.commands.borrow().iter().map(|command| words(command)).collect();
        assert_eq!(commands.len(), 2);
        let guids = ["11111111-2222-4333-8444-555555555555", "22222222-2222-4333-8444-555555555555"];
        for (command, guid) in commands.iter().zip(guids) {
            assert_eq!(command.len(), 34, "{command:?}");
            let recipient = String::from_utf8_lossy(&health.config().default_recipient).into_owned();
            assert_eq!(command[..3], [String::new(), recipient, "testregistry".to_owned()]);
            assert_eq!(command[7..11], ["a", "t.c", "WARNING", "UNINITIALIZED"]);
            assert_eq!((command[26].as_str(), command[28].as_str()), ("Unknown", guid));
            // the rule's file has an absolute path here: its edit command is not empty
            let edit = edit_command_from_source(command[13].as_bytes(), b"/etc/netdata", b"localregistry");
            assert!(command[13].starts_with("line=1,file=/") && edit.ends_with(b"=localregistry"), "{command:?}");
            assert_eq!(command[27].as_bytes(), edit);
        }

        // an entry as a row without a source loads it
        let alerts = health.host(&hosts[0]).expect("the host's alerts");
        let mut entry = alerts.log_entries().into_iter().find(|entry| entry.new_status == Status::Warning).unwrap();
        (entry.source, entry.flags) = (None, 0);
        let sent = send(&hosts[0], &alerts, &entry, &mut RaisedSummary::default(), &health, &env, &|| NOW + 7);
        assert_eq!((sent.exec_run_timestamp, sent.save, sent.execution.is_some()), (Some(NOW + 7), true, true));
        let command = words(env.commands.borrow().last().expect("a command"));
        assert_eq!((command[13].as_str(), command[27].as_str()), ("UNKNOWN", "UNKNOWN=0=UNKNOWN"));

        // an entry as a row whose chart is the empty text loads it: C's entry has no chart, and its command says
        // NOCHART (`health_notifications.c:480`); no oracle scenario reaches it (D222)
        entry.chart = Vec::new();
        send(&hosts[0], &alerts, &entry, &mut RaisedSummary::default(), &health, &env, &|| NOW + 8);
        let command = words(env.commands.borrow().last().expect("a command"));
        assert_eq!(command[7..9], ["a", "NOCHART"]);
    }

    /// C's `health_send_notification()` over its decision table, through `send`: whether the table is asked and a
    /// command is spawned, the marks and the time the entry gets, that it is saved, and the record.
    #[test]
    fn the_decision_table_through_the_send() {
        let env = raising();
        let (health, hosts) = two_raised(&env);
        let (host, alerts) = (&hosts[0], health.host(&hosts[0]).expect("the host's alerts"));
        let mut template = alerts.log_entries().into_iter().next().expect("an entry");
        (template.chart, template.name) = (b"d.chart".to_vec(), Some(b"d_alert".to_vec()));
        let statuses = [
            Status::Removed,
            Status::Undefined,
            Status::Uninitialized,
            Status::Clear,
            Status::Raised,
            Status::Warning,
            Status::Critical,
        ];
        let status = |name: &str| *statuses.iter().find(|status| status.name() == name).expect("a status");

        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/decide.tsv");
        let table = std::fs::read_to_string(path).expect("decide.tsv");
        let (mut checked, mut failures) = (0, Vec::new());
        for (n, line) in table.lines().enumerate().filter(|(_, line)| !line.starts_with('#')) {
            let row: Vec<&str> = line.split('\t').collect();
            let flags = u32::from_str_radix(row[2], 16).expect("the flags");
            let entry = Entry { new_status: status(row[0]), old_status: status(row[1]), flags, ..template.clone() };
            let last_executed = match row[3] {
                "fail" | "none" => None,
                name => Some(status(name) as i32),
            };
            let env = Scripted { command: Some(0), last_executed, ..Scripted::default() };
            let (sent, records) = netdata_agent_log::capture(|| {
                send(host, &alerts, &entry, &mut RaisedSummary::default(), &health, &env, &|| NOW)
            });
            let trace = env.take_trace();
            let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
            let message = if messages.is_empty() { "-".to_owned() } else { messages.join(" | ") };
            let actual = (
                trace.iter().any(|what| what.starts_with("asked ")),
                trace.iter().any(|what| what.starts_with("spawn ")),
                // C's function sets the processed mark itself; here the caller does, before the send
                flags | entry_flags::PROCESSED | sent.flags,
                sent.exec_run_timestamp.is_some(),
                usize::from(sent.save),
                message,
            );
            let expected = (
                row[4] == "1",
                row[5] == "1",
                u32::from_str_radix(row[6], 16).expect("the flags after"),
                row[7] == "1",
                row[8].parse::<usize>().expect("the saves"),
                row[9].replace("[oracle-host]", "[testhost]"),
            );
            if actual != expected {
                failures.push(format!("decide.tsv:{}: {line}\n  C    {expected:?}\n  Rust {actual:?}", n + 1));
            }
            checked += 1;
        }
        let shown = failures[..failures.len().min(8)].join("\n");
        assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
        assert_eq!(checked, 7056);
    }

    /// The record of a kill carries the errno the last slice's wait left: ECANCELED when the thread was cancelled.
    #[test]
    fn the_kill_s_record_has_the_errno_of_the_wait() {
        let env = Scripted { command: Some(usize::MAX), wait_errno: 125, ..Scripted::default() };
        env.running_for.set(Some(0));
        let command = env.exec(b"x").expect("a command");
        let (code, records) = netdata_agent_log::capture(|| wait_for_execution(b"a", command, 120, &env));
        assert_eq!((code, records.len(), records[0].errno), (128, 1, 125));
    }

    #[test]
    fn a_duration_saturates_as_c() {
        let cases = [(-5, 0), (0, 0), (7, 7), (i64::from(u32::MAX), u32::MAX), (i64::from(u32::MAX) + 1, u32::MAX)];
        for (duration, want) in cases {
            assert_eq!(duration_to_u32(duration), want, "{duration}");
        }
    }
}
