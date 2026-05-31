//! Multi-file log-query engine.
//!
//! Opens every SFST in the supplied candidate set, runs
//! [`sfst::IndexReader::evaluate`] + [`sfst::IndexReader::facets`] +
//! [`sfst::IndexReader::timeline`] per file against the query's bucket
//! grid, paginates and materializes a page of rows, and merges
//! everything into a single [`LogsData`].
//!
//! The caller supplies a fully-specified [`LogsQuery`] — including the
//! histogram [`grid`](LogsQuery::grid), whose span is the query window —
//! selects the candidates whose range overlaps that window, then hands
//! both to [`run`]. `run` is pure and synchronous — no I/O scheduling, no
//! locks, and no window/geometry policy (deciding the grid is the
//! caller's job) — but since it reads and decompresses files the caller
//! is expected to invoke it off any async runtime thread.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use super::cursor::Cursor;
use super::merge::{merge_facet_results, merge_timelines, union_field_tables};
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

/// Run the merged query over the candidate files.
///
/// Opens every SFST candidate, runs the three per-file queries against
/// the query's [`grid`](LogsQuery::grid), merges the per-file results,
/// and assembles the [`LogsData`]. The grid's span is the window every
/// count and the materialized page clip to.
///
/// Per-file errors (corrupt file, missing field, etc.) are logged and
/// that file is skipped — other files still contribute. An empty
/// candidate set (or one where every file fails to open) yields an empty
/// `LogsData` aligned to the grid.
///
/// Pure sync — no I/O scheduling, no locks, no geometry policy — but
/// since it reads and decompresses files the caller is expected to invoke
/// it off any async runtime thread.
pub fn run(candidates: Vec<SfstCandidate>, query: LogsQuery) -> LogsData {
    let grid = query.grid;
    let filter = build_filter(&query.selections);
    let histogram_field = pick_histogram_field(query.histogram_field.as_deref());

    // Read every candidate's bytes, pairing each buffer with its path
    // and file `seq`. The `IndexReader` borrows from the bytes, so the
    // owned buffers must outlive the readers — we hold them in `opened`
    // for the duration of the per-file work. Files that fail to read
    // are logged and skipped.
    let mut opened: Vec<(Vec<u8>, &PathBuf, u64)> = Vec::with_capacity(candidates.len());
    for c in &candidates {
        match std::fs::read(&c.path) {
            Ok(bytes) => opened.push((bytes, &c.path, c.seq)),
            Err(e) => tracing::warn!("sfsq: failed to read {}: {e}", c.path.display()),
        }
    }

    // Open readers + collect field tables; skip on open failure. The
    // reader, its path, its `seq`, and its field table travel together
    // by position across the parallel vecs.
    let mut readers: Vec<sfst::IndexReader<'_>> = Vec::new();
    let mut reader_paths: Vec<&PathBuf> = Vec::new();
    let mut reader_seqs: Vec<u64> = Vec::new();
    let mut field_tables: Vec<sfst::FieldTable> = Vec::new();
    for (bytes, path, seq) in &opened {
        match sfst::IndexReader::open(bytes) {
            Ok(reader) => {
                field_tables.push(reader.field_table().clone());
                reader_paths.push(path);
                reader_seqs.push(*seq);
                readers.push(reader);
            }
            Err(e) => {
                tracing::warn!("sfsq: failed to open {}: {e}", path.display());
            }
        }
    }

    if readers.is_empty() {
        return LogsData {
            matched: 0,
            facets: Vec::new(),
            histogram_field,
            histogram: empty_timeline(grid),
            available_fields: sfst::FieldTable::default(),
            columns: Vec::new(),
            rows: Vec::new(),
            has_newer: false,
            has_older: false,
        };
    }

    // Picked facet field set against the unioned table — gives a
    // consumer a consistent facet sidebar across files.
    let union_table = union_field_tables(&field_tables);
    let facet_fields = pick_facet_fields(&query.facet_fields, &union_table);

    // Every per-file query is bounded by the same grid, so matched,
    // facets, and the histogram describe the same set of logs and agree.
    let mut matched_total: u64 = 0;
    let mut per_file_facets: Vec<Vec<sfst::FacetResult>> = Vec::new();
    let mut per_file_timelines: Vec<sfst::Timeline> = Vec::new();

    for (reader, path) in readers.iter().zip(reader_paths.iter()) {
        // matched: filter-matching logs restricted to the grid window.
        match per_file_matched(reader, &filter, grid) {
            Ok(count) => matched_total += count,
            Err(e) => tracing::warn!("sfsq: matched count failed for {}: {e}", path.display()),
        }

        // Facets: filter the picked set to fields that exist in this
        // file. Unknown fields would make `facets()` error and cost us
        // the whole file.
        let file_facet_fields: Vec<String> = facet_fields
            .iter()
            .filter(|name| reader.field_table().contains(name))
            .cloned()
            .collect();
        match reader.facets(&file_facet_fields, &filter, grid.range_ns()) {
            Ok(facets) => per_file_facets.push(facets),
            Err(e) => tracing::warn!("sfsq: facets failed for {}: {e}", path.display()),
        }

        // Histogram: every file contributes a timeline on the shared
        // grid. A file that lacks the histogram field yields a
        // dimensionless timeline whose matching logs all land in
        // `unset`, so the merged histogram total stays equal to
        // `matched`. (`timeline` only errors here if the picked field
        // is high-card, which `available_fields` never offers.)
        match reader.timeline(&histogram_field, &filter, grid) {
            Ok(timeline) => per_file_timelines.push(timeline),
            Err(e) => tracing::warn!("sfsq: timeline failed for {}: {e}", path.display()),
        }
    }

    let merged_facets = merge_facet_results(per_file_facets);

    // If no file contributed a timeline (histogram field absent
    // everywhere, or all timelines errored), synthesize an empty one
    // aligned to the grid so the shape stays valid.
    let merged_timeline =
        merge_timelines(per_file_timelines).unwrap_or_else(|| empty_timeline(grid));

    // The row-table column schema is the union of every candidate file's
    // field names — all tiers, so high-card attributes still get a
    // column — sorted for a stable schema.
    let mut field_set: BTreeSet<String> = BTreeSet::new();
    for field_table in &field_tables {
        field_set.extend(field_table.names().map(str::to_owned));
    }
    let columns: Vec<String> = field_set.into_iter().collect();

    let files: Vec<(&sfst::IndexReader<'_>, u64)> =
        readers.iter().zip(reader_seqs.iter().copied()).collect();
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
    let page = select_page(&files, &filter, grid, anchor, query.direction, query.limit)
        .unwrap_or_else(|e| {
            tracing::warn!("sfsq: page selection failed: {e}");
            Page::default()
        });

    LogsData {
        matched: matched_total as usize,
        facets: merged_facets,
        histogram_field,
        histogram: merged_timeline,
        available_fields: union_table,
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

/// A page of materialized log rows plus the has-more flags a consumer
/// uses to gate infinite scroll in each direction.
#[derive(Default)]
struct Page {
    /// Rows newest-first (`rows[0]` is the newest).
    rows: Vec<(Cursor, sfst::MaterializedRow)>,
    /// An older row exists beyond the page.
    has_older: bool,
    /// A newer row exists beyond the page.
    has_newer: bool,
}

/// Select one page of log rows across all candidate files.
///
/// Gathers every window-matching position from each file, tags it with
/// its [`Cursor`] `(timestamp_ns, file_seq, position)`, sorts by that
/// total order, then slices the page relative to `anchor` (exclusive)
/// and `direction`. Only the page's positions are materialized.
///
/// `anchor` is the boundary row from the previous page; `None` starts
/// at the newest edge (backward) or oldest edge (forward). The page is
/// returned newest-first regardless of direction.
///
/// Correctness-first: this sorts all window matches rather than seeking
/// per file. The expensive work (row materialization) is bounded to the
/// page; the sort is O(window-matches), comparable to the facet/matched
/// scan already performed. A seek-based variant that avoids the full
/// sort is a later optimization.
fn select_page(
    files: &[(&sfst::IndexReader<'_>, u64)],
    filter: &sfst::Filter,
    grid: sfst::Grid,
    anchor: Option<Cursor>,
    direction: Direction,
    limit: usize,
) -> Result<Page, sfst::Error> {
    let window_ns = grid.range_ns();
    // 1. Gather (cursor, file_index, position) for every window match.
    let mut matches: Vec<(Cursor, usize, u32)> = Vec::new();
    for (file_index, (reader, seq)) in files.iter().enumerate() {
        let matched = reader.evaluate(filter)? & &reader.range_bitmap(window_ns.clone())?;
        if matched.is_empty() {
            continue;
        }
        let timestamps = reader.load_timestamps()?;
        for position in matched.iter() {
            let timestamp_ns = timestamps.get(position as usize).copied().unwrap_or(0);
            matches.push((
                Cursor {
                    timestamp_ns,
                    file_seq: *seq,
                    position,
                },
                file_index,
                position,
            ));
        }
    }
    matches.sort_by_key(|(c, _, _)| *c);
    let len = matches.len();

    // 2. Slice the page. `matches` is ascending (oldest→newest); the anchor
    //    comparison is exclusive so the boundary row never repeats.
    let (lo, hi) = match direction {
        Direction::Backward => {
            let hi = match anchor {
                Some(a) => matches.partition_point(|(c, _, _)| *c < a),
                None => len,
            };
            (hi.saturating_sub(limit), hi)
        }
        Direction::Forward => {
            let lo = match anchor {
                Some(a) => matches.partition_point(|(c, _, _)| *c <= a),
                None => 0,
            };
            (lo, (lo + limit).min(len))
        }
    };
    let has_older = lo > 0;
    let has_newer = hi < len;
    let page = &matches[lo..hi];

    // 3. Materialize, batching positions per file so each file's chunks
    //    decompress once. Reassemble newest-first.
    let mut per_file: HashMap<usize, Vec<u32>> = HashMap::new();
    for (_, file_index, position) in page {
        per_file.entry(*file_index).or_default().push(*position);
    }
    let mut by_pos: HashMap<(usize, u32), sfst::MaterializedRow> = HashMap::new();
    for (file_index, positions) in &per_file {
        let rows = files[*file_index].0.materialize_rows(positions)?;
        for (position, row) in positions.iter().zip(rows) {
            by_pos.insert((*file_index, *position), row);
        }
    }
    let rows = page
        .iter()
        .rev()
        .filter_map(|(cursor, file_index, position)| {
            by_pos
                .remove(&(*file_index, *position))
                .map(|row| (*cursor, row))
        })
        .collect();

    Ok(Page {
        rows,
        has_older,
        has_newer,
    })
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
