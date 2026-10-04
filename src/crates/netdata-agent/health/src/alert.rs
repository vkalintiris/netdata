//! An alert: a rule linked to a chart (`rrdcalc.c`).

use netdata_agent_eval::Expression;

use crate::expr::parse_logged;
use crate::prototype::AlertConfig;

/// The three expressions of a rule as an alert of it gets them (`health_prototype_copy_config()`): calc, warn and
/// crit, each parsed again from its source text. A source that no longer parses (the reader wrote `green` and
/// `red` into it) logs the evaluator's line, and the alert goes without that expression.
pub fn copy_expressions(config: &AlertConfig) -> [Option<Expression>; 3] {
    [&config.calculation, &config.warning, &config.critical]
        .map(|expression| expression.as_ref().and_then(|expression| parse_logged(expression.source()).ok()))
}
