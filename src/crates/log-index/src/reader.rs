//! Reader for the split-FST index format.
//!
//! Opens an `.sfst` file (typically via mmap) and provides query methods
//! that follow the access pattern described in `sfst/FORMAT.md`:
//!
//! 1. Decode SUMR + META + PRIM eagerly on open (always needed).
//! 2. Look up low-card `key=value` pairs in the primary FST → bitmap.
//! 3. Load secondary chunks on demand (mid-card FST or high-card blob).
//! 4. Load per-stream log entries for attribute resolution.

use fst_index::FstIndex;
use sfst::{
    BitmapValue, FieldEntry, FieldTier, Summary, IdRanges, Metadata, KvId, Histogram,
    ServiceStream,
};

/// A successfully opened split-FST index.
///
/// Holds the mmap'd data, the deserialized summary, and the primary
/// FST (both eagerly loaded on open since every query needs them).
/// [`Metadata`] is cached on the underlying [`sfst::Reader`] and
/// surfaced via [`metadata`](Self::metadata).
pub struct IndexReader<'a> {
    sfst: sfst::Reader<'a>,
    summary: Summary,
    primary: FstIndex<BitmapValue>,
}

impl<'a> IndexReader<'a> {
    /// Open a split-FST index from a byte slice (typically an mmap).
    ///
    /// Immediately deserializes the summary, metadata, and primary FST.
    /// Metadata stays cached on the underlying [`sfst::Reader`].
    pub fn open(data: &'a [u8]) -> Result<Self, sfst::Error> {
        let sfst = sfst::Reader::open(data)?;
        let summary = sfst.summary()?;
        // Force the metadata cache so subsequent accessors are infallible.
        sfst.metadata()?;
        let primary = sfst.primary()?;
        Ok(Self {
            sfst,
            summary,
            primary,
        })
    }

    /// The cheap summary fields (timestamps, total logs, stream).
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// The heavy index metadata (histogram + id_ranges + field table).
    pub fn metadata(&self) -> &Metadata {
        self.sfst
            .metadata()
            .expect("metadata cached at IndexReader::open")
    }

    /// Total number of log entries in this index.
    pub fn total_logs(&self) -> u32 {
        self.summary.total_logs
    }

    /// The ID ranges for the three cardinality tiers.
    pub fn id_ranges(&self) -> &IdRanges {
        &self.metadata().id_ranges
    }

    /// The sparse histogram for time-range estimation.
    pub fn histogram(&self) -> &Histogram {
        &self.metadata().histogram
    }

    /// The file's single stream.
    pub fn stream(&self) -> &ServiceStream {
        &self.summary.stream
    }

    // ── Field table ─────────────────────────────────────────────────

    /// The field table (carried inside [`Metadata`]).
    pub fn field_table(&self) -> &[FieldEntry] {
        &self.metadata().fields
    }

    // ── Primary FST lookups ─────────────────────────────────────────

    /// Look up a low-card `key=value` pair in the primary FST.
    pub fn primary_lookup(&self, key_value: &[u8]) -> Option<&BitmapValue> {
        self.primary.get(key_value)
    }

    /// Iterate over all entries in the primary FST.
    pub fn primary_for_each(&self, f: impl FnMut(&[u8], &BitmapValue)) {
        self.primary.for_each(f);
    }

    /// Prefix search on the primary FST.
    pub fn primary_prefix(&self, prefix: &[u8]) -> Vec<(Vec<u8>, &BitmapValue)> {
        self.primary.prefix_pairs(prefix)
    }

    // ── Secondary chunk loading ─────────────────────────────────────

    /// Load a mid-cardinality field's FST. `mid_index` is `0..num_mid`.
    pub fn load_mid_field(&self, mid_index: u16) -> Result<FstIndex<BitmapValue>, sfst::Error> {
        self.sfst.mid_field(mid_index)
    }

    /// Load a high-cardinality field's entries. `high_index` is `0..num_high`.
    ///
    /// Returns the decompressed list of `(key_value, bitmap)` pairs.
    pub fn load_high_field(
        &self,
        high_index: u16,
    ) -> Result<Vec<(String, BitmapValue)>, sfst::Error> {
        self.sfst.high_field(high_index)
    }

    // ── Per-log timestamps ──────────────────────────────────────────

    /// Load the per-log nanosecond timestamps, chronologically ordered
    /// and parallel-indexed to [`load_stream_entries`](Self::load_stream_entries).
    pub fn load_timestamps(&self) -> Result<Vec<i64>, sfst::Error> {
        self.sfst.timestamps()
    }

    // ── Stream log entries ──────────────────────────────────────────

    /// Load the file's stream log entries.
    ///
    /// Each SFST has exactly one stream (see [`sfst::ServiceStream`]); its
    /// log entries chunk is the trailing secondary chunk.
    pub fn load_stream_entries(&self) -> Result<Vec<Vec<KvId>>, sfst::Error> {
        self.sfst.stream_entries()
    }

    // ── KvId resolution ───────────────────────────────────────────

    /// Determine which cardinality tier a [`KvId`] belongs to.
    pub fn kv_id_tier(&self, id: KvId) -> FieldTier {
        let ranges = self.id_ranges();
        if id.0 < ranges.low_end.0 {
            FieldTier::Low
        } else if id.0 < ranges.mid_end.0 {
            FieldTier::Mid
        } else {
            FieldTier::High
        }
    }

    /// Build a reverse lookup table: `KvId → key=value` string.
    ///
    /// Walks the primary FST and every secondary chunk, decompressing as
    /// it goes. Returns one entry per `key=value` pair in the file.
    pub fn build_string_table(
        &self,
        field_table: &[FieldEntry],
    ) -> Result<Vec<String>, sfst::Error> {
        let total = self.metadata().id_ranges.high_end.0 as usize;
        let mut table = vec![String::new(); total];
        let mut kv_id = 0usize;

        // Low-card: iterate primary FST.
        self.primary.for_each(|key, _| {
            if kv_id < table.len() {
                table[kv_id] = String::from_utf8_lossy(key).into_owned();
            }
            kv_id += 1;
        });

        // Mid/high-card: iterate secondary chunks in field_table order,
        // tracking mid- and high-relative positions independently.
        let mut mid_index: u16 = 0;
        let mut high_index: u16 = 0;
        for field in field_table {
            match field.tier {
                FieldTier::Low => continue,
                FieldTier::Mid => {
                    let fst = self.sfst.mid_field(mid_index)?;
                    fst.for_each(|key, _| {
                        if kv_id < table.len() {
                            table[kv_id] = String::from_utf8_lossy(key).into_owned();
                        }
                        kv_id += 1;
                    });
                    mid_index += 1;
                }
                FieldTier::High => {
                    let entries = self.sfst.high_field(high_index)?;
                    for (key, _) in entries {
                        if kv_id < table.len() {
                            table[kv_id] = key;
                        }
                        kv_id += 1;
                    }
                    high_index += 1;
                }
            }
        }

        Ok(table)
    }
}
