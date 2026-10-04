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

/// What the health loop works on: the live fields of `struct rrdcalc`.
#[derive(Debug)]
pub struct Run {
    pub calculation: Option<Expression>,
    pub warning: Option<Expression>,
    pub critical: Option<Expression>,
    /// The event id the alert's next log entry takes.
    pub next_event_id: u32,
    pub value: f64,
    pub old_value: f64,
    pub status: Status,
    pub old_status: Status,
    pub last_status_change: i64,
    pub last_status_change_value: f64,
    pub db_after: i64,
    pub db_before: i64,
    /// The chart's label version the runtime texts were made for.
    pub labels_version: u32,
}

/// `RRDCALC_RUNTIME_SNAPSHOT` and the runtime `summary` and `info`: what the API reads of an alert.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub status: Status,
    pub value: f64,
    pub db_after: i64,
    pub db_before: i64,
    pub last_status_change: i64,
    pub summary: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
}

impl Snapshot {
    /// The live fields as they are published, beside the runtime texts.
    fn of(run: &Run, summary: Option<Vec<u8>>, info: Option<Vec<u8>>) -> Snapshot {
        Snapshot {
            status: run.status,
            value: run.value,
            db_after: run.db_after,
            db_before: run.db_before,
            last_status_change: run.last_status_change,
            summary,
            info,
        }
    }
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
    /// The rule's copy. Its expressions are in [`Run`].
    pub config: AlertConfig,
    run: Mutex<Run>,
    snapshot: RwLock<Snapshot>,
}

impl Alert {
    /// `rrdcalc_rrdhost_insert_callback()`, from the copy of the rule to the first published snapshot: the alert of
    /// `rule` on `chart`, not yet linked.
    pub(crate) fn new(key: Vec<u8>, chart: &Arc<Chart>, rule: &AlertConfig, id: u32, now: i64) -> Alert {
        let (mut config, [calculation, warning, critical]) = copy_config(rule);
        if config.units.is_none() {
            let units = chart.meta().units;
            config.units = (!units.is_empty()).then(|| units.into_bytes());
        }
        let run = Run {
            calculation,
            warning,
            critical,
            next_event_id: 1,
            value: f64::NAN,
            old_value: f64::NAN,
            status: Status::Uninitialized,
            old_status: Status::Uninitialized,
            last_status_change: now,
            last_status_change_value: f64::NAN,
            db_after: 0,
            db_before: 0,
            labels_version: 0,
        };
        let snapshot = Snapshot::of(&run, None, None);
        let (chart, run, snapshot) = (Arc::clone(chart), Mutex::new(run), RwLock::new(snapshot));
        let alert = Alert { id, key, chart, config, run, snapshot };
        alert.update_info_using_labels(&mut alert.run());
        alert
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

    /// `rrdcalc_runtime_snapshot_publish()`: the API reads these live fields from now on.
    pub fn publish(&self, run: &Run) {
        let mut snapshot = self.snapshot.write().unwrap_or_else(PoisonError::into_inner);
        *snapshot = Snapshot::of(run, snapshot.summary.take(), snapshot.info.take());
    }

    /// `rrdcalc_update_info_using_rrdset_labels()`: the runtime `info` and `summary`, made again when the chart's
    /// label version is not the one they were made for; an empty result leaves the configured text.
    pub fn update_info_using_labels(&self, run: &mut Run) {
        let meta = self.chart.meta();
        let labels_version = meta.labels.version();
        let mut snapshot = self.snapshot.write().unwrap_or_else(PoisonError::into_inner);
        if run.labels_version != labels_version {
            let replace = |text: &Option<Vec<u8>>| {
                replace_variables_with_labels(text.as_deref().unwrap_or(b""), meta.family.as_bytes(), Some(&meta.labels))
            };
            snapshot.info = replace(&self.config.info);
            snapshot.summary = replace(&self.config.summary);
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
