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

use std::path::PathBuf;
use std::sync::Arc;

use super::aggregate::LogsShard;
use super::page::paginate;
use super::query::LogsQuery;
use super::result::LogsData;
use super::wal_scan::WalScan;

/// Where an SFST candidate's bytes come from.
///
/// `File` is the steady-state case — a sealed index on disk, memory-
/// mapped lazily. `Memory` is an in-memory SFST built from a chunk of an
/// active WAL ([`sfst::index_range`]); the bytes are shared (`Arc`) so a
/// query holds them alive even if the producing cache evicts the entry
/// mid-query.
#[derive(Clone)]
pub enum Source {
    File(PathBuf),
    Memory(Arc<Vec<u8>>),
}

impl Source {
    /// A short label for log/error context.
    pub(super) fn describe(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Source::File(p) => p.display().to_string().into(),
            Source::Memory(_) => "<in-memory chunk>".into(),
        }
    }
}

/// A query candidate: an SFST whose time range overlaps the request
/// window — either a sealed on-disk file or an in-memory chunk built
/// from an active WAL's durable prefix. Owned, so the caller can release
/// any lock on the file source before the query does I/O.
///
/// `seq` and `sub_id` together place this candidate's rows in the
/// pagination cursor's total order (see [`Cursor`](super::cursor::Cursor)).
pub struct SfstCandidate {
    /// Cheap time/stream/size facts ([`sfst::Summary`]); its `[min, max]`
    /// second-range is what overlapped the request window to make this a
    /// candidate.
    pub summary: sfst::Summary,
    /// Globally-unique sequence of the underlying file — the sealed
    /// SFST's own seq, or the active WAL's seq for an in-memory chunk
    /// (so all chunks of one WAL share it). The cursor's second key.
    pub seq: u64,
    /// Distinguishes the parts of one active WAL that share a `seq`:
    /// [`Cursor::SFST_SUB_ID`](super::cursor::Cursor::SFST_SUB_ID) (`0`)
    /// for a sealed SFST, or the chunk index for an in-memory chunk. The
    /// cursor's third key, breaking ties at equal `(timestamp, seq)`.
    pub sub_id: u32,
    /// Where the candidate's bytes come from — [`Source::File`] for a
    /// sealed index, [`Source::Memory`] for an in-memory WAL chunk.
    pub source: Source,
}

/// A byte range of an active WAL whose log records have not been indexed
/// into an SFST — the sub-chunk *tail*. Evaluated by a row scan
/// ([`WalScan`]) rather than the SFST engine. Bounded (< one chunk) by
/// construction, so re-scanning it per query is affordable.
pub struct WalTail {
    /// The active WAL's sequence — the same globally-unique id the
    /// pagination cursor orders by as `file_seq`. The tail's rows sort
    /// under it with `sub_id` =
    /// [`Cursor::TAIL_SUB_ID`](super::cursor::Cursor::TAIL_SUB_ID), after
    /// every chunk of the same seq.
    pub seq: u64,
    /// Path to the active WAL file to scan.
    pub path: PathBuf,
    /// Byte offset of the first frame to scan — a frame boundary (the end
    /// of the last indexed chunk, or `HEADER_SIZE` if none was built). A
    /// mid-frame offset would decode garbage.
    pub start: u64,
    /// Byte offset just past the last frame — the WAL's durable bound
    /// (`valid_up_to`), also a frame boundary. The range is half-open
    /// `[start, end)`: a frame is scanned only if it fits fully below
    /// `end`, so a torn trailing frame past the bound is never read.
    pub end: u64,
}

/// Run the merged query over the candidate files.
///
/// Evaluates every candidate into a [`LogsShard`] (step 1), merges them,
/// then paginates and materializes a page (step 2), and assembles the
/// [`LogsData`]. The grid's span is the window every count and the
/// materialized page clip to.
///
/// Per-source errors (corrupt file, missing field, unreadable WAL tail,
/// etc.) are logged and that source is skipped — others still
/// contribute. An empty candidate set (or one where everything fails)
/// yields an empty `LogsData` aligned to the grid (the monoid identity).
///
/// Statistics (matched, facets, histogram, field table) and the row
/// table both reflect **every** source — sealed SFSTs, in-memory chunks
/// of active WALs, and the WAL tails — interleaved under the unified
/// cursor order `(timestamp_ns, file_seq, sub_id, position)`, where
/// `sub_id` distinguishes the parts of one active WAL that share a `seq`.
///
/// Pure sync — no I/O scheduling, no locks, no geometry policy — but
/// since it reads and decompresses files the caller is expected to invoke
/// it off any async runtime thread.
pub fn run(
    sfst_candidates: Vec<SfstCandidate>,
    wal_tails: Vec<WalTail>,
    query: LogsQuery,
) -> LogsData {
    let grid = query.grid;

    // Step 1: evaluate every SFST (on-disk + in-memory chunk) and every
    // WAL tail (row scan) into a shard, then merge into one.
    let mut shards: Vec<LogsShard> = sfst_candidates
        .iter()
        .map(|c| LogsShard::evaluate(c, &query))
        .collect();
    for tail in &wal_tails {
        match WalScan::scan_range(&tail.path, tail.start, tail.end) {
            Ok(scan) => shards.push(scan.evaluate(&query)),
            Err(e) => tracing::warn!(
                "sfsq: WAL tail scan failed for {} [{}..{}]: {e}",
                tail.path.display(),
                tail.start,
                tail.end
            ),
        }
    }
    let stats = LogsShard::merge(shards);

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

    // Step 2: paginate across every source under the unified cursor
    // order — on-disk SFSTs, in-memory chunks (`sub_id` = chunk index),
    // and the WAL tails (`sub_id` = TAIL).
    let page_candidates: Vec<&SfstCandidate> = sfst_candidates.iter().collect();
    let page = paginate(&page_candidates, &wal_tails, &query);

    LogsData {
        matched: stats.matched as usize,
        facets: stats.facets,
        histogram_field: query.histogram_field,
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
