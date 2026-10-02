# chart-size-vs-density — per-list size ratio vs density, by representation

Placement: draft §6 (treight vs roaring, honestly — the "which density band
each side wins" evidence) and/or §8; writer to choose.

## Intent

The mechanism behind the footprint totals: EVERY posting list as one dot,
x = its density (log), y = treight/roaring serialized-size ratio (log;
below the dashed parity line ⇒ treight smaller), with the encoder's
representation choice visible (direct = filled circles, complemented =
open triangles) and the >50% complement region shaded.

## Data + transform

- Input: `posts/treight-bitmap/evidence/bench/per-list-<corpus>.csv.gz`
  (schema.md §2): gzip'd CSV, header `card,density,tier,
  treight_blob_bytes,roaring_serialized_bytes,roaring_heap_bytes,
  complemented`. bench-eng's deterministic stride sampling caps each
  file at 200,000 rows (stride + full count ride in the gzip comment,
  surfaced by the script as `[stride=… of N]`); the totals aggregates
  stay exact over ALL lists — the scatter is the sample, chart-footprint
  is the exact side.
- Per row: `ratio = treight_blob_bytes / roaring_serialized_bytes`
  (rows with `roaring_serialized_bytes == 0` skipped — cannot divide);
  `density` and `complemented` used as-is (the flag is authoritative for
  the strict complement rule — an exactly-50% list plots as direct).
  The ratio uses the BLOB bytes (no 5-B descriptor); stated in the
  footnote for honesty.
- One panel per corpus, shared axes; marker shape + fill encode the
  representation choice so the figure survives greyscale and colorblind
  readers.

## Script invocation

```
cd posts/treight-bitmap/figures
MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-size-vs-density.py [--strict]
```

Synthetic fallback behavior: identical to chart-density (stamped, stdout
banner, `--strict` to fail). The synthetic generator uses a toy size model
(6 B/member treight vs 2 B/member + per-container floor roaring; holes
below 50%) — shapes only, NOT measurements.

## Manual recreation

1. The scatter loops live in `main()` of
   `scripts/chart-size-vs-density.py`; the parity line is `axhline(1.0)`,
   the complement region `axvspan(0.5, 1.0)`.
2. Dots are drawn with `rasterized=True` so all text stays vector and
  editable while the dot layer embeds as one bitmap (the pre-sampling
  full per-list SVG was 919 MB of vector dots — the stride sample plus
  rasterization makes this a non-issue).
3. Alpha/size tuned for up to 200,000 sampled dots per panel.
3. Re-run to regenerate `chart-size-vs-density.svg` / `.png` here.

## Verification performed

- FINAL REAL-DATA run 2026-10-02 (post-freeze) against the shipped CSVs:
  strides 13 / 7 / 10 of 2 437 940 / 1 350 739 / 1 971 504 lists (gzip
  comment surfaced by the script); sampled complemented counts 2 / 1 / 0
  vs totals.complement 23 / 10 / 3 — the stride misses most of the rare
  complemented lists, so open-triangle markers are nearly absent. The
  dense-band size story is still exactly told by chart-footprint's
  per-density-bucket table (totals stay exact). Flagged to the Lead:
  a stratified CSV (all complemented rows + stride-sampled direct rows)
  would restore the marker dimension without changing totals.
- Pre-sampling full per_list run (earlier 2026-10-02): plottable lists
  equaled distinct_kvs per corpus; complemented 23 / 10 / 3 matching
  stats.json.
- Earlier synthetic-mode run rendered; `verify_layout.py` clean; three panels,
  parity line, shaded band, legend with counts confirmed in preview.
- Field names cross-checked against schema.md §2 (`per_list`).
