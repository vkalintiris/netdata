//! Step 2: row materialization (select-then-fetch).
//!
//! Returning a page of rows needs a global order over cursors
//! `(timestamp_ns, file_seq, position)` across files, so — unlike step 1 —
//! it isn't a plain fold. It decomposes into seams a cross-node fan-out
//! reuses:
//!
//! - [`evaluate_page`] (map: one file -> its page candidates),
//! - [`PageShard::merge`] (reduce: combine candidate sets — associative),
//! - [`finalize_page`] (root: pick the page + the has-more flags),
//! - [`materialize`] (fetch: chosen cursors -> row bodies).
//!
//! [`paginate`] is the local orchestration of all four; a distributed
//! parent would run `merge`/`finalize_page` on candidate sets received
//! from children and route `materialize` back to the file's owning node by
//! `file_seq`.

use std::collections::HashMap;
use std::path::Path;

use memmap2::Mmap;

use super::cursor::Cursor;
use super::engine::SfstCandidate;
use super::mmap;
use super::query::{Direction, LogsQuery, build_filter};

/// One file's (or one node's) page candidates: the window-matching
/// cursors on the requested side of the anchor, ordered closest-to-anchor
/// first, plus whether the file has any match on the *opposite* side
/// (which becomes the opposite direction's has-more flag).
///
/// [`evaluate_page`] produces one per file; [`PageShard::merge`] folds
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
}

/// Map: evaluate one file's page candidates against the query.
///
/// Intersects the filter with the window, tags each matching position
/// with its [`Cursor`], and splits at `anchor` (exclusive): the candidates
/// are the matches on `query.direction`'s side, ordered closest-to-anchor
/// first and optionally truncated to `bound`; `has_opposite` records
/// whether any match falls on the other side. `anchor == None` starts at
/// the edge — every match is a candidate and there is no opposite side.
pub fn evaluate_page(
    reader: &sfst::IndexReader<'_>,
    seq: u64,
    query: &LogsQuery,
    anchor: Option<Cursor>,
    bound: Option<usize>,
) -> Result<PageShard, sfst::Error> {
    let filter = build_filter(&query.selections);
    let matched = reader.evaluate(&filter)? & &reader.range_bitmap(query.grid.range_ns())?;
    let timestamps = reader.load_timestamps()?;

    // Cursors for every match, ascending — within a file, position order
    // is cursor order (timestamps are chronological and `seq` is constant).
    let mut ascending: Vec<Cursor> = matched
        .iter()
        .map(|position| Cursor {
            timestamp_ns: timestamps.get(position as usize).copied().unwrap_or(0),
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

/// Open the candidate files and select + materialize one page across them.
///
/// This re-maps the files step 1 already touched (see the module docs):
/// step 1's shards are fully owned and dropped their readers, so the page
/// path maps the files again. The readers borrow from the mappings, so the
/// mappings are held alive for the duration. Files that fail to map,
/// parse, or evaluate are logged and skipped; an empty candidate set
/// yields an empty page. Each opened file's cold suffix is released from
/// the page cache once the page is materialized.
pub(super) fn paginate(
    candidates: &[SfstCandidate],
    query: &LogsQuery,
    anchor: Option<Cursor>,
) -> Page {
    // Map every candidate first; readers borrow the mappings, so the
    // mappings must be fully in place before any reader opens (a later
    // push could reallocate and invalidate an earlier borrow).
    let mut mappings: Vec<(Mmap, &Path, u64)> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if let Some(mapping) = mmap::map_file(&candidate.path) {
            mappings.push((mapping, candidate.path.as_path(), candidate.seq));
        }
    }

    // Open a reader per mapping, remembering which mapping it borrows so we
    // can release that mapping's cold suffix afterwards.
    let mut readers: Vec<sfst::IndexReader<'_>> = Vec::new();
    let mut seqs: Vec<u64> = Vec::new();
    let mut reader_mapping: Vec<usize> = Vec::new();
    for (index, (mapping, path, seq)) in mappings.iter().enumerate() {
        match sfst::IndexReader::open(mapping) {
            Ok(reader) => {
                readers.push(reader);
                seqs.push(*seq);
                reader_mapping.push(index);
            }
            Err(e) => tracing::warn!("sfsq: failed to parse {}: {e}", path.display()),
        }
    }
    let files: Vec<(&sfst::IndexReader<'_>, u64)> =
        readers.iter().zip(seqs.iter().copied()).collect();

    // Map: one candidate shard per file, each bounded to the page size.
    // A page of `limit` rows draws at most `limit` from any one file, so
    // `limit + 1` candidates per file suffice — the `+1` lets the root
    // detect a row beyond the page (the has-more flag). This is the
    // bounded top-K: a node ships only a page-sized candidate set.
    let bound = Some(query.limit.saturating_add(1));
    let mut shards: Vec<PageShard> = Vec::with_capacity(files.len());
    for (reader, seq) in &files {
        match evaluate_page(reader, *seq, query, anchor, bound) {
            Ok(shard) => shards.push(shard),
            Err(e) => tracing::warn!("sfsq: page candidates failed: {e}"),
        }
    }

    // Reduce + finalize: choose the page, then materialize its rows. A
    // materialize failure collapses to an empty page rather than reporting
    // has-more flags with no rows behind them.
    let merged = PageShard::merge(shards, query.direction, bound);
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

    // Release each opened file's cold suffix (the mid/high field chunks
    // read for the string table and the stream batches read for
    // materialization), keeping the hot prefix resident across queries.
    // Compute the regions while the readers are alive, then drop the
    // borrows before advising the mappings.
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

#[cfg(test)]
mod tests;
