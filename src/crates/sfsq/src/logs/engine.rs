//! Multi-file otel-logs query engine.
//!
//! Opens every overlapping SFST across the supplied candidates, runs
//! [`sfst::IndexReader::evaluate`] + [`sfst::IndexReader::facets`] +
//! [`sfst::IndexReader::timeline`] per file against a shared
//! request-aligned bucket grid, paginates + materializes a page of
//! rows, and merges everything into a single [`LogsResult`].
//!
//! Pure and synchronous — no I/O scheduling, no locks. The caller
//! (the ledger's `FunctionHandler`) resolves the window via
//! [`effective_window`], selects candidates, and wraps [`run`] in
//! `spawn_blocking`.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use super::adapter::{
    NS_PER_S, available_histograms_from_fields, facet_from_sfst, histogram_from_sfst,
    merge_facet_results, merge_timelines, union_field_tables,
};
use super::cursor::Cursor;
use super::types::{ACCEPTED_PARAMS, AnchorParam, Direction, LogsRequest};
use super::wire::{Items, LogsResult, Pagination, Version};

/// Default histogram dimension when the request doesn't specify one.
/// Always `severity_text` — it's the OTel canonical log-level field,
/// and what makes a meaningful chart is the producer's responsibility
/// (set it, populate it with varied values). The UI exposes the full
/// `available_histograms` list for users to pick something else.
const DEFAULT_HISTOGRAM_FIELD: &str = "severity_text";

/// Aim for at least this many time buckets across the request
/// window when picking from [`VALID_BUCKET_WIDTHS_S`]. With the
/// curated widths and a 15-minute window this yields 15-second
/// buckets (60 of them).
const TARGET_BUCKETS: u32 = 60;

/// "Nice" bucket widths in seconds. Ported from the legacy
/// systemd-journal plugin's `calculate_bucket_duration` to keep
/// histograms anchored to wall-clock-friendly intervals (1s, 2s,
/// 5s, 10s, 15s, 30s, 1m, 5m, …). [`bucket_width_for_span_s`] picks
/// the largest entry that produces at least [`TARGET_BUCKETS`]
/// buckets across the span, so the chart density is stable as the
/// requested window scales.
const VALID_BUCKET_WIDTHS_S: &[u32] = &[
    1, 2, 5, 10, 15, 30, // seconds
    60, 120, 180, 300, 600, 900, 1800, // minutes
    3600, 7200, 21600, 28800, 43200, // hours
    86400, 172800, 259200, 432000, 604800, 1209600, 2592000, // days
];

/// Default request window in seconds when the caller doesn't specify
/// `after`/`before`. Matches the cloud-frontend default time range.
const DEFAULT_WINDOW_SECS: u32 = 15 * 60;

/// Default facet field when the request doesn't specify any. The UI
/// sends an empty facet list on first load, so we can't infer which
/// fields the user cares about; rather than auto-curate a set (which
/// can't be done well across multiple SFSTs — a field's cardinality
/// composes unpredictably across files), we surface only this one.
/// Always `severity_text` — the OTel canonical log-level field, same
/// rationale as [`DEFAULT_HISTOGRAM_FIELD`]. Users add more via the
/// UI's "+ Add Filter Field" control, which sends explicit
/// `req.facets`.
const DEFAULT_FACET_FIELD: &str = "severity_text";

/// A query candidate: an SFST file whose range overlaps the request
/// window. Owned so the caller can drop the registry lock before I/O.
/// `seq` is the file's monotonic per-file id, used as the cross-file
/// tiebreaker in the pagination cursor's total order.
pub struct SfstCandidate {
    pub summary: sfst::Summary,
    pub seq: u64,
    pub path: std::path::PathBuf,
}

/// Resolve the bucket grid for the (effective) request window and run
/// the merged query. `req.after`/`req.before` are assumed already
/// defaulted by [`effective_window`]; this snaps them outward to a
/// "nice" bucket width, builds the shared grid, and delegates to
/// [`build_merged_logs_response`]. An empty candidate set yields the
/// empty envelope aligned to that window.
///
/// Pure sync — the caller wraps it in `spawn_blocking`.
pub fn run(candidates: Vec<SfstCandidate>, mut req: LogsRequest) -> LogsResult {
    // Snap the window outward to multiples of a "nice" bucket width.
    // This stabilises the histogram x-axis across the UI's per-second
    // polling: successive polls within the same bucket-width slot all
    // see identical `[after, before)`, so the chart only shifts when
    // crossing a real boundary.
    let span_s = req.before.saturating_sub(req.after);
    let bucket_width_s = bucket_width_for_span_s(span_s);
    (req.after, req.before) = align_window(req.after, req.before, bucket_width_s);

    if candidates.is_empty() {
        return LogsResult::empty_stub(req.after, req.before, req.last);
    }

    // Bucket grid derived directly from the aligned window —
    // `bucket_width_s` divides `(before - after)` exactly by
    // construction, so no `div_ceil` is needed. All per-file timelines
    // share this grid and are directly mergeable.
    let grid = sfst::Grid::new(
        (req.after as i64) * NS_PER_S,
        (bucket_width_s as i64) * NS_PER_S,
        ((req.before - req.after) / bucket_width_s) as usize,
    );
    build_merged_logs_response(candidates, req, grid)
}

/// Open every SFST candidate, run the three queries per file against
/// a shared request-aligned bucket grid, merge the per-file results,
/// and assemble the wire envelope. Pure sync — no awaits — so the
/// caller wraps this in `tokio::task::spawn_blocking`.
///
/// Per-file errors (corrupt file, missing field, etc.) are logged
/// and that file is skipped — other files still contribute to the
/// response. If *every* file errors we fall through to an empty
/// stub.
fn build_merged_logs_response(
    candidates: Vec<SfstCandidate>,
    req: LogsRequest,
    grid: sfst::Grid,
) -> LogsResult {
    let filter = build_filter(&req.selections);
    let histogram_field = pick_histogram_field(&req.histogram);

    // Read every candidate's bytes, pairing each buffer with its path
    // and file `seq`. The `IndexReader` borrows from the bytes, so the
    // owned buffers must outlive the readers — we hold them in `opened`
    // for the duration of the per-file work. Files that fail to read
    // are logged and skipped.
    let mut opened: Vec<(Vec<u8>, &PathBuf, u64)> = Vec::with_capacity(candidates.len());
    for c in &candidates {
        match std::fs::read(&c.path) {
            Ok(bytes) => opened.push((bytes, &c.path, c.seq)),
            Err(e) => tracing::warn!("otel-logs: failed to read {}: {e}", c.path.display()),
        }
    }

    // Open readers + collect field tables; skip on open failure. The
    // reader, its path, its `seq`, and its field table travel together
    // by position across the parallel vecs.
    let mut readers: Vec<sfst::IndexReader<'_>> = Vec::new();
    let mut reader_paths: Vec<&PathBuf> = Vec::new();
    let mut reader_seqs: Vec<u64> = Vec::new();
    let mut field_tables: Vec<Vec<sfst::FieldEntry>> = Vec::new();
    for (bytes, path, seq) in &opened {
        match sfst::IndexReader::open(bytes) {
            Ok(reader) => {
                field_tables.push(reader.field_table().to_vec());
                reader_paths.push(path);
                reader_seqs.push(*seq);
                readers.push(reader);
            }
            Err(e) => {
                tracing::warn!("otel-logs: failed to open {}: {e}", path.display());
            }
        }
    }

    if readers.is_empty() {
        return LogsResult::empty_stub(req.after, req.before, req.last);
    }

    // Picked facet field set against the unioned table — gives the
    // UI a consistent sidebar across files.
    let table_refs: Vec<&[sfst::FieldEntry]> = field_tables.iter().map(|t| t.as_slice()).collect();
    let unioned = union_field_tables(&table_refs);
    let facet_fields = pick_facet_fields(&req.facets, &unioned);

    // The request window in ns. Every per-file query — matched,
    // facets, and the histogram grid — clips to this same window, so
    // their counts describe the same set of logs and agree.
    let window_ns = (req.after as i64) * NS_PER_S..(req.before as i64) * NS_PER_S;

    let mut matched_total: u64 = 0;
    let mut per_file_facets: Vec<Vec<sfst::FacetResult>> = Vec::new();
    let mut per_file_timelines: Vec<sfst::Timeline> = Vec::new();

    for (reader, path) in readers.iter().zip(reader_paths.iter()) {
        // matched: filter-matching logs restricted to the request
        // window.
        match per_file_matched(reader, &filter, window_ns.clone()) {
            Ok(m) => matched_total += m,
            Err(e) => tracing::warn!(
                "otel-logs: matched count failed for {}: {e}",
                path.display()
            ),
        }

        // Facets: filter the picked set to fields that exist in
        // this file. Unknown fields would make `facets()` error and
        // cost us the whole file.
        let file_facet_fields: Vec<String> = facet_fields
            .iter()
            .filter(|name| reader.field_table().iter().any(|f| f.name == **name))
            .cloned()
            .collect();
        match reader.facets(&file_facet_fields, &filter, window_ns.clone()) {
            Ok(facets) => per_file_facets.push(facets),
            Err(e) => tracing::warn!("otel-logs: facets failed for {}: {e}", path.display()),
        }

        // Histogram: every file contributes a timeline on the shared
        // grid. A file that lacks the histogram field yields a
        // dimensionless timeline whose matching logs all land in
        // `unset`, so the merged histogram total stays equal to
        // `matched`. (`timeline` only errors here if the picked field
        // is high-card, which `available_histograms` never offers.)
        match reader.timeline(&histogram_field, &filter, grid) {
            Ok(t) => per_file_timelines.push(t),
            Err(e) => tracing::warn!("otel-logs: timeline failed for {}: {e}", path.display()),
        }
    }

    let merged_facets = merge_facet_results(per_file_facets);
    let wire_facets = merged_facets
        .iter()
        .enumerate()
        .map(|(i, f)| facet_from_sfst(i, f))
        .collect();

    // If no file contributed a timeline (histogram field absent
    // everywhere, or all timelines errored), synthesize an empty one
    // aligned to the grid so the wire shape stays valid.
    let merged_timeline = merge_timelines(per_file_timelines).unwrap_or_else(|| sfst::Timeline {
        grid,
        dimensions: Vec::new(),
        buckets: vec![Vec::new(); grid.num_buckets],
        unset: vec![0u64; grid.num_buckets],
    });
    let wire_histogram = histogram_from_sfst(&histogram_field, &merged_timeline);
    let wire_available = available_histograms_from_fields(&unioned);

    // Materialize the page of log rows. The column schema is the union
    // of every candidate file's field names — all tiers, so high-card
    // attributes still get a column — sorted for a stable schema.
    let mut field_set: BTreeSet<String> = BTreeSet::new();
    for t in &field_tables {
        for f in t {
            field_set.insert(f.name.clone());
        }
    }
    let column_fields: Vec<String> = field_set.into_iter().collect();
    // Facet-eligible fields (low/mid-card) carry `filter: "facet"` so
    // the UI's "+ Add Filter Field" picker offers them. High-card
    // fields remain columns but aren't facetable — `facets()` rejects
    // them, so offering one would yield an empty/erroring facet.
    let facetable: BTreeSet<&str> = unioned.iter().map(|f| f.name.as_str()).collect();

    let files: Vec<(&sfst::IndexReader<'_>, u64)> =
        readers.iter().zip(reader_seqs.iter().copied()).collect();
    // Resolve the anchor to a cursor in the global total order. A row
    // cursor decodes directly; a histogram-click timestamp becomes a
    // synthetic cursor at the end of that microsecond (file_seq/position
    // maxed), so a backward page shows the newest rows up to that time.
    let anchor = req.anchor.as_ref().and_then(|a| match a {
        AnchorParam::Cursor(s) => Cursor::decode(s),
        AnchorParam::TimestampUs(us) => Some(Cursor {
            timestamp_ns: (*us as i64).saturating_mul(1_000),
            file_seq: u64::MAX,
            position: u32::MAX,
        }),
    });
    let page = select_page(&files, &filter, window_ns, anchor, req.direction, req.last)
        .unwrap_or_else(|e| {
            tracing::warn!("otel-logs: page selection failed: {e}");
            Page::default()
        });
    let (columns, data) = build_table(&page, &column_fields, &facetable);

    let matched = matched_total as usize;

    LogsResult {
        progress: 100,
        version: Version::default(),
        accepted_params: ACCEPTED_PARAMS.to_vec(),
        required_params: Vec::new(),
        facets: wire_facets,
        available_histograms: wire_available,
        histogram: wire_histogram,
        columns,
        data,
        default_charts: Vec::new(),
        items: Items {
            evaluated: matched,
            unsampled: 0,
            estimated: matched,
            matched,
            // before ⇒ newer rows exist (UI "scroll up"); after ⇒
            // older rows exist (UI "scroll down").
            before: page.has_newer as usize,
            after: page.has_older as usize,
            returned: page.rows.len(),
            max_to_return: req.last,
        },
        show_ids: false,
        has_history: true,
        status: 200,
        response_type: String::from("table"),
        help: String::from("Query and visualize OpenTelemetry logs."),
        pagination: Pagination::default(),
    }
}

/// A page of materialized log rows plus the has-more flags the UI
/// uses to gate infinite scroll in each direction.
#[derive(Default)]
struct Page {
    /// Rows newest-first (`rows[0]` is the newest), as the UI expects.
    rows: Vec<(Cursor, sfst::MaterializedRow)>,
    /// An older row exists beyond the page (UI `items.after`).
    has_older: bool,
    /// A newer row exists beyond the page (UI `items.before`).
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
    window_ns: std::ops::Range<i64>,
    anchor: Option<Cursor>,
    direction: Direction,
    limit: usize,
) -> Result<Page, sfst::Error> {
    // 1. Gather (cursor, file_index, position) for every window match.
    let mut all: Vec<(Cursor, usize, u32)> = Vec::new();
    for (file_index, (reader, seq)) in files.iter().enumerate() {
        let matched = reader.evaluate(filter)? & &reader.range_bitmap(window_ns.clone())?;
        if matched.is_empty() {
            continue;
        }
        let timestamps = reader.load_timestamps()?;
        for position in matched.iter() {
            let timestamp_ns = timestamps.get(position as usize).copied().unwrap_or(0);
            all.push((
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
    all.sort_by_key(|(c, _, _)| *c);
    let len = all.len();

    // 2. Slice the page. `all` is ascending (oldest→newest); the anchor
    //    comparison is exclusive so the boundary row never repeats.
    let (lo, hi) = match direction {
        Direction::Backward => {
            let hi = match anchor {
                Some(a) => all.partition_point(|(c, _, _)| *c < a),
                None => len,
            };
            (hi.saturating_sub(limit), hi)
        }
        Direction::Forward => {
            let lo = match anchor {
                Some(a) => all.partition_point(|(c, _, _)| *c <= a),
                None => 0,
            };
            (lo, (lo + limit).min(len))
        }
    };
    let has_older = lo > 0;
    let has_newer = hi < len;
    let page = &all[lo..hi];

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

/// Build the wire `columns` schema and `data` rows from a page.
///
/// Columns: a visible µs `timestamp` and `severity`, a hidden string
/// `cursor` (the `pagination.column` the UI echoes as `anchor`), then
/// one hidden column per attribute field. Fields in `facetable` get
/// `filter: "facet"` so the UI's "+ Add Filter Field" picker offers
/// them; everything else is `"none"`. Each data row is a positional
/// array aligned to the column `index`; absent attributes are `null`.
fn build_table(
    page: &Page,
    fields: &[String],
    facetable: &BTreeSet<&str>,
) -> (serde_json::Value, serde_json::Value) {
    use serde_json::{Value, json};

    let mut columns = serde_json::Map::new();
    // The UI formats the cell from `valueOptions.transform`, not from
    // `type` (which only selects the cell component). Match the legacy
    // journal column: a `timestamp` cell carrying a µs value rendered
    // via the `datetime_usec` transform.
    columns.insert(
        "timestamp".into(),
        json!({ "index": 0, "id": "timestamp", "name": "Timestamp", "type": "timestamp",
                "visible": true, "sortable": false, "filter": "none",
                "valueOptions": { "transform": "datetime_usec", "decimal_points": 0 } }),
    );
    columns.insert(
        "severity".into(),
        json!({ "index": 1, "id": "severity", "name": "Severity",
                "type": "string", "visible": false, "sortable": false, "filter": "none" }),
    );
    columns.insert(
        "cursor".into(),
        json!({ "index": 2, "id": "cursor", "name": "cursor", "type": "string",
                "visible": false, "sortable": false, "filter": "none", "unique_key": true }),
    );
    for (i, name) in fields.iter().enumerate() {
        let filter = if facetable.contains(name.as_str()) {
            "facet"
        } else {
            "none"
        };
        columns.insert(
            name.clone(),
            json!({ "index": 3 + i, "id": name, "name": name, "type": "string",
                    "visible": false, "sortable": false, "filter": filter }),
        );
    }

    let data: Vec<Value> = page
        .rows
        .iter()
        .map(|(cursor, row)| {
            let lookup: HashMap<&str, &str> = row
                .fields
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let cell = |name: &str| match lookup.get(name) {
                Some(v) => json!(v),
                None => Value::Null,
            };
            let mut cells: Vec<Value> = Vec::with_capacity(3 + fields.len());
            cells.push(json!(cursor.timestamp_ns / 1_000)); // ns → µs (JS-safe)
            cells.push(cell("severity_text"));
            cells.push(json!(cursor.encode()));
            cells.extend(fields.iter().map(|f| cell(f)));
            Value::Array(cells)
        })
        .collect();

    (Value::Object(columns), Value::Array(data))
}

/// Per-file matched count: filter-matching logs restricted to the
/// request window. `evaluate` returns positions across the file's
/// full range; intersect with the file's window range bitmap (the
/// same primitive `facets` uses) to clip outside-window logs.
fn per_file_matched(
    reader: &sfst::IndexReader<'_>,
    filter: &sfst::Filter,
    window_ns: std::ops::Range<i64>,
) -> Result<u64, sfst::Error> {
    let bm = reader.evaluate(filter)?;
    let range = reader.range_bitmap(window_ns)?;
    Ok((bm & &range).len())
}

/// Translate the request's `selections` map into an [`sfst::Filter`].
/// Same shape, just a constructor walk: OR within field, AND across
/// fields (matches the UI's selection semantics).
fn build_filter(selections: &HashMap<String, Vec<String>>) -> sfst::Filter {
    let mut filter = sfst::Filter::new();
    for (field, values) in selections {
        for value in values {
            filter = filter.select(field.clone(), value.clone());
        }
    }
    filter
}

/// Pick the histogram field. Honors the request's `histogram` param
/// when set; otherwise returns [`DEFAULT_HISTOGRAM_FIELD`]. No
/// eligibility filtering — if the chosen field isn't in this SFST or
/// is high-cardinality, `sfst::timeline` will surface that as an
/// error and the handler falls back to the empty envelope. The UI
/// can then drive the user toward a different field via
/// `available_histograms`.
fn pick_histogram_field(requested: &str) -> String {
    if requested.is_empty() {
        DEFAULT_HISTOGRAM_FIELD.to_string()
    } else {
        requested.to_string()
    }
}

/// Pick the facet field set. With no explicit request — the UI's
/// first-load behavior — return just [`DEFAULT_FACET_FIELD`]; we don't
/// try to auto-curate a wider set (see that constant). Explicit
/// `requested` selections are honored as-is, modulo high-card /
/// unknown fields (those would error or surface no options).
fn pick_facet_fields(requested: &[String], fields: &[sfst::FieldEntry]) -> Vec<String> {
    if requested.is_empty() {
        return vec![DEFAULT_FACET_FIELD.to_string()];
    }
    requested
        .iter()
        .filter(|name| fields.iter().any(|f| f.name == **name && !is_high_card(f)))
        .cloned()
        .collect()
}

/// Pick a "nice" bucket width (seconds) for a given span. Walks
/// [`VALID_BUCKET_WIDTHS_S`] from largest to smallest and returns
/// the first one that produces at least [`TARGET_BUCKETS`] buckets.
/// Falls back to `1` for spans too short to satisfy the heuristic.
fn bucket_width_for_span_s(span_s: u32) -> u32 {
    VALID_BUCKET_WIDTHS_S
        .iter()
        .rev()
        .find(|&&w| span_s / w >= TARGET_BUCKETS)
        .copied()
        .unwrap_or(1)
}

/// Round `[after, before)` outward to multiples of `width_s`. The
/// returned bounds are still in `(seconds since epoch)`, but
/// `after` is floored and `before` is ceiled, so the histogram grid
/// anchors to absolute wall-clock boundaries (e.g. 15s buckets snap
/// to `t % 15 == 0`).
///
/// This is what keeps the chart x-axis stable across the UI's
/// per-second polling: requests within the same bucket-width slot
/// align to the same grid, so the chart only shifts when crossing a
/// real boundary.
fn align_window(after: u32, before: u32, width_s: u32) -> (u32, u32) {
    let aligned_after = (after / width_s) * width_s;
    let aligned_before = before.div_ceil(width_s) * width_s;
    (aligned_after, aligned_before)
}

/// Resolve a request's `[after, before)` to a usable time window.
/// Returns the inputs verbatim when they form a valid non-empty
/// range; falls back to the last [`DEFAULT_WINDOW_SECS`] computed
/// from system time otherwise — the legacy "no time bound" form
/// `(0, 0)` and any inverted / zero-width window.
///
/// A defaulted window with no overlapping data returns the empty
/// envelope just like an explicit one; the handler does not
/// second-guess the caller by reaching for the most-recent file.
pub fn effective_window(after: u32, before: u32) -> (u32, u32) {
    let malformed = (after == 0 && before == 0) || after >= before;
    if !malformed {
        return (after, before);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(u32::MAX);
    (now.saturating_sub(DEFAULT_WINDOW_SECS), now)
}

fn is_high_card(f: &sfst::FieldEntry) -> bool {
    matches!(f.tier, sfst::FieldTier::High)
}

#[cfg(test)]
mod tests;
