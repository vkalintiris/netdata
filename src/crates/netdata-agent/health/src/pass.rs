//! A host's health pass (`health_event_loop.c` `health_event_loop_for_host()`): what it needs from the daemon,
//! and its three walks over the host's alerts: the values (lookup and calculation), the statuses (warning and
//! critical, with the transitions they cause), and the repeats.

use std::sync::Arc;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_query::value::{ValueRequest, ValueResult};
use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;

use crate::alert::{Alert, Run, Status, run_flags};
use crate::alerts::HostAlerts;
use crate::entry::Entry;
use crate::keywords::lossy;
use crate::log::AlarmLog;
use crate::prototype::AlertConfig;
use crate::variable::{AlertResolver, This};
use crate::{Clock, Health, journal, lookup};

/// What the runnable test reads of a chart (`rrdcalc_isrunnable()`).
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
    /// `rrdset_first_entry_s()` and `rrdset_last_entry_s()`.
    pub first_entry_s: i64,
    pub last_entry_s: i64,
}

/// What the daemon gives the pass; a test's is scripted.
pub trait Env {
    /// The chart as the runnable test and the obsolete rule see it.
    fn facts(&self, chart: &Chart) -> ChartFacts;
    /// The database lookup of an alert (`rrdset2value_api_v1_with_owa()`). Nothing is locked while it runs.
    fn lookup(&self, host: &Arc<Host>, chart: &Arc<Chart>, request: &ValueRequest) -> ValueResult;
    /// `now_realtime_usec()`: an entry's `global_id`.
    fn now_usec(&self) -> u64;
    /// A new random UUID: an entry's transition id.
    fn transition_id(&self) -> [u8; 16];
    /// `exit_initiated_get()`: once the agent's exit has begun an unlink logs nothing.
    fn exiting(&self) -> bool;
    /// `health_alarm_log_save()`: the entry goes to the database. C's insert marks it as saved.
    fn save(&self, entry: &mut Entry, is_async: bool);
    /// The entry is due: its notification (`health_send_notification()` before it marks the entry as processed).
    fn notify(&self, entry: &mut Entry);
}

/// An [`Env`] for a caller that only links and unlinks: no chart is collected, no lookup answers, nothing is
/// saved or notified, and every entry has the same zero ids.
pub struct Idle;

impl Env for Idle {
    fn facts(&self, _: &Chart) -> ChartFacts {
        ChartFacts {
            obsolete: false,
            last_collected_s: 0,
            counter_done: 0,
            update_every: 0,
            first_entry_s: 0,
            last_entry_s: 0,
        }
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

    fn save(&self, _: &mut Entry, _: bool) {}

    fn notify(&self, _: &mut Entry) {}
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
/// `next_run` to its next update.
pub fn is_runnable(run: &Run, config: &AlertConfig, facts: &ChartFacts, now: i64, next_run: &mut i64) -> bool {
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
    if add_compare(now, update_every, facts.first_entry_s).is_lt() {
        return false;
    }
    if config.after != 0 {
        let offset = i64::from(config.before) + i64::from(config.after);
        if add_compare(now, offset + update_every, facts.first_entry_s).is_lt()
            || add_compare(now, offset - update_every, facts.last_entry_s).is_gt()
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
            if !is_runnable(&run, &alert.config, &facts, now, pass.next_run) {
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
                    // the wait for the notification's execution comes with the notifications
                    AlarmLog::send_notification(&mut entry, env);
                }
            }
        }

        if complete {
            alerts.publish_counts(counts);
        }
        if stopped(pass) {
            return;
        }
        alerts.process_log(env, clock);
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
        let facts = ChartFacts {
            obsolete: false,
            last_collected_s: NOW,
            counter_done: 2,
            update_every: 2,
            first_entry_s: NOW - 100,
            last_entry_s: NOW,
        };
        let runnable = |run: &Run, config: &AlertConfig, facts: ChartFacts| {
            let mut next_run = NOW + 10;
            (is_runnable(run, config, &facts, NOW, &mut next_run), next_run)
        };
        assert_eq!(runnable(&idle_run(0), &plain, facts), (true, NOW + 10));
        // due now, and due a second later: the pass is asked to come back then
        assert_eq!(runnable(&idle_run(NOW), &plain, facts), (true, NOW + 10));
        assert_eq!(runnable(&idle_run(NOW + 1), &plain, facts), (false, NOW + 1));
        assert_eq!(runnable(&idle_run(NOW + 20), &plain, facts), (false, NOW + 10));

        let not = |facts: ChartFacts, config: &AlertConfig| !runnable(&idle_run(0), config, facts).0;
        assert!(not(ChartFacts { obsolete: true, ..facts }, &plain));
        assert!(not(ChartFacts { last_collected_s: 0, ..facts }, &plain));
        assert!(not(ChartFacts { counter_done: 1, ..facts }, &plain));
        // the first entry is ahead by more than the chart's update every, then by exactly that
        assert!(not(ChartFacts { first_entry_s: NOW + 3, ..facts }, &plain));
        assert!(!not(ChartFacts { first_entry_s: NOW + 2, ..facts }, &plain));
        // the lookup's window starts 15 seconds back: the chart's update every on both sides of it
        assert!(not(ChartFacts { first_entry_s: NOW - 12, ..facts }, &lookup));
        assert!(!not(ChartFacts { first_entry_s: NOW - 13, ..facts }, &lookup));
        assert!(not(ChartFacts { last_entry_s: NOW - 18, ..facts }, &lookup));
        assert!(!not(ChartFacts { last_entry_s: NOW - 17, ..facts }, &lookup));
        // a rule without a period
        let mut never = rule("");
        never.update_every = 0;
        assert!(not(facts, &never));
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
