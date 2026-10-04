//! An alert: a rule linked to a chart (`rrdcalc.c`, `struct rrdcalc`).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use netdata_agent_eval::Expression;
use netdata_agent_rrd::chart::Chart;

use crate::expr::parse_logged;
use crate::prototype::AlertConfig;
use crate::template::replace_variables_with_labels;

/// `RRDCALC_STATUS`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Removed = -2,
    Undefined = -1,
    Uninitialized = 0,
    Clear = 1,
    Raised = 2,
    Warning = 3,
    Critical = 4,
}

impl Status {
    /// `rrdcalc_status2string()`.
    pub fn name(self) -> &'static str {
        match self {
            Status::Removed => "REMOVED",
            Status::Undefined => "UNDEFINED",
            Status::Uninitialized => "UNINITIALIZED",
            Status::Clear => "CLEAR",
            Status::Raised => "RAISED",
            Status::Warning => "WARNING",
            Status::Critical => "CRITICAL",
        }
    }
}

/// The three expressions of a rule as an alert of it gets them (`health_prototype_copy_config()`): calc, warn and
/// crit, each parsed again from its source text. A source that no longer parses (the reader wrote `green` and
/// `red` into it) logs the evaluator's line, and the alert goes without that expression.
pub fn copy_expressions(config: &AlertConfig) -> [Option<Expression>; 3] {
    [&config.calculation, &config.warning, &config.critical]
        .map(|expression| expression.as_ref().and_then(|expression| parse_logged(expression.source()).ok()))
}

/// `health_prototype_copy_config()`: the rule's configuration for an alert. The expressions come apart: the alert
/// evaluates its own.
pub fn copy_config(src: &AlertConfig) -> (AlertConfig, [Option<Expression>; 3]) {
    let config = AlertConfig {
        hash_id: src.hash_id,
        name: src.name.clone(),
        exec: src.exec.clone(),
        recipient: src.recipient.clone(),
        classification: src.classification.clone(),
        component: src.component.clone(),
        r#type: src.r#type.clone(),
        source_type: src.source_type,
        source: src.source.clone(),
        units: src.units.clone(),
        summary: src.summary.clone(),
        info: src.info.clone(),
        update_every: src.update_every,
        alert_action_options: src.alert_action_options,
        dimensions: src.dimensions.clone(),
        time_group: src.time_group,
        time_group_condition: src.time_group_condition,
        time_group_value: src.time_group_value,
        dims_group: src.dims_group,
        data_source: src.data_source,
        before: src.before,
        after: src.after,
        options: src.options,
        calculation: None,
        warning: None,
        critical: None,
        delay_up_duration: src.delay_up_duration,
        delay_down_duration: src.delay_down_duration,
        delay_max_duration: src.delay_max_duration,
        delay_multiplier: src.delay_multiplier,
        has_custom_repeat_config: src.has_custom_repeat_config,
        warn_repeat_every: src.warn_repeat_every,
        crit_repeat_every: src.crit_repeat_every,
    };
    (config, copy_expressions(src))
}

/// `RRDCALC_FLAG_*`: an alert's run flags.
pub mod run_flags {
    pub const DB_ERROR: u32 = 1 << 0;
    pub const DB_NAN: u32 = 1 << 1;
    pub const CALC_ERROR: u32 = 1 << 3;
    pub const WARN_ERROR: u32 = 1 << 4;
    pub const CRIT_ERROR: u32 = 1 << 5;
    pub const RUNNABLE: u32 = 1 << 6;
    pub const DISABLED: u32 = 1 << 7;
    pub const SILENCED: u32 = 1 << 8;
    pub const RUN_ONCE: u32 = 1 << 9;
}

/// What the health loop works on: the live fields of `struct rrdcalc`.
#[derive(Debug)]
pub struct Run {
    /// The event id the alert's next log entry takes.
    pub next_event_id: u32,
    pub value: f64,
    pub old_value: f64,
    pub status: Status,
    pub old_status: Status,
    pub run_flags: u32,
    pub last_status_change: i64,
    pub last_status_change_value: f64,
    pub last_updated: i64,
    pub next_update: i64,
    pub db_after: i64,
    pub db_before: i64,
    pub delay_up_to_timestamp: i64,
    pub delay_up_current: i32,
    pub delay_down_current: i32,
    pub delay_last: i32,
    pub last_repeat: i64,
    pub times_repeat: u32,
    /// The chart's label version the runtime texts were made for.
    pub labels_version: u32,
}

/// `RRDCALC_RUNTIME_SNAPSHOT` and the runtime `summary` and `info`: what the API reads of an alert.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub status: Status,
    pub run_flags: u32,
    pub value: f64,
    pub last_updated: i64,
    pub last_status_change: i64,
    pub last_status_change_value: f64,
    /// The `global_id` and transition id of the last entry published with the snapshot.
    pub global_id: u64,
    pub last_transition_id: [u8; 16],
    pub next_update: i64,
    pub db_after: i64,
    pub db_before: i64,
    pub delay_up_to_timestamp: i64,
    pub last_repeat: i64,
    pub delay_last: i32,
    pub times_repeat: u32,
    pub summary: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
}

/// The three expressions of an alert. Only the health loop evaluates them, and it holds nothing else while it
/// does: the variables an expression names are read through the host's store and other alerts' live fields.
#[derive(Debug)]
pub struct Expressions {
    pub calculation: Option<Expression>,
    pub warning: Option<Expression>,
    pub critical: Option<Expression>,
}

/// An expression's two texts, as the API shows them: fixed when the alert is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionText {
    pub source: Vec<u8>,
    pub parsed_as: Vec<u8>,
}

/// `RRDCALC`: one rule on one chart.
#[derive(Debug)]
pub struct Alert {
    /// `rc->id`: the alarm id.
    pub id: u32,
    /// `NAME,on[CHART_ID]`.
    pub key: Vec<u8>,
    /// `rc->rrdset`; `rc->chart` is its id.
    pub chart: Arc<Chart>,
    /// The rule's copy, without its expressions.
    pub config: AlertConfig,
    /// The texts of calc, warn and crit.
    pub texts: [Option<ExpressionText>; 3],
    run: Mutex<Run>,
    expressions: Mutex<Expressions>,
    snapshot: RwLock<Snapshot>,
}

impl Alert {
    /// `rrdcalc_rrdhost_insert_callback()`, from the copy of the rule to the first published snapshot: the alert of
    /// `rule` on `chart`, not yet linked. `id` and `next_event_id` are what the host's log gave for it.
    pub(crate) fn new(
        key: Vec<u8>,
        chart: &Arc<Chart>,
        rule: &AlertConfig,
        id: u32,
        next_event_id: u32,
        now: i64,
    ) -> Alert {
        let (mut config, [calculation, warning, critical]) = copy_config(rule);
        if config.units.is_none() {
            let units = chart.meta().units;
            config.units = (!units.is_empty()).then(|| units.into_bytes());
        }
        let run = Run {
            next_event_id,
            value: f64::NAN,
            old_value: f64::NAN,
            status: Status::Uninitialized,
            old_status: Status::Uninitialized,
            run_flags: 0,
            last_status_change: now,
            last_status_change_value: f64::NAN,
            last_updated: 0,
            next_update: 0,
            db_after: 0,
            db_before: 0,
            delay_up_to_timestamp: 0,
            delay_up_current: 0,
            delay_down_current: 0,
            delay_last: 0,
            last_repeat: 0,
            times_repeat: 0,
            labels_version: 0,
        };
        let snapshot = Snapshot {
            status: run.status,
            run_flags: run.run_flags,
            value: run.value,
            last_updated: run.last_updated,
            last_status_change: run.last_status_change,
            last_status_change_value: run.last_status_change_value,
            global_id: 0,
            last_transition_id: [0; 16],
            next_update: run.next_update,
            db_after: run.db_after,
            db_before: run.db_before,
            delay_up_to_timestamp: run.delay_up_to_timestamp,
            last_repeat: run.last_repeat,
            delay_last: run.delay_last,
            times_repeat: run.times_repeat,
            summary: None,
            info: None,
        };
        let text = |expression: &Option<Expression>| {
            expression.as_ref().map(|expression| ExpressionText {
                source: expression.source().to_vec(),
                parsed_as: expression.parsed_as().to_vec(),
            })
        };
        let texts = [text(&calculation), text(&warning), text(&critical)];
        let alert = Alert {
            id,
            key,
            chart: Arc::clone(chart),
            config,
            texts,
            run: Mutex::new(run),
            expressions: Mutex::new(Expressions { calculation, warning, critical }),
            snapshot: RwLock::new(snapshot),
        };
        alert.update_info_using_labels(&mut alert.run());
        alert
    }

    /// `rrdcalc_isrepeating()`.
    pub fn is_repeating(&self) -> bool {
        self.config.warn_repeat_every > 0 || self.config.crit_repeat_every > 0
    }

    pub fn name(&self) -> &[u8] {
        self.config.name.as_deref().unwrap_or(b"")
    }

    /// The live fields, for the health loop.
    pub fn run(&self) -> MutexGuard<'_, Run> {
        self.run.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the API reads.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The alert's expressions, for the health loop. Taken before the host's store and before any alert's live
    /// fields, never after them.
    pub(crate) fn expressions(&self) -> MutexGuard<'_, Expressions> {
        self.expressions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `rrdcalc_runtime_snapshot_publish()`: the API reads these live fields from now on. The ids of the last
    /// entry change only when one is given.
    pub fn publish(&self, run: &Run, entry: Option<(u64, [u8; 16])>) {
        let mut snapshot = self.snapshot.write().unwrap_or_else(PoisonError::into_inner);
        snapshot.status = run.status;
        snapshot.run_flags = run.run_flags;
        snapshot.value = run.value;
        snapshot.last_updated = run.last_updated;
        snapshot.last_status_change = run.last_status_change;
        snapshot.last_status_change_value = run.last_status_change_value;
        snapshot.next_update = run.next_update;
        snapshot.db_after = run.db_after;
        snapshot.db_before = run.db_before;
        snapshot.delay_up_to_timestamp = run.delay_up_to_timestamp;
        snapshot.last_repeat = run.last_repeat;
        snapshot.delay_last = run.delay_last;
        snapshot.times_repeat = run.times_repeat;
        if let Some((global_id, transition_id)) = entry {
            snapshot.global_id = global_id;
            snapshot.last_transition_id = transition_id;
        }
    }

    /// `rrdcalc_runtime_snapshot_publish_run_flags()`.
    pub fn publish_run_flags(&self, run: &Run) {
        self.snapshot.write().unwrap_or_else(PoisonError::into_inner).run_flags = run.run_flags;
    }

    /// `rrdcalc_runtime_snapshot_publish_repeat_state()`.
    pub fn publish_repeat_state(&self, run: &Run) {
        let mut snapshot = self.snapshot.write().unwrap_or_else(PoisonError::into_inner);
        snapshot.run_flags = run.run_flags;
        snapshot.last_repeat = run.last_repeat;
        snapshot.times_repeat = run.times_repeat;
    }

    /// `rrdcalc_update_info_using_rrdset_labels()`: the runtime `info` and `summary`, made again when the chart's
    /// label version is not the one they were made for; an empty result leaves the configured text.
    pub fn update_info_using_labels(&self, run: &mut Run) {
        // every alert of every pass comes here: the chart's metadata is read in place, its labels only when
        // their version moved
        let replaced = self.chart.with_meta(|meta| {
            let labels_version = meta.labels.version();
            (run.labels_version != labels_version).then(|| {
                let replace = |text: &Option<Vec<u8>>| {
                    let text = text.as_deref().unwrap_or(b"");
                    replace_variables_with_labels(text, meta.family.as_bytes(), Some(&meta.labels))
                };
                (labels_version, replace(&self.config.info), replace(&self.config.summary))
            })
        });
        let mut snapshot = self.snapshot.write().unwrap_or_else(PoisonError::into_inner);
        if let Some((labels_version, info, summary)) = replaced {
            snapshot.info = info;
            snapshot.summary = summary;
            run.labels_version = labels_version;
        }
        if snapshot.summary.is_none() {
            snapshot.summary.clone_from(&self.config.summary);
        }
        if snapshot.info.is_none() {
            snapshot.info.clone_from(&self.config.info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{chart, health_with, host};

    /// An alert holds every field of its rule but the expressions, which it parses for itself.
    #[test]
    fn the_copy_holds_every_field_of_the_rule() {
        let text = "template: full\n on: t.ctx\n class: Errors\n type: System\n component: Disk\n \
                    lookup: sum -5m at -1m unaligned percentage of a,b\n every: 13s\n units: things\n \
                    summary: a summary\n info: an info\n delay: up 2m down 3m multiplier 1.5 max 1h\n \
                    repeat: warning 7s critical 11s\n options: no-clear-notification\n exec: /bin/true\n \
                    to: someone\n\n";
        let health = health_with(text);
        let prototypes = health.prototypes();
        let rule = &prototypes.get(b"full").unwrap().rules()[0].config;
        let stated = format!("{rule:?}");
        // a value of its own in each field that has a neighbour of its type
        for field in [
            "update_every: 13",
            "before: -60",
            "after: -300",
            "delay_up_duration: 120",
            "delay_down_duration: 180",
            "delay_max_duration: 3600",
            "delay_multiplier: 1.5",
            "warn_repeat_every: 7",
            "crit_repeat_every: 11",
        ] {
            assert!(stated.contains(field), "{field} in {stated}");
        }
        assert!(rule.calculation.is_none() && rule.warning.is_none() && rule.critical.is_none());
        assert_eq!(format!("{:?}", copy_config(rule).0), stated);
    }

    /// The runtime summary and info are made when the alert is: the family and the labels of its chart filled in;
    /// a text that comes out empty falls back to the configured one; a rule without them has none.
    #[test]
    fn an_alert_s_texts_are_made_at_its_creation() {
        let text = "template: texts\n on: t.ctx\n every: 10s\n calc: 1\n \
                    summary: ${family} of ${label:kind}\n info: ${family}\n\n\
                    template: bare\n on: t.ctx\n every: 10s\n calc: 1\n\n";
        let health = health_with(text);
        let prototypes = health.prototypes();
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[("kind", "x")]);
        let alert = |name: &[u8], chart: &Arc<Chart>| {
            Alert::new(b"key".to_vec(), chart, &prototypes.get(name).unwrap().rules()[0].config, 1, 1, 0).snapshot()
        };

        let texts = alert(b"texts", &c);
        assert_eq!(texts.summary.as_deref(), Some(&b"family of x"[..]));
        assert_eq!(texts.info.as_deref(), Some(&b"family"[..]));
        let bare = alert(b"bare", &c);
        assert_eq!((bare.summary, bare.info), (None, None));

        // a chart without a family: the info comes out empty, and the configured text stands
        let d = chart(&host, "t.d", None, "t.ctx", &[("kind", "y")]);
        d.update_meta(|meta| meta.family.clear());
        let texts = alert(b"texts", &d);
        assert_eq!(texts.summary.as_deref(), Some(&b" of y"[..]));
        assert_eq!(texts.info.as_deref(), Some(&b"${family}"[..]));
    }
}
