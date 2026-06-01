//! Composition: the all-in-one local query.
//!
//! [`run`] ties the two steps together for the local case: it evaluates
//! every candidate into a [`LogsShard`](super::LogsShard) (step 1, see
//! [`aggregate`](super::aggregate)), merges the shards, then paginates and
//! materializes a page (step 2, see [`page`](super::page)), and assembles
//! a single [`LogsData`].
//!
//! It opens each file once for step 1 and again for step 2; the re-open is
//! deliberate — step 1's shards are fully owned and drop their readers, and
//! the heavy work is the bounded page materialization, not the re-read.
//!
//! The caller supplies a fully-specified [`LogsQuery`] — including the
//! histogram [`grid`](LogsQuery::grid), whose span is the query window —
//! and selects the candidates whose range overlaps that window. The work
//! is pure and synchronous — no I/O scheduling, no locks, no
//! window/geometry policy — but since it reads and decompresses files the
//! caller is expected to invoke it off any async runtime thread.

use super::aggregate::LogsShard;
use super::cursor::Cursor;
use super::page::paginate;
use super::query::{Anchor, LogsQuery};
use super::result::LogsData;

/// A query candidate: an SFST file whose range overlaps the request
/// window. Owned so the caller can release any lock on its file source
/// before the query does I/O. `seq` is the file's monotonic per-file
/// id, used as the cross-file tiebreaker in the pagination cursor's
/// total order.
pub struct SfstCandidate {
    pub summary: sfst::Summary,
    pub seq: u64,
    pub path: std::path::PathBuf,
}

/// Run the merged query over the candidate files.
///
/// Evaluates every candidate into a [`LogsShard`] (step 1), merges them,
/// then paginates and materializes a page (step 2), and assembles the
/// [`LogsData`]. The grid's span is the window every count and the
/// materialized page clip to.
///
/// Per-file errors (corrupt file, missing field, etc.) are logged and
/// that file is skipped — other files still contribute. An empty
/// candidate set (or one where every file fails to open) yields an empty
/// `LogsData` aligned to the grid (the monoid identity).
///
/// Pure sync — no I/O scheduling, no locks, no geometry policy — but
/// since it reads and decompresses files the caller is expected to invoke
/// it off any async runtime thread.
pub fn run(candidates: Vec<SfstCandidate>, query: LogsQuery) -> LogsData {
    let grid = query.grid;

    // Step 1: evaluate each file, then merge into one statistics shard.
    let stats = LogsShard::merge(
        candidates
            .iter()
            .map(|c| LogsShard::evaluate(c, &query))
            .collect(),
    );

    // `available_fields` is the merged table with high-card fields
    // dropped — the offerable facet / histogram set — while `columns`
    // keeps the full name set, all tiers. The high-card drop happens
    // here, once, at the root.
    let available_fields: sfst::FieldTable = stats
        .fields
        .iter()
        .filter(|field| !field.is_high_card())
        .cloned()
        .collect();
    let columns: Vec<String> = stats.fields.names().map(str::to_owned).collect();
    let histogram = stats.timeline.unwrap_or_else(|| empty_timeline(grid));
    let histogram_field = query.histogram_field.clone();

    // Step 2: select and materialize one page across the same files.
    // Resolve the anchor to a cursor in the global total order. A row
    // cursor is used directly; a timestamp becomes a synthetic cursor at
    // the end of that instant (file_seq/position maxed), so a backward
    // page shows the newest rows up to that time.
    let anchor = query.anchor.map(|anchor| match anchor {
        Anchor::Cursor(c) => c,
        Anchor::Timestamp(ns) => Cursor {
            timestamp_ns: ns,
            file_seq: u64::MAX,
            position: u32::MAX,
        },
    });
    let page = paginate(&candidates, &query, anchor);

    LogsData {
        matched: stats.matched as usize,
        facets: stats.facets,
        histogram_field,
        histogram,
        available_fields,
        columns,
        rows: page.rows,
        has_newer: page.has_newer,
        has_older: page.has_older,
    }
}

/// An empty timeline aligned to `grid`: no dimensions, all-zero buckets.
fn empty_timeline(grid: sfst::Grid) -> sfst::Timeline {
    sfst::Timeline {
        grid,
        dimensions: Vec::new(),
        buckets: vec![
            sfst::Bucket {
                counts: Vec::new(),
                unset: 0,
            };
            grid.num_buckets
        ],
    }
}
