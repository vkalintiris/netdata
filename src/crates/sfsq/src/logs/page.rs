//! Step 2: row materialization (select-then-fetch).
//!
//! Returning a page of rows needs a global order over cursors
//! `(timestamp_ns, file_seq, position)` across files, so — unlike step 1 —
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

use memmap2::Mmap;

use super::cursor::Cursor;
use super::engine::SfstCandidate;
use super::mmap;
use super::query::{Anchor, Direction, LogsQuery};

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
        query: &LogsQuery,
        anchor: Option<Cursor>,
        bound: Option<usize>,
    ) -> Result<PageShard, sfst::Error> {
        let filter = reader.compile_filter(&query.filter, query.query())?;
        let matched = reader.matched_positions(&filter, query.grid.range_ns())?;
        let timestamps = reader.load_timestamps()?;

        // Cursors for every match, ascending — within a file, position order
        // is cursor order (timestamps are chronological and `seq` is constant).
        let mut ascending: Vec<Cursor> = matched
            .into_iter()
            .map(|position| Cursor {
                timestamp_ns: timestamps.at(position).unwrap_or(0),
                file_seq: seq,
                position,
            })
            .collect();

        // Split at the anchor (exclusive). Backward's page side is `< anchor`
        // (opposite `>= anchor`); forward's is `> anchor` (opposite `<= anchor`).
        let (mut cursors, has_opposite) = match (anchor, query.direction) {
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

        order_by_closeness(&mut cursors, query.direction);
        if let Some(bound) = bound {
            cursors.truncate(bound);
        }

        Ok(PageShard {
            cursors,
            has_opposite,
        })
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
    files: &[(&sfst::IndexReader<'_>, u64)],
    selected: &SelectedPage,
) -> Result<Vec<(Cursor, sfst::MaterializedRow)>, sfst::Error> {
    let by_seq: HashMap<u64, &sfst::IndexReader<'_>> =
        files.iter().map(|(reader, seq)| (*seq, *reader)).collect();

    let mut positions_by_seq: HashMap<u64, Vec<u32>> = HashMap::new();
    for cursor in &selected.cursors {
        positions_by_seq
            .entry(cursor.file_seq)
            .or_default()
            .push(cursor.position);
    }
    let mut row_by_key: HashMap<(u64, u32), sfst::MaterializedRow> = HashMap::new();
    for (seq, positions) in &positions_by_seq {
        let Some(reader) = by_seq.get(seq) else {
            continue;
        };
        for (position, row) in positions.iter().zip(reader.materialize_rows(positions)?) {
            row_by_key.insert((*seq, *position), row);
        }
    }

    let rows = selected
        .cursors
        .iter()
        .filter_map(|cursor| {
            row_by_key
                .remove(&(cursor.file_seq, cursor.position))
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
pub(super) fn paginate(candidates: &[SfstCandidate], query: &LogsQuery) -> Page {
    // Process closest-to-anchor first so we can stop once the page is full:
    // backward walks newest -> oldest, forward oldest -> newest.
    let mut order: Vec<&SfstCandidate> = candidates.iter().collect();
    match query.direction {
        Direction::Backward => {
            order.sort_by_key(|c| std::cmp::Reverse(c.summary.max_timestamp_s));
        }
        Direction::Forward => order.sort_by_key(|c| c.summary.min_timestamp_s),
    }

    // Map up front (cheap — no chunk is read until a reader opens) so the
    // readers borrowing the mappings see a stable Vec.
    let mappings: Vec<(Mmap, &SfstCandidate)> = order
        .iter()
        .filter_map(|candidate| mmap::map_file(&candidate.path).map(|m| (m, *candidate)))
        .collect();

    // Open + evaluate one file at a time, folding into a running merge, and
    // stop once the page is full and the next file (hence every later one,
    // since they're time-sorted) is entirely beyond the page boundary. The
    // `+1` on the bound lets the root detect a row past the page edge.
    let bound = Some(query.limit.saturating_add(1));
    let anchor = query.anchor.map(Anchor::to_cursor);
    let mut readers: Vec<sfst::IndexReader<'_>> = Vec::new();
    let mut seqs: Vec<u64> = Vec::new();
    let mut reader_mapping: Vec<usize> = Vec::new();
    let mut merged = PageShard::default();
    for (index, (mapping, candidate)) in mappings.iter().enumerate() {
        let reader = match sfst::IndexReader::open(mapping) {
            Ok(reader) => reader,
            Err(e) => {
                tracing::warn!("sfsq: failed to parse {}: {e}", candidate.path.display());
                continue;
            }
        };
        match PageShard::evaluate(&reader, candidate.seq, query, anchor, bound) {
            Ok(shard) => merged = PageShard::merge(vec![merged, shard], query.direction, bound),
            Err(e) => {
                tracing::warn!(
                    "sfsq: page candidates failed for {}: {e}",
                    candidate.path.display()
                );
                continue;
            }
        }
        readers.push(reader);
        seqs.push(candidate.seq);
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
    let files: Vec<(&sfst::IndexReader<'_>, u64)> =
        readers.iter().zip(seqs.iter().copied()).collect();

    // Finalize the page, then materialize its rows. A materialize failure
    // collapses to an empty page rather than reporting has-more flags with
    // no rows behind them.
    let selected = finalize_page(merged, query.direction, query.limit);
    let page = match materialize(&files, &selected) {
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
    // batches), keeping the hot prefix resident. Compute the regions while
    // the readers are alive, then drop the borrows before advising.
    let cold: Vec<(usize, (usize, usize))> = readers
        .iter()
        .zip(&reader_mapping)
        .filter_map(|(reader, &index)| reader.cold_region().map(|region| (index, region)))
        .collect();
    drop(files);
    drop(readers);
    for (index, region) in cold {
        mmap::release_cold_region(&mappings[index].0, region);
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
