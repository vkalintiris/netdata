//! Step 2: row materialization (select-then-fetch).
//!
//! Returning a page of rows needs a global order over cursors
//! `(timestamp_ns, file_seq, sub_id, position)` across files, so — unlike step 1 —
//! it isn't a plain fold. It decomposes into seams a cross-node fan-out
//! reuses:
//!
//! - [`PageShard::evaluate`] (map: one file -> its page candidates),
//! - [`PageShard::merge`] (reduce: combine candidate sets — associative),
//! - [`finalize_page`] (root: pick the page + the has-more flags),
//! - [`materialize`] (fetch: chosen cursors -> row bodies).
//!
//! [`paginate`] is the local orchestration of all four; a distributed
//! parent would run `merge`/`finalize_page` on candidate sets received
//! from children and route `materialize` back to the file's owning node by
//! `file_seq`.

use std::collections::HashMap;

use super::mmap::Mapped;

use super::cursor::Cursor;
use super::engine::{LogSource, SfstCandidate, WalTail};
use super::mmap;
use super::query::{Anchor, Direction, LogsQuery};
use super::wal_scan::WalScan;

const NS_PER_S: i64 = 1_000_000_000;

/// One file's (or one node's) page candidates: the window-matching
/// cursors on the requested side of the anchor, ordered closest-to-anchor
/// first, plus whether the file has any match on the *opposite* side
/// (which becomes the opposite direction's has-more flag).
///
/// [`PageShard::evaluate`] produces one per file; [`PageShard::merge`] folds
/// them. The candidate list may be bounded to the page size (a later
/// step) — all a fan-out needs to ship — or unbounded.
#[derive(Debug, Default)]
pub struct PageShard {
    /// Candidate cursors, ordered closest-to-anchor first — the order
    /// [`merge`](PageShard::merge) and [`finalize_page`] take a prefix of.
    pub cursors: Vec<Cursor>,
    /// Whether this shard has any match on the side of the anchor *away*
    /// from the page direction (the source of the opposite has-more flag).
    pub has_opposite: bool,
}

impl PageShard {
    /// Reduce: combine page-candidate shards into one.
    ///
    /// Pools the candidates, re-orders them closest-to-anchor first for
    /// `direction`, optionally keeps only the nearest `bound`, and ORs
    /// `has_opposite`. Associative, so a node can merge the files it owns
    /// and a parent can merge the node-shards the same way.
    pub fn merge(shards: Vec<PageShard>, direction: Direction, bound: Option<usize>) -> PageShard {
        let mut cursors: Vec<Cursor> = Vec::new();
        let mut has_opposite = false;
        for shard in shards {
            cursors.extend(shard.cursors);
            has_opposite |= shard.has_opposite;
        }
        order_by_closeness(&mut cursors, direction);
        if let Some(bound) = bound {
            cursors.truncate(bound);
        }
        PageShard {
            cursors,
            has_opposite,
        }
    }

    /// Map: evaluate one file's page candidates against the query.
    ///
    /// Intersects the filter with the window, tags each matching position
    /// with its [`Cursor`], and splits at `anchor` (exclusive): the candidates
    /// are the matches on `query.direction`'s side, ordered closest-to-anchor
    /// first and optionally truncated to `bound`; `has_opposite` records
    /// whether any match falls on the other side. `anchor == None` starts at
    /// the edge — every match is a candidate and there is no opposite side.
    pub fn evaluate(
        reader: &sfst::IndexReader<'_>,
        seq: u64,
        sub_id: u32,
        query: &LogsQuery,
        anchor: Option<Cursor>,
        bound: Option<usize>,
    ) -> Result<PageShard, sfst::Error> {
        let filter = reader.compile_filter(&query.filter, query.query())?;
        let matched = reader.matched_positions(&filter, query.grid.range_ns())?;
        let timestamps = reader.load_timestamps()?;

        // Cursors for every match, ascending — within a file, position order
        // is cursor order (timestamps are chronological and `seq`/`sub_id`
        // are constant). A matched position with no timestamp means the
        // file's chunks disagree (corrupt SFST); fail so the caller skips
        // this source rather than emitting a bogus epoch-0 cursor.
        let ascending: Vec<Cursor> = matched
            .into_iter()
            .map(|position| {
                let timestamp_ns = timestamps.at(position).ok_or_else(|| {
                    sfst::Error::CorruptIndex(format!(
                        "matched position {position} has no timestamp \
                         (file_seq={seq}, sub_id={sub_id})"
                    ))
                })?;
                Ok(Cursor {
                    timestamp_ns,
                    file_seq: seq,
                    sub_id,
                    position,
                })
            })
            .collect::<Result<Vec<_>, sfst::Error>>()?;

        Ok(Self::from_cursors(ascending, query.direction, anchor, bound))
    }

    /// Build a shard from this source's cursors, already sorted ascending
    /// by [`Cursor`] order. Splits at `anchor` (exclusive), keeps the
    /// page side ordered closest-to-anchor first, and bounds it. Shared
    /// by the SFST path ([`evaluate`](Self::evaluate), whose cursors are
    /// ascending by position) and the WAL-tail row scan (whose cursors
    /// must be sorted first, since the tail isn't time-ordered).
    pub fn from_cursors(
        mut ascending: Vec<Cursor>,
        direction: Direction,
        anchor: Option<Cursor>,
        bound: Option<usize>,
    ) -> PageShard {
        // Split at the anchor (exclusive). Backward's page side is `< anchor`
        // (opposite `>= anchor`); forward's is `> anchor` (opposite `<= anchor`).
        let (mut cursors, has_opposite) = match (anchor, direction) {
            (None, _) => (std::mem::take(&mut ascending), false),
            (Some(a), Direction::Backward) => {
                let split = ascending.partition_point(|c| *c < a);
                let has_opposite = split < ascending.len();
                ascending.truncate(split);
                (ascending, has_opposite)
            }
            (Some(a), Direction::Forward) => {
                let split = ascending.partition_point(|c| *c <= a);
                let has_opposite = split > 0;
                (ascending.split_off(split), has_opposite)
            }
        };

        order_by_closeness(&mut cursors, direction);
        if let Some(bound) = bound {
            cursors.truncate(bound);
        }

        PageShard {
            cursors,
            has_opposite,
        }
    }
}

/// Order cursors closest-to-anchor first for `direction`: backward walks
/// toward older rows, so the largest (newest) cursors come first;
/// forward walks toward newer rows, so the smallest (oldest) come first.
fn order_by_closeness(cursors: &mut [Cursor], direction: Direction) {
    match direction {
        Direction::Backward => cursors.sort_unstable_by(|a, b| b.cmp(a)),
        Direction::Forward => cursors.sort_unstable(),
    }
}

/// The chosen page: cursors newest-first, plus the has-more flags a
/// consumer uses to gate infinite scroll in each direction.
#[derive(Default)]
struct SelectedPage {
    /// Cursors newest-first (`cursors[0]` is the newest).
    cursors: Vec<Cursor>,
    /// A newer row exists beyond the page (consumer "scroll up").
    has_newer: bool,
    /// An older row exists beyond the page (consumer "scroll down").
    has_older: bool,
}

/// Root: pick the page from the merged candidates.
///
/// The nearest `limit` cursors form the page; one more candidate beyond
/// them means there are more rows in `direction`, and `merged.has_opposite`
/// means more on the other side. The page is returned newest-first
/// regardless of direction.
fn finalize_page(merged: PageShard, direction: Direction, limit: usize) -> SelectedPage {
    let has_more_in_direction = merged.cursors.len() > limit;
    let mut cursors = merged.cursors;
    cursors.truncate(limit);
    // `cursors` is closest-to-anchor first: backward (toward older) is
    // already newest-first; forward (toward newer) is oldest-first, so
    // reverse it to present newest-first like the other direction.
    if direction == Direction::Forward {
        cursors.reverse();
    }
    let (has_newer, has_older) = match direction {
        Direction::Backward => (merged.has_opposite, has_more_in_direction),
        Direction::Forward => (has_more_in_direction, merged.has_opposite),
    };
    SelectedPage {
        cursors,
        has_newer,
        has_older,
    }
}

/// Fetch: materialize the chosen cursors into rows.
///
/// Routes each cursor to its owning file by `file_seq`, batches positions
/// per file so each file's chunks decompress once, and reassembles the
/// rows in the page's newest-first order. Locally the files are the open
/// readers; a cross-node fetch would route each cursor to its owning node.
fn materialize(
    sfst_readers: &[(sfst::IndexReader<'_>, u64, u32)],
    tail_scans: &[(u64, WalScan)],
    selected: &SelectedPage,
) -> Result<Vec<(Cursor, sfst::MaterializedRow)>, sfst::Error> {
    // Route by `(file_seq, sub_id)`: an SFST reader (on-disk or chunk)
    // for `sub_id != TAIL`, the WAL row scanner for `sub_id == TAIL`.
    let sfst_by_key: HashMap<(u64, u32), &sfst::IndexReader<'_>> = sfst_readers
        .iter()
        .map(|(reader, seq, sub_id)| ((*seq, *sub_id), reader))
        .collect();
    let tail_by_seq: HashMap<u64, &WalScan> =
        tail_scans.iter().map(|(seq, scan)| (*seq, scan)).collect();

    // Batch positions per source so each source decompresses once.
    let mut positions: HashMap<(u64, u32), Vec<u32>> = HashMap::new();
    for cursor in &selected.cursors {
        positions
            .entry((cursor.file_seq, cursor.sub_id))
            .or_default()
            .push(cursor.position);
    }

    let mut row_by_key: HashMap<(u64, u32, u32), sfst::MaterializedRow> = HashMap::new();
    for ((seq, sub_id), pos) in &positions {
        if *sub_id == Cursor::TAIL_SUB_ID {
            if let Some(scan) = tail_by_seq.get(seq) {
                for (p, row) in pos.iter().zip(scan.materialize_rows(pos)) {
                    row_by_key.insert((*seq, *sub_id, *p), row);
                }
            }
        } else if let Some(reader) = sfst_by_key.get(&(*seq, *sub_id)) {
            for (p, row) in pos.iter().zip(reader.materialize_rows(pos)?) {
                row_by_key.insert((*seq, *sub_id, *p), row);
            }
        }
    }

    let rows = selected
        .cursors
        .iter()
        .filter_map(|cursor| {
            row_by_key
                .remove(&(cursor.file_seq, cursor.sub_id, cursor.position))
                .map(|row| (*cursor, row))
        })
        .collect();
    Ok(rows)
}

/// A materialized page: rows newest-first plus the has-more flags.
#[derive(Default)]
pub(super) struct Page {
    pub(super) rows: Vec<(Cursor, sfst::MaterializedRow)>,
    pub(super) has_newer: bool,
    pub(super) has_older: bool,
}

/// Open the candidate files in time-priority order and materialize one
/// page, stopping as soon as the remaining files can't contribute.
///
/// Candidates are processed closest-to-anchor first (backward: newest
/// file first; forward: oldest first). Each file's bounded candidates fold
/// into a running merge; once the page is full *and* the next file is
/// entirely beyond the page boundary, the rest are skipped — never opened
/// or decoded (they pay only the up-front mmap, which reads nothing).
///
/// The files are mapped up front so the readers, which borrow the
/// mappings, see a stable `Vec`. Files that fail to map/parse/evaluate are
/// logged and skipped. Each opened file's cold suffix is released from the
/// page cache once the page is materialized.
pub(super) fn paginate(sources: &[LogSource], query: &LogsQuery) -> Page {
    // Split into tails and SFSTs up front. Tails must seed the merge
    // first: the SFST early-termination below samples
    // `merged.cursors[limit - 1]` as its boundary, and that sample must
    // already include every tail cursor. Adding tails later can only push
    // the boundary older, so a boundary taken without them is too new —
    // `beyond_boundary` would fire too eagerly and could skip an SFST
    // whose rows belong on the page. (Order *within* each kind is
    // irrelevant; the merge re-sorts by cursor.)
    let mut wal_tails: Vec<&WalTail> = Vec::new();
    let mut sfst_candidates: Vec<&SfstCandidate> = Vec::new();
    for source in sources {
        match source {
            LogSource::Tail(t) => wal_tails.push(t),
            LogSource::Sfst(c) => sfst_candidates.push(c),
        }
    }

    let bound = Some(query.limit.saturating_add(1));
    let anchor = query.anchor.map(Anchor::to_cursor);
    let mut merged = PageShard::default();

    // WAL tails first: there are few and each must be scanned anyway, so
    // seeding the merge with their cursors means the SFST early-
    // termination boundary below already accounts for them. The scans are
    // kept for the materialize step.
    let mut tail_scans: Vec<(u64, WalScan)> = Vec::new();
    for &tail in &wal_tails {
        let scan = match WalScan::scan_range(&tail.path, tail.start, tail.end) {
            Ok(scan) => scan,
            Err(e) => {
                tracing::warn!(
                    "sfsq: tail scan failed for {} [{}..{}]: {e}",
                    tail.path.display(),
                    tail.start,
                    tail.end
                );
                continue;
            }
        };
        match scan.page_shard(tail.seq, query, anchor, bound) {
            Ok(shard) => merged = PageShard::merge(vec![merged, shard], query.direction, bound),
            Err(e) => {
                tracing::warn!("sfsq: tail page candidates failed (seq={}): {e}", tail.seq);
                continue;
            }
        }
        tail_scans.push((tail.seq, scan));
    }

    // SFSTs (on-disk + in-memory chunks) closest-to-anchor first so we can
    // stop once the page is full and the next file (hence every later one,
    // since they're time-sorted) is entirely beyond the page boundary.
    let mut order = sfst_candidates;
    match query.direction {
        Direction::Backward => {
            order.sort_by_key(|c| std::cmp::Reverse(c.summary.max_timestamp_s));
        }
        Direction::Forward => order.sort_by_key(|c| c.summary.min_timestamp_s),
    }

    // Map up front (cheap — no chunk is read until a reader opens) so the
    // readers borrowing the mappings see a stable Vec.
    let mappings: Vec<(Mapped, &SfstCandidate)> = order
        .iter()
        .filter_map(|candidate| mmap::map_source(&candidate.source).map(|m| (m, *candidate)))
        .collect();

    let mut readers: Vec<(sfst::IndexReader<'_>, u64, u32)> = Vec::new();
    let mut reader_mapping: Vec<usize> = Vec::new();
    for (index, (mapping, candidate)) in mappings.iter().enumerate() {
        let reader = match sfst::IndexReader::open(mapping.bytes()) {
            Ok(reader) => reader,
            Err(e) => {
                tracing::warn!("sfsq: failed to parse {}: {e}", candidate.source.describe());
                continue;
            }
        };
        match PageShard::evaluate(&reader, candidate.seq, candidate.sub_id, query, anchor, bound) {
            Ok(shard) => merged = PageShard::merge(vec![merged, shard], query.direction, bound),
            Err(e) => {
                tracing::warn!(
                    "sfsq: page candidates failed for {}: {e}",
                    candidate.source.describe()
                );
                continue;
            }
        }
        readers.push((reader, candidate.seq, candidate.sub_id));
        reader_mapping.push(index);

        if query.limit > 0 && merged.cursors.len() > query.limit {
            let boundary = merged.cursors[query.limit - 1];
            if mappings.get(index + 1).is_some_and(|(_, next)| {
                let summary = &next.summary;
                beyond_boundary(
                    query.direction,
                    boundary,
                    summary.min_timestamp_s,
                    summary.max_timestamp_s,
                )
            }) {
                break;
            }
        }
    }

    // Finalize the page, then materialize its rows. A materialize failure
    // collapses to an empty page rather than reporting has-more flags with
    // no rows behind them.
    let selected = finalize_page(merged, query.direction, query.limit);
    let page = match materialize(&readers, &tail_scans, &selected) {
        Ok(rows) => Page {
            rows,
            has_newer: selected.has_newer,
            has_older: selected.has_older,
        },
        Err(e) => {
            tracing::warn!("sfsq: materialize failed: {e}");
            Page::default()
        }
    };

    // Release each opened file's cold suffix (mid/high field chunks + stream
    // batches), keeping the hot prefix resident. In-memory chunks have no
    // file pages to drop.
    let cold: Vec<(usize, (usize, usize))> = readers
        .iter()
        .zip(&reader_mapping)
        .filter_map(|((reader, _, _), &index)| reader.cold_region().map(|region| (index, region)))
        .collect();
    drop(readers);
    for (index, region) in cold {
        if let Mapped::File(m) = &mappings[index].0 {
            mmap::release_cold_region(m, region);
        }
    }

    page
}

/// Whether a candidate with second-granular range `[min_ts_s, max_ts_s]`
/// lies entirely beyond the page boundary — so it (and, since candidates
/// are processed in time-priority order, every one after it) cannot
/// contribute a cursor nearer the anchor. `boundary` is the page's
/// farthest-from-anchor cursor (the L-th).
///
/// Conservative across the second→nanosecond gap: a file is skipped only
/// when its *entire* second-range is past the boundary, so a file that
/// could still hold a contributing cursor is never skipped.
fn beyond_boundary(direction: Direction, boundary: Cursor, min_ts_s: u32, max_ts_s: u32) -> bool {
    match direction {
        // Backward: the file's newest possible cursor is `< (max_ts_s + 1)·s`.
        // If that's at or below the boundary, no cursor can sit nearer the
        // anchor (a larger cursor) than the boundary.
        Direction::Backward => (i64::from(max_ts_s) + 1) * NS_PER_S <= boundary.timestamp_ns,
        // Forward: the file's oldest possible cursor is `>= min_ts_s·s`. If
        // that's beyond the boundary, no cursor can sit nearer the anchor (a
        // smaller cursor) than the boundary.
        Direction::Forward => i64::from(min_ts_s) * NS_PER_S > boundary.timestamp_ns,
    }
}

#[cfg(test)]
mod tests;
