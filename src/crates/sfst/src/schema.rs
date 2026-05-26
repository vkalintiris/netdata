//! On-disk schema for SFST log indexes.
//!
//! These are the typed payloads carried by an SFST file's named chunks.
//! Producers (the [`crate::indexer`] module) construct them; consumers
//! decode them via the typed accessors on [`crate::Reader`]. The
//! container layout and chunk encoding are specified in `FORMAT.md`.

use file_registry::ServiceStream;
use serde::{Deserialize, Serialize};
use treight::Bitmap;

// ── SUMR ─────────────────────────────────────────────────────────

/// Cheap-to-read summary of an SFST file (the `SUMR` chunk payload).
///
/// Stored in its own chunk so a registry can rebuild itself from the
/// file without decompressing the heavier `META` chunk (histogram +
/// id_ranges).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub min_timestamp_s: u32,
    pub max_timestamp_s: u32,
    pub total_logs: u32,
    pub stream: ServiceStream,
}

// ── META ─────────────────────────────────────────────────────────

/// Heavy query-time metadata (the `META` chunk payload).
///
/// Holds the data a reader needs to bootstrap any query against the
/// file: the sparse timestamp histogram, the cardinality-tier id
/// ranges, and the field table. Readers that only need the cheap
/// summary fields (min/max timestamp, total log count, stream) should
/// decode [`Summary`] from the `SUMR` chunk instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub histogram: Histogram,
    pub id_ranges: IdRanges,
    pub fields: Vec<FieldEntry>,
}

/// Sparse timestamp histogram: one entry per second that has at least
/// one log record, paired with the cumulative log count up to and
/// including that second. Built from chronologically-sorted log
/// timestamps during indexing; used at query time for time-range
/// narrowing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Histogram {
    /// Second-boundary timestamps as u32 seconds since Unix epoch.
    pub timestamps: Vec<u32>,
    /// Cumulative log count at each second boundary.
    pub counts: Vec<u32>,
}

/// Contiguous [`KvId`] ranges for the three cardinality tiers.
///
/// Ids are assigned sequentially: `0..low_end` for low-card,
/// `low_end..mid_end` for mid-card, `mid_end..high_end` for high-card.
/// The reader uses these ranges to decide which section (primary FST,
/// mid-card FST, or high-card sorted list) to consult for a given id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdRanges {
    pub low_end: KvId,
    pub mid_end: KvId,
    pub high_end: KvId,
}

// ── Field table (carried inside META) ────────────────────────────

/// One entry in the field table carried by [`Metadata::fields`].
///
/// The table is ordered low → mid → high, with each tier internally
/// sorted by field name. Readers walk it to count mid-card and
/// high-card fields, to look up a field's tier when resolving a
/// [`KvId`], and to discover which secondary chunks the file carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldEntry {
    pub name: String,
    pub cardinality: u32,
    pub tier: FieldTier,
}

/// Cardinality tier for a field. The cardinality threshold `T` and
/// its 10× cutoff (set by the producer; default 100) define the
/// boundaries: `< T` is low, `[T, 10·T)` is mid, `≥ 10·T` is high.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldTier {
    Low,
    Mid,
    High,
}

// ── PRIM / secondary chunks ──────────────────────────────────────

/// Value type for FST entries in the primary chunk and mid-card field
/// chunks, and for the pairs inside high-card field chunks.
///
/// Carries a [`treight::Bitmap`] over time-sorted log positions where
/// the `key=value` pair appears. `desc` is the bitmap metadata; `data`
/// holds the encoded payload bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitmapValue {
    pub desc: Bitmap,
    pub data: Vec<u8>,
}

// ── Stream-log-entries chunk ─────────────────────────────────────

/// Tier-aligned identifier for a `key=value` pair within one SFST.
///
/// Assigned during writing in FST iteration order across the three
/// cardinality tiers; the stream-log-entries chunk stores sequences of
/// these instead of duplicating the strings.
///
/// Not to be confused with [`file_registry::FileId`] (which identifies
/// a file on disk) or with the build-time `KeyValueId` (which is in
/// insertion order, not iteration order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvId(pub u32);

impl KvId {
    #[inline]
    pub fn idx(self) -> usize {
        self.0 as usize
    }
}
