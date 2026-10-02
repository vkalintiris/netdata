# fig-setops — lockstep subtree walkers: the AND (and SUB) walk

## Intent

The blog's §"One walker, four operations": two treight trees walked in
lockstep by one recursive shape, differing only in (leaf op, A-only column,
B-only column). Shows the mechanics the prose names — reserve node byte,
back-patch surviving child bits, prune empty results — on a universe small
enough to draw every byte (64 = 8², 2 levels: root + leaves).

## The verified data (do not change)

- A = {0..7} ∪ {16} ∪ {40} → root `25` (children {0, 2, 5}), leaves
  `ff` [0, 8) = {0..7}, `01` [16, 24) = {16}, `01` [40, 48) = {40};
  bytes `25 ff 01 01`.
- B = {1, 5, 16, 17} → root `05` (children {0, 2}), leaves `22` [0, 8) =
  {1, 5}, `03` [16, 24) = {16, 17}; bytes `05 22 03`.
- A ∩ B: child 0 → `ff & 22` = `22` ✓; child 2 → `01 & 03` = `01` ✓;
  child 5 → A-only → `a.skip_subtree()`; root back-patched to {0, 2} = `05`.
  Result `05 22 01` = {1, 5, 16}.
- A − B: child 0 → `ff & !22` = `dd` ✓; child 2 → `01 & !03` = `00` →
  leaf pruned (byte popped, bit never set); child 5 → A-only →
  `copy_subtree` verbatim; root back-patched to {0, 5} = `21`.
  Result `21 dd 01` = {0, 2, 3, 4, 6, 7} ∪ {40}.

The example extends the brief's suggested operands ({0..7} ∪ {16} and
{1, 5, 16, 17}) with one extra A-only leaf (40) so the skip-vs-copy
distinction is actually exercised; at 2 levels the only possible node kinds
are root and leaf, so every walk mechanic is visible.

## Source provenance

- `src/crates/treight/src/ops.rs:183-199` — the four-walker table this
  figure reproduces (OR/AND/SUB/XOR × leaf op, A-only child, B-only child).
- `src/crates/treight/src/ops.rs:240-293` — `intersect_subtree` (the
  reserve → walk → back-patch/pop shape; A-only = `a.skip_subtree`).
- `src/crates/treight/src/ops.rs:299-351` — `difference_subtree` (A-only =
  `copy_subtree` + bit set; the zero-leaf prune).
- `src/crates/treight/src/ops.rs:174-181` — `copy_subtree` = skip +
  verbatim byte-range append (no re-encode).
- `src/crates/treight/src/raw.rs:529-589` — the `intersect`/`difference`
  wrappers: empty-blob short-circuits, unconditional universe
  `assert_eq!` before dispatch.

## How to regenerate

```
cd posts/treight-bitmap/figures/fig-setops
~/.cargo/bin/typst compile --format svg fig-setops.typ fig-setops.svg
~/.cargo/bin/typst compile --format png --ppi 144 fig-setops.typ fig-setops.png
```

## Manual-recreation guide (typst)

1. Fixed canvas 940 × 710 pt, margin 0. Three tree panels across the top
   (A, B, result), the lockstep-walk table bottom-left, the four-op table
   bottom-right, results strip at the very bottom.
2. `leafchip(hexbyte, range, vals, color, tag: ...)` draws each leaf:
   rounded box, hex byte (mono bold), the leaf's value range + members, and
   an optional vermillion tag ("only in A"). The `dashed: true` variant is
   reserved for skipped/pruned ghosts (with an × glyph).
3. `rootchip(hexbyte)` is the orange root chip; edges are `#place`d
   `#line`s from root bottom to leaf tops, each labeled with its child
   index in a pale-backed box.
4. The lockstep table is a `#grid` with columns child/in A/in B/AND action/
   SUB action — one row per occupied child slot (0, 2, 5) plus a condensed
   row for slots in neither operand. `●` marks presence, `—` absence.
5. The four-op table reproduces ops.rs's comment table verbatim; keep it
   byte-identical to the source comment if the source ever changes.
6. Literal `[` in prose must be escaped `\[` (`leaf \[0, 8)`); footers and
   long lines go inside `#box(width: 900pt)[...]` so they wrap instead of
   running off-canvas.
7. If you change the operands, recompute: root bytes (present-child sets),
   leaf hexes, both result strips, and every row of the walk table — all
   follow from applying the byte ops at child slots 0..7.

## Verification performed

- Compiled clean (typst 0.14.1); 144 ppi PNG = 1880 × 1420.
- `verify_layout.py`: margins ≥ 24 px on all edges (after wrapping the
  footer, which initially ran past the right edge — the exact failure mode
  the clip check exists for); no saturated-region pixel overlap.
- All six byte strings in this DESCRIPTION cross-checked by hand against
  `intersect_subtree`/`difference_subtree` semantics (see above).

## Known limitations

- Fine typography verified programmatically only (no image input in the
  authoring environment); edge-label positions are hand-placed and may
  need nudging.
- The XOR and OR walkers are shown only in the four-op table, not walked
  step-by-step (the blog walks AND; the table generalizes).
