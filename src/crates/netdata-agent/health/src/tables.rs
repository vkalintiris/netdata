//! The small enums of an alert's configuration and their names (`health_prototypes.h`, the tables at the top of
//! `health_prototypes.c`), and the lookup options health keeps for itself (`health.h`).

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_query::tables::options;

/// `ALERT_LOOKUP_TIME_GROUP_CONDITION`: how a `countif` compares each value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum GroupCondition {
    #[default]
    Equal = 0,
    NotEqual = 1,
    Greater = 2,
    Less = 3,
    GreaterEqual = 4,
    LessEqual = 5,
}

impl GroupCondition {
    /// `alerts_group_conditions_id2txt()`.
    pub fn name(self) -> &'static str {
        match self {
            GroupCondition::Equal => "=",
            GroupCondition::NotEqual => "!=",
            GroupCondition::Greater => ">",
            GroupCondition::GreaterEqual => ">=",
            GroupCondition::Less => "<",
            GroupCondition::LessEqual => "<=",
        }
    }

    /// `alerts_group_condition2id()`: the condition a text names; an empty one is the first of C's table, and so
    /// is one that names none, which is recorded, in C's words, which name a data source.
    pub fn parse(name: &[u8]) -> Self {
        use GroupCondition::{Equal, Greater, GreaterEqual, Less, LessEqual, NotEqual};
        let named = [Equal, NotEqual, Greater, GreaterEqual, Less, LessEqual].into_iter();
        parse_name(name, named, GroupCondition::name, "Alert data source")
    }

    /// `alerts_group_conditions_id2txt()` of a number a table holds (C reads it into its one-byte enum): one that
    /// is no condition is recorded, in C's words, which name a data source, and reads as the first of C's table.
    pub fn name_of_id(id: u8) -> &'static str {
        use GroupCondition::{Equal, Greater, GreaterEqual, Less, LessEqual, NotEqual};
        match [Equal, NotEqual, Greater, Less, GreaterEqual, LessEqual].into_iter().find(|c| *c as u8 == id) {
            Some(condition) => condition.name(),
            None => {
                nd_log!(Source::Daemon, Priority::Warning, "Alert data source {id} is not valid");
                Equal.name()
            }
        }
    }
}

/// `ALERT_LOOKUP_DIMS_GROUPING`: how a lookup's dimensions become one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum DimsGrouping {
    #[default]
    Sum = 0,
    Min = 1,
    Max = 2,
    Average = 3,
    Min2Max = 4,
}

impl DimsGrouping {
    /// `alerts_dims_grouping_id2group()`.
    pub fn name(self) -> &'static str {
        match self {
            DimsGrouping::Sum => "sum",
            DimsGrouping::Min => "min",
            DimsGrouping::Max => "max",
            DimsGrouping::Average => "average",
            DimsGrouping::Min2Max => "min2max",
        }
    }

    /// `alerts_dims_grouping2id()`: as [`GroupCondition::parse`], with its own record.
    pub fn parse(name: &[u8]) -> Self {
        use DimsGrouping::{Average, Max, Min, Min2Max, Sum};
        let named = [Sum, Min, Max, Average, Min2Max].into_iter();
        parse_name(name, named, DimsGrouping::name, "Alert lookup dimensions grouping")
    }

    /// `alerts_dims_grouping_id2group()` of a number a table holds: one that is no grouping is recorded and reads
    /// as the first of C's table.
    pub fn name_of_id(id: u8) -> &'static str {
        use DimsGrouping::{Average, Max, Min, Min2Max, Sum};
        match [Sum, Min, Max, Average, Min2Max].into_iter().find(|grouping| *grouping as u8 == id) {
            Some(grouping) => grouping.name(),
            None => {
                nd_log!(Source::Daemon, Priority::Warning, "Alert lookup dimensions grouping {id} is not valid");
                Sum.name()
            }
        }
    }
}

/// `ALERT_LOOKUP_DATA_SOURCE`: what a lookup reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum DataSource {
    #[default]
    Samples = 0,
    Percentages = 1,
    Anomalies = 2,
}

impl DataSource {
    /// `alerts_data_source_id2source()`.
    pub fn name(self) -> &'static str {
        match self {
            DataSource::Samples => "samples",
            DataSource::Percentages => "percentages",
            DataSource::Anomalies => "anomalies",
        }
    }

    /// `alerts_data_sources2id()`: as [`GroupCondition::parse`].
    pub fn parse(name: &[u8]) -> Self {
        use DataSource::{Anomalies, Percentages, Samples};
        parse_name(name, [Samples, Percentages, Anomalies].into_iter(), DataSource::name, "Alert data source")
    }

    /// `alerts_data_source_id2source()` of a number a table holds: one that is no data source is recorded and
    /// reads as the first of C's table.
    pub fn name_of_id(id: u8) -> &'static str {
        use DataSource::{Anomalies, Percentages, Samples};
        match [Samples, Percentages, Anomalies].into_iter().find(|source| *source as u8 == id) {
            Some(source) => source.name(),
            None => {
                nd_log!(Source::Daemon, Priority::Warning, "Alert data source {id} is not valid");
                Samples.name()
            }
        }
    }
}

/// What the three `*2id()` functions share: the value of `values` (C's table, its first one first) that `name`
/// names; the first for an empty text, and for a text that names none, with the WARNING `<what> '<name>' is not
/// valid`.
fn parse_name<T: Copy>(name: &[u8], mut values: impl Iterator<Item = T>, text: fn(T) -> &'static str, what: &str) -> T {
    let first = values.next().expect("a table with a first value");
    if name.is_empty() {
        return first;
    }
    std::iter::once(first).chain(values).find(|value| text(*value).as_bytes() == name).unwrap_or_else(|| {
        nd_log!(Source::Daemon, Priority::Warning, "{what} '{}' is not valid", String::from_utf8_lossy(name));
        first
    })
}

/// `ALERT_ACTION_OPTION_NO_CLEAR_NOTIFICATION`, the one bit of `ALERT_ACTION_OPTIONS`.
pub const ACTION_OPTION_NO_CLEAR_NOTIFICATION: u8 = 1 << 0;
/// Its name in `alert_action_options[]`.
pub const ACTION_OPTION_NO_CLEAR_NOTIFICATION_NAME: &str = "no-clear-notification";

/// `alert_action_options_parse_one()`: the bit of one name, 0 for a text that is none.
pub fn action_options_parse_one(name: &[u8]) -> u8 {
    if name == ACTION_OPTION_NO_CLEAR_NOTIFICATION_NAME.as_bytes() { ACTION_OPTION_NO_CLEAR_NOTIFICATION } else { 0 }
}

/// `RRDR_OPTIONS_DATA_SOURCES`: the lookup options `data_source` carries.
pub const OPTIONS_DATA_SOURCES: u64 = options::PERCENTAGE | options::ANOMALY_BIT;
/// `RRDR_OPTIONS_DIMS_AGGREGATION`: the lookup options `dims_group` carries.
pub const OPTIONS_DIMS_AGGREGATION: u64 =
    options::DIMS_MIN | options::DIMS_MAX | options::DIMS_AVERAGE | options::DIMS_MIN2MAX;

/// `RRDR_OPTIONS_REMOVE_OVERLAPPING()`.
pub fn options_remove_overlapping(bits: u64) -> u64 {
    bits & !(OPTIONS_DIMS_AGGREGATION | OPTIONS_DATA_SOURCES)
}
