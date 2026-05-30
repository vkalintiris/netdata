//! Mapping between the netdata `otel-logs` wire types and the
//! wire-neutral [`sfsq::logs`] engine.
//!
//! [`to_query`] turns an [`OtelLogsRequest`] into the engine's
//! [`LogsQuery`]; [`to_result`] turns the engine's [`LogsData`] into the
//! [`LogsResult`] wire envelope the cloud-frontend renders. The
//! per-structure converters ([`facet_from_sfst`], [`histogram_from_sfst`],
//! [`available_histograms_from_fields`], [`build_table`]) are pure
//! transformers over `sfst` values — no I/O — so they're exercisable
//! against synthetic inputs.

use std::collections::{BTreeSet, HashMap};

use sfsq::logs::{Anchor, Cursor, LogsData, LogsQuery};

use super::wire::{
    ACCEPTED_PARAMS, AnchorParam, AvailableHistogram, Chart, ChartDimensions, ChartPoint,
    ChartResult, ChartView, DataPoint, Facet, FacetOption, Histogram, Items, LogsResult,
    OtelLogsRequest, Pagination, Version,
};

/// One nanosecond expressed as a millisecond fraction. Histogram bucket
/// timestamps go on the wire in milliseconds (legacy chart contract).
const NS_PER_MS: i64 = 1_000_000;

/// One nanosecond expressed as a second fraction. ChartView `after` /
/// `before` / `update_every` are u32 seconds (legacy chart contract).
const NS_PER_S: i64 = 1_000_000_000;

// ── Wire request → engine query ─────────────────────────────────────

/// Map a deserialized [`OtelLogsRequest`] onto the engine's neutral
/// [`LogsQuery`]. The empty `histogram` string becomes `None`; a
/// histogram-click µs timestamp becomes an [`Anchor::Timestamp`] in ns;
/// a malformed cursor string is dropped (treated as "no anchor").
pub fn to_query(req: OtelLogsRequest) -> LogsQuery {
    LogsQuery {
        after: req.after,
        before: req.before,
        selections: req.selections,
        histogram_field: (!req.histogram.is_empty()).then_some(req.histogram),
        facet_fields: req.facets,
        anchor: req.anchor.and_then(|a| match a {
            AnchorParam::Cursor(s) => Cursor::decode(&s).map(Anchor::Cursor),
            AnchorParam::TimestampUs(us) => {
                Some(Anchor::Timestamp((us as i64).saturating_mul(1_000)))
            }
        }),
        direction: req.direction,
        limit: req.last,
    }
}

// ── Engine result → wire envelope ───────────────────────────────────

/// Shape an engine [`LogsData`] into the wire [`LogsResult`].
/// `max_to_return` echoes the request's `last` into `items.max_to_return`.
pub fn to_result(data: LogsData, max_to_return: usize) -> LogsResult {
    let facets = data
        .facets
        .iter()
        .enumerate()
        .map(|(i, f)| facet_from_sfst(i, f))
        .collect();
    let histogram = histogram_from_sfst(&data.histogram_field, &data.histogram);
    let available_histograms = available_histograms_from_fields(&data.available_fields);
    let facetable = data.facetable();
    let (columns, rows) = build_table(&data.rows, &data.columns, &facetable);
    let matched = data.matched;

    LogsResult {
        progress: 100,
        version: Version::default(),
        accepted_params: ACCEPTED_PARAMS.to_vec(),
        required_params: Vec::new(),
        facets,
        available_histograms,
        histogram,
        columns,
        data: rows,
        default_charts: Vec::new(),
        items: Items {
            evaluated: matched,
            unsampled: 0,
            estimated: matched,
            matched,
            // before ⇒ newer rows exist (UI "scroll up"); after ⇒ older
            // rows exist (UI "scroll down").
            before: data.has_newer as usize,
            after: data.has_older as usize,
            returned: data.rows.len(),
            max_to_return,
        },
        show_ids: false,
        has_history: true,
        status: 200,
        response_type: String::from("table"),
        help: String::from("Query and visualize OpenTelemetry logs."),
        pagination: Pagination::default(),
    }
}

// ── Per-structure converters ────────────────────────────────────────

/// Convert one [`sfst::FacetResult`] into a [`Facet`].
///
/// Option order is preserved from the input — `sfst::IndexReader::facets`
/// already surfaces values in FST iteration order, which is lexicographic
/// and stable across runs.
fn facet_from_sfst(order: usize, sfst_facet: &sfst::FacetResult) -> Facet {
    let options = sfst_facet
        .values
        .iter()
        .enumerate()
        .map(|(opt_order, (value, count))| FacetOption {
            id: value.clone(),
            name: value.clone(),
            order: opt_order,
            count: *count as usize,
        })
        .collect();

    Facet {
        id: sfst_facet.field.clone(),
        name: sfst_facet.field.clone(),
        order,
        options,
    }
}

/// Convert an [`sfst::Timeline`] into the UI's [`Histogram`] shape.
///
/// `field` is the histogram dimension field (e.g. `"severity_text"`).
/// The timeline's grid drives `chart.view.after` / `before` /
/// `update_every`; per-bucket dimension counts become flat `[count, 0, 0]`
/// triples on each [`DataPoint`].
///
/// Appends an `"(unset)"` trailer dimension counting per-bucket logs that
/// match the filter but don't carry `field`. Matches the legacy
/// systemd-journal wire shape — `result.labels` ends with `"(unset)"`,
/// each `DataPoint.items` carries an extra trailing triple.
fn histogram_from_sfst(field: &str, timeline: &sfst::Timeline) -> Histogram {
    const UNSET_LABEL: &str = "(unset)";

    let total_dim_count = timeline.dimensions.len() + 1; // value dims + (unset)
    let grid = timeline.grid;
    let bucket_start_ms = (grid.bucket_start_ns / NS_PER_MS).max(0) as u64;
    let bucket_width_ms = (grid.bucket_width_ns / NS_PER_MS).max(1) as u64;

    let after_s = (grid.bucket_start_ns / NS_PER_S).max(0) as u32;
    let span_ns = grid.bucket_width_ns * grid.num_buckets as i64;
    let before_s = ((grid.bucket_start_ns + span_ns) / NS_PER_S).max(0) as u32;
    let update_every_s = (grid.bucket_width_ns / NS_PER_S).max(1) as u32;

    let mut dimension_ids: Vec<String> = timeline.dimensions.clone();
    dimension_ids.push(UNSET_LABEL.to_string());
    let dimension_names: Vec<String> = dimension_ids.clone();
    let dimension_units: Vec<String> =
        std::iter::repeat_n("events".to_string(), total_dim_count).collect();

    // Labels carry a leading "time" entry to match the legacy chart
    // contract: result.labels[0] is the timestamp column header,
    // result.labels[1..] line up with each DataPoint's items —
    // dimension values first, then the trailing "(unset)" entry.
    let mut labels: Vec<String> = Vec::with_capacity(total_dim_count + 1);
    labels.push("time".to_string());
    labels.extend(timeline.dimensions.iter().cloned());
    labels.push(UNSET_LABEL.to_string());

    let data: Vec<DataPoint> = timeline
        .buckets
        .iter()
        .enumerate()
        .map(|(bucket_i, counts)| {
            let timestamp_ms = bucket_start_ms + (bucket_i as u64) * bucket_width_ms;
            let mut items: Vec<[usize; 3]> = counts.iter().map(|&c| [c as usize, 0, 0]).collect();
            let unset = timeline.unset.get(bucket_i).copied().unwrap_or(0);
            items.push([unset as usize, 0, 0]);
            DataPoint {
                timestamp_ms,
                items,
            }
        })
        .collect();

    Histogram {
        id: field.to_string(),
        name: field.to_string(),
        chart: Chart {
            view: ChartView {
                title: format!("Events distribution by {field}"),
                after: after_s,
                before: before_s,
                update_every: update_every_s,
                units: String::from("events"),
                chart_type: String::from("stackedBar"),
                dimensions: ChartDimensions {
                    ids: dimension_ids,
                    names: dimension_names,
                    units: dimension_units,
                },
            },
            result: ChartResult {
                labels,
                point: ChartPoint {
                    value: 0,
                    arp: 1,
                    pa: 2,
                },
                data,
            },
        },
    }
}

/// Build the `available_histograms` list from the engine's available
/// (low/mid-card) field set. The engine already excludes high-card
/// fields, so this is a straight enumeration in field order.
fn available_histograms_from_fields(fields: &[sfst::FieldEntry]) -> Vec<AvailableHistogram> {
    fields
        .iter()
        .enumerate()
        .map(|(order, f)| AvailableHistogram {
            id: f.name.clone(),
            name: f.name.clone(),
            order,
        })
        .collect()
}

/// Build the wire `columns` schema and `data` rows from a materialized
/// page.
///
/// Columns: a visible µs `timestamp` and `severity`, a hidden string
/// `cursor` (the `pagination.column` the UI echoes as `anchor`), then one
/// hidden column per attribute field. Fields in `facetable` get
/// `filter: "facet"` so the UI's "+ Add Filter Field" picker offers them;
/// everything else is `"none"`. Each data row is a positional array
/// aligned to the column `index`; absent attributes are `null`.
fn build_table(
    rows: &[(Cursor, sfst::MaterializedRow)],
    fields: &[String],
    facetable: &BTreeSet<&str>,
) -> (serde_json::Value, serde_json::Value) {
    use serde_json::{Value, json};

    let mut columns = serde_json::Map::new();
    // The UI formats the cell from `valueOptions.transform`, not from
    // `type` (which only selects the cell component). Match the legacy
    // journal column: a `timestamp` cell carrying a µs value rendered via
    // the `datetime_usec` transform.
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

    let data: Vec<Value> = rows
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

#[cfg(test)]
mod tests;
