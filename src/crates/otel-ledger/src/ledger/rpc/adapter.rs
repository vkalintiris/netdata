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
/// "(unset)" — the legacy systemd plugin's catch-all bucket for logs
/// missing the dimension field — is **not** emitted in this MVP.
/// `sfst::IndexReader::timeline` doesn't surface that count; computing
/// it would require an extra per-bucket range scan. Left for a later
/// pass if the UI calls for it.
pub(super) fn histogram_from_sfst(field: &str, timeline: &sfst::Timeline) -> Histogram {
    let dim_count = timeline.dimensions.len();
    let bucket_start_ms = (timeline.bucket_start_ns / NS_PER_MS).max(0) as u64;
    let bucket_width_ms = (timeline.bucket_width_ns / NS_PER_MS).max(1) as u64;

    let after_s = (timeline.bucket_start_ns / NS_PER_S).max(0) as u32;
    let span_ns = timeline.bucket_width_ns * timeline.buckets.len() as i64;
    let before_s = ((timeline.bucket_start_ns + span_ns) / NS_PER_S).max(0) as u32;
    let update_every_s = (timeline.bucket_width_ns / NS_PER_S).max(1) as u32;

    let dimension_ids: Vec<String> = timeline.dimensions.clone();
    let dimension_names: Vec<String> = timeline.dimensions.clone();
    let dimension_units: Vec<String> = std::iter::repeat_n("events".to_string(), dim_count).collect();

    // Labels carry a leading "time" entry to match the legacy chart
    // contract: result.labels[0] is the timestamp column header,
    // result.labels[1..] line up with each DataPoint's items.
    let mut labels: Vec<String> = Vec::with_capacity(dim_count + 1);
    labels.push("time".to_string());
    labels.extend(timeline.dimensions.iter().cloned());

    let data: Vec<DataPoint> = timeline
        .buckets
        .iter()
        .enumerate()
        .map(|(bucket_i, counts)| {
            let timestamp_ms = bucket_start_ms + (bucket_i as u64) * bucket_width_ms;
            let items: Vec<[usize; 3]> = counts.iter().map(|&c| [c as usize, 0, 0]).collect();
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
mod tests {
    use super::*;

    #[test]
    fn facet_preserves_value_order_and_counts() {
        let f = sfst::FacetResult {
            field: "level".into(),
            values: vec![("error".into(), 3), ("info".into(), 5)],
        };
        let wire = facet_from_sfst(7, &f);
        assert_eq!(wire.id, "level");
        assert_eq!(wire.name, "level");
        assert_eq!(wire.order, 7);
        assert_eq!(wire.options.len(), 2);
        assert_eq!(wire.options[0].id, "error");
        assert_eq!(wire.options[0].count, 3);
        assert_eq!(wire.options[0].order, 0);
        assert_eq!(wire.options[1].id, "info");
        assert_eq!(wire.options[1].count, 5);
        assert_eq!(wire.options[1].order, 1);
    }

    #[test]
    fn facet_with_no_values_yields_empty_options() {
        let f = sfst::FacetResult {
            field: "service".into(),
            values: Vec::new(),
        };
        let wire = facet_from_sfst(0, &f);
        assert!(wire.options.is_empty());
    }

    #[test]
    fn histogram_emits_one_datapoint_per_bucket() {
        // 3 buckets × 2 dimensions; bucket width = 2 seconds.
        let t = sfst::Timeline {
            bucket_start_ns: 1_700_000_000 * NS_PER_S,
            bucket_width_ns: 2 * NS_PER_S,
            dimensions: vec!["error".into(), "info".into()],
            buckets: vec![vec![1, 4], vec![0, 3], vec![2, 2]],
        };

        let h = histogram_from_sfst("level", &t);

        assert_eq!(h.id, "level");
        assert_eq!(h.chart.view.after, 1_700_000_000);
        assert_eq!(h.chart.view.before, 1_700_000_000 + 6);
        assert_eq!(h.chart.view.update_every, 2);
        assert_eq!(h.chart.view.chart_type, "stackedBar");
        assert_eq!(h.chart.view.dimensions.ids, vec!["error", "info"]);
        assert_eq!(h.chart.view.dimensions.units, vec!["events", "events"]);

        // labels: ["time", "error", "info"]; no "(unset)" trailer.
        assert_eq!(h.chart.result.labels, vec!["time", "error", "info"]);

        // 3 buckets — timestamps advance by 2000 ms.
        let dps = &h.chart.result.data;
        assert_eq!(dps.len(), 3);
        assert_eq!(dps[0].timestamp_ms, 1_700_000_000_000);
        assert_eq!(dps[1].timestamp_ms, 1_700_000_002_000);
        assert_eq!(dps[2].timestamp_ms, 1_700_000_004_000);

        // Per-bucket counts pass through, padded to [c, 0, 0].
        assert_eq!(dps[0].items, vec![[1, 0, 0], [4, 0, 0]]);
        assert_eq!(dps[1].items, vec![[0, 0, 0], [3, 0, 0]]);
        assert_eq!(dps[2].items, vec![[2, 0, 0], [2, 0, 0]]);
    }

    #[test]
    fn histogram_with_zero_buckets_still_well_formed() {
        let t = sfst::Timeline {
            bucket_start_ns: 0,
            bucket_width_ns: NS_PER_S,
            dimensions: Vec::new(),
            buckets: Vec::new(),
        };
        let h = histogram_from_sfst("severity_text", &t);
        assert!(h.chart.result.data.is_empty());
        assert_eq!(h.chart.result.labels, vec!["time"]);
        assert!(h.chart.view.dimensions.ids.is_empty());
    }

    #[test]
    fn available_histograms_drops_high_card_fields() {
        let fields = vec![
            sfst::FieldEntry {
                name: "level".into(),
                cardinality: 3,
                tier: sfst::FieldTier::Low,
            },
            sfst::FieldEntry {
                name: "host".into(),
                cardinality: 200,
                tier: sfst::FieldTier::Mid,
            },
            sfst::FieldEntry {
                name: "trace_id".into(),
                cardinality: 50_000,
                tier: sfst::FieldTier::High,
            },
        ];
        let av = available_histograms_from_fields(&fields);
        let names: Vec<&str> = av.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["level", "host"]);
        assert_eq!(av[0].order, 0);
        assert_eq!(av[1].order, 1);
    }
}
