//! Cross-file merge helpers.
//!
//! The multi-file engine queries each candidate SFST independently and
//! then folds the per-file results together. These are the pure folds —
//! no I/O, no wire shaping — operating entirely on `sfst` types.

/// Merge per-file [`sfst::FacetResult`] sets into a single combined set.
/// Union by field name; per field, sum counts across files for each
/// value. Output values are emitted in BTreeMap iteration order
/// (lexicographic by value string), matching the FST iteration-order
/// contract documented on [`sfst::FacetResult`].
pub fn merge_facet_results(per_file: Vec<Vec<sfst::FacetResult>>) -> Vec<sfst::FacetResult> {
    use std::collections::BTreeMap;

    // Accumulate in `u64` so summing across many files can't wrap
    // `u32::MAX` mid-merge. Output is saturating-cast back to `u32` to
    // match `sfst::FacetResult::values`'s on-the-wire type.
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
/// Precondition: every input must share the same [`sfst::Grid`] — the
/// multi-file caller builds them off a single request-aligned grid, so
/// `grid.bucket_start_ns`, `grid.bucket_width_ns`, and `grid.num_buckets`
/// all match across inputs. Dimensions are unioned via [`BTreeSet`]
/// (sorted lexicographically) and each input's per-bucket counts are
/// reindexed onto the union order before bucket-wise summation. `unset`
/// sums bucket-wise.
///
/// Returns `None` if `per_file` is empty.
///
/// [`BTreeSet`]: std::collections::BTreeSet
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
        // grid established by `first`. A violation silently produces
        // wrong merged data — better to panic than serve misaligned
        // buckets. The cost is one comparison per file, not per bucket,
        // so the check is free at runtime.
        assert_eq!(t.grid, grid);
        assert_eq!(t.buckets.len(), grid.num_buckets);
        assert_eq!(t.unset.len(), grid.num_buckets);

        // Map this file's local dim index → union dim index.
        let local_to_union: Vec<usize> =
            t.dimensions.iter().map(|d| dim_index[d.as_str()]).collect();

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

/// Union per-file field tables into the set of fields usable as facets
/// or histogram dimensions. A field is dropped if it's
/// [`sfst::FieldTier::High`] in **any** file — both
/// [`sfst::IndexReader::facets`] and [`sfst::IndexReader::timeline`]
/// reject high-card fields, so offering one that errors on some files
/// would yield a runtime failure when a consumer picks it. Per-file
/// `cardinality` values are not summed (the concept is per-file, not
/// global); the union keeps the maximum as a conservative estimate.
/// Output is sorted by name.
pub fn union_field_tables(per_file: &[&[sfst::FieldEntry]]) -> Vec<sfst::FieldEntry> {
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

#[cfg(test)]
mod tests;
