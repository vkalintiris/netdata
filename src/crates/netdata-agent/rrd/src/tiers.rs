//! The tiers above 0, ported from `src/database/rrddim-collection.c` (`store_metric_at_tier()` and its parked point)
//! and the per-tier part of `struct rrddim_tier`: each tier aggregates the collected points into windows of
//! `update every × grouping` seconds. A completed window is parked and written later: at a second that is a multiple
//! of the tier's flush modulo, when the next window completes, or when the collection ends. Decisions D72.

use netdata_agent_storage::storage_number::SN_FLAG_NOT_ANOMALOUS;
use netdata_agent_storage::storage_point::StoragePoint;

/// `rrdset_collection_modulo_init()`'s range: the chart counter wraps below it.
pub const COLLECTION_MODULO_RANGE: usize = 65535;

/// `rrddim_collection_modulo()`: the tier's flush modulo, `1 + chart modulo % spread`, the spread (the tier's update
/// every) taken as 65,535 when 0 and capped there.
pub fn flush_modulo(collection_modulo: u16, spread: u32) -> u16 {
    let spread = if spread == 0 {
        COLLECTION_MODULO_RANGE as u32
    } else {
        spread.min(COLLECTION_MODULO_RANGE as u32)
    };
    1 + (u32::from(collection_modulo) % spread) as u16
}

/// `rrddim_store_metric()`'s tier point: the collected value covering the chart's last update every.
pub fn collected_point(
    point_end_time_ut: u64,
    value: f64,
    flags: u32,
    chart_update_every: i64,
) -> StoragePoint {
    let now_s = (point_end_time_ut / 1_000_000) as i64;
    StoragePoint {
        start_time_s: now_s - chart_update_every,
        end_time_s: now_s,
        min: value,
        max: value,
        sum: value,
        count: 1,
        anomaly_count: u32::from(flags & SN_FLAG_NOT_ANOMALOUS == 0),
        flags,
    }
}

/// A tier record to write: what `storage_engine_store_metric()` takes, its counts narrowed to 16 bits as C's call
/// does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierRecord {
    pub end_time_s: i64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    pub count: u16,
    pub anomaly_count: u16,
    pub flags: u32,
}

impl TierRecord {
    /// The record of a parked window: its values, or an empty record (count 0) for a window without any.
    fn of(point: &StoragePoint) -> Self {
        if point.is_unset() {
            TierRecord {
                end_time_s: point.end_time_s,
                sum: f64::NAN,
                min: f64::NAN,
                max: f64::NAN,
                count: 0,
                anomaly_count: 0,
                flags: 0,
            }
        } else {
            TierRecord {
                end_time_s: point.end_time_s,
                sum: point.sum,
                min: point.min,
                max: point.max,
                count: point.count as u16,
                anomaly_count: point.anomaly_count as u16,
                flags: point.flags,
            }
        }
    }
}

/// `struct rrddim_tier`'s aggregation state for one tier of one dimension.
#[derive(Debug, Clone)]
pub struct Rollup {
    /// `tier_grouping`.
    grouping: i64,
    /// `last_completed_point_flush_modulo`.
    flush_modulo: u16,
    /// `virtual_point`: the window being filled.
    virtual_point: StoragePoint,
    /// `last_completed_point`: a completed window waiting to be written; its end time is 0 when there is none.
    last_completed: StoragePoint,
    /// `next_point_end_time_s`: where the window being filled ends, 0 before the first point.
    next_point_end_time_s: i64,
}

impl Rollup {
    pub fn new(grouping: u64, flush_modulo: u16) -> Self {
        Rollup {
            grouping: grouping as i64,
            flush_modulo,
            virtual_point: StoragePoint::UNSET,
            last_completed: StoragePoint::UNSET,
            next_point_end_time_s: 0,
        }
    }

    /// `tier_next_point_time_s()`: the next multiple of the window after `now_s`.
    fn next_point_time_s(&self, chart_update_every: i64, now_s: i64) -> i64 {
        let window = chart_update_every * self.grouping;
        now_s + window - (now_s + window) % window
    }

    /// `store_metric_at_tier_flush_last_completed()`: the parked window's record, if one is parked.
    pub fn flush(&mut self) -> Option<TierRecord> {
        if self.last_completed.end_time_s == 0 {
            return None;
        }
        let record = TierRecord::of(&self.last_completed);
        self.last_completed.count = 0;
        self.last_completed.end_time_s = 0;
        Some(record)
    }

    /// `store_metric_at_tier_save_last_completed()`: a parked window is written first; the new one ends at the
    /// boundary it completed at.
    fn park(&mut self, point: StoragePoint) -> Option<TierRecord> {
        let written = self.flush();
        self.last_completed = point;
        self.last_completed.end_time_s = self.next_point_end_time_s;
        written
    }

    /// `store_metric_at_tier()`: the point joins the window being filled, after the parked window is written at its
    /// modulo and a completed window is parked. Returns the record to write, if any (at most one per point). Gaps
    /// (a sum that is not finite) only move the window's dates.
    pub fn store(&mut self, chart_update_every: i64, point: StoragePoint) -> Option<TierRecord> {
        let mut written = None;
        if self.last_completed.end_time_s != 0
            && point.start_time_s % i64::from(self.flush_modulo) == 0
        {
            written = self.flush();
        }
        if self.next_point_end_time_s == 0 {
            self.next_point_end_time_s =
                self.next_point_time_s(chart_update_every, point.end_time_s);
        }
        if point.start_time_s >= self.next_point_end_time_s {
            let completed = if self.virtual_point.is_unset() {
                StoragePoint::UNSET
            } else {
                self.virtual_point
            };
            if let Some(record) = self.park(completed) {
                written = Some(record);
            }
            self.virtual_point.count = 0;
            self.next_point_end_time_s =
                self.next_point_time_s(chart_update_every, point.end_time_s);
        }
        let v = &mut self.virtual_point;
        if point.start_time_s < v.start_time_s {
            v.start_time_s = point.start_time_s;
        }
        if point.end_time_s > v.end_time_s {
            v.end_time_s = point.end_time_s;
        }
        if !point.is_gap() {
            if v.is_unset() {
                *v = point;
            } else {
                v.sum += point.sum;
                // C's MIN()/MAX() ternaries
                v.min = if v.min < point.min { v.min } else { point.min };
                v.max = if v.max > point.max { v.max } else { point.max };
                v.count = v.count.wrapping_add(point.count);
                v.anomaly_count = v.anomaly_count.wrapping_add(point.anomaly_count);
                v.flags |= point.flags;
            }
        }
        written
    }
}

#[cfg(test)]
mod tests;
