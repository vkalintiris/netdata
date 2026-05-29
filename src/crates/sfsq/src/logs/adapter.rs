//! SFST query results → Netdata UI wire envelope.
//!
//! These adapters translate the structured outputs of
//! [`sfst::IndexReader::facets`] and [`sfst::IndexReader::timeline`] into
//! the [`super::wire`] envelope shape the cloud-frontend renders. They
//! are pure transformers — no I/O, no SFST opens — so they can be
//! exercised against synthetic inputs without touching the filesystem.
//!
//! Wired into [`super::handler::OtelLogsHandler::on_call`] after opening
//! every SFST whose time range overlaps the request window, querying
//! each, and merging the per-file results.

use super::wire::{
    AvailableHistogram, Chart, ChartDimensions, ChartPoint, ChartResult, ChartView, DataPoint,
    Facet, FacetOption, Histogram,
};

/// One nanosecond expressed as a millisecond fraction. Histogram bucket
/// timestamps go on the wire in milliseconds (legacy chart contract).
const NS_PER_MS: i64 = 1_000_000;

/// One nanosecond expressed as a second fraction. ChartView `after` /
/// `before` / `update_every` are u32 seconds (legacy chart contract);
/// also reused by the handler when aligning the request's `[after,
/// before)` to the per-file bucket grid.
pub const NS_PER_S: i64 = 1_000_000_000;

/// Convert one [`sfst::FacetResult`] into a [`Facet`].
///
/// Option order is preserved from the input — `sfst::IndexReader::facets`
/// already surfaces values in FST iteration order, which is
/// lexicographic and stable across runs.
pub fn facet_from_sfst(order: usize, sfst_facet: &sfst::FacetResult) -> Facet {
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
pub fn histogram_from_sfst(field: &str, timeline: &sfst::Timeline) -> Histogram {
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

/// Merge per-file [`sfst::FacetResult`] sets into a single combined
/// set. Union by field name; per field, sum counts across files for
/// each value. Output values are emitted in BTreeMap iteration order
/// (lexicographic by value string), matching the FST iteration-order
/// contract documented on [`sfst::FacetResult`].
pub fn merge_facet_results(
    per_file: Vec<Vec<sfst::FacetResult>>,
) -> Vec<sfst::FacetResult> {
    use std::collections::BTreeMap;

    // Accumulate in `u64` so summing across many files can't wrap
    // `u32::MAX` mid-merge. Output is saturating-cast back to `u32`
    // to match `sfst::FacetResult::values`'s on-the-wire type.
    let mut by_field: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for file_facets in per_file {
        for f in file_facets {
            let bucket = by_field.entry(f.field).or_default();
            for (value, count) in f.values {
                *bucket.entry(value).or_insert(0) += u64::from(count);
            }
        }
    }
    by_field
        .into_iter()
        .map(|(field, values)| sfst::FacetResult {
            field,
            values: values
                .into_iter()
                .map(|(v, c)| (v, c.min(u32::MAX as u64) as u32))
                .collect(),
        })
        .collect()
}

/// Merge per-file [`sfst::Timeline`]s into a single combined timeline.
///
/// Precondition: every input must share the same [`sfst::Grid`] —
/// the multi-file caller builds them off a single request-aligned
/// grid, so `grid.bucket_start_ns`, `grid.bucket_width_ns`, and
/// `grid.num_buckets` all match across inputs. Dimensions are
/// unioned via [`BTreeSet`] (sorted lexicographically) and each
/// input's per-bucket counts are reindexed onto the union order
/// before bucket-wise summation. `unset` sums bucket-wise.
///
/// Returns `None` if `per_file` is empty.
pub fn merge_timelines(per_file: Vec<sfst::Timeline>) -> Option<sfst::Timeline> {
    use std::collections::BTreeSet;

    let mut iter = per_file.into_iter();
    let first = iter.next()?;
    let grid = first.grid;

    // Collect into a Vec so we can iterate it twice (union pass +
    // reindex pass).
    let mut all: Vec<sfst::Timeline> = vec![first];
    all.extend(iter);

    // Union of dimension labels across all files.
    let mut dim_set: BTreeSet<String> = BTreeSet::new();
    for t in &all {
        for d in &t.dimensions {
            dim_set.insert(d.clone());
        }
    }
    let dimensions: Vec<String> = dim_set.into_iter().collect();
    let dim_index: std::collections::HashMap<&str, usize> = dimensions
        .iter()
        .enumerate()
        .map(|(i, d)| (d.as_str(), i))
        .collect();

    let mut buckets = vec![vec![0u64; dimensions.len()]; grid.num_buckets];
    let mut unset = vec![0u64; grid.num_buckets];

    for t in &all {
        // Hard-assert the precondition: every input must share the
        // grid established by `first`. A violation silently
        // produces wrong merged data — better to panic than serve
        // misaligned buckets. The cost is one comparison per file,
        // not per bucket, so the check is free at runtime.
        assert_eq!(t.grid, grid);
        assert_eq!(t.buckets.len(), grid.num_buckets);
        assert_eq!(t.unset.len(), grid.num_buckets);

        // Map this file's local dim index → union dim index.
        let local_to_union: Vec<usize> = t
            .dimensions
            .iter()
            .map(|d| dim_index[d.as_str()])
            .collect();

        for (bucket_i, file_bucket) in t.buckets.iter().enumerate() {
            for (local_i, count) in file_bucket.iter().enumerate() {
                buckets[bucket_i][local_to_union[local_i]] += count;
            }
            unset[bucket_i] += t.unset[bucket_i];
        }
    }

    Some(sfst::Timeline {
        grid,
        dimensions,
        buckets,
        unset,
    })
}

/// Union per-file field tables for `available_histograms` selection.
/// A field is dropped if it's [`sfst::FieldTier::High`] in **any**
/// file — both [`sfst::IndexReader::facets`] and
/// [`sfst::IndexReader::timeline`] reject high-card fields, so
/// offering one that errors on some files would yield a runtime
/// failure when the user picks it. Per-file `cardinality` values are
/// not summed (the concept is per-file, not global); the union keeps
/// the maximum as a conservative estimate for facet eligibility
/// gates. Output is sorted by name.
pub fn union_field_tables(
    per_file: &[&[sfst::FieldEntry]],
) -> Vec<sfst::FieldEntry> {
    use std::collections::BTreeMap;

    // name → (max_cardinality_so_far, tier, ever_high_card)
    let mut by_name: BTreeMap<String, (u32, sfst::FieldTier, bool)> = BTreeMap::new();
    for table in per_file {
        for f in *table {
            let is_high = matches!(f.tier, sfst::FieldTier::High);
            by_name
                .entry(f.name.clone())
                .and_modify(|(card, tier, ever_high)| {
                    *card = (*card).max(f.cardinality);
                    if is_high {
                        *tier = sfst::FieldTier::High;
                        *ever_high = true;
                    }
                })
                .or_insert((f.cardinality, f.tier, is_high));
        }
    }
    by_name
        .into_iter()
        .filter(|(_, (_, _, ever_high))| !ever_high)
        .map(|(name, (cardinality, tier, _))| sfst::FieldEntry {
            name,
            cardinality,
            tier,
        })
        .collect()
}

/// Build the `available_histograms` list from a SFST field table.
///
/// High-cardinality fields are excluded — [`sfst::IndexReader::timeline`]
/// rejects them with [`sfst::Error::HighCardFacet`], so offering them
/// in the UI would just produce errors. Low- and mid-cardinality
/// fields are surfaced in field-table order.
pub fn available_histograms_from_fields(
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
