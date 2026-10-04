//! An alert rule as a `health.d` file or DynCfg states it (`health_prototypes.h` `struct rrd_alert_match`,
//! `struct rrd_alert_config`).

use indexmap::IndexMap;
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

/// `struct rrd_alert_config`. A text is `None` where C's `STRING` is NULL. C never holds an empty one (an empty
/// `STRING` is NULL), and the reader gives none: a line without a value is refused.
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

impl Rule {
    /// `health_prototype_rule_validate()`: why the rule cannot stand, in C's words.
    pub fn validate(&self) -> Option<&'static str> {
        if self.r#match.on.is_none() {
            return Some(if self.r#match.is_template {
                "missing match 'on' parameter for context"
            } else {
                "missing match 'on' parameter for instance"
            });
        }
        if self.config.update_every <= 0 {
            return Some("missing update frequency");
        }
        if !self.config.has_db_lookup()
            && self.config.calculation.is_none()
            && self.config.warning.is_none()
            && self.config.critical.is_none()
        {
            return Some("no db lookup, calculation and warning/critical conditions");
        }
        if !self.config.delay_multiplier.is_finite() {
            return Some("non-finite delay multiplier");
        }
        None
    }

    fn is_dyncfg(&self) -> bool {
        self.config.source_type == SourceType::Dyncfg
    }
}

/// The rules of one alert name: C's dictionary value and the chain behind it.
#[derive(Debug)]
pub struct Prototype {
    rules: Vec<Rule>,
    enabled: bool,
}

impl Prototype {
    /// The rules, in the order they were added.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Whether any rule was enabled when it was added (`_internal.enabled`).
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// The prototype store: `health_globals.prototypes.dict`, by alert name, in insertion order.
#[derive(Debug, Default)]
pub struct Prototypes {
    by_name: IndexMap<Vec<u8>, Prototype>,
}

impl Prototypes {
    /// In insertion order: the order DynCfg registers them in and alerts are linked in.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &Prototype)> {
        self.by_name.iter().map(|(name, prototype)| (name.as_slice(), prototype))
    }

    pub fn get(&self, name: &[u8]) -> Option<&Prototype> {
        self.by_name.get(name)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// `dictionary_flush()`.
    pub fn flush(&mut self) {
        self.by_name.clear();
    }

    /// `dictionary_set_advanced()` with the store's insert and conflict callbacks: a new name is inserted; rules
    /// of a known name are appended to its chain, unless that chain came from DynCfg (they are dropped) or they
    /// come from DynCfg themselves (they replace it). The dictionary takes no empty name. (C also marks the name as
    /// seen on disk, `_internal.is_on_disk`, which nothing reads.)
    pub(crate) fn set(&mut self, name: &[u8], rules: Vec<Rule>, enabled: bool) {
        let Some(first) = rules.first() else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let new_is_dyncfg = first.is_dyncfg();

        let Some(old) = self.by_name.get_mut(name) else {
            self.by_name.insert(name.to_vec(), Prototype { rules, enabled });
            return;
        };
        if new_is_dyncfg {
            *old = Prototype { rules, enabled };
        } else if !old.rules[0].is_dyncfg() {
            old.rules.extend(rules);
            if enabled {
                old.enabled = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rule of `source_type`, told from the others by its units.
    fn rule(source_type: SourceType, mark: &[u8]) -> Rule {
        let mut rule = Rule::default();
        rule.config.source_type = source_type;
        rule.config.units = Some(mark.to_vec());
        rule
    }

    /// The chain of `name`: each rule's mark, then whether the name is enabled.
    fn chain(store: &Prototypes, name: &[u8]) -> (Vec<String>, bool) {
        let prototype = store.get(name).expect("the name");
        let marks = prototype
            .rules()
            .iter()
            .map(|rule| String::from_utf8_lossy(rule.config.units.as_deref().unwrap_or(b"")).into_owned())
            .collect();
        (marks, prototype.enabled())
    }

    #[test]
    fn file_rules_of_one_name_are_chained_in_order() {
        let mut store = Prototypes::default();
        store.set(b"a", vec![rule(SourceType::User, b"1"), rule(SourceType::User, b"2")], false);
        store.set(b"b", vec![rule(SourceType::Stock, b"b")], true);
        store.set(b"a", vec![rule(SourceType::Stock, b"3")], true);
        assert_eq!(chain(&store, b"a"), (vec!["1".to_owned(), "2".to_owned(), "3".to_owned()], true));
        // an enabled chain stays enabled
        store.set(b"a", vec![rule(SourceType::User, b"4")], false);
        assert!(chain(&store, b"a").1);
        assert_eq!(chain(&store, b"a").0.len(), 4);
        // the names keep their first order
        let names: Vec<&[u8]> = store.iter().map(|(name, _)| name).collect();
        assert_eq!(names, [b"a", b"b"]);
    }

    #[test]
    fn dyncfg_rules_replace_the_chain() {
        let mut store = Prototypes::default();
        store.set(b"a", vec![rule(SourceType::User, b"1"), rule(SourceType::User, b"2")], true);
        store.set(b"a", vec![rule(SourceType::Dyncfg, b"d1")], false);
        // the new rules bring their own enabled flag
        assert_eq!(chain(&store, b"a"), (vec!["d1".to_owned()], false));

        // DynCfg over DynCfg
        store.set(b"a", vec![rule(SourceType::Dyncfg, b"d2"), rule(SourceType::Dyncfg, b"d3")], true);
        assert_eq!(chain(&store, b"a"), (vec!["d2".to_owned(), "d3".to_owned()], true));
    }

    #[test]
    fn file_rules_after_dyncfg_ones_are_dropped() {
        let mut store = Prototypes::default();
        store.set(b"a", vec![rule(SourceType::Dyncfg, b"d")], false);
        assert_eq!(chain(&store, b"a"), (vec!["d".to_owned()], false));
        store.set(b"a", vec![rule(SourceType::User, b"file")], true);
        // dropped, and its enabled flag is not taken
        assert_eq!(chain(&store, b"a"), (vec!["d".to_owned()], false));
    }

    #[test]
    fn nothing_is_stored_without_a_name_or_a_rule() {
        let mut store = Prototypes::default();
        store.set(b"", vec![rule(SourceType::User, b"1")], true);
        store.set(b"a", Vec::new(), true);
        assert!(store.is_empty());
        store.set(b"a", vec![rule(SourceType::User, b"1")], true);
        assert_eq!(store.len(), 1);
        store.flush();
        assert!(store.is_empty());
    }
}
