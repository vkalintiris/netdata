//! The command-line contract.
//!
//! The daemon passes 33 positional arguments in a fixed order
//! (`src/health/health_notifications.c`). Values are kept as strings because the
//! script rendered them verbatim into payloads - `value` can be `nan`, `chart` can
//! be `NOCHART` - and re-formatting a parsed number would change the wire output.
//! Numeric views are provided where arithmetic is genuinely needed.

use std::fmt;

/// How the process was invoked.
pub enum Invocation {
    /// Normal notification dispatch.
    Notify(Box<AlertArgs>),
    /// `test [role]` or `[role] test`: send three synthetic transitions.
    Test { role: String },
    /// `unittest <role> <cfgfile> <status> <old_status>`: print resolved recipients.
    UnitTest {
        role: String,
        config_file: String,
        status: String,
        old_status: String,
    },
    /// `dump_methods`: print the enabled `SEND_*` variable names, one per line.
    DumpMethods,
}

/// The 33 positional arguments, in the daemon's order.
#[derive(Clone, Debug, Default)]
pub struct AlertArgs {
    pub roles: String,
    pub args_host: String,
    pub unique_id: String,
    pub alarm_id: String,
    pub event_id: String,
    pub when: String,
    pub name: String,
    pub chart: String,
    pub status: String,
    pub old_status: String,
    pub value: String,
    pub old_value: String,
    pub src: String,
    pub duration: String,
    pub non_clear_duration: String,
    pub units: String,
    pub info: String,
    pub value_string: String,
    pub old_value_string: String,
    pub calc_expression: String,
    pub calc_param_values: String,
    pub total_warnings: String,
    pub total_critical: String,
    pub total_warn_alarms: String,
    pub total_crit_alarms: String,
    pub classification: String,
    pub edit_command_line: String,
    pub child_machine_guid: String,
    pub transition_id: String,
    pub summary: String,
    pub context: String,
    pub component: String,
    pub alert_type: String,
}

/// Alert status values the notification path cares about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Clear,
    Warning,
    Critical,
    Other,
}

impl Status {
    pub fn parse(s: &str) -> Self {
        match s {
            "CLEAR" => Status::Clear,
            "WARNING" => Status::Warning,
            "CRITICAL" => Status::Critical,
            _ => Status::Other,
        }
    }

    pub fn is_notifiable(self) -> bool {
        matches!(self, Status::Clear | Status::Warning | Status::Critical)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Status::Clear => "CLEAR",
            Status::Warning => "WARNING",
            Status::Critical => "CRITICAL",
            Status::Other => "UNKNOWN",
        };
        f.write_str(s)
    }
}

impl AlertArgs {
    pub fn status(&self) -> Status {
        Status::parse(&self.status)
    }

    pub fn old_status(&self) -> Status {
        Status::parse(&self.old_status)
    }

    /// Seconds, tolerating the empty/garbage values the shell treated as 0.
    pub fn duration_secs(&self) -> i64 {
        self.duration.trim().parse().unwrap_or(0)
    }

    pub fn non_clear_duration_secs(&self) -> i64 {
        self.non_clear_duration.trim().parse().unwrap_or(0)
    }

    pub fn when_secs(&self) -> i64 {
        self.when.trim().parse().unwrap_or(0)
    }

    pub fn total_warnings_num(&self) -> i64 {
        self.total_warnings.trim().parse().unwrap_or(0)
    }

    pub fn total_critical_num(&self) -> i64 {
        self.total_critical.trim().parse().unwrap_or(0)
    }

    /// The 33 arguments in the order the daemon passed them.
    pub fn as_positional(&self) -> Vec<String> {
        vec![
            self.roles.clone(),
            self.args_host.clone(),
            self.unique_id.clone(),
            self.alarm_id.clone(),
            self.event_id.clone(),
            self.when.clone(),
            self.name.clone(),
            self.chart.clone(),
            self.status.clone(),
            self.old_status.clone(),
            self.value.clone(),
            self.old_value.clone(),
            self.src.clone(),
            self.duration.clone(),
            self.non_clear_duration.clone(),
            self.units.clone(),
            self.info.clone(),
            self.value_string.clone(),
            self.old_value_string.clone(),
            self.calc_expression.clone(),
            self.calc_param_values.clone(),
            self.total_warnings.clone(),
            self.total_critical.clone(),
            self.total_warn_alarms.clone(),
            self.total_crit_alarms.clone(),
            self.classification.clone(),
            self.edit_command_line.clone(),
            self.child_machine_guid.clone(),
            self.transition_id.clone(),
            self.summary.clone(),
            self.context.clone(),
            self.component.clone(),
            self.alert_type.clone(),
        ]
    }

    /// `${transition_id//-/}` - the journal field carries the UUID without dashes.
    pub fn transition_id_compact(&self) -> String {
        self.transition_id.replace('-', "")
    }
}

/// Classify `argv[1..]`.
///
/// Every argument is positional: nothing here may be interpreted as a flag, because
/// alert text routinely starts with `-` and the daemon passes it through verbatim.
pub fn parse(argv: &[String]) -> Invocation {
    // `test`, `test <role>` and `<role> test`, with at most two arguments - the
    // same shape the script accepted.
    if argv.len() <= 2 {
        let first = argv.first().map(String::as_str).unwrap_or("");
        let second = argv.get(1).map(String::as_str).unwrap_or("");
        if first == "test" || second == "test" {
            let role = if second == "test" { first } else { second };
            let role = if role.is_empty() { "sysadmin" } else { role };
            return Invocation::Test {
                role: role.to_string(),
            };
        }
    }

    if argv.first().map(String::as_str) == Some("unittest") {
        return Invocation::UnitTest {
            role: at(argv, 1),
            config_file: at(argv, 2),
            status: at(argv, 3),
            old_status: at(argv, 4),
        };
    }

    if argv.first().map(String::as_str) == Some("dump_methods") {
        return Invocation::DumpMethods;
    }

    Invocation::Notify(Box::new(AlertArgs {
        roles: at(argv, 0),
        args_host: at(argv, 1),
        unique_id: at(argv, 2),
        alarm_id: at(argv, 3),
        event_id: at(argv, 4),
        when: at(argv, 5),
        name: at(argv, 6),
        chart: at(argv, 7),
        status: at(argv, 8),
        old_status: at(argv, 9),
        value: at(argv, 10),
        old_value: at(argv, 11),
        src: at(argv, 12),
        duration: at(argv, 13),
        non_clear_duration: at(argv, 14),
        units: at(argv, 15),
        info: at(argv, 16),
        value_string: at(argv, 17),
        old_value_string: at(argv, 18),
        calc_expression: at(argv, 19),
        calc_param_values: at(argv, 20),
        total_warnings: at(argv, 21),
        total_critical: at(argv, 22),
        total_warn_alarms: at(argv, 23),
        total_crit_alarms: at(argv, 24),
        classification: at(argv, 25),
        edit_command_line: at(argv, 26),
        child_machine_guid: at(argv, 27),
        transition_id: at(argv, 28),
        summary: at(argv, 29),
        context: at(argv, 30),
        component: at(argv, 31),
        alert_type: at(argv, 32),
    }))
}

fn at(argv: &[String], i: usize) -> String {
    argv.get(i).cloned().unwrap_or_default()
}

#[cfg(test)]
#[path = "args_tests.rs"]
mod tests;
