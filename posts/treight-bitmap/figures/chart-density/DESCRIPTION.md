# chart-density — posting-list density histogram per corpus

Placement: draft §8 (Benchmarks — density-sliced interpretation); can also
support §2's density-variance paragraph (writer to choose).

## Intent

Shows where real posting lists actually live on the density axis (log
buckets from ≤0.001% to >50%) per corpus, and marks the >50% band where
the encoder flips to the complement representation (`from_sorted_iter_
complemented`, build.rs `remap_one_bitmap`). This is the "density variance
is wild" claim made visible before the size/CPU numbers are read.

## Data + transform

- Input: `data/derived/<corpus>/stats.json` → `posting_density_histogram`
  (schema.md §1; path pinned by bench-eng via the Lead, observed for all
  six derived dirs; `scripts/chartlib.py::find_stats` probes it first).
- Transform: bucket counts plotted as-is (log y); bars grouped per corpus,
  colored per corpus with distinct hatches (greyscale-safe); the >50%
  bucket is outlined with a vermillion band + annotation pointing at the
  complement decision.
- Corpus set: `certstream-250k`, `jetstream-250k`, `mixed-250k`
  (`sample-*`/smoke variants skipped by design).

## Script invocation

```
cd posts/treight-bitmap/figures
MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-density.py [--strict]
```

`--strict` makes missing evidence an error (for the final pre-publish run).
Without evidence the script emits clearly-marked SYNTHETIC sample data
(title stamp + stdout banner) so the layout can be checked early — the
synthetic numbers are plausible shapes, NOT measurements.

## Manual recreation (if tweaking by hand)

1. Edit `scripts/chart-density.py`: bucket order lives in
   `chartlib.DENSITY_BUCKETS` (keep schema.md's bucket strings verbatim —
   they are dict keys); corpus colors/hatches in `chartlib.CORPUS_COLORS`.
2. Re-run the invocation above; outputs land in this directory as
   `chart-density.svg` (primary; text kept as text via
   `svg.fonttype = "none"`) and `chart-density.png` (150 dpi fallback).
3. The annotation text ("bucket \[0.5, 1.0] … strictly > 50%") mirrors
   build.rs's density rule (`card > universe/2`, STRICT). The histogram's
   top bucket is labeled ">=50%" in the regenerated stats.json (bench-eng
   relabeled after we verified mixed-250k held 9 lists at density exactly
   0.5 in it that stay direct; only strictly-above-half lists complement).
   `chartlib.DENSITY_BUCKETS` keeps the shipped label verbatim — do not
   edit it independently of harvest.py.

## Verification performed

- REAL-DATA run 2026-10-02 against data/derived/*/stats.json (all three
  corpora); bucket sums equal distinct_kvs exactly per corpus. NOTE: the
  22:21 regeneration renamed the top bucket to ">=50%"; the final
  post-freeze --strict pass regenerated the exports against it (bucket
  sums re-verified: they equal distinct_kvs exactly per corpus).
- Earlier synthetic-mode run compiled and rendered; `verify_layout.py`: no edge
  clipping, no region collisions; ASCII preview confirmed grouped bars,
  log axis, annotation, legend.
- Bucket strings cross-checked against schema.md §1.
