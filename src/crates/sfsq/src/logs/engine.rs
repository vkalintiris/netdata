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
    let filter = build_filter(&query.selections);
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
    let page = paginate(&candidates, &filter, grid, anchor, query.direction, query.limit);

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

// ── Step 2: pagination + materialization ─────────────────────────────

/// Open the candidate files and select one page of rows across them.
///
/// This re-opens the files step 1 already read (see the module docs):
/// step 1's shards are fully owned and dropped their readers, so the page
/// path opens the files again. The readers borrow from the byte buffers,
/// so the buffers are held alive for the duration. Files that fail to
/// read or open are logged and skipped; an empty candidate set yields an
/// empty page.
fn paginate(
    candidates: &[SfstCandidate],
    filter: &sfst::Filter,
    grid: sfst::Grid,
    anchor: Option<Cursor>,
    direction: Direction,
    limit: usize,
) -> Page {
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
    select_page(&files, filter, grid, anchor, direction, limit).unwrap_or_else(|e| {
        tracing::warn!("sfsq: page selection failed: {e}");
        Page::default()
    })
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
