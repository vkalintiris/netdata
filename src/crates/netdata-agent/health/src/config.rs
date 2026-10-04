//! What health keeps of `[health]` (`health.h` `struct health_plugin_globals`, filled by
//! `health_load_config_defaults()`).

use netdata_agent_text::simple_pattern::{Separators, SimplePattern, SimplePatternMode};

/// `health_globals.config`.
#[derive(Debug)]
pub struct HealthConfig {
    pub enabled: bool,
    pub stock_enabled: bool,
    pub use_summary_for_notifications: bool,
    pub health_log_entries_max: u32,
    pub health_log_retention_s: u32,
    /// `script to execute on alarm`: a rule's `exec` when its file gives none. Empty is C's NULL.
    pub default_exec: Vec<u8>,
    /// A rule's `to` when its file gives none.
    pub default_recipient: Vec<u8>,
    /// `enabled alarms`: applied when rules are matched against charts.
    pub enabled_alerts: SimplePattern,
    pub default_warn_repeat_every: u32,
    pub default_crit_repeat_every: u32,
    pub run_at_least_every_s: i32,
    pub postpone_s: i32,
    pub notification_execution_timeout_s: i32,
}

impl HealthConfig {
    /// `simple_pattern_create(value, NULL, SIMPLE_PATTERN_EXACT, true)` of `enabled alarms`.
    pub fn enabled_alerts_pattern(value: &[u8]) -> SimplePattern {
        SimplePattern::new(value, Separators::Whitespace, SimplePatternMode::Exact, true)
    }
}

impl Default for HealthConfig {
    /// The initializer of `health_globals` and the defaults of the `[health]` keys.
    fn default() -> Self {
        HealthConfig {
            enabled: true,
            stock_enabled: true,
            use_summary_for_notifications: true,
            health_log_entries_max: 1000,
            health_log_retention_s: 5 * 86400,
            default_exec: Vec::new(),
            default_recipient: b"root".to_vec(),
            enabled_alerts: Self::enabled_alerts_pattern(b"*"),
            default_warn_repeat_every: 0,
            default_crit_repeat_every: 0,
            run_at_least_every_s: 10,
            postpone_s: 60,
            notification_execution_timeout_s: 120,
        }
    }
}
