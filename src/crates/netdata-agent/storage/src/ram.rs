//! The ram and alloc storage engines, ported from `src/database/ram/rrddim_mem.c`: one fixed ring of
//! `storage_number`s per dimension, overwritten round-robin.
//!
//! One collector thread stores while queries read concurrently. As in C, every field is a relaxed atomic: a query
//! racing a store may see a slot before the counters that describe it, which the C engine tolerates too.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicUsize, Ordering::Relaxed};

use crate::storage_number::{self, SN_EMPTY_SLOT, SN_USER_FLAGS};
use crate::storage_point::StoragePoint;

/// `rrddim_db_mode` entries of a ram chart (`[db] retention` default): 4096 slots.
pub const RAM_ENTRIES: usize = 4096;
/// The alloc engine allocates at least this many slots (`rrddim_insert_callback`).
pub const ALLOC_MIN_ENTRIES: usize = 5;

/// The chart state a ring is seeded from (`update_metric_handle_from_rrddim()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seed {
    pub counter: usize,
    pub current_entry: usize,
    pub last_updated_s: i64,
    pub update_every_s: i64,
}

/// `struct mem_metric_handle`.
#[derive(Debug)]
pub struct RamMetric {
    data: Box<[AtomicU32]>,
    counter: AtomicUsize,
    current_entry: AtomicUsize,
    last_updated_s: AtomicI64,
    update_every_s: AtomicI64,
}

impl RamMetric {
    /// A ring of `entries` zero-filled slots (the anonymous mapping or `callocz()` of C), seeded from the chart.
    pub fn new(entries: usize, seed: Seed) -> Self {
        assert!(entries > 0, "a ring needs at least one slot");
        RamMetric {
            data: (0..entries).map(|_| AtomicU32::new(0)).collect(),
            counter: AtomicUsize::new(seed.counter),
            current_entry: AtomicUsize::new(seed.current_entry),
            last_updated_s: AtomicI64::new(seed.last_updated_s),
            update_every_s: AtomicI64::new(seed.update_every_s),
        }
    }

    pub fn entries(&self) -> usize {
        self.data.len()
    }

    /// The raw storage number in a slot (`rd->db.data[slot]`).
    pub fn slot(&self, slot: usize) -> u32 {
        self.data[slot].load(Relaxed)
    }

    /// `rrddim_collect_init()` re-seeds the handle from the chart.
    pub fn reseed(&self, seed: Seed) {
        self.counter.store(seed.counter, Relaxed);
        self.current_entry.store(seed.current_entry, Relaxed);
        self.last_updated_s.store(seed.last_updated_s, Relaxed);
        self.update_every_s.store(seed.update_every_s, Relaxed);
    }

    fn last_slot(&self) -> usize {
        match self.current_entry.load(Relaxed) {
            0 => self.entries() - 1,
            current => current - 1,
        }
    }

    fn first_slot(&self) -> usize {
        if self.counter.load(Relaxed) >= self.entries() {
            self.current_entry.load(Relaxed)
        } else {
            0
        }
    }

    /// `rrddim_store_metric_flush()`: every slot empty, counters zeroed.
    pub fn flush(&self) {
        for slot in self.data.iter() {
            slot.store(SN_EMPTY_SLOT, Relaxed);
        }
        self.counter.store(0, Relaxed);
        self.last_updated_s.store(0, Relaxed);
        self.current_entry.store(0, Relaxed);
    }

    /// `rrddim_store_metric_change_collection_frequency()`.
    pub fn change_update_every(&self, update_every_s: i64) {
        self.flush();
        self.update_every_s.store(update_every_s, Relaxed);
    }

    /// `rrddim_fill_the_gap()`. The loop writes an empty slot for every step up to and including `now_s`, and the
    /// caller then stores the sample in the next slot: each gap shifts the older data one step back, as in C.
    fn fill_the_gap(&self, now_s: i64) {
        let entries = self.entries();
        let update_every_s = self.update_every_s.load(Relaxed);
        let last_stored_s = self.last_updated_s.load(Relaxed);
        let gap_entries = ((now_s - last_stored_s) / update_every_s) as usize;
        if gap_entries >= entries {
            self.flush();
            return;
        }
        let mut current = self.current_entry.load(Relaxed);
        let mut store_s = last_stored_s + update_every_s;
        let mut filled = 0;
        while filled < entries && store_s <= now_s {
            self.data[current].store(SN_EMPTY_SLOT, Relaxed);
            current += 1;
            if current >= entries {
                current = 0;
            }
            store_s += update_every_s;
            filled += 1;
        }
        self.counter.fetch_add(filled, Relaxed);
        self.current_entry.store(current, Relaxed);
        self.last_updated_s.store(store_s, Relaxed);
    }

    /// `rrddim_collect_store_metric()`: min, max, count and anomaly count are not kept by this engine. Samples at or
    /// before the last stored second are dropped silently.
    pub fn store(&self, point_in_time_ut: u64, value: f64, flags: u32) {
        let point_s = (point_in_time_ut / 1_000_000) as i64;
        let last_updated_s = self.last_updated_s.load(Relaxed);
        let update_every_s = self.update_every_s.load(Relaxed);
        if point_s <= last_updated_s {
            return;
        }
        if last_updated_s != 0 && point_s - update_every_s > last_updated_s {
            self.fill_the_gap(point_s);
        }
        let current = self.current_entry.load(Relaxed);
        self.data[current].store(storage_number::pack(value, flags), Relaxed);
        self.counter.fetch_add(1, Relaxed);
        self.current_entry.store(
            if current + 1 >= self.entries() {
                0
            } else {
                current + 1
            },
            Relaxed,
        );
        self.last_updated_s.store(point_s, Relaxed);
    }

    /// `rrddim_query_latest_time_s()`.
    pub fn latest_time_s(&self) -> i64 {
        self.last_updated_s.load(Relaxed)
    }

    /// `rrddim_query_oldest_time_s()`: the start of the first stored interval.
    pub fn oldest_time_s(&self) -> i64 {
        let counter = self.counter.load(Relaxed) as i64;
        let entries = self.entries() as i64;
        self.last_updated_s.load(Relaxed) - counter.min(entries) * self.update_every_s.load(Relaxed)
    }

    pub fn update_every_s(&self) -> i64 {
        self.update_every_s.load(Relaxed)
    }

    /// `rrddim_time2slot()`: always a valid slot, the nearest one for times outside the ring.
    fn time2slot(&self, t: i64) -> usize {
        let last_s = self.latest_time_s();
        let entries = self.entries();
        let last_slot = self.last_slot();
        let slot = if t >= last_s {
            last_slot
        } else if t <= self.oldest_time_s() {
            self.first_slot()
        } else {
            let back = ((last_s - t) / self.update_every_s.load(Relaxed)) as usize;
            if last_slot >= back {
                last_slot - back
            } else {
                (last_slot + entries).wrapping_sub(back)
            }
        };
        // C logs an internal error here.
        if slot >= entries { entries - 1 } else { slot }
    }

    /// `rrddim_slot2time()`, clamped to the stored range.
    fn slot2time(&self, slot: usize) -> i64 {
        let last_s = self.latest_time_s();
        let entries = self.entries();
        let last_slot = self.last_slot();
        let update_every_s = self.update_every_s.load(Relaxed);
        // C logs internal errors for an invalid slot and for times outside the range.
        let slot = slot.min(entries - 1);
        let back = if slot > last_slot {
            last_slot + entries - slot
        } else {
            last_slot - slot
        } as i64;
        (last_s - update_every_s * back)
            .clamp(self.oldest_time_s(), last_s.max(self.oldest_time_s()))
    }

    /// `rrddim_query_init()`.
    pub fn query(&self, start_time_s: i64, end_time_s: i64) -> RamQuery<'_> {
        let slot = self.time2slot(start_time_s);
        let last_slot = self.time2slot(end_time_s);
        RamQuery {
            metric: self,
            end_time_s,
            slot,
            dt: self.update_every_s.load(Relaxed),
            next_timestamp: start_time_s,
            slot_timestamp: self.slot2time(slot),
            last_timestamp: self.slot2time(last_slot),
        }
    }
}

/// `struct mem_query_handle`: points follow the request's grid from `start_time_s`, one per update interval.
#[derive(Debug)]
pub struct RamQuery<'a> {
    metric: &'a RamMetric,
    end_time_s: i64,
    slot: usize,
    dt: i64,
    next_timestamp: i64,
    slot_timestamp: i64,
    last_timestamp: i64,
}

impl RamQuery<'_> {
    /// `rrddim_query_next_metric()`: times before the next stored slot or after the last one give empty points
    /// without advancing the slot.
    pub fn next_metric(&mut self) -> StoragePoint {
        let this_timestamp = self.next_timestamp;
        self.next_timestamp += self.dt;
        let (start, end) = (this_timestamp - self.dt, this_timestamp);
        if this_timestamp < self.slot_timestamp || this_timestamp > self.last_timestamp {
            return StoragePoint::empty(start, end);
        }
        let n = self.metric.data[self.slot].load(Relaxed);
        self.slot += 1;
        if self.slot >= self.metric.entries() {
            self.slot = 0;
        }
        self.slot_timestamp += self.dt;
        let value = storage_number::unpack(n);
        StoragePoint {
            min: value,
            max: value,
            sum: value,
            start_time_s: start,
            end_time_s: end,
            count: 1,
            anomaly_count: u32::from(storage_number::is_anomalous(n)),
            flags: n & SN_USER_FLAGS,
        }
    }

    /// `rrddim_query_is_finished()`.
    pub fn is_finished(&self) -> bool {
        self.next_timestamp > self.end_time_s
    }

    /// `rrddim_query_align_to_optimal_before()`.
    pub fn align_to_optimal_before(&self) -> i64 {
        self.end_time_s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_number::SN_DEFAULT_FLAGS;

    const T0: i64 = 1_700_000_000;

    fn metric(entries: usize) -> RamMetric {
        RamMetric::new(
            entries,
            Seed {
                counter: 0,
                current_entry: 0,
                last_updated_s: 0,
                update_every_s: 1,
            },
        )
    }

    fn store(m: &RamMetric, t: i64, v: f64) {
        m.store(t as u64 * 1_000_000, v, SN_DEFAULT_FLAGS);
    }

    fn sums(m: &RamMetric, after: i64, before: i64) -> Vec<Option<f64>> {
        let mut q = m.query(after, before);
        let mut out = Vec::new();
        while !q.is_finished() {
            let p = q.next_metric();
            out.push((!p.is_gap()).then_some(p.sum));
        }
        out
    }

    #[test]
    fn oldest_is_the_start_of_the_first_interval() {
        let m = metric(RAM_ENTRIES);
        for i in 1..=6 {
            store(&m, T0 + i, i as f64);
        }
        assert_eq!((m.oldest_time_s(), m.latest_time_s()), (T0, T0 + 6));
        let expected: Vec<_> = (1..=6).map(|i| Some(i as f64)).collect();
        assert_eq!(sums(&m, T0 + 1, T0 + 6), expected);
        // Before the first slot and after the last one: empty points, the slot does not move.
        assert_eq!(
            sums(&m, T0 - 1, T0 + 8),
            [
                None,
                None,
                Some(1.0),
                Some(2.0),
                Some(3.0),
                Some(4.0),
                Some(5.0),
                Some(6.0),
                None,
                None
            ]
        );
    }

    #[test]
    fn older_and_duplicate_samples_are_dropped() {
        let m = metric(16);
        store(&m, T0 + 2, 2.0);
        store(&m, T0 + 2, 9.0);
        store(&m, T0 + 1, 9.0);
        assert_eq!(sums(&m, T0 + 2, T0 + 2), [Some(2.0)]);
    }

    #[test]
    fn a_gap_takes_one_slot_more_than_it_spans() {
        let m = metric(16);
        store(&m, T0 + 1, 1.0);
        store(&m, T0 + 2, 2.0);
        store(&m, T0 + 5, 5.0);
        // Three empty slots (T0+3..T0+5) then the value: six slots for five seconds.
        assert_eq!((m.oldest_time_s(), m.latest_time_s()), (T0 - 1, T0 + 5));
        assert_eq!(
            sums(&m, T0, T0 + 5),
            [Some(1.0), Some(2.0), None, None, None, Some(5.0)]
        );
    }

    #[test]
    fn a_gap_longer_than_the_ring_flushes_it() {
        let m = metric(5);
        store(&m, T0 + 1, 1.0);
        store(&m, T0 + 7, 7.0);
        assert_eq!((m.oldest_time_s(), m.latest_time_s()), (T0 + 6, T0 + 7));
        assert_eq!(sums(&m, T0 + 7, T0 + 7), [Some(7.0)]);
    }

    #[test]
    fn the_ring_wraps() {
        let m = metric(5);
        for i in 1..=12 {
            store(&m, T0 + i, i as f64);
        }
        assert_eq!((m.oldest_time_s(), m.latest_time_s()), (T0 + 7, T0 + 12));
        let expected: Vec<_> = (8..=12).map(|i| Some(i as f64)).collect();
        assert_eq!(sums(&m, T0 + 8, T0 + 12), expected);
        assert_eq!(sums(&m, T0 + 10, T0 + 11), [Some(10.0), Some(11.0)]);
    }

    #[test]
    fn empty_slots_are_gaps_and_flags_come_back() {
        let m = metric(8);
        store(&m, T0 + 1, 1.0);
        m.store((T0 + 2) as u64 * 1_000_000, 2.0, 0);
        let mut q = m.query(T0 + 1, T0 + 2);
        let a = q.next_metric();
        let b = q.next_metric();
        assert_eq!((a.anomaly_count, a.flags), (0, SN_DEFAULT_FLAGS));
        assert_eq!((b.anomaly_count, b.flags), (1, 0));
        m.change_update_every(2);
        assert_eq!(
            (m.latest_time_s(), m.oldest_time_s(), m.update_every_s()),
            (0, 0, 2)
        );
    }
}
