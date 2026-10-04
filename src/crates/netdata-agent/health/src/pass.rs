//! A host's health pass (`health_event_loop.c` `health_event_loop_for_host()`): what it needs from the daemon,
//! and its three walks over the host's alerts: the values (lookup and calculation), the statuses (warning and
//! critical, with the transitions they cause), and the repeats.

use std::sync::Arc;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_metadata::health_log::LoadedRow;
use netdata_agent_query::value::{ValueRequest, ValueResult};
use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;

use crate::alert::{Alert, Run, Status, run_flags};
use crate::alerts::HostAlerts;
use crate::entry::Entry;
use crate::keywords::lossy;
use crate::notify::{Execution, RaisedSummary};
use crate::prototype::AlertConfig;
use crate::variable::{AlertResolver, This};
use crate::{Clock, Health, journal, lookup};

/// What the obsolete rule and the runnable test (`rrdcalc_isrunnable()`) read of a chart's collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChartFacts {
    /// `RRDSET_FLAG_OBSOLETE`.
    pub obsolete: bool,
    /// `st->last_collected_time.tv_sec`.
    pub last_collected_s: i64,
    /// `st->counter_done`.
    pub counter_done: usize,
    /// `st->update_every`.
    pub update_every: i32,
}

/// What the daemon gives the pass; a test's is scripted.
pub trait Env {
    /// The chart as the obsolete rule and the runnable test see it.
    fn facts(&self, chart: &Chart) -> ChartFacts;
    /// `rrdset_first_entry_s()` and `rrdset_last_entry_s()`: the span of the chart's data, which the runnable test
    /// asks for only once an alert is due on a collected chart. The alert's live fields are held meanwhile.
    fn retention(&self, chart: &Chart) -> (i64, i64);
    /// The database lookup of an alert (`rrdset2value_api_v1_with_owa()`). Nothing is locked while it runs.
    fn lookup(&self, host: &Arc<Host>, chart: &Arc<Chart>, request: &ValueRequest) -> ValueResult;
    /// `now_realtime_usec()`: an entry's `global_id`.
    fn now_usec(&self) -> u64;
    /// A new random UUID: an entry's transition id.
    fn transition_id(&self) -> [u8; 16];
    /// `exit_initiated_get()`: once the agent's exit has begun an unlink logs nothing.
    fn exiting(&self) -> bool;
    /// C's `is_health_thread`: a save the metadata queue refuses is made at once only on the HEALTH thread.
    fn is_health_thread(&self) -> bool;
    /// `service_running(SERVICE_HEALTH)`, as the pass's `running` looks at it.
    fn service_running(&self) -> bool;
    /// `sql_health_alarm_log_load()`'s statements at a host's first pass: the table gets a REMOVED entry for each
    /// alarm whose last saved one is none (`sql_check_removed_alerts_state()`), then gives the last entry of each
    /// alarm whose rule it knows. `None` without a database, or when the load's statement cannot be prepared: C
    /// then returns with the host's ids as the first pass seeded them. No lock of health is held while it runs.
    fn load(&self, host: &Host) -> Option<Vec<LoadedRow>>;
    /// `sql_get_alarm_id()`: the alarm id and the next event id the alert log's table has for a chart and a rule's
    /// name, whatever the rule's hash. No lock of health is held while it runs.
    fn sql_alarm_id(&self, host: &Host, chart: &[u8], name: Option<&[u8]>) -> Option<(u32, u32)>;
    /// `metadata_queue_ae_save()`: the metadata thread is asked to save the entry of that unique id, as it will
    /// stand when its store job runs (`HostAlerts::save_queued()`). False when the queue refuses.
    fn queue_save(&self, alerts: &Arc<HostAlerts>, unique_id: u32) -> bool;
    /// `sql_health_alarm_log_save()`: the entry's row is inserted, or updated when the entry is marked as saved.
    /// True when a row was inserted: the caller marks the entry as saved. No lock of health but the host's save
    /// lock is held while it runs.
    fn sql_save(&self, host: &Host, entry: &Entry) -> bool;
    /// `commit_alert_transitions()`: the metadata thread is asked for a store job now.
    fn commit_transitions(&self);
    /// `process_alert_pending_queue()`: the host's due rows of `alert_queue` move toward the Cloud's queue.
    fn process_pending_queue(&self, host: &Host) -> bool;
    /// `sql_health_get_last_executed_event()`: the status of the alarm's newest entry whose notification command
    /// was run, the entry `unique_id` aside. `None` without one, and when the question failed. No lock of health
    /// is held while it runs.
    fn last_executed_event(&self, host: &Host, alarm_id: u32, unique_id: u32) -> Option<i32>;
    /// `spawn_popen_run()`: the notification's command, started through the shell. `None` when it cannot be
    /// started. No lock of health is held while it runs.
    fn exec(&self, command: &[u8]) -> Option<Box<dyn Execution>>;
    /// `now_monotonic_usec()`: what a wait's deadline is counted on.
    fn monotonic_usec(&self) -> u64;
    /// What a notification's edit command is made of: the user configuration directory and localhost's registry
    /// hostname.
    fn edit_context(&self) -> (Vec<u8>, Vec<u8>);
}

/// An [`Env`] for a caller that only links and unlinks: no chart is collected, no lookup answers, nothing is
/// saved or notified, and every entry has the same zero ids.
pub struct Idle;

impl Env for Idle {
    fn facts(&self, _: &Chart) -> ChartFacts {
        ChartFacts { obsolete: false, last_collected_s: 0, counter_done: 0, update_every: 0 }
    }

    fn retention(&self, _: &Chart) -> (i64, i64) {
        (0, 0)
    }

    fn lookup(&self, _: &Arc<Host>, _: &Arc<Chart>, _: &ValueRequest) -> ValueResult {
        ValueResult { code: 500, value: f64::NAN, window: None, value_is_null: true }
    }

    fn now_usec(&self) -> u64 {
        0
    }

    fn transition_id(&self) -> [u8; 16] {
        [0; 16]
    }

    fn exiting(&self) -> bool {
        false
    }

    fn is_health_thread(&self) -> bool {
        true
    }

    fn service_running(&self) -> bool {
        true
    }

    fn load(&self, _: &Host) -> Option<Vec<LoadedRow>> {
        None
    }

    fn sql_alarm_id(&self, _: &Host, _: &[u8], _: Option<&[u8]>) -> Option<(u32, u32)> {
        None
    }

    fn queue_save(&self, _: &Arc<HostAlerts>, _: u32) -> bool {
        false
    }

    fn sql_save(&self, _: &Host, _: &Entry) -> bool {
        false
    }

    fn commit_transitions(&self) {}

    fn process_pending_queue(&self, _: &Host) -> bool {
        false
    }

    fn last_executed_event(&self, _: &Host, _: u32, _: u32) -> Option<i32> {
        None
    }

    fn exec(&self, _: &[u8]) -> Option<Box<dyn Execution>> {
        None
    }

    fn monotonic_usec(&self) -> u64 {
        0
    }

    fn edit_context(&self) -> (Vec<u8>, Vec<u8>) {
        (Vec::new(), Vec::new())
    }
}

/// One call of `health_event_loop_for_host()`.
pub struct Pass<'a> {
    /// The loop's `now`: read once per iteration, before the hosts.
    pub now: i64,
    /// The system was just resumed from suspension: health is postponed.
    pub apply_hibernation_delay: bool,
    /// When the loop runs next; an alert that is due earlier lowers it.
    pub next_run: &'a mut i64,
    /// `rrdhost_should_run_health()`.
    pub gate: &'a dyn Fn() -> bool,
}

/// `struct health_alert_status_counts`: the host's alerts by status, as a complete pass found them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PassCounts {
    pub clear: u32,
    pub warning: u32,
    pub critical: u32,
    pub undefined: u32,
    pub uninitialized: u32,
}

impl PassCounts {
    fn slot(&mut self, status: Status) -> Option<&mut u32> {
        match status {
            Status::Clear => Some(&mut self.clear),
            Status::Warning => Some(&mut self.warning),
            Status::Critical => Some(&mut self.critical),
            Status::Undefined => Some(&mut self.undefined),
            Status::Uninitialized => Some(&mut self.uninitialized),
            Status::Removed | Status::Raised => None,
        }
    }

    /// `health_alert_status_counts_add()`: REMOVED and RAISED count nowhere.
    fn add(&mut self, status: Status) {
        if let Some(count) = self.slot(status) {
            *count += 1;
        }
    }

    /// `health_alert_status_counts_sub()`.
    fn sub(&mut self, status: Status) {
        if let Some(count) = self.slot(status) {
            *count = count.saturating_sub(1);
        }
    }
}

/// `nd_time_t_add_compare()`: `a + b` against `c`, without overflow.
fn add_compare(a: i64, b: i64, c: i64) -> std::cmp::Ordering {
    (i128::from(a) + i128::from(b)).cmp(&i128::from(c))
}

/// `nd_time_t_elapsed_saturating()`: the seconds from `then` to `now`, 0 when there are none.
pub(crate) fn elapsed(now: i64, then: i64) -> i64 {
    if now <= then { 0 } else { now.saturating_sub(then) }
}

/// `rrdcalc_isrunnable()`: whether the alert is evaluated in a pass at `now`. An alert that is not due yet lowers
/// `next_run` to its next update. `retention` gives the first and the last second of the chart's data; as in C it
/// is asked only when every other reason not to run is ruled out.
pub fn is_runnable(
    run: &Run,
    config: &AlertConfig,
    facts: &ChartFacts,
    retention: impl FnOnce() -> (i64, i64),
    now: i64,
    next_run: &mut i64,
) -> bool {
    if run.next_update > now {
        if *next_run > run.next_update {
            *next_run = run.next_update;
        }
        return false;
    }
    if config.update_every <= 0 || facts.obsolete || facts.last_collected_s == 0 || facts.counter_done < 2 {
        return false;
    }

    // the chart's update every, also for the lookup's window
    let update_every = i64::from(facts.update_every);
    let (first_entry_s, last_entry_s) = retention();
    if add_compare(now, update_every, first_entry_s).is_lt() {
        return false;
    }
    if config.after != 0 {
        let offset = i64::from(config.before) + i64::from(config.after);
        if add_compare(now, offset + update_every, first_entry_s).is_lt()
            || add_compare(now, offset - update_every, last_entry_s).is_gt()
        {
            return false;
        }
    }
    true
}

/// `rrdcalc_value2status()`.
pub fn value2status(value: f64) -> Status {
    if !value.is_finite() {
        Status::Undefined
    } else if value != 0.0 {
        Status::Raised
    } else {
        Status::Clear
    }
}

/// The alert's status from what its warning and its critical expression say: a raised critical wins, then a
/// raised warning; a clear critical counts only when the warning says nothing.
pub fn combine(warning: Status, critical: Status) -> Status {
    let mut status = match warning {
        Status::Clear => Status::Clear,
        Status::Raised => Status::Warning,
        _ => Status::Undefined,
    };
    match critical {
        Status::Clear if status == Status::Undefined => status = Status::Clear,
        Status::Raised => status = Status::Critical,
        _ => {}
    }
    status
}

/// `health_delay_apply_multiplier()`: the delay times the multiplier, in C's float arithmetic, capped at `maximum`.
pub fn delay_apply_multiplier(delay: i32, multiplier: f32, maximum: i32) -> i32 {
    let float_delay = delay as f32;
    let wide = f64::from(float_delay) * f64::from(multiplier);
    if wide > f64::from(i32::MAX) {
        return maximum;
    }
    if wide < f64::from(i32::MIN) {
        return i32::MIN;
    }
    let multiplied = float_delay * multiplier;
    if f64::from(multiplied) > f64::from(maximum) {
        return maximum;
    }
    if f64::from(multiplied) < f64::from(i32::MIN) {
        return i32::MIN;
    }
    // cvttss2si answers a NaN with the smallest int
    if multiplied.is_nan() { i32::MIN } else { multiplied as i32 }
}

/// The trigger hysteresis of a status change at `now`: past the last delay's end the delays start again from the
/// rule's; inside it they grow by the multiplier. The delay is the up one for a change to a higher status. Returns
/// the delay; the alert's delay window ends that much after `now`.
pub(crate) fn hysteresis(run: &mut Run, config: &AlertConfig, status: Status, now: i64) -> i32 {
    if now > run.delay_up_to_timestamp {
        run.delay_up_current = config.delay_up_duration;
        run.delay_down_current = config.delay_down_duration;
        run.delay_last = 0;
        run.delay_up_to_timestamp = 0;
    } else {
        let multiply = |current| delay_apply_multiplier(current, config.delay_multiplier, config.delay_max_duration);
        run.delay_up_current = multiply(run.delay_up_current);
        run.delay_down_current = multiply(run.delay_down_current);
    }
    let delay = if status as i32 > run.status as i32 { run.delay_up_current } else { run.delay_down_current };
    run.delay_last = delay;
    run.delay_up_to_timestamp = now.saturating_add(i64::from(delay));
    delay
}

/// `health_silencers_update_disabled_silenced()`: clears the two flags and sets what a silencer says; true when the
/// alert is disabled. The silencers come with their own commit: nothing is ever disabled or silenced.
fn silence(run: &mut Run) -> bool {
    run.run_flags &= !(run_flags::DISABLED | run_flags::SILENCED);
    false
}

/// Which of an alert's expressions.
#[derive(Clone, Copy)]
enum Which {
    Calculation,
    Warning,
    Critical,
}

impl Health {
    /// `do_eval_expression()`: one expression of the alert, evaluated with nothing held but the alert's
    /// expressions. A calculation leaves its result as the alert's value (NaN when it fails); a warning or a
    /// critical expression answers with the status its result stands for, or with nothing when the alert has no
    /// such expression or it failed.
    fn evaluate(
        &self,
        host: &Arc<Host>,
        alerts: &HostAlerts,
        alert: &Arc<Alert>,
        which: Which,
        clock: Clock,
    ) -> Option<Status> {
        let mut expressions = alert.expressions();
        let (expression, error_flag) = match which {
            Which::Calculation => (&mut expressions.calculation, run_flags::CALC_ERROR),
            Which::Warning => (&mut expressions.warning, run_flags::WARN_ERROR),
            Which::Critical => (&mut expressions.critical, run_flags::CRIT_ERROR),
        };
        let expression = expression.as_mut()?;
        let is_calculation = matches!(which, Which::Calculation);

        let failed = |run: &mut Run| {
            run.run_flags |= error_flag;
            if is_calculation {
                run.value = f64::NAN;
            }
        };
        if !alerts.is_linked(host, alert) {
            failed(&mut alert.run());
            return None;
        }

        // the alert's own fields are copied out: the resolver reads other alerts through the store
        let (value, db_after, db_before, status) = {
            let run = alert.run();
            (run.value, run.db_after, run.db_before, run.status)
        };
        let this = This { alert: Some(alert), chart: &alert.chart, value, db_after, db_before, status };
        let mut resolver = AlertResolver { host, alerts, this, clock };
        let ok = expression.evaluate(&mut resolver);

        let mut run = alert.run();
        if !ok {
            failed(&mut run);
            return None;
        }
        run.run_flags &= !error_flag;
        if is_calculation {
            run.value = expression.result();
            None
        } else {
            Some(value2status(expression.result()))
        }
    }

    /// The three walks of `health_event_loop_for_host()` and what follows them, for a host that is linked and not
    /// postponed.
    pub(crate) fn evaluate_host(
        &self,
        host: &Arc<Host>,
        alerts: &HostAlerts,
        pass: &mut Pass,
        env: &dyn Env,
        clock: Clock,
        running: &dyn Fn() -> bool,
    ) {
        let now = pass.now;
        let stopped = |pass: &Pass| !running() || !(pass.gate)();
        let hostname = host.hostname();
        let mut counts = PassCounts::default();
        let mut complete = true;
        let mut runnable = 0usize;
        // built by the pass's first notification, if any
        let mut summary = RaisedSummary::default();

        // the values
        for alert in alerts.alerts() {
            if stopped(pass) {
                complete = false;
                break;
            }
            if !alerts.is_linked(host, &alert) {
                let mut run = alert.run();
                if run.run_flags & run_flags::RUNNABLE != 0 {
                    run.run_flags &= !run_flags::RUNNABLE;
                    alert.publish_run_flags(&run);
                }
                continue;
            }

            // one read serves the obsolete rule and the runnable test; C reads the flag and the collection time
            // for each, so a collection that lands between its two reads shows a pass earlier there
            let facts = env.facts(&alert.chart);
            let remove = {
                let mut run = alert.run();
                counts.add(run.status);
                alert.update_info_using_labels(&mut run);
                if silence(&mut run) {
                    alert.publish_run_flags(&run);
                    continue;
                }
                // an obsolete chart that was not collected for more than a minute loses its alerts, unless they
                // repeat
                run.status != Status::Removed
                    && facts.obsolete
                    && add_compare(facts.last_collected_s, 60, now).is_lt()
                    && !alert.is_repeating()
            };
            let removed = if remove { alerts.obsolete_removed(host, &alert, env, clock) } else { None };
            if let Some((entry, old_status)) = &removed {
                journal::log_alert(&hostname, entry);
                counts.sub(*old_status);
            }

            let mut run = alert.run();
            if !is_runnable(&run, &alert.config, &facts, || env.retention(&alert.chart), now, pass.next_run) {
                run.run_flags &= !run_flags::RUNNABLE;
                match &removed {
                    Some((entry, _)) => alert.publish(&run, Some((entry.global_id, entry.transition_id))),
                    None => alert.publish_run_flags(&run),
                }
                continue;
            }
            runnable += 1;
            run.old_value = run.value;
            run.run_flags |= run_flags::RUNNABLE;
            drop(run);

            if alert.config.has_db_lookup() {
                let result = env.lookup(host, &alert.chart, &lookup::request(&alert.config));
                lookup::apply(&mut alert.run(), &result);
            }
            self.evaluate(host, alerts, &alert, Which::Calculation, clock);
        }

        if runnable > 0 && running() {
            // the statuses
            for alert in alerts.alerts() {
                if stopped(pass) {
                    complete = false;
                    break;
                }
                {
                    let run = alert.run();
                    if run.run_flags & run_flags::RUNNABLE == 0 || run.run_flags & run_flags::DISABLED != 0 {
                        continue;
                    }
                }
                let unlinked = |alert: &Alert| {
                    let mut run = alert.run();
                    run.run_flags &= !run_flags::RUNNABLE;
                    alert.publish_run_flags(&run);
                };
                if !alerts.is_linked(host, &alert) {
                    unlinked(&alert);
                    continue;
                }

                let warning = self.evaluate(host, alerts, &alert, Which::Warning, clock);
                let critical = self.evaluate(host, alerts, &alert, Which::Critical, clock);
                let status = combine(warning.unwrap_or(Status::Undefined), critical.unwrap_or(Status::Undefined));

                let mut entry = None;
                let old_status = alert.run().status;
                if status != old_status {
                    let Some(transition) = alerts.transition(host, &alert, status, now, env) else {
                        unlinked(&alert);
                        continue;
                    };
                    journal::log_alert(&hostname, &transition);
                    nd_log!(
                        Source::Daemon,
                        Priority::Debug,
                        "[{hostname}]: Alert event for [{}.{}], value [{}], status [{}].",
                        lossy(&transition.chart),
                        lossy(transition.name.as_deref().unwrap_or(b"")),
                        lossy(&transition.new_value_string),
                        transition.new_status.name()
                    );
                    counts.sub(old_status);
                    counts.add(status);
                    entry = Some(transition);
                }

                let mut run = alert.run();
                run.last_updated = now;
                run.next_update = now.saturating_add(i64::from(alert.config.update_every));
                alert.publish(&run, entry.as_ref().map(|entry| (entry.global_id, entry.transition_id)));
                if *pass.next_run > run.next_update {
                    *pass.next_run = run.next_update;
                }
            }

            // the repeats
            for alert in alerts.alerts() {
                if stopped(pass) {
                    break;
                }
                let repeat_every = {
                    let mut run = alert.run();
                    if !alert.is_repeating() || run.delay_up_to_timestamp > now {
                        continue;
                    }
                    let flags_before = run.run_flags;
                    let raised = |status| matches!(status, Status::Warning | Status::Critical);
                    // C's repeat period is an int
                    let repeat_every = match run.status {
                        Status::Warning => {
                            run.run_flags &= !run_flags::RUN_ONCE;
                            alert.config.warn_repeat_every as i32
                        }
                        Status::Critical => {
                            run.run_flags &= !run_flags::RUN_ONCE;
                            alert.config.crit_repeat_every as i32
                        }
                        Status::Clear if run.run_flags & run_flags::RUN_ONCE == 0 && raised(run.old_status) => 1,
                        _ => 0,
                    };
                    if run.run_flags != flags_before {
                        alert.publish_run_flags(&run);
                    }
                    if repeat_every <= 0 || add_compare(run.last_repeat, i64::from(repeat_every), now).is_gt() {
                        continue;
                    }
                    repeat_every
                };
                debug_assert!(repeat_every > 0);
                if let Some(mut entry) = alerts.repeat(host, &alert, now, env) {
                    journal::log_alert(&hostname, &entry);
                    alerts.notify_repeat(host, &mut entry, &mut summary, self, env, clock);
                }
            }
        }

        if complete {
            alerts.publish_counts(counts);
        }
        if stopped(pass) {
            return;
        }
        alerts.process_log(host, &mut summary, self, env, clock);

        // saves the metadata queue took: a store job is asked for now. With none pending the host's due rows of
        // the pending queue move on (the ACLK's snapshot branch comes with the Cloud)
        if alerts.pending_transitions() != 0 {
            env.commit_transitions();
        }
        if alerts.pending_transitions() == 0 {
            env.process_pending_queue(host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_comes_from_the_two_expressions() {
        use Status::*;
        for (warning, critical, status) in [
            (Undefined, Undefined, Undefined),
            (Clear, Undefined, Clear),
            (Raised, Undefined, Warning),
            (Undefined, Clear, Clear),
            (Undefined, Raised, Critical),
            (Clear, Clear, Clear),
            (Clear, Raised, Critical),
            (Raised, Clear, Warning),
            (Raised, Raised, Critical),
        ] {
            assert_eq!(combine(warning, critical), status, "{warning:?} {critical:?}");
        }
        assert_eq!(value2status(f64::NAN), Undefined);
        assert_eq!(value2status(f64::INFINITY), Undefined);
        assert_eq!(value2status(0.0), Clear);
        assert_eq!(value2status(-0.5), Raised);
    }

    fn idle_run(next_update: i64) -> Run {
        Run {
            next_event_id: 1,
            value: f64::NAN,
            old_value: f64::NAN,
            status: Status::Uninitialized,
            old_status: Status::Uninitialized,
            run_flags: 0,
            last_status_change: 0,
            last_status_change_value: f64::NAN,
            last_updated: 0,
            next_update,
            db_after: 0,
            db_before: 0,
            delay_up_to_timestamp: 0,
            delay_up_current: 0,
            delay_down_current: 0,
            delay_last: 0,
            last_repeat: 0,
            times_repeat: 0,
            labels_version: 0,
        }
    }

    /// `rrdcalc_isrunnable()`: each reason alone, at its boundary. C's own answers for a chart collected every 2
    /// seconds are in `tests/vectors/loop.tsv` (scenario `runnable`).
    #[test]
    fn an_alert_runs_when_it_is_due_and_its_chart_has_the_data() {
        const NOW: i64 = 1_700_000_000;
        let rule = |lookup: &str| {
            let text = format!("template: r\n on: t.ctx\n {lookup}every: 5s\n calc: 1\n\n");
            let health = crate::testing::health_with(&text);
            let prototypes = health.prototypes();
            crate::alert::copy_config(&prototypes.get(b"r").unwrap().rules()[0].config).0
        };
        let (plain, lookup) = (rule(""), rule("lookup: average -10s at -5s\n "));
        let facts = ChartFacts { obsolete: false, last_collected_s: NOW, counter_done: 2, update_every: 2 };
        let span = (NOW - 100, NOW);
        // the answer, the pass's next run, and whether the chart's span was asked for
        let runnable = |run: &Run, config: &AlertConfig, facts: ChartFacts, span: (i64, i64)| {
            let (mut next_run, mut asked) = (NOW + 10, false);
            let retention = || {
                asked = true;
                span
            };
            (is_runnable(run, config, &facts, retention, NOW, &mut next_run), next_run, asked)
        };
        assert_eq!(runnable(&idle_run(0), &plain, facts, span), (true, NOW + 10, true));
        // due now, and due a second later: the pass is asked to come back then
        assert_eq!(runnable(&idle_run(NOW), &plain, facts, span), (true, NOW + 10, true));
        assert_eq!(runnable(&idle_run(NOW + 1), &plain, facts, span), (false, NOW + 1, false));
        assert_eq!(runnable(&idle_run(NOW + 20), &plain, facts, span), (false, NOW + 10, false));

        // what the chart's collection rules out costs no read of its span
        let not_collected = |facts: ChartFacts| runnable(&idle_run(0), &plain, facts, span) == (false, NOW + 10, false);
        assert!(not_collected(ChartFacts { obsolete: true, ..facts }));
        assert!(not_collected(ChartFacts { last_collected_s: 0, ..facts }));
        assert!(not_collected(ChartFacts { counter_done: 1, ..facts }));

        let not = |span: (i64, i64), config: &AlertConfig| !runnable(&idle_run(0), config, facts, span).0;
        // the first entry is ahead by more than the chart's update every, then by exactly that
        assert!(not((NOW + 3, NOW), &plain));
        assert!(!not((NOW + 2, NOW), &plain));
        // the lookup's window starts 15 seconds back: the chart's update every on both sides of it
        assert!(not((NOW - 12, NOW), &lookup));
        assert!(!not((NOW - 13, NOW), &lookup));
        assert!(not((NOW - 100, NOW - 18), &lookup));
        assert!(!not((NOW - 100, NOW - 17), &lookup));
        // a rule without a period
        let mut never = rule("");
        never.update_every = 0;
        assert_eq!(runnable(&idle_run(0), &never, facts, span), (false, NOW + 10, false));
    }

    /// A chart that leaves the host's index between its alert's evaluation and the status change that follows
    /// (the clock is read in between, for `$now`), while its free has not reached health: the change is not made,
    /// nothing is logged, and the alert is no longer marked as runnable. The four other places where a pass finds
    /// an alert without its chart are C's to judge (`tests/corpus/loop/free.scn`).
    #[test]
    fn a_status_change_of_an_alert_whose_chart_just_left_is_not_made() {
        use crate::testing::{Scripted, chart, health_with, host, rule_text};
        const NOW: i64 = 1_700_000_000;
        let health = health_with(&rule_text("template", "a", "t.ctx", &["warn: $now > 0"]));
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        health.host_link(&host, &|| NOW, &|| true);
        let alerts = health.host(&host).expect("the host's alerts");
        let alert = Arc::clone(&alerts.chart_alerts(&c)[0]);
        assert_eq!((alerts.log_entries().len(), alerts.transitions()), (1, 1), "the link's entry");

        let env = Scripted { collected: Some(NOW), ..Scripted::default() };
        // the pass's first read of the clock is the warning expression's
        let reads = std::cell::Cell::new(0);
        let clock = || {
            if reads.replace(reads.get() + 1) == 0 {
                assert_eq!(alert.run().run_flags & run_flags::RUNNABLE, run_flags::RUNNABLE);
                assert!(host.charts().free_if(&c, |_| true));
            }
            NOW
        };
        let mut next_run = NOW + 100;
        let mut pass = Pass { now: NOW, apply_hibernation_delay: false, next_run: &mut next_run, gate: &|| true };
        health.evaluate_host(&host, &alerts, &mut pass, &env, &clock, &|| true);

        assert!(reads.get() >= 1);
        let run = alert.run();
        assert_eq!((run.status, run.last_updated, run.next_update), (Status::Uninitialized, 0, 0));
        assert_eq!((run.run_flags, alert.snapshot().run_flags), (0, 0));
        assert_eq!(run.value, 1.0, "the calculation ran, in the first walk");
        assert_eq!((alerts.log_entries().len(), alerts.transitions()), (1, 1));
        assert_eq!(alerts.pass_counts(), Some(PassCounts { uninitialized: 1, ..PassCounts::default() }));
        assert_eq!(next_run, NOW + 100);
    }

    /// The hysteresis: past the last delay's end the rule's delays apply again; at its last second and before, the
    /// current ones grow. A change to a lower status, UNDEFINED after UNINITIALIZED among them, takes the down delay.
    #[test]
    fn the_delays_grow_inside_a_delay_and_start_again_after_it() {
        let text = "template: d\n on: t.ctx\n every: 1s\n calc: 1\n delay: up 4s down 6s multiplier 2 max 20s\n\n";
        let health = crate::testing::health_with(text);
        let prototypes = health.prototypes();
        let config = crate::alert::copy_config(&prototypes.get(b"d").unwrap().rules()[0].config).0;
        let mut run = idle_run(0);

        assert_eq!(hysteresis(&mut run, &config, Status::Clear, 100), 4);
        assert_eq!((run.delay_up_to_timestamp, run.delay_last), (104, 4));
        run.status = Status::Clear;
        // at the window's last second: not past it
        assert_eq!(hysteresis(&mut run, &config, Status::Warning, 104), 8);
        assert_eq!((run.delay_up_current, run.delay_down_current, run.delay_up_to_timestamp), (8, 12, 112));
        run.status = Status::Warning;
        assert_eq!(hysteresis(&mut run, &config, Status::Clear, 105), 20, "24, capped");
        run.status = Status::Clear;
        // one second past the window
        assert_eq!(hysteresis(&mut run, &config, Status::Warning, 126), 4);
        assert_eq!((run.delay_up_current, run.delay_down_current), (4, 6));

        let mut first = idle_run(0);
        assert_eq!(hysteresis(&mut first, &config, Status::Undefined, 100), 6);
    }
}
