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

- Input: `posts/treight-bitmap/evidence/bench/totals.json` →
  `corpora.<name>.per_list` rows (schema.md §2).
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
2. Alpha/size chosen for ~10⁵ dots per corpus; increase `s` if the final
  list counts are smaller.
3. Re-run to regenerate `chart-size-vs-density.svg` / `.png` here.

## Verification performed

- Synthetic-mode run rendered; `verify_layout.py` clean; three panels,
  parity line, shaded band, legend with counts confirmed in preview.
- Field names cross-checked against schema.md §2 (`per_list`).
