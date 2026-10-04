//! A host's alert log in memory (`host->health_log`): `health_log.c` `health_alarm_log_add_entry()`,
//! `health_notifications.c` `health_alarm_log_process_to_send_notifications()`.

use std::collections::VecDeque;

use crate::Clock;
use crate::alert::Status;
use crate::entry::{Entry, entry_flags};
use crate::pass::Env;

/// The entries, newest first, and the host's counters around them.
#[derive(Default)]
pub(crate) struct AlarmLog {
    pub entries: VecDeque<Entry>,
    /// `host->health_log.next_log_id`: the unique id the next entry takes.
    pub next_log_id: u32,
    /// `host->health_last_processed_id`: no entry below it waits for its notification.
    pub last_processed_id: u32,
    /// `host->health_transitions`: the entries added so far.
    pub transitions: u64,
}

impl AlarmLog {
    /// `health_alarm_entry_assign_unique_id()`: the next id, for an entry that has none.
    pub fn assign_unique_id(&mut self, entry: &mut Entry) {
        if entry.unique_id == 0 {
            entry.unique_id = self.next_log_id;
            self.next_log_id = self.next_log_id.wrapping_add(1);
        }
    }

    /// `health_alarm_log_add_entry()`: the entry takes its unique id and its place (before the first entry whose
    /// id is not greater). The newest older entry of its alarm, unless something replaced it already, is marked as
    /// updated by it; if that one went from raised to raised, its non-clear duration is carried over. The older
    /// entry is saved, then the new one. Returns the entry as it stands in the log.
    pub fn add(&mut self, mut entry: Entry, is_async: bool, env: &dyn Env) -> Entry {
        self.transitions += 1;
        self.assign_unique_id(&mut entry);
        let at = self.entries.iter().position(|other| other.unique_id <= entry.unique_id).unwrap_or(self.entries.len());

        let raised = |status| matches!(status, Status::Warning | Status::Critical);
        let mut updated = None;
        if let Some(older) = self.entries.iter_mut().skip(at).find(|older| older.alarm_id == entry.alarm_id)
            && older.flags & entry_flags::UPDATED == 0
            && older.updated_by_id == 0
        {
            older.flags |= entry_flags::UPDATED;
            older.updated_by_id = entry.unique_id;
            entry.updates_id = older.unique_id;
            if raised(older.new_status) && raised(older.old_status) {
                entry.non_clear_duration = entry.non_clear_duration.saturating_add(older.non_clear_duration);
            }
            updated = Some(older.unique_id);
        }
        self.entries.insert(at, entry);

        if let Some(older) = updated.and_then(|id| self.entries.iter_mut().find(|other| other.unique_id == id)) {
            env.save(older, is_async);
        }
        let entry = &mut self.entries[at];
        env.save(entry, is_async);
        entry.clone()
    }

    /// `health_send_notification()` as far as the log goes: the entry is offered for a notification, marked as
    /// processed and saved. What a notification is comes with its own commit.
    pub fn send_notification(entry: &mut Entry, env: &dyn Env) {
        env.notify(entry);
        entry.flags |= entry_flags::PROCESSED;
        env.save(entry, false);
    }

    /// `health_alarm_log_process_to_send_notifications()`: every entry that still waits (not processed, not
    /// replaced) and whose delay is over gets its notification; the lowest waiting id is remembered for the next
    /// call. Then entries memory needs no longer are dropped: one a newer entry replaced, once saved and not being
    /// executed, unless its alert repeats; and a saved REMOVED entry more than a day old.
    ///
    /// C reads the clock when it starts, again at the first due entry (it builds the summary of raised alerts
    /// there and starts over), and once for each saved REMOVED entry the first rule keeps.
    pub fn process(&mut self, env: &dyn Env, clock: Clock) {
        let mut now = clock();
        let mut summary_built = false;
        let first_waiting = 'scan: loop {
            let mut first_waiting = self.entries.front().map_or(0, |entry| entry.unique_id);
            for entry in &mut self.entries {
                if entry.unique_id < self.last_processed_id {
                    break;
                }
                if entry.flags & (entry_flags::PROCESSED | entry_flags::UPDATED) != 0 {
                    continue;
                }
                first_waiting = first_waiting.min(entry.unique_id);
                if now >= entry.delay_up_to_timestamp {
                    if !summary_built {
                        summary_built = true;
                        now = clock();
                        continue 'scan;
                    }
                    Self::send_notification(entry, env);
                }
            }
            break first_waiting;
        };
        self.last_processed_id = first_waiting;

        self.entries.retain(|entry| {
            let has = |flag| entry.flags & flag != 0;
            let replaced = !has(entry_flags::IS_REPEATING)
                && has(entry_flags::UPDATED)
                && has(entry_flags::SAVED)
                && !has(entry_flags::EXEC_IN_PROGRESS);
            let old_removal = || {
                entry.new_status == Status::Removed
                    && has(entry_flags::SAVED)
                    && entry.when.saturating_add(86400) < clock()
            };
            !(replaced || old_removal())
        });
    }

    /// `health_apply_prototypes_to_host()`'s walk of the log: every entry that is not a removal counts as replaced.
    pub fn mark_updated(&mut self) {
        for entry in &mut self.entries {
            if entry.new_status != Status::Removed {
                entry.flags |= entry_flags::UPDATED;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::testing::Scripted;

    const NOW: i64 = 1_700_000_000;

    /// An entry of alarm `alarm_id` that is due at `due`.
    fn entry(alarm_id: u32, new_status: Status, due: i64) -> Entry {
        Entry {
            unique_id: 0,
            alarm_id,
            alarm_event_id: 1,
            global_id: 0,
            config_hash_id: [0; 16],
            transition_id: [0; 16],
            when: due,
            duration: 0,
            non_clear_duration: 0,
            name: None,
            chart: b"t.c".to_vec(),
            chart_context: b"t.ctx".to_vec(),
            chart_name: b"t.c".to_vec(),
            classification: None,
            component: None,
            r#type: None,
            exec: None,
            recipient: None,
            source: None,
            units: None,
            summary: None,
            info: None,
            exec_run_timestamp: 0,
            exec_code: 0,
            old_value: f64::NAN,
            new_value: f64::NAN,
            old_value_string: b"-".to_vec(),
            new_value_string: b"-".to_vec(),
            old_status: Status::Uninitialized,
            new_status,
            flags: 0,
            delay: 0,
            delay_up_to_timestamp: due,
            updated_by_id: 0,
            updates_id: 0,
            last_repeat: 0,
        }
    }

    fn log(first_id: u32) -> AlarmLog {
        AlarmLog { next_log_id: first_id, ..AlarmLog::default() }
    }

    /// The scan reads the clock when it starts, and once more at the first entry that is due (C builds its summary
    /// of raised alerts there and starts over): an entry that became due between the two reads is processed too.
    #[test]
    fn the_scan_reads_the_clock_again_at_its_first_due_entry() {
        let reads = Cell::new(0);
        let clock = || {
            reads.set(reads.get() + 1);
            NOW + reads.get() - 1
        };
        let env = Scripted::default();
        let processed = |log: &AlarmLog| -> Vec<bool> {
            log.entries.iter().map(|entry| entry.flags & entry_flags::PROCESSED != 0).collect()
        };

        // nothing due: one read
        let mut waiting = log(1);
        waiting.add(entry(1, Status::Warning, NOW + 5), false, &env);
        waiting.process(&env, &clock);
        assert_eq!((reads.get(), processed(&waiting), waiting.last_processed_id), (1, vec![false], 1));

        // the older entry is due at the first read, the newer one only at the second
        reads.set(0);
        let mut two = log(1);
        two.add(entry(1, Status::Warning, NOW), false, &env);
        two.add(entry(2, Status::Warning, NOW + 1), false, &env);
        two.process(&env, &clock);
        assert_eq!((reads.get(), processed(&two)), (2, vec![true, true]));
        // the lowest id that waited when the scan came to it is remembered; the next scan, which finds nothing
        // waiting, moves on to the newest
        assert_eq!(two.last_processed_id, 1);
        two.process(&env, &clock);
        assert_eq!((reads.get(), two.last_processed_id), (3, 2));
    }

    /// The trim: an entry a newer one replaced goes once it is saved, unless its alert repeats or its command
    /// runs; a saved removal goes when it is more than a day old, which costs a read of the clock.
    #[test]
    fn the_trim_drops_what_was_replaced_and_old_removals() {
        let saves = Scripted { saves: true, ..Scripted::default() };
        let reads = Cell::new(0);
        let at = |second: i64| {
            let reads = &reads;
            move || {
                reads.set(reads.get() + 1);
                second
            }
        };
        let ids = |log: &AlarmLog| -> Vec<u32> { log.entries.iter().map(|entry| entry.unique_id).collect() };

        let mut log = log(1);
        log.add(entry(1, Status::Clear, NOW), false, &saves);
        log.add(entry(1, Status::Warning, NOW), false, &saves);
        let mut repeating = entry(2, Status::Clear, NOW);
        repeating.flags = entry_flags::IS_REPEATING;
        log.add(repeating, false, &saves);
        log.add(entry(2, Status::Warning, NOW), false, &saves);
        let mut running = entry(3, Status::Clear, NOW);
        running.flags = entry_flags::EXEC_IN_PROGRESS;
        log.add(running, false, &saves);
        log.add(entry(3, Status::Removed, NOW), false, &saves);
        assert_eq!(ids(&log), [6, 5, 4, 3, 2, 1]);

        // the replaced entry of the plain alarm goes; the repeating one and the one being executed stay
        log.process(&saves, &at(NOW + 86400));
        assert_eq!(ids(&log), [6, 5, 4, 3, 2]);
        // one read at the start, one at the first due entry, one for the saved removal
        assert_eq!(reads.get(), 3);

        // a day and a second after the removal
        log.process(&saves, &at(NOW + 86401));
        assert_eq!(ids(&log), [5, 4, 3, 2]);

        // an entry that was never saved stays, replaced or not
        let unsaved = Scripted::default();
        let mut log = self::log(1);
        log.add(entry(1, Status::Clear, NOW), false, &unsaved);
        log.add(entry(1, Status::Removed, NOW), false, &unsaved);
        log.process(&unsaved, &at(NOW + 10 * 86400));
        assert_eq!(ids(&log), [2, 1]);
    }

    /// An entry links to the newest older entry of its alarm, once: one that something replaced already is left
    /// alone, and the search ends at it.
    #[test]
    fn an_entry_replaces_the_newest_older_one_of_its_alarm() {
        let env = Scripted::default();
        let mut log = log(10);
        log.add(entry(1, Status::Warning, NOW), false, &env);
        log.add(entry(2, Status::Warning, NOW), false, &env);
        let second = log.add(entry(1, Status::Critical, NOW), false, &env);
        assert_eq!((second.unique_id, second.updates_id), (12, 10));
        let links: Vec<_> = log.entries.iter().map(|e| (e.unique_id, e.updated_by_id, e.updates_id)).collect();
        assert_eq!(links, [(12, 0, 10), (11, 0, 0), (10, 12, 0)]);
        assert_eq!(log.transitions, 3);

        // the alarm's newest older entry is marked as replaced by a walk of the log, not by an entry: a new entry
        // finds it, leaves it, and looks no further
        log.mark_updated();
        let third = log.add(entry(1, Status::Clear, NOW), false, &env);
        assert_eq!((third.unique_id, third.updates_id), (13, 0));
        assert_eq!(log.entries[1].updated_by_id, 0);
    }
}
