//! A rule as its `alert_hash` row (`sqlite_health.c` `sql_alert_store_config()`): what C binds when the rule is
//! hashed, before the default exec and recipient are filled.

use netdata_agent_metadata::health::{AlertHashLookup, AlertHashRow};
use netdata_agent_text::print::print_fixed;

use crate::prototype::Rule;
use crate::tables::{ACTION_OPTION_NO_CLEAR_NOTIFICATION, ACTION_OPTION_NO_CLEAR_NOTIFICATION_NAME, options_remove_overlapping};

/// The `delay` column: each part that is not zero; `up`, `down` and the multiplier each end in a space, `max` does
/// not. A file's rule always has the multiplier, so its text ends with a space unless a maximum follows.
fn delay_text(up: i32, down: i32, multiplier: f32, max: i32) -> Vec<u8> {
    let mut text = Vec::new();
    if up != 0 {
        text.extend_from_slice(format!("up {up}s ").as_bytes());
    }
    if down != 0 {
        text.extend_from_slice(format!("down {down}s ").as_bytes());
    }
    // a NaN is not zero
    if multiplier != 0.0 {
        text.extend_from_slice(b"multiplier ");
        print_fixed(&mut text, f64::from(multiplier), 1);
        text.push(b' ');
    }
    if max != 0 {
        text.extend_from_slice(format!("max {max}s").as_bytes());
    }
    text
}

/// The row `sql_alert_store_config()` binds for `rule`.
pub fn alert_hash_row(rule: &Rule) -> AlertHashRow {
    let (am, ac) = (&rule.r#match, &rule.config);
    let (alarm, template) = if am.is_template { (None, ac.name.clone()) } else { (ac.name.clone(), None) };
    let source = |expression: &Option<netdata_agent_eval::Expression>| {
        expression.as_ref().map(|expression| expression.source().to_vec())
    };
    AlertHashRow {
        hash_id: ac.hash_id,
        alarm,
        template,
        on_key: am.on.clone(),
        class: ac.classification.clone(),
        component: ac.component.clone(),
        r#type: ac.r#type.clone(),
        update_every: ac.update_every,
        units: ac.units.clone(),
        calc: source(&ac.calculation),
        warn: source(&ac.warning),
        crit: source(&ac.critical),
        exec: ac.exec.clone(),
        to_key: ac.recipient.clone(),
        info: ac.info.clone(),
        delay: delay_text(ac.delay_up_duration, ac.delay_down_duration, ac.delay_multiplier, ac.delay_max_duration),
        options: (ac.alert_action_options & ACTION_OPTION_NO_CLEAR_NOTIFICATION != 0)
            .then_some(ACTION_OPTION_NO_CLEAR_NOTIFICATION_NAME),
        repeat: ac
            .has_custom_repeat_config
            .then(|| format!("warning {}s critical {}s", ac.warn_repeat_every, ac.crit_repeat_every)),
        host_labels: am.host_labels.clone(),
        lookup: (ac.after != 0).then(|| AlertHashLookup {
            dimensions: ac.dimensions.clone(),
            method: ac.time_group_name(),
            // C's `(int)` of the options
            options: options_remove_overlapping(ac.options) as i32,
            after: ac.after,
            before: ac.before,
        }),
        source: ac.source.clone(),
        chart_labels: am.chart_labels.clone(),
        summary: ac.summary.clone(),
        time_group_condition: ac.time_group_condition as i32,
        time_group_value: ac.time_group_value,
        dims_group: ac.dims_group as i32,
        data_source: ac.data_source as i32,
    }
}

#[cfg(test)]
mod tests {
    use netdata_agent_eval::Expression;
    use netdata_agent_query::tables::{TimeGrouping, options};

    use super::*;
    use crate::tables::{DataSource, DimsGrouping, GroupCondition};

    #[test]
    fn the_delay_text_has_each_part_that_is_not_zero() {
        let cases: [(i32, i32, f32, i32, &str); 10] = [
            // what a file's rule without a `delay` line stores
            (0, 0, 1.0, 0, "multiplier 1.0 "),
            (60, 0, 1.0, 0, "up 60s multiplier 1.0 "),
            (0, 300, 1.0, 0, "down 300s multiplier 1.0 "),
            (60, 300, 1.5, 3600, "up 60s down 300s multiplier 1.5 max 3600s"),
            (0, 0, 1.0, 3600, "multiplier 1.0 max 3600s"),
            // a rule that never met the reader has no multiplier
            (0, 0, 0.0, 0, ""),
            (60, 0, 0.0, 0, "up 60s "),
            (-5, -6, 1.25, -7, "up -5s down -6s multiplier 1.2 max -7s"),
            (0, 0, f32::NAN, 0, "multiplier nan "),
            (0, 0, f32::INFINITY, 0, "multiplier inf "),
        ];
        for (up, down, multiplier, max, expected) in cases {
            assert_eq!(String::from_utf8(delay_text(up, down, multiplier, max)).unwrap(), expected);
        }
    }

    #[test]
    fn a_template_without_a_lookup() {
        let mut rule = Rule::default();
        rule.r#match.is_template = true;
        rule.r#match.on = Some(b"system.cpu".to_vec());
        rule.config.name = Some(b"t".to_vec());
        rule.config.hash_id = [7; 16];
        rule.config.update_every = 10;
        // what the reader starts every rule with
        rule.config.delay_multiplier = 1.0;
        rule.config.time_group_value = f64::NAN;
        let row = alert_hash_row(&rule);
        assert!(row.time_group_value.is_nan());
        assert_eq!(
            AlertHashRow { time_group_value: 0.0, ..row },
            AlertHashRow {
                hash_id: [7; 16],
                alarm: None,
                template: Some(b"t".to_vec()),
                on_key: Some(b"system.cpu".to_vec()),
                class: None,
                component: None,
                r#type: None,
                update_every: 10,
                units: None,
                calc: None,
                warn: None,
                crit: None,
                exec: None,
                to_key: None,
                info: None,
                delay: b"multiplier 1.0 ".to_vec(),
                options: None,
                repeat: None,
                host_labels: None,
                lookup: None,
                source: None,
                chart_labels: None,
                summary: None,
                time_group_condition: 0,
                time_group_value: 0.0,
                dims_group: DimsGrouping::Sum as i32,
                data_source: DataSource::Samples as i32,
            }
        );
    }

    #[test]
    fn an_alarm_with_everything() {
        let mut rule = Rule::default();
        rule.r#match.on = Some(b"system.cpu".to_vec());
        rule.r#match.host_labels = Some(b"a=b".to_vec());
        rule.r#match.chart_labels = Some(b"c=d".to_vec());
        let ac = &mut rule.config;
        ac.name = Some(b"a".to_vec());
        ac.hash_id = [8; 16];
        ac.exec = Some(b"/bin/true".to_vec());
        ac.recipient = Some(b"sysadmin".to_vec());
        ac.classification = Some(b"Utilization".to_vec());
        ac.component = Some(b"Processor".to_vec());
        ac.r#type = Some(b"System".to_vec());
        ac.source = Some(b"line=1,file=x.conf".to_vec());
        ac.units = Some(b"%".to_vec());
        ac.summary = Some(b"CPU".to_vec());
        ac.info = Some(b"info".to_vec());
        ac.update_every = 60;
        ac.alert_action_options = ACTION_OPTION_NO_CLEAR_NOTIFICATION;
        ac.dimensions = Some(b"user".to_vec());
        ac.time_group = Some(TimeGrouping::Max);
        ac.time_group_condition = GroupCondition::Greater;
        ac.time_group_value = 1.5;
        ac.dims_group = DimsGrouping::Average;
        ac.data_source = DataSource::Percentages;
        ac.before = -60;
        ac.after = -600;
        // the bits the row's two enums carry are left out of its options
        ac.options = options::NOT_ALIGNED | options::PERCENTAGE | options::DIMS_AVERAGE;
        ac.calculation = Some(Expression::parse(b"$this * 2").unwrap());
        ac.warning = Some(Expression::parse(b"$this > 1").unwrap());
        ac.critical = Some(Expression::parse(b"$this > 2").unwrap());
        ac.delay_up_duration = 60;
        ac.delay_down_duration = 300;
        ac.delay_max_duration = 3600;
        ac.delay_multiplier = 1.5;
        ac.has_custom_repeat_config = true;
        ac.warn_repeat_every = 120;
        ac.crit_repeat_every = 0;
        assert_eq!(
            alert_hash_row(&rule),
            AlertHashRow {
                hash_id: [8; 16],
                alarm: Some(b"a".to_vec()),
                template: None,
                on_key: Some(b"system.cpu".to_vec()),
                class: Some(b"Utilization".to_vec()),
                component: Some(b"Processor".to_vec()),
                r#type: Some(b"System".to_vec()),
                update_every: 60,
                units: Some(b"%".to_vec()),
                calc: Some(b"$this * 2".to_vec()),
                warn: Some(b"$this > 1".to_vec()),
                crit: Some(b"$this > 2".to_vec()),
                exec: Some(b"/bin/true".to_vec()),
                to_key: Some(b"sysadmin".to_vec()),
                info: Some(b"info".to_vec()),
                delay: b"up 60s down 300s multiplier 1.5 max 3600s".to_vec(),
                options: Some("no-clear-notification"),
                repeat: Some("warning 120s critical 0s".to_owned()),
                host_labels: Some(b"a=b".to_vec()),
                lookup: Some(AlertHashLookup {
                    dimensions: Some(b"user".to_vec()),
                    method: "max",
                    options: options::NOT_ALIGNED as i32,
                    after: -600,
                    before: -60,
                }),
                source: Some(b"line=1,file=x.conf".to_vec()),
                chart_labels: Some(b"c=d".to_vec()),
                summary: Some(b"CPU".to_vec()),
                time_group_condition: GroupCondition::Greater as i32,
                time_group_value: 1.5,
                dims_group: DimsGrouping::Average as i32,
                data_source: DataSource::Percentages as i32,
            }
        );
    }

    /// The five lookup columns go by `after` alone, as C's `if (ap->config.after)`.
    #[test]
    fn the_lookup_is_stored_when_after_is_set() {
        let mut rule = Rule::default();
        rule.config.time_group = Some(TimeGrouping::Min);
        rule.config.after = -600;
        assert_eq!(
            alert_hash_row(&rule).lookup,
            Some(AlertHashLookup { dimensions: None, method: "min", options: 0, after: -600, before: 0 })
        );

        let mut rule = Rule::default();
        rule.config.time_group = Some(TimeGrouping::Min);
        rule.config.before = -60;
        rule.config.dimensions = Some(b"user".to_vec());
        rule.config.options = options::NOT_ALIGNED;
        assert_eq!(alert_hash_row(&rule).lookup, None);
    }
}
