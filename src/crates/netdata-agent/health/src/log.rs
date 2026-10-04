//! A host's alert log in memory (`host->health_log`): `health_log.c` `health_alarm_log_add_entry()`,
//! `health_notifications.c` `health_alarm_log_process_to_send_notifications()`.
//!
//! Nothing here saves or notifies: an entry's saves and its notification are made by the host's alerts once the
//! store's lock is released (`alerts.rs`), so these functions hand back what is owed.

use std::collections::VecDeque;

use crate::Clock;
use crate::alert::Status;
use crate::entry::{Entry, entry_flags};

/// The entries, newest first, and the host's counters around them.
#[derive(Default)]
pub(crate) struct AlarmLog {
    pub entries: VecDeque<Entry>,
    /// Entries that left the log while the metadata queue still held a save of theirs (C hands them to
    /// `metadata_queue_ae_deletion()`): the store job saves each as it stands, and it goes once none is left.
    pub parked: Vec<Entry>,
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
    /// updated by it; if that one went from raised to raised, its non-clear duration is carried over. Returns the
    /// entry as it stands in the log, and the unique id of the older entry it marked: C saves that one, then the
    /// new one, after the log's lock is released.
    pub fn add(&mut self, mut entry: Entry) -> (Entry, Option<u32>) {
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
        self.entries.insert(at, entry.clone());
        (entry, updated)
    }

    /// The entry of that unique id: in the log, else among those that left it with a save queued.
    pub fn entry_mut(&mut self, unique_id: u32) -> Option<&mut Entry> {
        let parked = &mut self.parked;
        match self.entries.iter_mut().find(|entry| entry.unique_id == unique_id) {
            Some(entry) => Some(entry),
            None => parked.iter_mut().find(|entry| entry.unique_id == unique_id),
        }
    }

    /// A queued save of that entry was made: one fewer is pending. An entry that left the log goes with its last.
    pub fn save_done(&mut self, unique_id: u32) {
        if let Some(entry) = self.entry_mut(unique_id) {
            entry.pending_save_count = entry.pending_save_count.saturating_sub(1);
        }
        self.parked.retain(|entry| entry.unique_id != unique_id || entry.waits_for_a_save());
    }

    /// A save of that entry was asked for, and is made once the store's lock is released.
    pub fn owe_save(&mut self, unique_id: u32) {
        if let Some(entry) = self.entry_mut(unique_id) {
            entry.owed_saves = entry.owed_saves.saturating_add(1);
        }
    }

    /// An owed save of that entry was made, handed to the metadata queue, or given up. An entry that left the log
    /// goes when it waits for no other.
    pub fn save_settled(&mut self, unique_id: u32) {
        if let Some(entry) = self.entry_mut(unique_id) {
            entry.owed_saves = entry.owed_saves.saturating_sub(1);
        }
        self.parked.retain(|entry| entry.unique_id != unique_id || entry.waits_for_a_save());
    }

    /// The walk of `health_alarm_log_process_to_send_notifications()`: the unique ids of the entries that still
    /// wait (not processed, not replaced) and whose delay is over, in the log's order; the lowest waiting id is
    /// remembered for the next call. The caller notifies each, marks it as processed and saves it.
    ///
    /// C reads the clock when it starts. At the first due entry it builds the summary of raised alerts, unless
    /// the pass has built it already, reads the clock again and starts over: the second answer says whether the
    /// caller is to build the summary now.
    pub fn scan(&mut self, clock: Clock, summary_is_built: bool) -> (Vec<u32>, bool) {
        let mut now = clock();
        let mut summary_built = summary_is_built;
        let (first_waiting, due) = 'scan: loop {
            let mut first_waiting = self.entries.front().map_or(0, |entry| entry.unique_id);
            let mut due = Vec::new();
            for entry in &self.entries {
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
                    due.push(entry.unique_id);
                }
            }
            break (first_waiting, due);
        };
        self.last_processed_id = first_waiting;
        (due, summary_built && !summary_is_built)
    }

    /// The end of that function: entries memory needs no longer are dropped: one a newer entry replaced, once
    /// saved and not being executed, unless its alert repeats; and a saved REMOVED entry more than a day old, which
    /// costs a read of the clock for each saved REMOVED entry the first rule keeps.
    pub fn trim(&mut self, clock: Clock) {
        let goes = |entry: &Entry| {
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
            replaced || old_removal()
        };
        self.remove_where(goes);
    }

    /// `health_alarm_log_cleanup()`: the entries older than the retention that a newer one replaced and that have
    /// no save pending go; nothing is asked of whether they were saved.
    pub fn cleanup(&mut self, retention_s: u32, now: i64) {
        self.remove_where(|entry| {
            now.saturating_sub(i64::from(retention_s)) > entry.when
                && entry.flags & entry_flags::UPDATED != 0
                && entry.pending_save_count == 0
        });
    }

    /// `health_alarm_log_free()`: every entry leaves the log.
    pub fn clear(&mut self) {
        self.remove_where(|_| true);
    }

    /// The entries `goes` names leave the log; one with a save still queued is kept aside for the store job
    /// (`health_alarm_log_free_one_nochecks_nounlink()`), and one whose save is owed since it was logged for the
    /// thread that is about to make it.
    fn remove_where(&mut self, goes: impl Fn(&Entry) -> bool) {
        let mut kept = VecDeque::with_capacity(self.entries.len());
        for entry in self.entries.drain(..) {
            if !goes(&entry) {
                kept.push_back(entry);
            } else if entry.waits_for_a_save() {
                self.parked.push(entry);
            }
        }
        self.entries = kept;
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
            pending_save_count: 0,
            owed_saves: 0,
        }
    }

    fn log(first_id: u32) -> AlarmLog {
        AlarmLog { next_log_id: first_id, ..AlarmLog::default() }
    }

    fn ids(log: &AlarmLog) -> Vec<u32> {
        log.entries.iter().map(|entry| entry.unique_id).collect()
    }

    /// What the caller does to every entry once it was added (C's insert) and once the scan named it.
    fn mark_all(log: &mut AlarmLog, flag: u32) {
        for entry in &mut log.entries {
            entry.flags |= flag;
        }
    }

    /// The scan reads the clock when it starts, and once more at the first entry that is due (C builds its summary
    /// of raised alerts there and starts over): an entry that became due between the two reads is named too.
    /// An entry that leaves the log before the save it was logged with is made waits for that save: the thread
    /// that logged it finds it and saves it, and only then is it let go.
    #[test]
    fn an_entry_that_leaves_the_log_before_its_save_waits_for_it() {
        let mut log = log(100);
        let (added, _) = log.add(entry(7, Status::Clear, NOW));
        log.owe_save(added.unique_id);
        log.clear();
        assert!(log.entries.is_empty());
        assert_eq!(log.entry_mut(added.unique_id).map(|entry| entry.owed_saves), Some(1), "kept aside");
        log.save_settled(added.unique_id);
        assert!(log.entry_mut(added.unique_id).is_none(), "its save is made: it goes");

        // one that also has a save queued waits for both
        let (added, _) = log.add(entry(7, Status::Clear, NOW));
        log.owe_save(added.unique_id);
        log.entry_mut(added.unique_id).expect("in the log").pending_save_count = 1;
        log.clear();
        log.save_settled(added.unique_id);
        assert!(log.entry_mut(added.unique_id).is_some(), "the queued save is still to come");
        log.save_done(added.unique_id);
        assert!(log.entry_mut(added.unique_id).is_none());
    }

    #[test]
    fn the_scan_reads_the_clock_again_at_its_first_due_entry() {
        let reads = Cell::new(0);
        let clock = || {
            reads.set(reads.get() + 1);
            NOW + reads.get() - 1
        };

        // nothing due: one read
        let mut waiting = log(1);
        waiting.add(entry(1, Status::Warning, NOW + 5));
        assert_eq!((waiting.scan(&clock, false), reads.get(), waiting.last_processed_id), ((vec![], false), 1, 1));

        // the older entry is due at the first read, the newer one only at the second
        reads.set(0);
        let mut two = log(1);
        two.add(entry(1, Status::Warning, NOW));
        two.add(entry(2, Status::Warning, NOW + 1));
        assert_eq!((two.scan(&clock, false), reads.get()), ((vec![2, 1], true), 2));
        // the lowest id that waited when the scan came to it is remembered; the next scan, which finds nothing
        // waiting, moves on to the newest
        assert_eq!(two.last_processed_id, 1);
        mark_all(&mut two, entry_flags::PROCESSED);
        assert_eq!((two.scan(&clock, false), reads.get(), two.last_processed_id), ((vec![], false), 3, 2));

        // a pass whose repeats built the summary already does not start over: one read, and the entry that is
        // due only at a second read waits
        reads.set(0);
        let mut built = log(1);
        built.add(entry(1, Status::Warning, NOW));
        built.add(entry(2, Status::Warning, NOW + 1));
        assert_eq!((built.scan(&clock, true), reads.get()), ((vec![1], false), 1));
    }

    /// The trim: an entry a newer one replaced goes once it is saved, unless its alert repeats or its command
    /// runs; a saved removal goes when it is more than a day old, which costs a read of the clock.
    #[test]
    fn the_trim_drops_what_was_replaced_and_old_removals() {
        let reads = Cell::new(0);
        let at = |second: i64| {
            let reads = &reads;
            move || {
                reads.set(reads.get() + 1);
                second
            }
        };

        let mut log = log(1);
        log.add(entry(1, Status::Clear, NOW));
        log.add(entry(1, Status::Warning, NOW));
        let mut repeating = entry(2, Status::Clear, NOW);
        repeating.flags = entry_flags::IS_REPEATING;
        log.add(repeating);
        log.add(entry(2, Status::Warning, NOW));
        let mut running = entry(3, Status::Clear, NOW);
        running.flags = entry_flags::EXEC_IN_PROGRESS;
        log.add(running);
        log.add(entry(3, Status::Removed, NOW));
        assert_eq!(ids(&log), [6, 5, 4, 3, 2, 1]);
        mark_all(&mut log, entry_flags::SAVED);

        // the replaced entry of the plain alarm goes; the repeating one and the one being executed stay
        log.trim(&at(NOW + 86400));
        assert_eq!(ids(&log), [6, 5, 4, 3, 2]);
        // one read, for the saved removal
        assert_eq!(reads.get(), 1);

        // a day and a second after the removal
        log.trim(&at(NOW + 86401));
        assert_eq!(ids(&log), [5, 4, 3, 2]);
        assert!(log.parked.is_empty());

        // an entry that was never saved stays, replaced or not
        let mut log = self::log(1);
        log.add(entry(1, Status::Clear, NOW));
        log.add(entry(1, Status::Removed, NOW));
        log.trim(&at(NOW + 10 * 86400));
        assert_eq!(ids(&log), [2, 1]);
    }

    /// An entry that leaves the log while a save of its is queued is kept aside until that save was made: the
    /// store job finds it by its id, as C's job holds a pointer to it.
    #[test]
    fn an_entry_with_a_queued_save_is_parked_when_it_leaves_the_log() {
        let mut log = log(1);
        log.add(entry(1, Status::Clear, NOW));
        log.add(entry(1, Status::Warning, NOW));
        mark_all(&mut log, entry_flags::SAVED);
        log.entry_mut(1).expect("the entry").pending_save_count = 2;

        log.trim(&|| NOW);
        assert_eq!((ids(&log), log.parked.len()), (vec![2], 1));
        assert_eq!(log.entry_mut(1).map(|entry| entry.pending_save_count), Some(2), "found among the parked");
        log.save_done(1);
        assert_eq!(log.parked.len(), 1, "one save is still queued");
        log.save_done(1);
        assert!(log.parked.is_empty() && log.entry_mut(1).is_none());

        // an entry in the log only counts down
        log.entry_mut(2).expect("the entry").pending_save_count = 1;
        log.save_done(2);
        assert_eq!((ids(&log), log.entry_mut(2).map(|entry| entry.pending_save_count)), (vec![2], Some(0)));
    }

    /// The retention cleanup of memory: older than the retention and replaced, with no save pending; saved or not.
    #[test]
    fn the_cleanup_takes_old_replaced_entries_saved_or_not() {
        let mut log = log(1);
        log.add(entry(1, Status::Clear, NOW));
        log.add(entry(1, Status::Warning, NOW + 10));
        log.add(entry(1, Status::Clear, NOW + 20));
        // the first two are replaced; 50 s after the first is not "older than 50 s" yet
        log.cleanup(50, NOW + 50);
        assert_eq!(ids(&log), [3, 2, 1]);
        log.cleanup(50, NOW + 51);
        assert_eq!(ids(&log), [3, 2]);
        // one with a queued save waits for it
        log.entry_mut(2).expect("the entry").pending_save_count = 1;
        log.cleanup(0, NOW + 1000);
        assert_eq!((ids(&log), log.parked.len()), (vec![3, 2], 0));
        log.save_done(2);
        log.cleanup(0, NOW + 1000);
        assert_eq!(ids(&log), [3], "the last entry was not replaced");
    }

    /// An entry links to the newest older entry of its alarm, once: one that something replaced already is left
    /// alone, and the search ends at it. The older entry's id is handed back: it is saved before the new one.
    #[test]
    fn an_entry_replaces_the_newest_older_one_of_its_alarm() {
        let mut log = log(10);
        assert_eq!(log.add(entry(1, Status::Warning, NOW)).1, None);
        log.add(entry(2, Status::Warning, NOW));
        let (second, older) = log.add(entry(1, Status::Critical, NOW));
        assert_eq!((second.unique_id, second.updates_id, older), (12, 10, Some(10)));
        let links: Vec<_> = log.entries.iter().map(|e| (e.unique_id, e.updated_by_id, e.updates_id)).collect();
        assert_eq!(links, [(12, 0, 10), (11, 0, 0), (10, 12, 0)]);
        assert_eq!(log.transitions, 3);

        // the alarm's newest older entry is marked as replaced by a walk of the log, not by an entry: a new entry
        // finds it, leaves it, and looks no further
        log.mark_updated();
        let (third, older) = log.add(entry(1, Status::Clear, NOW));
        assert_eq!((third.unique_id, third.updates_id, older), (13, 0, None));
        assert_eq!(log.entries[1].updated_by_id, 0);
    }
}
