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
use roaring::RoaringBitmap;

use crate::{
    BitmapValue, FacetResult, FieldEntry, FieldTier, Filter, Grid, HighField, Histogram, IdRanges,
    KvId, Metadata, ServiceStream, Summary, Timeline, bitmap_value_to_roaring,
};

/// A successfully opened split-FST index.
///
/// Holds the mmap'd data, the deserialized summary, and the primary
/// FST (both eagerly loaded on open since every query needs them).
/// [`Metadata`] is cached on the underlying [`crate::Reader`] and
/// surfaced via [`metadata`](Self::metadata).
pub struct IndexReader<'a> {
    sfst: crate::Reader<'a>,
    summary: Summary,
    primary: FstIndex<BitmapValue>,
}

impl<'a> IndexReader<'a> {
    /// Open a split-FST index from a byte slice (typically an mmap).
    ///
    /// Immediately deserializes the summary, metadata, and primary FST.
    /// Metadata stays cached on the underlying [`crate::Reader`].
    pub fn open(data: &'a [u8]) -> Result<Self, crate::Error> {
        let sfst = crate::Reader::open(data)?;
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
    pub fn load_mid_field(&self, mid_index: u16) -> Result<FstIndex<BitmapValue>, crate::Error> {
        self.sfst.mid_field(mid_index)
    }

    /// Load a high-cardinality field's columnar entries. `high_index`
    /// is `0..num_high`.
    ///
    /// Returns the decompressed [`HighField`] for that field — parallel
    /// `keys` (sorted lexicographically) and `masks` vectors. Each
    /// `masks[j]` is a `u8` bitmask over the file's stream batches; bit
    /// `b` set iff `keys[j]` appears in batch `b`. Walk the set bits to
    /// decide which [`load_stream_batch`](Self::load_stream_batch)
    /// calls to make when resolving positions for the value.
    pub fn load_high_field(&self, high_index: u16) -> Result<HighField, crate::Error> {
        self.sfst.high_field(high_index)
    }

    // ── Per-log timestamps ──────────────────────────────────────────

    /// Load the per-log nanosecond timestamps, chronologically ordered
    /// and parallel-indexed to the concatenation of the stream-batch
    /// chunks (see [`load_all_stream_entries`](Self::load_all_stream_entries)).
    pub fn load_timestamps(&self) -> Result<Vec<i64>, crate::Error> {
        self.sfst.timestamps()
    }

    // ── Stream-batch chunks ─────────────────────────────────────────

    /// Number of stream-batch chunks in this file. Derived from
    /// `summary.total_logs` via [`crate::num_stream_batches`].
    pub fn num_stream_batches(&self) -> u8 {
        crate::num_stream_batches(self.summary.total_logs)
    }

    /// Load one stream-batch chunk by index (`0..num_stream_batches`).
    ///
    /// Returns the attribute lists for the logs in that batch, in
    /// chronological order. Concatenating batches in order yields the
    /// full chronological log stream.
    pub fn load_stream_batch(&self, batch_index: u8) -> Result<Vec<Vec<KvId>>, crate::Error> {
        self.sfst.stream_batch(batch_index)
    }

    /// Load and concatenate every stream-batch chunk in chronological
    /// order. Convenience for tooling and tests that want the full log
    /// stream rather than walking batches individually.
    pub fn load_all_stream_entries(&self) -> Result<Vec<Vec<KvId>>, crate::Error> {
        let n = self.num_stream_batches();
        let mut out = Vec::with_capacity(self.summary.total_logs as usize);
        for i in 0..n {
            out.extend(self.sfst.stream_batch(i)?);
        }
        Ok(out)
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
    ) -> Result<Vec<String>, crate::Error> {
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
                    let hf = self.sfst.high_field(high_index)?;
                    for key in hf.keys {
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

    // ── Query API ────────────────────────────────────────────────────

    /// Apply a [`Filter`] (OR within field, AND across fields) and return
    /// the position bitmap of matching logs.
    ///
    /// An empty filter returns the full position range `0..total_logs`.
    /// Fields mentioned in the filter that don't exist in this file
    /// contribute an empty set (no logs match), which collapses the
    /// overall result to empty — single-file SFSTs with disjoint field
    /// sets fall out of the query naturally.
    pub fn evaluate(&self, filter: &Filter) -> Result<RoaringBitmap, crate::Error> {
        if filter.is_empty() {
            let mut all = RoaringBitmap::new();
            all.insert_range(0..self.summary.total_logs);
            return Ok(all);
        }

        let mut result: Option<RoaringBitmap> = None;
        for (field, values) in &filter.selections {
            let field_bm = self.field_values_or(field, values)?;
            result = Some(match result {
                None => field_bm,
                Some(mut prev) => {
                    prev &= &field_bm;
                    prev
                }
            });
            if result.as_ref().is_some_and(|b| b.is_empty()) {
                return Ok(RoaringBitmap::new());
            }
        }
        Ok(result.unwrap_or_default())
    }

    /// Bitmap of log positions whose timestamp falls in `window_ns`
    /// (`[start, end)`). Built from the file's chronological
    /// timestamps via `partition_point`, so it clamps naturally when
    /// the window extends past the file's range. Shared by the
    /// windowed query paths ([`facets`](Self::facets) and the
    /// handler's matched-count) so they all clip to the same set.
    pub fn range_bitmap(&self, window_ns: std::ops::Range<i64>) -> Result<RoaringBitmap, crate::Error> {
        let timestamps = self.sfst.timestamps()?;
        let lo = timestamps.partition_point(|&t| t < window_ns.start) as u32;
        let hi = timestamps.partition_point(|&t| t < window_ns.end) as u32;
        let mut bm = RoaringBitmap::new();
        if lo < hi {
            bm.insert_range(lo..hi);
        }
        Ok(bm)
    }

    /// Compute per-field value counts for the UI's facet sidebar.
    ///
    /// For each facet field, the filter is evaluated **with that field's
    /// own selections removed** — so selecting `level=error` doesn't
    /// reduce the `level` facet to a single bar. Facets not present in
    /// the filter share a single evaluation of the full filter.
    ///
    /// Counts are restricted to `window_ns` (`[start, end)`), the same
    /// request window the histogram and matched-count use — so the
    /// sidebar reflects only the logs in view, not the whole file.
    ///
    /// Returns [`crate::Error::UnknownField`] for fields not in this
    /// file, or [`crate::Error::HighCardFacet`] for high-cardinality
    /// facets (where exact counts would require scanning stream batches).
    ///
    /// Note the deliberate asymmetry with
    /// [`timeline`](Self::timeline): `facets` *errors* on an absent
    /// field (a facet is requested per-field, so absence is a caller
    /// mistake), whereas `timeline` routes an absent field's logs to
    /// `unset`. Callers querying a heterogeneous set of files should
    /// pre-filter the field list to those present in each file.
    pub fn facets<S: AsRef<str>>(
        &self,
        fields: &[S],
        filter: &Filter,
        window_ns: std::ops::Range<i64>,
    ) -> Result<Vec<FacetResult>, crate::Error> {
        // Window mask applied to every counting bitmap so facet totals
        // match the in-window histogram/matched counts.
        let range_bm = self.range_bitmap(window_ns)?;

        // Computed once; reused by every facet whose field is not in
        // the filter's selections.
        let full_bm = self.evaluate(filter)? & &range_bm;

        let mut results = Vec::with_capacity(fields.len());
        for field in fields {
            let field = field.as_ref();
            let scoped = if filter.has_field(field) {
                Some(self.evaluate(&filter.without(field))? & &range_bm)
            } else {
                None
            };
            let bm: &RoaringBitmap = scoped.as_ref().unwrap_or(&full_bm);
            let values = self.value_counts_under(field, bm)?;
            results.push(FacetResult {
                field: field.to_string(),
                values,
            });
        }
        Ok(results)
    }

    /// Compute a 2D time × value-of-`field` count grid for chart rendering.
    ///
    /// `grid` is caller-supplied. A grid that extends past the file's
    /// actual log range produces zero counts in the outer buckets
    /// (handled naturally by `partition_point`). `field`'s own
    /// selections are excluded from the filter (same reason as in
    /// [`facets`](Self::facets)).
    ///
    /// A field that isn't present in this file is treated as "every
    /// matching log lacks it": the result has no dimensions and all
    /// matching logs land in `unset`. This keeps the histogram total
    /// equal to the matched count in a multi-file query where some
    /// files carry the field and others don't.
    ///
    /// Errors:
    /// - [`crate::Error::InvalidBucketWidth`] if `grid.bucket_width_ns <= 0`.
    /// - [`crate::Error::HighCardFacet`] if `field` is high-cardinality.
    pub fn timeline(
        &self,
        field: &str,
        filter: &Filter,
        grid: Grid,
    ) -> Result<Timeline, crate::Error> {
        if grid.bucket_width_ns <= 0 {
            return Err(crate::Error::InvalidBucketWidth(grid.bucket_width_ns));
        }

        // Filter with `field`'s own selections removed.
        let excluded = filter.without(field);
        let filter_bm = self.evaluate(&excluded)?;

        // Enumerate the field's values and pre-intersect each with the
        // filter bitmap. Each dimension's bitmap is then queried per
        // bucket via range_cardinality.
        let prefix = format!("{field}=");
        let prefix_len = prefix.len();
        let mut dimensions: Vec<String> = Vec::new();
        let mut intersections: Vec<RoaringBitmap> = Vec::new();

        // An absent field leaves `dimensions`/`intersections` empty, so
        // the bucket loop below routes every matching log into `unset`
        // (dim_sum == 0 ⇒ unset == bucket_total).
        match self.locate_field(field) {
            None => {}
            Some(FieldLocation::Low) => {
                for (kv_bytes, bv) in self.primary.prefix_pairs(prefix.as_bytes()) {
                    let value = String::from_utf8_lossy(&kv_bytes[prefix_len..]).into_owned();
                    let mut value_bm = bitmap_value_to_roaring(bv);
                    value_bm &= &filter_bm;
                    dimensions.push(value);
                    intersections.push(value_bm);
                }
            }
            Some(FieldLocation::Mid(idx)) => {
                let chunk = self.sfst.mid_field(idx)?;
                chunk.for_each(|kv_bytes, bv| {
                    let value = String::from_utf8_lossy(&kv_bytes[prefix_len..]).into_owned();
                    let mut value_bm = bitmap_value_to_roaring(bv);
                    value_bm &= &filter_bm;
                    dimensions.push(value);
                    intersections.push(value_bm);
                });
            }
            Some(FieldLocation::High(_)) => {
                return Err(crate::Error::HighCardFacet(field.to_string()));
            }
        }

        let timestamps = self.sfst.timestamps()?;
        let mut buckets = vec![vec![0u64; dimensions.len()]; grid.num_buckets];
        let mut unset = vec![0u64; grid.num_buckets];
        for bucket_i in 0..grid.num_buckets {
            let bucket_start = grid.bucket_start_ns + (bucket_i as i64) * grid.bucket_width_ns;
            let bucket_end = bucket_start + grid.bucket_width_ns;
            // `partition_point` clamps naturally: positions before
            // bucket_start_ns yield pos_lo=0, after the file's last
            // timestamp yield pos_hi=timestamps.len(). Outer buckets of
            // a request grid that extends past the file's range pick up
            // no positions and contribute zero counts.
            let pos_lo = timestamps.partition_point(|&t| t < bucket_start) as u32;
            let pos_hi = timestamps.partition_point(|&t| t < bucket_end) as u32;
            let mut dim_sum: u64 = 0;
            for (dim_i, intersection) in intersections.iter().enumerate() {
                let c = intersection.range_cardinality(pos_lo..pos_hi);
                buckets[bucket_i][dim_i] = c;
                dim_sum += c;
            }
            // Logs matching the filter that don't have this field set.
            // OTel attribute keys are unique per LogRecord, so every
            // matching log lands in exactly one of `dimensions` or
            // `unset` — the subtraction is exact.
            let bucket_total = filter_bm.range_cardinality(pos_lo..pos_hi);
            unset[bucket_i] = bucket_total.saturating_sub(dim_sum);
        }

        Ok(Timeline {
            grid,
            dimensions,
            buckets,
            unset,
        })
    }

    // ── Query helpers (private) ──────────────────────────────────────

    /// Locate a field by name and return its tier + tier-relative chunk
    /// index. Returns `None` if the field is absent from this file.
    fn locate_field(&self, field_name: &str) -> Option<FieldLocation> {
        let mut mid_idx = 0u16;
        let mut high_idx = 0u16;
        for f in &self.metadata().fields {
            if f.name == field_name {
                return Some(match f.tier {
                    FieldTier::Low => FieldLocation::Low,
                    FieldTier::Mid => FieldLocation::Mid(mid_idx),
                    FieldTier::High => FieldLocation::High(high_idx),
                });
            }
            match f.tier {
                FieldTier::Mid => mid_idx += 1,
                FieldTier::High => high_idx += 1,
                _ => {}
            }
        }
        None
    }

    /// Compute the on-disk `KvId` for the `local`-th value of high-card
    /// chunk `high_idx`. High-card KvIds are `mid_end + (cumulative
    /// high-card cardinalities before this field) + local`.
    fn high_kv_id(&self, high_idx: u16, local: usize) -> KvId {
        let id_ranges = &self.metadata().id_ranges;
        let mut kv = id_ranges.mid_end.0;
        let mut current = 0u16;
        for f in &self.metadata().fields {
            if let FieldTier::High = f.tier {
                if current == high_idx {
                    return KvId(kv + local as u32);
                }
                kv += f.cardinality;
                current += 1;
            }
        }
        panic!("high_kv_id: high_idx {high_idx} out of range");
    }

    /// Position bitmap matching `field=v` for any `v` in `values` (OR
    /// within field). Returns an empty bitmap if the field is absent
    /// from this file.
    fn field_values_or(
        &self,
        field: &str,
        values: &[String],
    ) -> Result<RoaringBitmap, crate::Error> {
        let location = match self.locate_field(field) {
            Some(loc) => loc,
            None => return Ok(RoaringBitmap::new()),
        };
        let mut result = RoaringBitmap::new();

        match location {
            FieldLocation::Low => {
                for value in values {
                    let kv = format!("{field}={value}");
                    if let Some(bv) = self.primary.get(kv.as_bytes()) {
                        result |= bitmap_value_to_roaring(bv);
                    }
                }
            }
            FieldLocation::Mid(idx) => {
                let chunk = self.sfst.mid_field(idx)?;
                for value in values {
                    let kv = format!("{field}={value}");
                    if let Some(bv) = chunk.get(kv.as_bytes()) {
                        result |= bitmap_value_to_roaring(bv);
                    }
                }
            }
            FieldLocation::High(idx) => {
                // High-card values are addressed by KvId; the filter
                // bitmap is built by scanning the SB batches indicated
                // by the union of the values' batch masks.
                let hf = self.sfst.high_field(idx)?;
                let mut targets: Vec<KvId> = Vec::new();
                let mut combined_mask: u8 = 0;
                for value in values {
                    let kv = format!("{field}={value}");
                    if let Ok(local) = hf.keys.binary_search_by(|k| k.as_str().cmp(&kv)) {
                        targets.push(self.high_kv_id(idx, local));
                        combined_mask |= hf.masks[local];
                    }
                }
                if targets.is_empty() {
                    return Ok(result);
                }
                let total_logs = self.summary.total_logs;
                let batch_size = crate::stream_batch_size(total_logs);
                let num_batches = crate::num_stream_batches(total_logs);
                for b in 0..num_batches {
                    if (combined_mask >> b) & 1 == 0 {
                        continue;
                    }
                    let batch_start = u32::from(b) * batch_size;
                    let batch = self.sfst.stream_batch(b)?;
                    for (i, kv_ids) in batch.iter().enumerate() {
                        if kv_ids.iter().any(|id| targets.contains(id)) {
                            result.insert(batch_start + i as u32);
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Per-value `(value, count)` pairs for `field` restricted to
    /// `filter_bm`. Walks the field's chunk once. Errors with
    /// [`crate::Error::UnknownField`] / [`crate::Error::HighCardFacet`]
    /// as appropriate.
    fn value_counts_under(
        &self,
        field: &str,
        filter_bm: &RoaringBitmap,
    ) -> Result<Vec<(String, u32)>, crate::Error> {
        let location = self
            .locate_field(field)
            .ok_or_else(|| crate::Error::UnknownField(field.to_string()))?;
        let prefix = format!("{field}=");
        let prefix_len = prefix.len();
        let mut results = Vec::new();

        match location {
            FieldLocation::Low => {
                for (kv_bytes, bv) in self.primary.prefix_pairs(prefix.as_bytes()) {
                    let value = String::from_utf8_lossy(&kv_bytes[prefix_len..]).into_owned();
                    let mut bm = bitmap_value_to_roaring(bv);
                    bm &= filter_bm;
                    let count = bm.len() as u32;
                    if count > 0 {
                        results.push((value, count));
                    }
                }
            }
            FieldLocation::Mid(idx) => {
                let chunk = self.sfst.mid_field(idx)?;
                chunk.for_each(|kv_bytes, bv| {
                    let value = String::from_utf8_lossy(&kv_bytes[prefix_len..]).into_owned();
                    let mut bm = bitmap_value_to_roaring(bv);
                    bm &= filter_bm;
                    let count = bm.len() as u32;
                    if count > 0 {
                        results.push((value, count));
                    }
                });
            }
            FieldLocation::High(_) => {
                return Err(crate::Error::HighCardFacet(field.to_string()));
            }
        }

        Ok(results)
    }
}

/// Tier + tier-relative chunk index for a single field. Private; used by
/// the query helpers ([`IndexReader::evaluate`], [`IndexReader::facets`],
/// [`IndexReader::timeline`]).
enum FieldLocation {
    Low,
    Mid(u16),
    High(u16),
}
