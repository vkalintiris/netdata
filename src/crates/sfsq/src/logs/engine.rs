//! Multi-file log-query engine.
//!
//! Satisfying a log query is two discrete steps, and the engine exposes
//! each so they can run apart:
//!
//! 1. **Statistics** — matched count, facets, histogram, field set. This
//!    step is an aggregatable monoid: [`evaluate`] turns one candidate
//!    file into a [`LogsShard`], and [`LogsShard::merge`] folds many
//!    shards into one. The fold is associative, so a child can merge the
//!    files it owns and a parent can merge the children's shards with the
//!    same function — the basis for fanning the query out across nodes
//!    without opening every file in one place.
//! 2. **Materialization** — selecting and decompressing the page of rows
//!    to return. This needs a global order across files, so it isn't a
//!    plain fold; it lives in the pagination path.
//!
//! [`run`] is the all-in-one convenience for the local case: it evaluates
//! every candidate, merges the shards, paginates, and assembles a single
//! [`LogsData`]. (It opens each file once for step 1 and again for step
//! 2; the re-open is deliberate — step 1's shards are fully owned and
//! drop their readers, and the heavy work is the bounded page
//! materialization, not the re-read.)
//!
//! The caller supplies a fully-specified [`LogsQuery`] — including the
//! histogram [`grid`](LogsQuery::grid), whose span is the query window —
//! and selects the candidates whose range overlaps that window. The work
//! is pure and synchronous — no I/O scheduling, no locks, no
//! window/geometry policy — but since it reads and decompresses files the
//! caller is expected to invoke it off any async runtime thread.

use std::collections::HashMap;
use std::path::Path;

use super::cursor::Cursor;
use super::merge::{merge_facet_results, merge_field_tables, merge_timelines};
use super::query::{Anchor, Direction, LogsQuery};
use super::result::LogsData;

/// Default histogram dimension when the query doesn't specify one.
/// Always `severity_text` — it's the OTel canonical log-level field,
/// and what makes a meaningful chart is the producer's responsibility
/// (set it, populate it with varied values). The consumer exposes the
/// full `available_fields` list for users to pick something else.
const DEFAULT_HISTOGRAM_FIELD: &str = "severity_text";

/// Default facet field when the query doesn't specify any. A consumer's
/// first-load request typically carries an empty facet list, so we can't
/// infer which fields the user cares about; rather than auto-curate a
/// set (which can't be done well across multiple SFSTs — a field's
/// cardinality composes unpredictably across files), we surface only
/// this one. Always `severity_text` — the OTel canonical log-level
/// field, same rationale as [`DEFAULT_HISTOGRAM_FIELD`]. Users add more
/// via an explicit `facet_fields`.
const DEFAULT_FACET_FIELD: &str = "severity_text";

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

// ── Step 1: statistics (aggregatable) ────────────────────────────────

/// One file's (or one node's) contribution to a query's statistics:
/// matched count, facets, histogram, and field table — everything in
/// step 1, with no materialized rows.
///
/// A shard is the unit of delegated work. [`evaluate`] produces one from
/// a single file; [`LogsShard::merge`] folds many into one. Because the
/// fold is an associative monoid, a node can merge the files it owns into
/// a single shard and a parent can merge those node-shards the same way —
/// the result is identical to merging every file at once.
#[derive(Debug, Default)]
pub struct LogsShard {
    /// Filter-matching logs within the window, summed across the shard.
    pub matched: u64,
    /// Per-field facet counts (unmerged across files until [`merge`]).
    ///
    /// [`merge`]: LogsShard::merge
    pub facets: Vec<sfst::FacetResult>,
    /// The histogram on the query grid, or `None` if this shard
    /// contributed none (histogram field high-card here, or the timeline
    /// errored). Merging keeps it `None` only when *no* shard had one.
    pub timeline: Option<sfst::Timeline>,
    /// The field table, all tiers kept and the tier bumped to `High` if
    /// high-card anywhere in the shard (see [`merge_field_tables`]).
    pub fields: sfst::FieldTable,
}

impl LogsShard {
    /// Fold per-file (or per-node) shards into one.
    ///
    /// `matched` sums; facets and timelines combine via the cross-file
    /// merge helpers; field tables merge associatively. Facets for a
    /// field that is high-card in *any* shard are dropped here — each
    /// shard's [`evaluate`] already skips a field high-card in its own
    /// file, and this completes the rule across shards so the facet set
    /// stays consistent with the offerable `available_fields`. The merged
    /// `timeline` is `None` only when no shard contributed one.
    ///
    /// The fold is associative and has an identity (the
    /// [`Default`](LogsShard::default) shard), so it is safe to apply at
    /// every level of a fan-out.
    pub fn merge(shards: Vec<LogsShard>) -> LogsShard {
        let mut matched: u64 = 0;
        let mut field_tables: Vec<sfst::FieldTable> = Vec::with_capacity(shards.len());
        let mut per_shard_facets: Vec<Vec<sfst::FacetResult>> = Vec::with_capacity(shards.len());
        let mut timelines: Vec<sfst::Timeline> = Vec::new();

        for shard in shards {
            matched += shard.matched;
            field_tables.push(shard.fields);
            per_shard_facets.push(shard.facets);
            if let Some(timeline) = shard.timeline {
                timelines.push(timeline);
            }
        }

        let fields = merge_field_tables(&field_tables);
        let facets = merge_facet_results(per_shard_facets)
            .into_iter()
            .filter(|facet| {
                !fields
                    .get(facet.field.as_str())
                    .is_some_and(|entry| entry.is_high_card())
            })
            .collect();
        let timeline = merge_timelines(timelines);

        LogsShard {
            matched,
            facets,
            timeline,
            fields,
        }
    }
}

/// Evaluate one candidate file into a [`LogsShard`] — step 1 for a single
/// file. Opens the file, computes the matched count, facets, histogram,
/// and field table against the query's [`grid`](LogsQuery::grid), and
/// returns a fully-owned shard (the reader is dropped before returning).
///
/// Any failure — unreadable/corrupt file, a per-computation error — is
/// logged and degrades that part to empty (an empty shard if the file
/// can't be opened), so one bad file never sinks the others when its
/// shard is merged.
///
/// Facets are picked against *this file's* table, so a field that's
/// high-card here is skipped; a field high-card in some *other* file is
/// dropped later, in [`LogsShard::merge`].
pub fn evaluate(candidate: &SfstCandidate, query: &LogsQuery) -> LogsShard {
    let bytes = match std::fs::read(&candidate.path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!("sfsq: failed to read {}: {e}", candidate.path.display());
            return LogsShard::default();
        }
    };
    let reader = match sfst::IndexReader::open(&bytes) {
        Ok(reader) => reader,
        Err(e) => {
            tracing::warn!("sfsq: failed to open {}: {e}", candidate.path.display());
            return LogsShard::default();
        }
    };

    let grid = query.grid;
    let filter = build_filter(&query.selections);
    let fields = reader.field_table().clone();

    // matched: filter-matching logs restricted to the grid window.
    let matched = match per_file_matched(&reader, &filter, grid) {
        Ok(count) => count,
        Err(e) => {
            tracing::warn!(
                "sfsq: matched count failed for {}: {e}",
                candidate.path.display()
            );
            0
        }
    };

    // Facets: pick the requested set against this file's table (skipping
    // a field high-card *here*), then keep only fields actually present —
    // an unknown field would make `facets()` error and cost the whole
    // file.
    let facet_fields: Vec<String> = pick_facet_fields(&query.facet_fields, &fields)
        .into_iter()
        .filter(|name| fields.contains(name))
        .collect();
    let facets = match reader.facets(&facet_fields, &filter, grid.range_ns()) {
        Ok(facets) => facets,
        Err(e) => {
            tracing::warn!("sfsq: facets failed for {}: {e}", candidate.path.display());
            Vec::new()
        }
    };

    // Histogram: a file lacking the field yields a dimensionless timeline
    // whose matching logs all land in `unset`; only a high-card field
    // errors, in which case the file contributes no timeline.
    let histogram_field = pick_histogram_field(query.histogram_field.as_deref());
    let timeline = match reader.timeline(&histogram_field, &filter, grid) {
        Ok(timeline) => Some(timeline),
        Err(e) => {
            tracing::warn!("sfsq: timeline failed for {}: {e}", candidate.path.display());
            None
        }
    };

    LogsShard {
        matched,
        facets,
        timeline,
        fields,
    }
}

// ── Composition: the all-in-one local query ──────────────────────────

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
    let stats = LogsShard::merge(candidates.iter().map(|c| evaluate(c, &query)).collect());

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
    let histogram_field = pick_histogram_field(query.histogram_field.as_deref());

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
        buckets: vec![Vec::new(); grid.num_buckets],
        unset: vec![0u64; grid.num_buckets],
    }
}

// ── Step 2: pagination + materialization ─────────────────────────────
//
// Returning a page is select-then-fetch, not a fold: it needs a global
// order over cursors `(timestamp_ns, file_seq, position)` across files.
// It decomposes into seams a cross-node fan-out reuses:
//
// - `evaluate_page` (map: one file -> its page candidates),
// - `PageShard::merge` (reduce: combine candidate sets — associative),
// - `finalize_page` (root: pick the page + the has-more flags),
// - `materialize` (fetch: chosen cursors -> row bodies).
//
// `paginate` is the local orchestration of all four; a distributed parent
// would run `merge`/`finalize_page` on candidate sets received from
// children and route `materialize` back to the file's owning node by
// `file_seq`.

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
    /// [`merge`](PageShard::merge) and `finalize_page` take a prefix of.
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
struct Page {
    rows: Vec<(Cursor, sfst::MaterializedRow)>,
    has_newer: bool,
    has_older: bool,
}

/// Open the candidate files and select + materialize one page across them.
///
/// This re-opens the files step 1 already read (see the module docs):
/// step 1's shards are fully owned and dropped their readers, so the page
/// path opens the files again. The readers borrow from the byte buffers,
/// so the buffers are held alive for the duration. Files that fail to
/// read, open, or evaluate are logged and skipped; an empty candidate set
/// yields an empty page.
fn paginate(candidates: &[SfstCandidate], query: &LogsQuery, anchor: Option<Cursor>) -> Page {
    let mut buffers: Vec<(Vec<u8>, &Path, u64)> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        match std::fs::read(&candidate.path) {
            Ok(bytes) => buffers.push((bytes, candidate.path.as_path(), candidate.seq)),
            Err(e) => tracing::warn!("sfsq: failed to read {}: {e}", candidate.path.display()),
        }
    }

    let mut readers: Vec<sfst::IndexReader<'_>> = Vec::new();
    let mut seqs: Vec<u64> = Vec::new();
    for (bytes, path, seq) in &buffers {
        match sfst::IndexReader::open(bytes) {
            Ok(reader) => {
                readers.push(reader);
                seqs.push(*seq);
            }
            Err(e) => tracing::warn!("sfsq: failed to open {}: {e}", path.display()),
        }
    }
    let files: Vec<(&sfst::IndexReader<'_>, u64)> =
        readers.iter().zip(seqs.iter().copied()).collect();

    // Map: one candidate shard per file. Unbounded for now (`None`); a
    // later step bounds each shard to the page size.
    let mut shards: Vec<PageShard> = Vec::with_capacity(files.len());
    for (reader, seq) in &files {
        match evaluate_page(reader, *seq, query, anchor, None) {
            Ok(shard) => shards.push(shard),
            Err(e) => tracing::warn!("sfsq: page candidates failed: {e}"),
        }
    }

    // Reduce + finalize: choose the page, then materialize its rows. A
    // materialize failure collapses to an empty page rather than reporting
    // has-more flags with no rows behind them.
    let merged = PageShard::merge(shards, query.direction, None);
    let selected = finalize_page(merged, query.direction, query.limit);
    match materialize(&files, &selected) {
        Ok(rows) => Page {
            rows,
            has_newer: selected.has_newer,
            has_older: selected.has_older,
        },
        Err(e) => {
            tracing::warn!("sfsq: materialize failed: {e}");
            Page::default()
        }
    }
}

/// Per-file matched count: filter-matching logs restricted to the grid's
/// window. `evaluate` returns positions across the file's full range;
/// intersect with the window range bitmap (the same primitive `facets`
/// uses) to clip outside-window logs.
fn per_file_matched(
    reader: &sfst::IndexReader<'_>,
    filter: &sfst::Filter,
    grid: sfst::Grid,
) -> Result<u64, sfst::Error> {
    let bm = reader.evaluate(filter)?;
    let range = reader.range_bitmap(grid.range_ns())?;
    Ok((bm & &range).len())
}

/// Translate the query's `selections` map into an [`sfst::Filter`].
/// Same shape, just a constructor walk: OR within field, AND across
/// fields.
fn build_filter(selections: &HashMap<String, Vec<String>>) -> sfst::Filter {
    let mut filter = sfst::Filter::new();
    for (field, values) in selections {
        for value in values {
            filter = filter.select(field.clone(), value.clone());
        }
    }
    filter
}

/// Pick the histogram field. Honors the query's `histogram_field` when
/// set; otherwise returns [`DEFAULT_HISTOGRAM_FIELD`]. No eligibility
/// filtering — if the chosen field isn't in a given SFST or is
/// high-cardinality, `sfst::timeline` surfaces that as an error and the
/// file is skipped. A consumer can steer the user toward a different
/// field via [`LogsData::available_fields`].
fn pick_histogram_field(requested: Option<&str>) -> String {
    requested.unwrap_or(DEFAULT_HISTOGRAM_FIELD).to_string()
}

/// Pick the facet field set. With no explicit request, return just
/// [`DEFAULT_FACET_FIELD`]; we don't try to auto-curate a wider set (see
/// that constant). Explicit `requested` fields are honored as-is, modulo
/// high-card / unknown fields (those would error or surface no options).
fn pick_facet_fields(requested: &[String], fields: &sfst::FieldTable) -> Vec<String> {
    if requested.is_empty() {
        return vec![DEFAULT_FACET_FIELD.to_string()];
    }
    requested
        .iter()
        .filter(|name| fields.get(name).is_some_and(|f| !f.is_high_card()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
