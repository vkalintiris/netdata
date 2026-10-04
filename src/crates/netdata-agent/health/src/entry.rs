//! An entry of the alert log: one status change of one alert (`ALARM_ENTRY`, `health_log.c`
//! `health_create_alarm_entry()`).

use netdata_agent_text::units::format_value_and_unit;

use crate::alert::{Alert, Run, Status};

/// `HEALTH_ENTRY_FLAG_*`.
pub mod entry_flags {
    pub const PROCESSED: u32 = 0x0000_0001;
    pub const UPDATED: u32 = 0x0000_0002;
    pub const EXEC_RUN: u32 = 0x0000_0004;
    pub const EXEC_FAILED: u32 = 0x0000_0008;
    pub const SILENCED: u32 = 0x0000_0010;
    pub const RUN_ONCE: u32 = 0x0000_0020;
    pub const EXEC_IN_PROGRESS: u32 = 0x0000_0040;
    pub const IS_REPEATING: u32 = 0x0000_0080;
    pub const SAVED: u32 = 0x1000_0000;
    pub const ACLK_QUEUED: u32 = 0x2000_0000;
    pub const NO_CLEAR_NOTIFICATION: u32 = 0x8000_0000;
}

/// What makes an entry (`health_create_alarm_entry()`'s arguments).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    pub when: i64,
    pub duration: i64,
    pub old_value: f64,
    pub new_value: f64,
    pub old_status: Status,
    pub new_status: Status,
    pub delay: i32,
    pub flags: u32,
}

/// `ALARM_ENTRY`.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Its place in the host's log; 0 until the log (or a repeat) gives it one.
    pub unique_id: u32,
    pub alarm_id: u32,
    pub alarm_event_id: u32,
    /// The wall clock in microseconds when the entry was made.
    pub global_id: u64,
    pub config_hash_id: [u8; 16],
    pub transition_id: [u8; 16],
    pub when: i64,
    pub duration: i64,
    pub non_clear_duration: i64,
    pub name: Option<Vec<u8>>,
    /// The chart's id, name and context when the entry was made.
    pub chart: Vec<u8>,
    pub chart_context: Vec<u8>,
    pub chart_name: Vec<u8>,
    pub classification: Option<Vec<u8>>,
    pub component: Option<Vec<u8>>,
    pub r#type: Option<Vec<u8>>,
    pub exec: Option<Vec<u8>>,
    pub recipient: Option<Vec<u8>>,
    pub source: Option<Vec<u8>>,
    pub units: Option<Vec<u8>>,
    pub summary: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
    pub exec_run_timestamp: i64,
    pub exec_code: i32,
    pub old_value: f64,
    pub new_value: f64,
    pub old_value_string: Vec<u8>,
    pub new_value_string: Vec<u8>,
    pub old_status: Status,
    pub new_status: Status,
    pub flags: u32,
    pub delay: i32,
    pub delay_up_to_timestamp: i64,
    pub updated_by_id: u32,
    pub updates_id: u32,
    pub last_repeat: i64,
    /// `ae->pending_save_count`: the saves of this entry the metadata queue holds.
    pub pending_save_count: u32,
}

impl Entry {
    /// `health_create_alarm_entry()`: the entry of `transition` for `alert` as it is now. It takes the alert's
    /// next event id; `global_id` and `transition_id` are the caller's reads of the clock and of a new UUID.
    pub(crate) fn create(
        alert: &Alert,
        run: &mut Run,
        transition: &Transition,
        global_id: u64,
        transition_id: [u8; 16],
    ) -> Entry {
        let alarm_event_id = run.next_event_id;
        run.next_event_id = run.next_event_id.wrapping_add(1);

        let config = &alert.config;
        let meta = alert.chart.meta();
        let snapshot = alert.snapshot();
        let units = config.units.as_deref().unwrap_or(b"");
        let duration = transition.duration.max(0);
        let raised = |status| matches!(status, Status::Warning | Status::Critical);
        Entry {
            unique_id: 0,
            alarm_id: alert.id,
            alarm_event_id,
            global_id,
            config_hash_id: config.hash_id,
            transition_id,
            when: transition.when,
            duration,
            non_clear_duration: if raised(transition.old_status) { duration } else { 0 },
            name: config.name.clone(),
            chart: alert.chart.id().as_bytes().to_vec(),
            chart_context: meta.context.into_bytes(),
            chart_name: meta.name.unwrap_or_else(|| alert.chart.id().to_owned()).into_bytes(),
            classification: config.classification.clone(),
            component: config.component.clone(),
            r#type: config.r#type.clone(),
            exec: config.exec.clone(),
            recipient: config.recipient.clone(),
            source: config.source.clone(),
            units: config.units.clone(),
            summary: snapshot.summary,
            info: snapshot.info,
            exec_run_timestamp: 0,
            exec_code: 0,
            old_value: transition.old_value,
            new_value: transition.new_value,
            old_value_string: format_value_and_unit(transition.old_value, units),
            new_value_string: format_value_and_unit(transition.new_value, units),
            old_status: transition.old_status,
            new_status: transition.new_status,
            flags: transition.flags,
            delay: transition.delay,
            delay_up_to_timestamp: transition.when.saturating_add(i64::from(transition.delay)),
            updated_by_id: 0,
            updates_id: 0,
            last_repeat: 0,
            pending_save_count: 0,
        }
    }
}
