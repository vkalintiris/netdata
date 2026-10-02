# Evidence data schemas for diagram scripts

Author: bench-eng (task-2). This file is the contract for the committed
evidence JSON files under `posts/treight-bitmap/evidence/bench/`. It is
written BEFORE the data so chart scripts can be developed now; if a field
evolves, this file is updated in the same change and the Lead relays it to
diagram-eng.

Three files carry chartable data. All numbers are exact counts (integers)
or f64 ratios/seconds; all byte sizes are plain bytes (no units suffix);
all times are nanoseconds unless a field name says otherwise. Corpus names
are the directory names under `data/derived/`: `certstream-250k`,
`jetstream-250k`, `mixed-250k` (and possibly `sample-*` smoke variants —
skip those in charts).

## 1. `stats.json` — per-corpus corpus shape (one file per corpus, copied
verbatim from the derived cache dir; also embedded per-corpus inside
`results.json` → `corpora.<name>.stats`)

```
{
  "universe": u64,                  // row count == bitmap universe
  "rows": u64,                      // == universe (rows fed to RowIndex)
  "lines_seen": u64,                // corpus lines consumed
  "rows_per_source": { "certstream": u64, "jetstream": u64 },
  "distinct_fields": u64,
  "distinct_kvs": u64,              // total posting lists
  "total_postings": u64,            // sum of cardinalities
  "avg_entries_per_row": f64,
  "cardinality_threshold": 100,     // production tier boundary (low < 100)
  "tier_summary": {
    "low":  { "fields": u64, "lists": u64, "postings": u64 },   // <100 values
    "mid":  { "fields": u64, "lists": u64, "postings": u64 },   // [100,1000)
    "high": { "fields": u64, "lists": u64, "postings": u64 }    // >=1000, NO on-disk bitmaps in SFST
  },
  "fields": [ { "field": str, "distinct_values": u64, "tier": "low|mid|high" }, ... ],
  "posting_density_histogram": [
    { "bucket": "<=0.001%" | "0.001-0.01%" | "0.01-0.1%" | "0.1-1%" |
                "1-10%" | "10-50%" | ">=50%", "lists": u64 }, ... ],
    // buckets are half-open [lo, hi); the top bucket is [0.5, 1.0]. The
    // ENCODER complements only strictly-above-half lists (card >
    // universe/2): an exactly-50% list sits in ">=50%" but is encoded
    // direct — totals.json's per-list "complemented" flag is authoritative.
  "posting_cardinality_histogram": [
    { "bucket": "1" | "2-4" | "5-16" | "17-64" | "65-256" | "257-1k" |
                "1k-4k" | "4k-16k" | "16k-64k" | ">64k", "lists": u64 }, ... ],
  "complement_candidates_gt50pct": u64,   // lists the encoder complements
  "at_bitset_threshold": { "threshold": u64, "lists": u64 },
  "top20_by_cardinality": [
    { "kv": str, "cardinality": u64, "density": f64 }, ... ],
  "derive_seconds": f64
}
```

## 2. `totals.json` — per-corpus index-footprint totals (treight vs
roaring over the same posting lists)

Root: `{ "corpora": { "<name>": { ... } }, "notes": [str, ...] }`. Per corpus:

```
{
  "universe": u64,
  "lists_total": u64,
  "bitsets": {                       // scoped sums (bytes = serialized bytes)
    "sfst_materialized": {           // low+mid tier — what the real SFST stores
      "lists": u64,
      "treight_blob_bytes": u64,     // sum of treight tree-blob bytes
      "treight_with_desc_bytes": u64,// + 5-byte descriptor per list (BitmapValue)
      "roaring_serialized_bytes": u64,
      "roaring_heap_bytes": u64,     // allocative deep size, same bitmaps
      "treight_heap_bytes": u64      // blob + 5-byte descriptor (same accounting)
    },
    "all_lists": { ...same fields... }   // includes high-card tier
  },
  "complement": {
    "complemented_lists": u64,           // encoded via from_sorted_iter_complemented
    "direct_lists": u64
  },
  "per_density_bucket": [               // same density scale as stats.json (top: ">=50%")
    { "bucket": str, "lists": u64,
      "treight_blob_bytes": u64, "roaring_serialized_bytes": u64,
      "avg_cardinality": f64, "avg_density": f64 }, ... ],
}

# Per-list scatter source: `per-list-<corpus>.csv.gz` (one per corpus)
gzip'd CSV, header row `card,density,tier,treight_blob_bytes,
roaring_serialized_bytes,roaring_heap_bytes,complemented`. Deterministic
stride sampling caps each file at 200,000 rows (stride + full count in the
gzip comment); the totals.json aggregates are EXACT over all lists. The
`kv` strings are deliberately not in the CSV — identity is not needed for
the scatter and keeps personal data (cert CNs, DIDs) out of committed
per-list artifacts.
```

## 3. `results.json` — criterion CPU medians, per corpus × density bucket ×
operation × implementation

Root: `{ "environment": {...see environment.txt...},
"corpora": { "<name>": { "stats": {...}, "buckets": { "<bucket>": {...} } } } }`.

Density buckets for benches: `lt1pct`, `1to50pct`, `gt50pct` — same
lower-inclusive convention (gt50pct = [0.5, 1.0]; membership in it does not
imply the encoder complemented the list — see the per-list flag).

Per bucket:

```
{
  "lists_sampled": u64,              // posting lists the bucket's numbers average over
  "positions_sampled": u64,          // total positions across those lists
  "insert_positions_sampled": u64,   // positions the insert-phase benches used
                                     //   (identically capped per list at 10,000 for
                                     //   BOTH impls — treight's insert cost grows
                                     //   with blob occupancy; uncapped dense runs
                                     //   would take minutes per iteration)
  "ops": {
    "<op>/<impl>": {                 // op in: bulk_build, insert_arrival,
                                     //   insert_shuffled, contains_hit, contains_miss,
                                     //   len_min_max, iterate, and, or, and_not,
                                     //   range_count
      "impl": "treight" | "roaring", // roaring bulk/insert rows carry a
                                     //   "path": "native|from_sorted|naive-iter" note
      "median_ns": f64,              // criterion median estimate
      "median_lo_ns": f64, "median_hi_ns": f64,  // 95% CI bounds
      "mad_ns": f64,                 // median absolute deviation
      "per_position_ns": f64 | null, // ops over N positions: median/positions
      "per_list_ns": f64 | null,     // list-shaped ops: median/lists_sampled
      "n": u64                       // items per benchmark iteration
    }
  },
  "pairs": [                          // the set-op operand pairs actually used
    { "op": "and|or|and_not", "kind": "same-field-disjoint|cross-field|dense-vs-sparse",
      "a": str, "b": str,            // kv strings of the operands
      "a_density": f64, "b_density": f64, "a_complemented": bool } ]
}
```

Each bucket also carries a `sanity` block (treight-vs-roaring equality
assertions that passed before timing: len/min/max/contains-sample).
