//! The [`WalIndex`] type — in-memory index built during Phase 1 (reading).
//!
//! Holds four data structures:
//!
//! 1. **KeyValueInterner** — assigns a [`KeyValueId`] to each unique `key=value` string.
//! 2. **Vec\<RoaringBitmap\>** — indexed by [`KeyValueId`]; each bitmap tracks which
//!    log positions contain that `key=value` pair (insertion order).
//! 3. **Vec\<Vec\<KeyValueId\>\>** — log entries: for each log position, the list
//!    of key=value IDs it contains (needed for per-stream serialization in Phase 2).
//! 4. **Vec\<i64\>** — nanosecond timestamp per log position, used to build the
//!    time-sort remap and sparse histogram.

use crate::{Histogram, IndexError};
use bumpalo::Bump;
use file_registry::ServiceStream;
use roaring::RoaringBitmap;

use super::kv_interner::{KeyValueId, KeyValueInterner};

/// The output of Phase 1: everything the frame loop extracts from the WAL.
///
/// Bundles the four data structures described in the module doc into a single
/// value, making the Phase 1 → Phase 2 handoff explicit.
pub struct WalIndex<'a> {
    pub kv_interner: KeyValueInterner<'a>,
    /// One bitmap per key=value ID. `kv_bitmaps[id.idx()]` tracks which log
    /// positions (insertion order) contain that `key=value` pair.
    pub kv_bitmaps: Vec<RoaringBitmap>,
    /// Per-log key=value IDs: `log_entries[log_pos]` lists all key=value IDs
    /// for that log's attributes. Used for per-stream serialization in Phase 2.
    pub log_entries: Vec<Vec<KeyValueId>>,
    /// Nanosecond timestamp per log position (insertion order).
    pub timestamps: Vec<i64>,
}

impl<'a> WalIndex<'a> {
    pub fn new(arena: &'a Bump, cardinality_threshold: u32) -> Self {
        Self {
            kv_interner: KeyValueInterner::new(arena, cardinality_threshold),
            kv_bitmaps: Vec::new(),
            log_entries: Vec::new(),
            timestamps: Vec::new(),
        }
    }

    /// Total number of logs processed so far.
    pub fn num_logs(&self) -> usize {
        self.log_entries.len()
    }

    /// Build a permutation table mapping insertion-order positions to
    /// time-sorted positions.
    ///
    /// All bitmaps in Phase 2 are translated from insertion order to
    /// chronological order so contiguous position ranges = contiguous time.
    pub fn time_order(&self) -> TimeOrder {
        build_time_order(&self.timestamps)
    }

    /// Build a sparse histogram from the timestamps in chronological order.
    pub fn sparse_histogram(&self, time_order: &TimeOrder) -> Histogram {
        build_sparse_histogram(&self.timestamps, time_order)
    }

    /// Resolve a key=value ID to its string.
    pub fn resolve(&self, id: KeyValueId) -> &str {
        self.kv_interner.resolve(id)
    }

    /// Get the roaring bitmap for a key=value ID.
    pub fn bitmap(&self, id: KeyValueId) -> &RoaringBitmap {
        &self.kv_bitmaps[id.idx()]
    }

    /// Low-cardinality fields (< threshold), sorted by field name.
    pub fn low_fields(&self) -> Vec<(&str, &[KeyValueId])> {
        self.kv_interner.low_fields()
    }

    /// Mid-cardinality fields ([threshold, 10*threshold)), sorted by field name.
    pub fn mid_fields(&self) -> Vec<(&str, &[KeyValueId])> {
        self.kv_interner.mid_fields()
    }

    /// High-cardinality fields (>= 10*threshold), sorted by field name.
    pub fn high_fields(&self) -> Vec<(&str, &[KeyValueId])> {
        self.kv_interner.high_fields()
    }

    /// Tier-aligned assignment of key=value IDs.
    pub fn tier_assignment(&self) -> [Vec<KeyValueId>; 3] {
        self.kv_interner.tier_assignment()
    }

    /// Cardinality threshold for field classification.
    pub fn cardinality_threshold(&self) -> u32 {
        self.kv_interner.cardinality_threshold()
    }

    /// All interned strings, ordered by KeyValueId.
    pub fn strings(&self) -> &[&str] {
        self.kv_interner.strings()
    }

    /// Ensure kv_bitmaps vec has an entry for the given key=value ID.
    pub fn ensure_bitmap(&mut self, kv_id: KeyValueId) {
        if kv_id.idx() >= self.kv_bitmaps.len() {
            self.kv_bitmaps
                .resize_with(kv_id.idx() + 1, RoaringBitmap::new);
        }
    }

    /// Extract the file's single `(service.namespace, service.name)` stream.
    ///
    /// Walks the interner once for `service.name=X` and `service.namespace=Y`
    /// entries. The ingestor partitions WAL files by `ns_hash` and rejects
    /// writes whose `(namespace, name)` doesn't match the canonical pair
    /// for that hash, so every WAL file should expose at most one of each.
    /// Missing values default to the empty string (the catch-all stream).
    ///
    /// Returns [`IndexError::MultipleStreams`] if more than one distinct
    /// value is found for either key — that means an `ns_hash` collision
    /// slipped past the ingestor's check and the file has no single
    /// stream identity to attach to the SFST.
    pub fn service_stream(&self) -> Result<ServiceStream, IndexError> {
        let mut namespaces: Vec<&str> = Vec::new();
        let mut names: Vec<&str> = Vec::new();

        for kv_pair in self.kv_interner.strings().iter() {
            if let Some(name) = kv_pair.strip_prefix("service.name=") {
                names.push(name);
            } else if let Some(namespace) = kv_pair.strip_prefix("service.namespace=") {
                namespaces.push(namespace);
            }
        }

        if namespaces.len() > 1 || names.len() > 1 {
            return Err(IndexError::MultipleStreams {
                namespaces: namespaces.into_iter().map(String::from).collect(),
                names: names.into_iter().map(String::from).collect(),
            });
        }

        Ok(ServiceStream {
            namespace: namespaces.first().copied().unwrap_or("").to_string(),
            name: names.first().copied().unwrap_or("").to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Time-sort remap and sparse histogram
// ---------------------------------------------------------------------------

/// Bidirectional mapping between insertion order and chronological order.
///
/// Built from the nanosecond timestamps collected during Phase 1. Used in
/// Phase 2 to translate all bitmaps and log entries from insertion order to
/// chronological order, so contiguous position ranges correspond to
/// contiguous time windows.
pub struct TimeOrder {
    /// `sorted_position[insertion_pos] = sorted_pos`
    sorted_position: Vec<u32>,
    /// `insertion_position[sorted_pos] = insertion_pos`
    insertion_position: Vec<u32>,
}

impl TimeOrder {
    /// Map an insertion-order position to its chronological position.
    #[inline]
    pub fn to_sorted(&self, insertion_pos: u32) -> u32 {
        self.sorted_position[insertion_pos as usize]
    }

    /// Map a chronological position back to its insertion-order position.
    #[inline]
    pub fn to_insertion(&self, sorted_pos: u32) -> u32 {
        self.insertion_position[sorted_pos as usize]
    }

    /// Iterate insertion-order positions in chronological order.
    pub fn iter_by_time(&self) -> impl Iterator<Item = u32> + '_ {
        self.insertion_position.iter().copied()
    }

    /// Total number of log positions (universe size).
    pub fn len(&self) -> u32 {
        self.sorted_position.len() as u32
    }
}

/// Build a permutation table that maps insertion-order positions to
/// time-sorted positions.
///
/// During indexing, each log gets a position based on the order it was read
/// from the WAL (insertion order). But for time-range queries we need
/// positions to correspond to chronological order, so that a contiguous
/// range of positions like `[100..200]` maps to a contiguous time window.
///
/// # Example
///
/// Suppose we indexed 5 logs with these timestamps:
///
/// ```text
///   insertion pos:  0     1     2     3     4
///   timestamp:     10:03  10:01  10:05  10:00  10:02
/// ```
///
/// After sorting by timestamp, the chronological order is:
///
/// ```text
///   sorted pos:     0      1      2      3      4
///   original pos:   3      1      4      0      2
///   timestamp:     10:00  10:01  10:02  10:03  10:05
/// ```
///
/// This gives us the remap table `remap[original] = sorted`:
///
/// ```text
///   remap[0] = 3   (10:03 is 4th chronologically)
///   remap[1] = 1   (10:01 is 2nd chronologically)
///   remap[2] = 4   (10:05 is 5th chronologically)
///   remap[3] = 0   (10:00 is 1st chronologically)
///   remap[4] = 2   (10:02 is 3rd chronologically)
/// ```
///
/// A bitmap that had bits `{0, 2}` (logs at 10:03 and 10:05 in insertion
/// order) becomes `{3, 4}` (positions 3 and 4 in chronological order —
/// the last two events).
///
fn build_time_order(timestamps: &[i64]) -> TimeOrder {
    let n = timestamps.len();

    let mut insertion_position: Vec<u32> = (0..n as u32).collect();
    insertion_position.sort_by_key(|&i| timestamps[i as usize]);

    let mut sorted_position = vec![0u32; n];
    for (sorted_pos, &original_pos) in insertion_position.iter().enumerate() {
        sorted_position[original_pos as usize] = sorted_pos as u32;
    }

    TimeOrder {
        sorted_position,
        insertion_position,
    }
}

#[cfg(test)]
mod service_stream_tests {
    use super::*;
    use bumpalo::Bump;

    fn idx<'a>(arena: &'a Bump) -> WalIndex<'a> {
        WalIndex::new(arena, 100)
    }

    #[test]
    fn returns_pair_when_one_namespace_and_one_name() {
        let arena = Bump::new();
        let mut w = idx(&arena);
        w.kv_interner.intern("service.namespace=prod");
        w.kv_interner.intern("service.name=api");
        let s = w.service_stream().unwrap();
        assert_eq!(s.namespace, "prod");
        assert_eq!(s.name, "api");
    }

    #[test]
    fn name_only_returns_empty_namespace() {
        let arena = Bump::new();
        let mut w = idx(&arena);
        w.kv_interner.intern("service.name=api");
        let s = w.service_stream().unwrap();
        assert_eq!(s.namespace, "");
        assert_eq!(s.name, "api");
    }

    #[test]
    fn namespace_only_returns_empty_name() {
        let arena = Bump::new();
        let mut w = idx(&arena);
        w.kv_interner.intern("service.namespace=prod");
        let s = w.service_stream().unwrap();
        assert_eq!(s.namespace, "prod");
        assert_eq!(s.name, "");
    }

    #[test]
    fn neither_returns_empty_pair() {
        let arena = Bump::new();
        let mut w = idx(&arena);
        // Some unrelated kv pairs in the interner — shouldn't affect the result.
        w.kv_interner.intern("host.name=foo");
        w.kv_interner.intern("k8s.pod.uid=bar");
        let s = w.service_stream().unwrap();
        assert_eq!(s.namespace, "");
        assert_eq!(s.name, "");
    }

    #[test]
    fn multiple_names_yield_error() {
        let arena = Bump::new();
        let mut w = idx(&arena);
        w.kv_interner.intern("service.namespace=prod");
        w.kv_interner.intern("service.name=api");
        w.kv_interner.intern("service.name=worker");
        let IndexError::MultipleStreams { namespaces, names } = w.service_stream().unwrap_err()
        else {
            panic!("expected MultipleStreams");
        };
        assert_eq!(namespaces, vec!["prod"]);
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"api".to_string()));
        assert!(names.contains(&"worker".to_string()));
    }

    #[test]
    fn multiple_namespaces_yield_error() {
        let arena = Bump::new();
        let mut w = idx(&arena);
        w.kv_interner.intern("service.namespace=prod");
        w.kv_interner.intern("service.namespace=staging");
        w.kv_interner.intern("service.name=api");
        let IndexError::MultipleStreams { namespaces, names } = w.service_stream().unwrap_err()
        else {
            panic!("expected MultipleStreams");
        };
        assert_eq!(names, vec!["api"]);
        assert_eq!(namespaces.len(), 2);
        assert!(namespaces.contains(&"prod".to_string()));
        assert!(namespaces.contains(&"staging".to_string()));
    }

    #[test]
    fn prefix_matching_does_not_pick_up_subkeys() {
        let arena = Bump::new();
        let mut w = idx(&arena);
        // Keys that share the prefix without the trailing `=` must not match.
        w.kv_interner.intern("service.name_extra=foo");
        w.kv_interner.intern("service.namespace_extra=bar");
        let s = w.service_stream().unwrap();
        assert_eq!(s.namespace, "");
        assert_eq!(s.name, "");
    }
}

/// Build a sparse histogram from chronologically sorted log timestamps.
///
/// Each entry records (second, running_count) — the cumulative number of
/// logs up to and including that second. One entry per second that has at
/// least one log.
fn build_sparse_histogram(timestamps: &[i64], time_order: &TimeOrder) -> Histogram {
    if timestamps.is_empty() {
        return Histogram {
            timestamps: Vec::new(),
            counts: Vec::new(),
        };
    }

    let mut hist_ts: Vec<u32> = Vec::new();
    let mut hist_counts: Vec<u32> = Vec::new();
    let mut prev_sec = u32::MAX;

    for (i, ins_pos) in time_order.iter_by_time().enumerate() {
        let sec = (timestamps[ins_pos as usize] / 1_000_000_000) as u32;
        if sec != prev_sec {
            if prev_sec != u32::MAX {
                hist_ts.push(prev_sec);
                hist_counts.push(i as u32);
            }
            prev_sec = sec;
        }
    }

    // Emit final bucket.
    hist_ts.push(prev_sec);
    hist_counts.push(timestamps.len() as u32);

    Histogram {
        timestamps: hist_ts,
        counts: hist_counts,
    }
}
