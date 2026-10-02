# chart-footprint — per-corpus index footprint: treight vs roaring bytes

Placement: draft §8 (Benchmarks — index-footprint totals).

## Intent

The headline size claim: summed over identical posting lists, how many
bytes treight stores vs roaring's serialized form (and roaring's in-memory
heap, for context). Two scopes side by side: SFST-materialized (low+mid
tier — what a real SFST file pays) and all lists (high tier included — the
like-for-like data-structure comparison the blog is careful to label).

## Data + transform

- Input: `posts/treight-bitmap/evidence/bench/totals-<corpus>.json` →
  `bitsets.{sfst_materialized,all_lists}` (schema.md §2 semantics; the
  shipped shape is one file per corpus with the per-corpus object at the
  root — a single `totals.json` with a `corpora` root is also supported
  by `chartlib.load_totals_summary`). Since the 22:26 regeneration the
  files are small summaries (the per-list rows moved to
  per-list-<corpus>.csv.gz; see chart-size-vs-density); a plain
  json.load suffices.
- Series per corpus and scope:
  - treight = `treight_with_desc_bytes` (blob + 5-B descriptor per list —
    the BitmapValue accounting bench-eng uses);
  - roaring serialized = `roaring_serialized_bytes`;
  - roaring heap = `roaring_heap_bytes` (allocative deep size; drawn as an
    outline-only bar with back-hatch so it reads as context, not a
    treight opponent).
- Values converted to MB, log y (scopes differ by >10×); exact MB
  annotated over each bar.

## Script invocation

```
cd posts/treight-bitmap/figures
MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-footprint.py [--strict]
```

Synthetic fallback behavior: identical to chart-density (stamped, stdout
banner, `--strict` to fail).

## Manual recreation

1. Series definitions live in the `series` list in
   `scripts/chart-footprint.py` — keys must match schema.md §2 field names.
2. Scope titles state what each panel covers; keep the "what the file
   stores" vs "data-structure comparison" wording aligned with the blog's
   honesty framing (SFST never materializes high-card bitmaps).
3. Re-run to regenerate `chart-footprint.svg` / `.png` here.

## Verification performed

- REAL-DATA run 2026-10-02 against the three landed totals-*.json:
  sfst_materialized.lists == tier low+mid lists from stats.json exactly
  (1152 / 24034 / 20316); all_lists.lists == distinct_kvs; complemented
  lists == stats.complement_candidates_gt50pct (23 / 10 / 3). The 22:26
  regeneration kept these aggregates identical (spot-checked mixed);
  re-verify in the final --strict pass.
- Earlier synthetic-mode run rendered; `verify_layout.py` clean (no clipping, no
  collisions); panels, annotations, and legend confirmed in preview.
- Field names cross-checked against schema.md §2.
