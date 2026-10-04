//! An alert rule as a `health.d` file or DynCfg states it (`health_prototypes.h` `struct rrd_alert_match`,
//! `struct rrd_alert_config`).

use netdata_agent_dyncfg::model::SourceType;
use netdata_agent_eval::Expression;
use netdata_agent_query::tables::TimeGrouping;

use crate::tables::{DataSource, DimsGrouping, GroupCondition};

/// `struct rrd_alert_match`: which charts a rule is for. C's label pattern arrays are compiled when rules are
/// matched against charts.
#[derive(Debug, Default)]
pub struct AlertMatch {
    pub enabled: bool,
    pub is_template: bool,
    /// The chart of an alarm, the context of a template.
    pub on: Option<Vec<u8>>,
    pub host_labels: Option<Vec<u8>>,
    pub chart_labels: Option<Vec<u8>>,
}

/// `struct rrd_alert_config`. A text is `None` where C's `STRING` is NULL: an empty text is never stored.
#[derive(Debug)]
pub struct AlertConfig {
    pub hash_id: [u8; 16],
    pub name: Option<Vec<u8>>,
    pub exec: Option<Vec<u8>>,
    pub recipient: Option<Vec<u8>>,
    pub classification: Option<Vec<u8>>,
    pub component: Option<Vec<u8>>,
    pub r#type: Option<Vec<u8>>,
    pub source_type: SourceType,
    pub source: Option<Vec<u8>>,
    pub units: Option<Vec<u8>>,
    pub summary: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
    pub update_every: i32,
    /// `ALERT_ACTION_OPTIONS` bits.
    pub alert_action_options: u8,

    // the database lookup
    pub dimensions: Option<Vec<u8>>,
    /// `None` is `RRDR_GROUPING_UNDEFINED`, which prints as `average`.
    pub time_group: Option<TimeGrouping>,
    pub time_group_condition: GroupCondition,
    pub time_group_value: f64,
    pub dims_group: DimsGrouping,
    pub data_source: DataSource,
    pub before: i32,
    pub after: i32,
    /// `RRDR_OPTIONS` bits.
    pub options: u64,

    pub calculation: Option<Expression>,
    pub warning: Option<Expression>,
    pub critical: Option<Expression>,

    // the notification delay; the multiplier is C's `float`
    pub delay_up_duration: i32,
    pub delay_down_duration: i32,
    pub delay_max_duration: i32,
    pub delay_multiplier: f32,

    pub has_custom_repeat_config: bool,
    pub warn_repeat_every: u32,
    pub crit_repeat_every: u32,
}

impl Default for AlertConfig {
    /// C's zeroed struct.
    fn default() -> Self {
        AlertConfig {
            hash_id: [0; 16],
            name: None,
            exec: None,
            recipient: None,
            classification: None,
            component: None,
            r#type: None,
            source_type: SourceType::Internal,
            source: None,
            units: None,
            summary: None,
            info: None,
            update_every: 0,
            alert_action_options: 0,
            dimensions: None,
            time_group: None,
            time_group_condition: GroupCondition::Equal,
            time_group_value: 0.0,
            dims_group: DimsGrouping::Sum,
            data_source: DataSource::Samples,
            before: 0,
            after: 0,
            options: 0,
            calculation: None,
            warning: None,
            critical: None,
            delay_up_duration: 0,
            delay_down_duration: 0,
            delay_max_duration: 0,
            delay_multiplier: 0.0,
            has_custom_repeat_config: false,
            warn_repeat_every: 0,
            crit_repeat_every: 0,
        }
    }
}

impl AlertConfig {
    /// `RRDCALC_HAS_DB_LOOKUP()`.
    pub fn has_db_lookup(&self) -> bool {
        self.after != 0
    }

    /// `time_grouping_id2txt()` of the rule's grouping.
    pub fn time_group_name(&self) -> &'static str {
        self.time_group.unwrap_or(TimeGrouping::Average).name()
    }
}

/// One rule: `RRD_ALERT_PROTOTYPE` without its chain links.
#[derive(Debug, Default)]
pub struct Rule {
    pub r#match: AlertMatch,
    pub config: AlertConfig,
}
