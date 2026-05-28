//! SFST query results → Netdata UI wire envelope.
//!
//! These adapters translate the structured outputs of
//! [`sfst::IndexReader::facets`] and [`sfst::IndexReader::timeline`] into
//! the [`super::wire`] envelope shape the cloud-frontend renders. They
//! are pure transformers — no I/O, no SFST opens — so they can be
//! exercised against synthetic inputs without touching the filesystem.
//!
//! Wired into [`super::handler::OtelLogsHandler::on_call`] after opening
//! the most-recent SFST and running the queries.

use super::wire::{
    AvailableHistogram, Chart, ChartDimensions, ChartPoint, ChartResult, ChartView, DataPoint,
    Facet, FacetOption, Histogram,
};

/// One nanosecond expressed as a millisecond fraction. Histogram bucket
/// timestamps go on the wire in milliseconds (legacy chart contract).
const NS_PER_MS: i64 = 1_000_000;

/// One nanosecond expressed as a second fraction. ChartView `after` /
/// `before` / `update_every` are u32 seconds (legacy chart contract).
const NS_PER_S: i64 = 1_000_000_000;

/// Convert one [`sfst::FacetResult`] into a [`Facet`].
///
/// Option order is preserved from the input — `sfst::IndexReader::facets`
/// already surfaces values in FST iteration order, which is
/// lexicographic and stable across runs.
pub(super) fn facet_from_sfst(order: usize, sfst_facet: &sfst::FacetResult) -> Facet {
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
/// The timeline's `bucket_start_ns` and per-bucket width drive
/// `chart.view.after` / `before` / `update_every`; per-bucket
/// dimension counts become flat `[count, 0, 0]` triples on each
/// [`DataPoint`].
///
/// Appends an `"(unset)"` trailer dimension counting per-bucket logs
/// that match the filter but don't carry `field`. Matches the legacy
/// systemd-journal wire shape — `result.labels` ends with `"(unset)"`,
/// each `DataPoint.items` carries an extra trailing triple.
pub(super) fn histogram_from_sfst(field: &str, timeline: &sfst::Timeline) -> Histogram {
    const UNSET_LABEL: &str = "(unset)";

    let total_dim_count = timeline.dimensions.len() + 1; // value dims + (unset)
    let bucket_start_ms = (timeline.bucket_start_ns / NS_PER_MS).max(0) as u64;
    let bucket_width_ms = (timeline.bucket_width_ns / NS_PER_MS).max(1) as u64;

    let after_s = (timeline.bucket_start_ns / NS_PER_S).max(0) as u32;
    let span_ns = timeline.bucket_width_ns * timeline.buckets.len() as i64;
    let before_s = ((timeline.bucket_start_ns + span_ns) / NS_PER_S).max(0) as u32;
    let update_every_s = (timeline.bucket_width_ns / NS_PER_S).max(1) as u32;

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
            let mut items: Vec<[usize; 3]> =
                counts.iter().map(|&c| [c as usize, 0, 0]).collect();
            let unset = timeline
                .unset
                .get(bucket_i)
                .copied()
                .unwrap_or(0);
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

/// Build the `available_histograms` list from a SFST field table.
///
/// High-cardinality fields are excluded — [`sfst::IndexReader::timeline`]
/// rejects them with [`sfst::Error::HighCardFacet`], so offering them
/// in the UI would just produce errors. Low- and mid-cardinality
/// fields are surfaced in field-table order.
pub(super) fn available_histograms_from_fields(
    fields: &[sfst::FieldEntry],
) -> Vec<AvailableHistogram> {
    fields
        .iter()
        .filter(|f| !matches!(f.tier, sfst::FieldTier::High))
        .enumerate()
        .map(|(order, f)| AvailableHistogram {
            id: f.name.clone(),
            name: f.name.clone(),
            order,
        })
        .collect()
}

#[cfg(test)]
mod tests;
