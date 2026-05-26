//! Single-file log query over an SFST log index.
//!
//! Layers on top of [`sfst::IndexReader`]'s query API
//! ([`evaluate`](sfst::IndexReader::evaluate)) to add anchor/direction
//! iteration, time-range narrowing, and per-position materialisation
//! (`KvId → "key=value"` resolution).
//!
//! [`Filter`] is re-exported from `sfst` so callers have a single filter
//! type across the API.
//!
//! # Example
//!
//! ```no_run
//! use sfsq::{Anchor, Direction, Filter, LogQuery, LogQueryParamsBuilder};
//!
//! let data = std::fs::read("path/to/file.sfst").unwrap();
//! let reader = sfst::IndexReader::open(&data).unwrap();
//!
//! let params = LogQueryParamsBuilder::new(Anchor::Latest, Direction::Backward)
//!     .with_limit(200)
//!     .with_filter(Filter::new().select("service.name", "certstream"))
//!     .build()
//!     .unwrap();
//!
//! let mut query = LogQuery::new(&reader, params);
//! let logs = query.run().unwrap();
//! ```

use std::cell::OnceCell;
use std::collections::{HashMap, VecDeque};

use roaring::RoaringBitmap;
use sfst::{IndexReader, KvId};
use thiserror::Error;

pub use sfst::Filter;

// ── Public types ─────────────────────────────────────────────────────────

/// Where in the chronological stream the query starts.
#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    /// The most recent log (position = `total_logs - 1`).
    Latest,
    /// The earliest log (position = 0).
    Earliest,
    /// Nanosecond timestamp. Snaps to the closest position consistent
    /// with the query's [`Direction`]: in `Forward` mode, the first
    /// position with `ts >= timestamp`; in `Backward`, the last with
    /// `ts <= timestamp`.
    At(i64),
}

/// Iteration direction for the position stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Ascending positions (oldest to newest).
    Forward,
    /// Descending positions (newest to oldest).
    Backward,
}

/// Validated immutable query parameters. Build via [`LogQueryParamsBuilder`].
#[derive(Debug, Clone)]
pub struct LogQueryParams {
    anchor: Anchor,
    direction: Direction,
    limit: Option<usize>,
    filter: Filter,
    after: Option<i64>,
    before: Option<i64>,
    resume_position: Option<u32>,
}

impl LogQueryParams {
    pub fn anchor(&self) -> Anchor {
        self.anchor
    }
    pub fn direction(&self) -> Direction {
        self.direction
    }
    pub fn limit(&self) -> Option<usize> {
        self.limit
    }
    pub fn filter(&self) -> &Filter {
        &self.filter
    }
    pub fn after(&self) -> Option<i64> {
        self.after
    }
    pub fn before(&self) -> Option<i64> {
        self.before
    }
    pub fn resume_position(&self) -> Option<u32> {
        self.resume_position
    }
}

/// Fluent builder for [`LogQueryParams`]. Required (`anchor`, `direction`)
/// at construction; optional via setters; validation runs in
/// [`build`](Self::build).
#[derive(Debug, Clone)]
pub struct LogQueryParamsBuilder {
    anchor: Anchor,
    direction: Direction,
    limit: Option<usize>,
    filter: Filter,
    after: Option<i64>,
    before: Option<i64>,
    resume_position: Option<u32>,
}

impl LogQueryParamsBuilder {
    pub fn new(anchor: Anchor, direction: Direction) -> Self {
        Self {
            anchor,
            direction,
            limit: None,
            filter: Filter::new(),
            after: None,
            before: None,
            resume_position: None,
        }
    }

    pub fn with_limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    pub fn with_filter(mut self, f: Filter) -> Self {
        self.filter = f;
        self
    }

    pub fn with_after(mut self, ts_ns: i64) -> Self {
        self.after = Some(ts_ns);
        self
    }

    pub fn with_before(mut self, ts_ns: i64) -> Self {
        self.before = Some(ts_ns);
        self
    }

    pub fn with_resume_position(mut self, pos: u32) -> Self {
        self.resume_position = Some(pos);
        self
    }

    pub fn build(self) -> Result<LogQueryParams, BuildError> {
        if let (Some(a), Some(b)) = (self.after, self.before)
            && a >= b
        {
            return Err(BuildError::TimeRangeInverted {
                after: a,
                before: b,
            });
        }
        Ok(LogQueryParams {
            anchor: self.anchor,
            direction: self.direction,
            limit: self.limit,
            filter: self.filter,
            after: self.after,
            before: self.before,
            resume_position: self.resume_position,
        })
    }
}

/// One fully-resolved log entry: timestamp + attribute list.
///
/// Attributes are returned as `(key, value)` pairs split from the on-disk
/// `"key=value"` form on the first `=` byte.
#[derive(Debug, Clone)]
pub struct ResolvedLog {
    pub position: u32,
    pub timestamp_ns: i64,
    pub attrs: Vec<(String, String)>,
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("time range inverted: after ({after}) >= before ({before})")]
    TimeRangeInverted { after: i64, before: i64 },
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("SFST error: {0}")]
    Sfst(#[from] sfst::Error),
    #[error("invalid query params: {0}")]
    Build(#[from] BuildError),
}

// ── Executor ─────────────────────────────────────────────────────────────

/// Executes a [`LogQueryParams`] against an [`IndexReader`]. Filter
/// evaluation is delegated to [`IndexReader::evaluate`]; this type adds
/// the anchor/direction/limit semantics, time-range narrowing, and
/// per-position log materialisation.
///
/// Caches `timestamps`, `string_table`, and `stream_batches` so each is
/// paid for at most once across the executor's lifetime.
pub struct LogQuery<'a> {
    reader: &'a IndexReader<'a>,
    params: LogQueryParams,
    timestamps: OnceCell<Vec<i64>>,
    string_table: OnceCell<Vec<String>>,
    stream_batches: HashMap<u8, Vec<Vec<KvId>>>,
}

impl<'a> LogQuery<'a> {
    pub fn new(reader: &'a IndexReader<'a>, params: LogQueryParams) -> Self {
        Self {
            reader,
            params,
            timestamps: OnceCell::new(),
            string_table: OnceCell::new(),
            stream_batches: HashMap::new(),
        }
    }

    /// Execute the query.
    pub fn run(&mut self) -> Result<Vec<ResolvedLog>, Error> {
        let total_logs = self.reader.summary().total_logs;
        if total_logs == 0 {
            return Ok(Vec::new());
        }

        let positions = self.compute_position_set()?;
        let start = self.resolve_anchor()?;
        let selected = self.select(positions.as_ref(), start, total_logs);

        let mut logs = Vec::with_capacity(selected.len());
        for pos in selected {
            logs.push(self.materialize(pos)?);
        }
        Ok(logs)
    }

    /// Compose the filter bitmap (from [`IndexReader::evaluate`]) with
    /// the time-range bitmap, if either is present. `None` means "no
    /// constraint" — all positions are candidates.
    fn compute_position_set(&self) -> Result<Option<RoaringBitmap>, Error> {
        let mut bm: Option<RoaringBitmap> = None;
        if !self.params.filter.is_empty() {
            bm = Some(self.reader.evaluate(&self.params.filter)?);
        }
        if self.params.after.is_some() || self.params.before.is_some() {
            let tr = self.time_range_bitmap()?;
            bm = Some(match bm {
                Some(prev) => prev & tr,
                None => tr,
            });
        }
        Ok(bm)
    }

    fn time_range_bitmap(&self) -> Result<RoaringBitmap, Error> {
        let timestamps = self.timestamps()?;
        let total = timestamps.len() as u32;
        let lo = self
            .params
            .after
            .map(|a| timestamps.partition_point(|&t| t < a) as u32)
            .unwrap_or(0);
        let hi = self
            .params
            .before
            .map(|b| timestamps.partition_point(|&t| t < b) as u32)
            .unwrap_or(total);
        let mut bm = RoaringBitmap::new();
        if lo < hi {
            bm.insert_range(lo..hi);
        }
        Ok(bm)
    }

    fn resolve_anchor(&self) -> Result<u32, Error> {
        let total = self.reader.summary().total_logs;
        Ok(match self.params.anchor {
            Anchor::Latest => total - 1,
            Anchor::Earliest => 0,
            Anchor::At(ts) => {
                let timestamps = self.timestamps()?;
                match self.params.direction {
                    Direction::Forward => timestamps.partition_point(|&t| t < ts) as u32,
                    Direction::Backward => {
                        let p = timestamps.partition_point(|&t| t <= ts);
                        if p == 0 { 0 } else { (p - 1) as u32 }
                    }
                }
            }
        })
    }

    fn select(&self, positions: Option<&RoaringBitmap>, start: u32, total: u32) -> Vec<u32> {
        let limit = self.params.limit.unwrap_or(usize::MAX);
        match self.params.direction {
            Direction::Forward => match positions {
                Some(bm) => bm.iter().filter(|&p| p >= start).take(limit).collect(),
                None => (start..total).take(limit).collect(),
            },
            Direction::Backward => match positions {
                Some(bm) => {
                    // Sliding window of the last `limit` ascending values
                    // ≤ start, then reverse. Memory bounded by `limit`.
                    let cap = limit.min(1024);
                    let mut window: VecDeque<u32> = VecDeque::with_capacity(cap);
                    for p in bm.iter().take_while(|&p| p <= start) {
                        if window.len() == limit {
                            window.pop_front();
                        }
                        window.push_back(p);
                    }
                    window.into_iter().rev().collect()
                }
                None => (0..=start).rev().take(limit).collect(),
            },
        }
    }

    fn materialize(&mut self, pos: u32) -> Result<ResolvedLog, Error> {
        let total_logs = self.reader.summary().total_logs;
        let batch_size = sfst::stream_batch_size(total_logs);
        let batch_idx = (pos / batch_size) as u8;
        let local = (pos % batch_size) as usize;

        let kv_ids: Vec<KvId> = {
            let batch = self.load_stream_batch(batch_idx)?;
            batch[local].clone()
        };

        let timestamp_ns = self.timestamps()?[pos as usize];
        let string_table = self.string_table()?;
        let mut attrs = Vec::with_capacity(kv_ids.len());
        for kv_id in &kv_ids {
            let s = &string_table[kv_id.idx()];
            let (key, value) = match s.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (s.clone(), String::new()),
            };
            attrs.push((key, value));
        }

        Ok(ResolvedLog {
            position: pos,
            timestamp_ns,
            attrs,
        })
    }

    // ── Caches ───────────────────────────────────────────────────────

    fn timestamps(&self) -> Result<&[i64], Error> {
        if self.timestamps.get().is_none() {
            let ts = self.reader.load_timestamps()?;
            let _ = self.timestamps.set(ts);
        }
        Ok(self.timestamps.get().expect("just initialized"))
    }

    fn string_table(&self) -> Result<&[String], Error> {
        if self.string_table.get().is_none() {
            let fields = self.reader.field_table().to_vec();
            let st = self.reader.build_string_table(&fields)?;
            let _ = self.string_table.set(st);
        }
        Ok(self.string_table.get().expect("just initialized"))
    }

    fn load_stream_batch(&mut self, idx: u8) -> Result<&Vec<Vec<KvId>>, Error> {
        if !self.stream_batches.contains_key(&idx) {
            let batch = self.reader.load_stream_batch(idx)?;
            self.stream_batches.insert(idx, batch);
        }
        Ok(self.stream_batches.get(&idx).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic SFST: 5 logs at ns 1000..5000, single low-card field
    /// `level` with values `info` at positions 0,2,4 and `error` at 1,3.
    fn build_test_sfst() -> Vec<u8> {
        let primary_entries: Vec<(&str, sfst::BitmapValue)> = vec![
            ("level=error", bitmap_with(&[1, 3], 5)),
            ("level=info", bitmap_with(&[0, 2, 4], 5)),
        ];
        let primary: fst_index::FstIndex<sfst::BitmapValue> =
            fst_index::FstIndex::build(primary_entries).unwrap();

        let summary = sfst::Summary {
            min_timestamp_s: 0,
            max_timestamp_s: 0,
            total_logs: 5,
            stream: sfst::ServiceStream::new("ns", "svc"),
        };
        let metadata = sfst::Metadata {
            histogram: sfst::Histogram {
                timestamps: vec![0],
                counts: vec![5],
            },
            id_ranges: sfst::IdRanges {
                low_end: sfst::KvId(2),
                mid_end: sfst::KvId(2),
                high_end: sfst::KvId(2),
            },
            fields: vec![sfst::FieldEntry {
                name: "level".into(),
                cardinality: 2,
                tier: sfst::FieldTier::Low,
            }],
        };
        let timestamps: Vec<i64> = vec![1000, 2000, 3000, 4000, 5000];
        // FST key order is lex: KvId(0)=error, KvId(1)=info.
        let stream_entries: Vec<Vec<KvId>> = vec![
            vec![KvId(1)],
            vec![KvId(0)],
            vec![KvId(1)],
            vec![KvId(0)],
            vec![KvId(1)],
        ];

        let mut writer = sfst::Writer::new();
        writer.set_summary(sfst::pack(&summary, 1).unwrap());
        writer.set_metadata(sfst::pack(&metadata, 1).unwrap());
        writer.set_primary(sfst::pack(&primary, 1).unwrap());
        writer.set_timestamps(sfst::pack(&timestamps, 1).unwrap());
        writer.add_stream_batch(sfst::pack(&stream_entries, 1).unwrap());
        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();
        buf
    }

    fn bitmap_with(positions: &[u32], universe: u32) -> sfst::BitmapValue {
        let mut data = Vec::new();
        let desc =
            treight::Bitmap::from_sorted_iter(positions.iter().copied(), universe, &mut data);
        sfst::BitmapValue { desc, data }
    }

    #[test]
    fn builder_rejects_inverted_time_range() {
        let result = LogQueryParamsBuilder::new(Anchor::Latest, Direction::Backward)
            .with_after(2000)
            .with_before(1000)
            .build();
        assert!(matches!(result, Err(BuildError::TimeRangeInverted { .. })));
    }

    #[test]
    fn latest_backward_returns_descending() {
        let data = build_test_sfst();
        let reader = sfst::IndexReader::open(&data).unwrap();
        let params = LogQueryParamsBuilder::new(Anchor::Latest, Direction::Backward)
            .with_limit(5)
            .build()
            .unwrap();
        let logs = LogQuery::new(&reader, params).run().unwrap();
        let positions: Vec<u32> = logs.iter().map(|l| l.position).collect();
        assert_eq!(positions, vec![4, 3, 2, 1, 0]);
    }

    #[test]
    fn earliest_forward_returns_ascending() {
        let data = build_test_sfst();
        let reader = sfst::IndexReader::open(&data).unwrap();
        let params = LogQueryParamsBuilder::new(Anchor::Earliest, Direction::Forward)
            .with_limit(3)
            .build()
            .unwrap();
        let logs = LogQuery::new(&reader, params).run().unwrap();
        let positions: Vec<u32> = logs.iter().map(|l| l.position).collect();
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[test]
    fn anchor_at_forward_snaps_to_first_ge() {
        let data = build_test_sfst();
        let reader = sfst::IndexReader::open(&data).unwrap();
        let params = LogQueryParamsBuilder::new(Anchor::At(2500), Direction::Forward)
            .with_limit(2)
            .build()
            .unwrap();
        let logs = LogQuery::new(&reader, params).run().unwrap();
        let positions: Vec<u32> = logs.iter().map(|l| l.position).collect();
        assert_eq!(positions, vec![2, 3]);
    }

    #[test]
    fn anchor_at_backward_snaps_to_last_le() {
        let data = build_test_sfst();
        let reader = sfst::IndexReader::open(&data).unwrap();
        let params = LogQueryParamsBuilder::new(Anchor::At(3500), Direction::Backward)
            .with_limit(2)
            .build()
            .unwrap();
        let logs = LogQuery::new(&reader, params).run().unwrap();
        let positions: Vec<u32> = logs.iter().map(|l| l.position).collect();
        assert_eq!(positions, vec![2, 1]);
    }

    #[test]
    fn filter_low_card_returns_matching() {
        let data = build_test_sfst();
        let reader = sfst::IndexReader::open(&data).unwrap();
        let params = LogQueryParamsBuilder::new(Anchor::Earliest, Direction::Forward)
            .with_filter(Filter::new().select("level", "info"))
            .with_limit(10)
            .build()
            .unwrap();
        let logs = LogQuery::new(&reader, params).run().unwrap();
        let positions: Vec<u32> = logs.iter().map(|l| l.position).collect();
        assert_eq!(positions, vec![0, 2, 4]);
    }

    #[test]
    fn time_range_narrows_positions() {
        let data = build_test_sfst();
        let reader = sfst::IndexReader::open(&data).unwrap();
        // [2500, 4500) covers t=3000 (pos 2), t=4000 (pos 3).
        let params = LogQueryParamsBuilder::new(Anchor::Earliest, Direction::Forward)
            .with_after(2500)
            .with_before(4500)
            .with_limit(10)
            .build()
            .unwrap();
        let logs = LogQuery::new(&reader, params).run().unwrap();
        let positions: Vec<u32> = logs.iter().map(|l| l.position).collect();
        assert_eq!(positions, vec![2, 3]);
    }

    #[test]
    fn backward_filter_limit_returns_last_matching() {
        let data = build_test_sfst();
        let reader = sfst::IndexReader::open(&data).unwrap();
        // level=info matches pos 0, 2, 4. Backward + limit=2 yields {4, 2}.
        let params = LogQueryParamsBuilder::new(Anchor::Latest, Direction::Backward)
            .with_filter(Filter::new().select("level", "info"))
            .with_limit(2)
            .build()
            .unwrap();
        let logs = LogQuery::new(&reader, params).run().unwrap();
        let positions: Vec<u32> = logs.iter().map(|l| l.position).collect();
        assert_eq!(positions, vec![4, 2]);
    }
}
