# fig-lifecycle — the index lifecycle: roaring accumulates, treight stores

## Intent

The blog's §"Where it lives: the SFST index lifecycle": the right-tool-per-
phase story. WRITE = roaring bitmaps absorbing arbitrary-order inserts in
insertion order; SEAL = the time remap plus the two density-aware encode
decisions; READ = mmap + native `PosSet` ops. A 3-row toy (`service=x` on
rows 0 and 2) is carried through all three stages, including its byte form
at the end.

## The verified data (toy example — keep consistent if edited)

- Arrival: row 0 (t=37, `service=x`), row 1 (t=12, `env=y`), row 2 (t=95,
  `service=x`).
- Accumulate: `kv_bitmaps["service=x"]` = roaring {0, 2} (insertion-order
  positions).
- Remap: `time_order` sorts by ts → row 1 → pos 0, row 0 → pos 1,
  row 2 → pos 2; so `to_sorted`: 0→1, 1→0, 2→2; bitmap {0, 2} → {1, 2}.
- Encode decisions for this list (universe 3): cardinality 2 < max(3/64,
  256) = 256 → sorted-vec path; density 2/3 > 1/2 → COMPLEMENT:
  `from_sorted_iter_complemented` over holes {0} → desc {universe 3,
  inverted true} + tree of {0}.
- Real corpora note: universe = `record_count` = 262,144 = 8⁶ → 6 levels;
  thresholds unchanged (the 256 floor keeps small bitmaps on the vec path).

## Source provenance

- `src/crates/sfst/src/row_index.rs:1-33` — Phase 1 structures +
  insertion-order contract (positions number rows in read order; Phase 2
  remaps to chronological).
- `src/crates/sfst/src/row_index.rs:47-60` — `kv_bitmaps:
  Vec<RoaringBitmap>`, `row_entries`, `timestamps`.
- `src/crates/sfst/src/build.rs:598-663` — `remap_one_bitmap`: bitset
  threshold `max(universe/64, 256)`, complement above `universe/2`, both
  encoding paths, empty short-circuit.
- `src/crates/sfst/src/build.rs:246-259` — `batch_mask` (high-card: roaring
  → time remap → u8 batch mask, no treight).
- `src/crates/sfst/src/build.rs:309-323` — `build_into` stage order.
- `src/crates/sfst/src/index_reader.rs:1749-1802` — `PosSet::from_value`
  (clone bytes once), `or_assign`/`and_assign`/`and_not_assign` buffer-swap
  convention, `range`.
- `src/crates/sfst/src/bitset.rs` — the scratch `Bitset`.
- `src/crates/sfst/FORMAT.md` "Encoding" — bincode + zstd (FST chunks at
  level 3).

## How to regenerate

```
cd posts/treight-bitmap/figures/fig-lifecycle
~/.cargo/bin/typst compile --format svg fig-lifecycle.typ fig-lifecycle.svg
~/.cargo/bin/typst compile --format png --ppi 144 fig-lifecycle.typ fig-lifecycle.png
```

## Manual-recreation guide (typst)

1. Fixed canvas 940 × 760 pt. Three phase columns (WRITE x20 w288, SEAL
   x334 w288, READ x648 w272) with big gray → arrows between; a full-width
   "same posting list at each stage" strip at the bottom; footer.
2. `stagechip(label, color)` renders each column's header chip — blue for
   the roaring phase, vermillion for seal, green for read; the phase names
   are in the text, so the story survives greyscale.
3. The WAL table and the remap table are small `#grid`s with
   `grid.hline` separators; `●` marks membership. Arrows between columns
   are plain text glyphs (→).
4. The decision box inside SEAL is a nested white box quoting
   `rem_one_bitmap`'s two decisions — keep the thresholds verbatim
   (`max(universe/64, 256)`, `> universe/2`); they are structural facts
   the blog states directly.
5. Braces in prose need escaping when they'd look like code: `\{0, 2\}`;
   literal brackets need `\[`.
6. If you change the toy, update: the WAL table, both bitmaps, the remap
   table, the two decisions, and the bottom strip — all derive from the
   arrival timestamps (see "verified data").

## Verification performed

- Compiled clean (typst 0.14.1); 144 ppi PNG = 1880 × 1520; no clipping,
  no region collisions (`verify_layout.py`); ASCII preview confirmed the
  three columns, both arrows, the decision box, the bottom strip, footer.
- Toy arithmetic re-derived by hand (remap and both decisions above).

## Known limitations

- The toy's 3-row universe makes the density decision trivially "complement";
  real files have 250K rows — stated in the note under the decision box so
  no reader mistakes the toy for a realistic density.
- Programmatic verification only (no image input in the authoring
  environment).
