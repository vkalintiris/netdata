# chart-ops — per-op CPU medians: treight vs roaring

Placement: draft §8 (Benchmarks — per-op results, density-sliced).

## Intent

The CPU comparison across the operations the blog benches: top row = the
eight per-list ops sliced by density bucket (lt1pct / 1to50pct / gt50pct —
the slicing the prose uses to say WHERE each side wins); bottom row = the
three set-op PAIR groups, per-OPERATION medians on their own log axis
(they are single measured operand pairs, not per-position populations,
so they must not share the top row's axis). Log y throughout.

## Data + transform

- Input: `posts/treight-bitmap/evidence/bench/results.json` →
  `corpora.<name>.buckets.<bucket>.ops["<op>/<impl>"]` (schema.md §3)
  for the top row, and the flat pair-group keys
  `corpora.<name>.buckets.pairs.ops["pairs/<op>/<kind>/<impl>"]` for the
  bottom row (and/same-field-disjoint, or/cross-field,
  and_not/dense-vs-sparse; each one measured operand pair, n = 1).
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

- REAL-DATA run 2026-10-02 (post-freeze, certified results.json),
  updated after the Lead's pointer to the pair-group keys: set-op
  medians live under `buckets.pairs.ops` as FLAT keys
  `pairs/<op>/<kind>/<impl>` (my earlier "no set-op medians" finding was
  a wrong-place lookup — `buckets.pairs.pairs` holds the operand
  descriptions, `buckets.pairs.ops` holds the timings). All three pair
  groups are charted in the bottom panel.
- Pair-panel numbers verified against the certified file (mixed-250k):
  and/same-field-disjoint 44 731 ns vs 17 415 ns (treight 2.6× slower);
  or/cross-field 4.79 ns vs 2 814 ns (treight ~588× faster — the
  complement-stored full-universe operand resolves the union via the De
  Morgan dispatch instead of materializing it); and_not/dense-vs-sparse
  36.5 ns vs 694 ns (treight ~19× faster). Cross-corpus orderings for
  or and and_not hold everywhere; the and/same-field-disjoint penalty
  ranges 2.6× (mixed) to ~43–52× (certstream/jetstream, whose operands
  are large sparse lists where roaring's vectorized kernels dominate).
- Cross-checked the certified orderings (t/r ratios of per-position or
  per-list medians, all three corpora): bulk_build 2.32–2.55 → 1.71–3.31
  → 0.71–1.13 (treight faster on dense gt50pct in certstream/jetstream);
  iterate 1.52–2.66 → 1.07–1.19; contains_hit 387–6973× roaring;
  range_count 35–288× roaring — orderings consistent across corpora,
  matching the Lead's relayed story (whose exact figures are the
  certstream column: 2.48/1.71/0.89 bulk, 1.98/1.68/1.07 iterate,
  387–3279× contains, 44–234× range).
- Earlier synthetic-mode run rendered and verified (layout parity).
