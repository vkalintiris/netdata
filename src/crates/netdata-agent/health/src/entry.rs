//! An entry of the alert log: one status change of one alert (`ALARM_ENTRY`, `health_log.c`
//! `health_create_alarm_entry()`).

use netdata_agent_text::json::JsonWriter;
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

/// `health_entry_flags_to_json_array()`: the names of an entry's flags as the array member `key`, in C's order. A
/// repeating entry reads `RECURRING`.
pub fn entry_flags_to_json_array(wb: &mut JsonWriter, key: &str, flags: u32) {
    const NAMES: [(u32, &str); 11] = [
        (entry_flags::PROCESSED, "PROCESSED"),
        (entry_flags::UPDATED, "UPDATED"),
        (entry_flags::EXEC_RUN, "EXEC_RUN"),
        (entry_flags::EXEC_FAILED, "EXEC_FAILED"),
        (entry_flags::SILENCED, "SILENCED"),
        (entry_flags::RUN_ONCE, "RUN_ONCE"),
        (entry_flags::EXEC_IN_PROGRESS, "EXEC_IN_PROGRESS"),
        (entry_flags::IS_REPEATING, "RECURRING"),
        (entry_flags::SAVED, "SAVED"),
        (entry_flags::ACLK_QUEUED, "ACLK_QUEUED"),
        (entry_flags::NO_CLEAR_NOTIFICATION, "NO_CLEAR_NOTIFICATION"),
    ];
    wb.member_add_array(Some(key.as_bytes()));
    for (flag, name) in NAMES {
        if flags & flag != 0 {
            wb.add_array_item_string(name);
        }
    }
    wb.array_close();
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
    /// The saves of this entry that were asked for under the store's lock and are made once it is released (C
    /// makes them on its pointer to the entry): an entry that leaves the log meanwhile is kept until they are.
    pub owed_saves: u32,
}

impl Entry {
    /// Whether a save of this entry is still to come: one the metadata queue holds, or one owed since the entry
    /// was logged. The memory log keeps such an entry aside when it lets go of it.
    pub(crate) fn waits_for_a_save(&self) -> bool {
        self.pending_save_count != 0 || self.owed_saves != 0
    }
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
            owed_saves: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use netdata_agent_text::json::JsonOptions;

    use super::*;

    /// `health_entry_flags_to_json_array()`: every flag's name in C's order, nothing for a bit that is no flag.
    #[test]
    fn an_entry_s_flags_are_named_in_c_s_order() {
        use entry_flags::{EXEC_IN_PROGRESS, EXEC_RUN, PROCESSED, SAVED, UPDATED};
        let printed = |flags: u32| {
            let mut wb = JsonWriter::new(JsonOptions::MINIFY);
            entry_flags_to_json_array(&mut wb, "flags", flags);
            wb.finalize();
            String::from_utf8(wb.into_bytes()).unwrap()
        };
        let all = concat!(
            r#"{"flags":["PROCESSED","UPDATED","EXEC_RUN","EXEC_FAILED","SILENCED","RUN_ONCE","EXEC_IN_PROGRESS","#,
            r#""RECURRING","SAVED","ACLK_QUEUED","NO_CLEAR_NOTIFICATION"]}"#
        );
        assert_eq!(printed(u32::MAX), all);
        assert_eq!(printed(0), r#"{"flags":[]}"#);
        // each name alone, by C's own bit (`health.h`)
        let alone: [(u32, &str); 11] = [
            (0x0000_0001, "PROCESSED"),
            (0x0000_0002, "UPDATED"),
            (0x0000_0004, "EXEC_RUN"),
            (0x0000_0008, "EXEC_FAILED"),
            (0x0000_0010, "SILENCED"),
            (0x0000_0020, "RUN_ONCE"),
            (0x0000_0040, "EXEC_IN_PROGRESS"),
            (0x0000_0080, "RECURRING"),
            (0x1000_0000, "SAVED"),
            (0x2000_0000, "ACLK_QUEUED"),
            (0x8000_0000, "NO_CLEAR_NOTIFICATION"),
        ];
        for (bit, name) in alone {
            assert_eq!(printed(bit), format!(r#"{{"flags":["{name}"]}}"#), "{bit:#x}");
        }
        // an entry whose notification runs, and one a later entry replaced
        let running = r#"{"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"]}"#;
        assert_eq!(printed(PROCESSED | EXEC_RUN | EXEC_IN_PROGRESS | SAVED), running);
        let replaced = r#"{"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"]}"#;
        assert_eq!(printed(PROCESSED | UPDATED | EXEC_RUN | SAVED), replaced);
        // the bits between the flags are no flags
        assert_eq!(printed(0x0fff_ff00 | 0x4000_0000), r#"{"flags":[]}"#);
    }
}
