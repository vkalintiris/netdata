//! The record of an alert's status change in the health log (`health_log.c`
//! `health_log_alert_transition_with_trace()`).

use netdata_agent_log::{Field, Priority, Source, Value, msgid, nd_log, push};
use netdata_agent_text::print::print_fixed;

use crate::alert::Status;
use crate::entry::Entry;
use crate::keywords::lossy;

/// The record's priority and the symbol its second line starts with, by the status the alert took and the one it
/// left.
fn priority_and_symbol(old: Status, new: Status) -> (Priority, &'static str) {
    let (old, clear, warning) = (old as i32, Status::Clear as i32, Status::Warning as i32);
    match new {
        Status::Undefined if old >= clear => (Priority::Notice, "\u{2753}"),
        Status::Undefined => (Priority::Debug, "\u{2753}"),
        Status::Uninitialized | Status::Raised => (Priority::Debug, "\u{23f3}"),
        Status::Removed => (Priority::Debug, "\u{1f6ab}"),
        Status::Clear if old == Status::Uninitialized as i32 => (Priority::Debug, "\u{1f49a}"),
        Status::Clear if old <= clear => (Priority::Info, "\u{2705}"),
        Status::Clear => (Priority::Notice, "\u{1f49a}"),
        Status::Warning if old <= warning => (Priority::Warning, "\u{26a0}\u{fe0f}"),
        Status::Warning => (Priority::Info, "\u{1f536}"),
        Status::Critical => (Priority::Crit, "\u{1f534}"),
    }
}

/// C's `%f` of a value.
fn fixed(value: f64) -> String {
    let mut text = Vec::with_capacity(24);
    print_fixed(&mut text, value, 6);
    String::from_utf8(text).unwrap_or_default()
}

/// `health_log_alert()`: the entry as a record of the health source, with the alert's fields. A text the entry
/// does not have is a field the record does not have. The source field carries the entry's `exec`, as in C; the
/// log has no key for it.
pub(crate) fn log_alert(hostname: &str, entry: &Entry) {
    let mut fields = vec![
        (Field::MessageId, Value::Uuid(msgid::HEALTH_ALERT_TRANSITION)),
        (Field::NidlNode, Value::Str(hostname.to_owned())),
        (Field::NidlInstance, Value::Str(lossy(&entry.chart_name).into_owned())),
        (Field::NidlContext, Value::Str(lossy(&entry.chart_context).into_owned())),
        (Field::AlertId, Value::U64(u64::from(entry.alarm_id))),
        (Field::AlertUniqueId, Value::U64(u64::from(entry.unique_id))),
        (Field::AlertEventId, Value::U64(u64::from(entry.alarm_event_id))),
        (Field::AlertConfigHash, Value::Uuid(entry.config_hash_id)),
        (Field::AlertTransitionId, Value::Uuid(entry.transition_id)),
    ];
    let texts = [
        (Field::AlertName, &entry.name),
        (Field::AlertClass, &entry.classification),
        (Field::AlertComponent, &entry.component),
        (Field::AlertType, &entry.r#type),
        (Field::AlertExec, &entry.exec),
        (Field::AlertRecipient, &entry.recipient),
        (Field::AlertSource, &entry.exec),
        (Field::AlertUnits, &entry.units),
        (Field::AlertSummary, &entry.summary),
        (Field::AlertInfo, &entry.info),
    ];
    for (field, text) in texts {
        if let Some(text) = text {
            fields.push((field, Value::Str(lossy(text).into_owned())));
        }
    }
    fields.extend([
        (Field::AlertValue, Value::Dbl(entry.new_value)),
        (Field::AlertValueOld, Value::Dbl(entry.old_value)),
        (Field::AlertStatus, Value::txt(entry.new_status.name())),
        (Field::AlertStatusOld, Value::txt(entry.old_status.name())),
        (Field::AlertDuration, Value::I64(entry.duration)),
        (Field::ResponseCode, Value::I64(i64::from(entry.exec_code))),
        (
            Field::AlertNotificationRealtimeUsec,
            Value::U64((entry.delay_up_to_timestamp as u64).wrapping_mul(1_000_000)),
        ),
    ]);
    let _frame = push(fields);

    let (priority, symbol) = priority_and_symbol(entry.old_status, entry.new_status);
    let transitioned = if (entry.old_status as i32) < entry.new_status as i32 { "raised" } else { "lowered" };
    let text = |text: &Option<Vec<u8>>| lossy(text.as_deref().unwrap_or(b"")).into_owned();
    let (name, chart, units) = (text(&entry.name), lossy(&entry.chart), text(&entry.units));
    nd_log!(
        Source::Health,
        priority,
        "ALERT '{name}' of '{chart}' on node '{hostname}', {transitioned} from {} to {}.\n\
         {symbol} {} on {hostname}.\n\
         {hostname}:{chart}:{name} value got from {} {units}, to {} {units}.",
        entry.old_status.name(),
        entry.new_status.name(),
        text(&entry.info),
        fixed(entry.old_value),
        fixed(entry.new_value)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C's switch on the new status, each arm with the old statuses that select it.
    #[test]
    fn the_priority_follows_the_two_statuses() {
        use Status::*;
        let cases = [
            (Clear, Undefined, Priority::Notice),
            (Critical, Undefined, Priority::Notice),
            (Uninitialized, Undefined, Priority::Debug),
            (Removed, Undefined, Priority::Debug),
            (Removed, Uninitialized, Priority::Debug),
            (Clear, Removed, Priority::Debug),
            (Uninitialized, Clear, Priority::Debug),
            (Removed, Clear, Priority::Info),
            (Undefined, Clear, Priority::Info),
            (Warning, Clear, Priority::Notice),
            (Critical, Clear, Priority::Notice),
            (Clear, Warning, Priority::Warning),
            (Uninitialized, Warning, Priority::Warning),
            (Critical, Warning, Priority::Info),
            (Warning, Critical, Priority::Crit),
            (Clear, Critical, Priority::Crit),
        ];
        for (old, new, priority) in cases {
            assert_eq!(priority_and_symbol(old, new).0, priority, "{old:?} to {new:?}");
        }
        assert_eq!(priority_and_symbol(Clear, Warning).1.as_bytes(), b"\xe2\x9a\xa0\xef\xb8\x8f");
        assert_eq!(priority_and_symbol(Critical, Warning).1, "\u{1f536}");
    }
}
