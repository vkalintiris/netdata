# chart-ops — per-op CPU medians: treight vs roaring

Placement: draft §8 (Benchmarks — per-op results, density-sliced).

## Intent

The CPU comparison across the operations the blog benches, sliced by
density bucket (lt1pct / 1to50pct / gt50pct) — the same slicing the prose
uses to say WHERE each side wins. Log y throughout (ops span ns…ms).

## Data + transform

- Input: `posts/treight-bitmap/evidence/bench/results.json` →
  `corpora.<name>.buckets.<bucket>.ops["<op>/<impl>"]` (schema.md §3).
- Default corpus `mixed-250k`; `--corpus <name>` emits another corpus's
  variant (the blog can inline one and link the rest).
- Bars = `median_ns` (the criterion median per benchmark iteration). The
  per-position / per-list normalizations from the schema
  (`per_position_ns`, `per_list_ns`) are PRINTED to stdout when present —
  mixing normalizations on one axis would mislead, so the figure keeps raw
  medians (valid for within-op treight-vs-roaring comparison) and the
  blog's per-position claims quote the printed values.
- Roaring build rows may carry a `"path": "native|from_sorted|naive-iter"`
  note in the evidence — stdout note printing surfaces it next to the
  normalization numbers.

## Script invocation

```
cd posts/treight-bitmap/figures
MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-ops.py [--corpus mixed-250k] [--strict]
```

Synthetic fallback behavior: identical to chart-density (stamped, stdout
banner, `--strict` to fail).

## Manual recreation

1. Op order = `chartlib.OPS_ORDER` (matches schema.md §3's op list);
   bucket order = `chartlib.BUCKETS`.
2. Impl styling = `chartlib.IMPL_STYLE` (treight vermillion + forward
   hatch, roaring blue solid) — hatches keep it greyscale-readable.
3. If the blog wants per-position charts instead, swap `median_ns` for
   `per_position_ns` (and filter to the ops where it is non-null); keep
   the axis label honest about the unit.

## Verification performed

- Synthetic-mode run rendered; `verify_layout.py` clean; three bucket
  panels, grouped pairs, log axis, legend confirmed in preview.
- Field names and op list cross-checked against schema.md §3.
