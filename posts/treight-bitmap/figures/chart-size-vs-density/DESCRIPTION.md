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

- Input: `posts/treight-bitmap/evidence/bench/totals-<corpus>.json` →
  `per_list` rows (schema.md §2 semantics, per-corpus file shape; see
  chart-footprint's note). Streamed via jq (`chartlib.iter_per_list`)
  because the three files total ~1.7 GB; json.load fallback without jq.
- Per row: `ratio = treight_blob_bytes / roaring_serialized_bytes`
  (rows with `roaring_serialized_bytes == 0` skipped — cannot divide);
  `density` and `complemented` used as-is. Note the ratio uses the BLOB
  bytes (no 5-B descriptor) — the per-list descriptor constant vanishes
  at log scale but is stated in the footnote for honesty.
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
2. Dots are drawn with `rasterized=True`: the ~5.7 M real dots become one
  embedded bitmap per panel inside the SVG (109 KB total) while all text
  stays vector/editable — without it the SVG was 919 MB of vector dots.
3. Alpha/size tuned for ~2.4 M dots per panel (real counts).
3. Re-run to regenerate `chart-size-vs-density.svg` / `.png` here.

## Verification performed

- REAL-DATA run 2026-10-02: plottable lists equal distinct_kvs per corpus
  (2 437 940 / 1 350 739 / 1 971 504); complemented counts 23 / 10 / 3,
  matching stats.json and chart-footprint's cross-checks.
- Earlier synthetic-mode run rendered; `verify_layout.py` clean; three panels,
  parity line, shaded band, legend with counts confirmed in preview.
- Field names cross-checked against schema.md §2 (`per_list`).
