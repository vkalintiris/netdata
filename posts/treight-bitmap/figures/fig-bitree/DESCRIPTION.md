# fig-bitree — the treight 8-way bit-tree, one byte per node

## Intent

The structural anatomy figure for the blog's §4.2 ("Node bytes and the tree
shape"): shows that a treight bitmap is an 8-way tree where every internal
node is ONE byte whose 8 bits mean "child i present", the bottom level's
bytes are leaves holding 8 value bits each, and empty subtrees are pruned
(canonical form). Uses a deliberately tiny universe (512 = 8³, 3 levels) so
the whole tree is drawable; the blog's real corpora use 262,144 = 8⁶ → the
same shape, 6 levels deep.

## What it shows

- A worked example set {5, 9, 173} over universe 512; the serialized tree is
  `05 03 20 02 20 20` — 6 bytes (1 root + 2 dl1 nodes + 3 leaves), vs 9 naive
  bytes (3 values × 3 levels): shared prefixes are shared bytes.
- The highlighted path for v = 173 = 0o255 (root digit 2 → dl1 digit 5 →
  leaf bit 5 → value 168 + 5).
- Node-byte anatomy (root byte `05` = children {0, 2}).
- Leaf-byte anatomy (leaf spanning [168, 176) = byte `20` = {173}).
- A dashed "pruned" ghost: root bit 7 = 0 → no subtree, no byte stored.
- The level table: dl ↔ subtree height ↔ which 3 bits of the value that
  level consumes (`child_index(dl, v) = (v >> 3·dl) & 7`).

## Verify the numbers (do not draw from memory)

Example set {5, 9, 173}, universe 512, levels = 3:

```
value  dl2  dl1  dl0(bit)
5       0    0    5
9       0    1    1        # 9 = 8+1 → leaf [8,16) bit 1
173     2    5    5        # 173 = 2·64 + 5·8 + 5 → leaf [168,176) bit 5
```

- root byte: children {0, 2} → `05`
- dl1 node 0 (covers [0,64)): children {0,1} → `03`
- dl1 node 2 (covers [128,192)): child {5} → `20`
- leaves: [0,8) bit 5 → `20`; [8,16) bit 1 → `02`; [168,176) bit 5 → `20`
- pre-order bytes: `05 03 20 02 20 20` — **6 bytes** (1 root + 2 inner + 3
  leaves). Three isolated values cost 3 × levels = 9 naive bytes; sharing the
  root and (for 5, 9) a dl1 node saves 3 → 6. The figure's byte strip shows
  exactly these six bytes in this order.

## Source provenance (file:line as of this figure)

- `src/crates/treight/src/lib.rs:65-86` — `ceil_log8` (universe → depth;
  8³ = 512 → 3 levels; u32 ⇒ ≤ 11 levels; 8⁶ = 262,144 ⇒ 6 levels).
- `src/crates/treight/src/node.rs:1-45` — node-byte vocabulary: bit i ⇔
  child i present; pre-order layout; `child_index`/`child_offset`; a child
  at level covers value bits `[3·level, 3·level+3)`; level = subtree height
  in node bytes (1 = single leaf byte).
- `src/crates/treight/src/node.rs:47-84` — `Node` (has_child/count_children/
  min_child/max_child/bits); leaves are node bytes whose bits are value bits.
- `src/crates/treight/src/raw.rs:59-115` — `from_sorted_iter` (how the bytes
  in the strip are actually produced: per-level open-a-byte / OR-the-bit).
- `src/crates/treight/src/lib.rs:88-149` — `estimate_data_size` (canonical
  size = one byte per distinct (level, node-index) pair + root; the
  "3 values, 6 bytes" arithmetic above).

## How to regenerate

```
cd posts/treight-bitmap/figures/fig-bitree
~/.cargo/bin/typst compile --format svg fig-bitree.typ fig-bitree.svg
~/.cargo/bin/typst compile --format png --ppi 144 fig-bitree.typ fig-bitree.png
```

## Manual-recreation guide (typst)

The file is a single self-contained typst canvas. To redo/tweak by hand:

1. **Canvas**: `#set page(width: 940pt, height: 680pt, margin: 0pt)` — change
   the height first if you add rows; everything is absolutely placed.
2. **Palette** (Okabe-Ito, colorblind-safe): `orange #e69f00` = root/structure,
   `sky #56b4e9` = highlighted path, `green #009e73` = leaves,
   `vermil #d55e00` = value-bit accents, `gray #9a9a9a` = pruned ghosts.
   Color is never the only cue: pruned = dashed + ∅ glyph; path = thick edges
   + bold vermillion child-index labels; set bits = filled squares with ●.
3. **Node chips** (`nodechip` helper): a rounded box showing the hex byte in
   `DejaVu Sans Mono` bold with a caption underneath. Leaves are green (the
   v=173 leaf is vermillion to bind it to the highlighted path); internal
   nodes sky; root orange. To move a node, change its `#place(dx:, dy:)`.
4. **Edges**: plain `#place(...)[#line(start:, end:, stroke:)]` segments.
   The highlighted edge uses `3.4pt + sky`; ordinary edges `1.2pt + ink`;
   the pruned edge is dashed gray. Child-index labels sit in small
   white-backed boxes ON the edges (bold vermillion on the highlighted path).
5. **Bit cells** (`bitcell` helper): 17pt × 21pt squares, one per bit, top
   label = bit number (node anatomy) or base+b value (leaf anatomy), filled
   square + ● when the bit is set. Two `#grid(columns: (auto,)*8)` rows render
   them left-to-right MSB-first (bit 7 … bit 0).
6. **Escaping gotcha**: literal `[` inside typst markup must be written
   `\[` (e.g. `Leaf \[168, 176)`). Unescaped it opens a nested content block
   and the file stops compiling with "unclosed delimiter".
7. **Numbers to keep in sync if you change the example set**: the level table
   (left rail), the three leaf captions, the root/dl1 hex bytes, the byte
   strip at the bottom, and the path annotation — all derive from the table
   in "Verify the numbers" above; recompute with `ceil_log8` /
   `estimate_data_size` semantics (or just count distinct nodes: every value
   contributes its root-to-leaf chain; shared prefixes share bytes).

## Verification performed

- Compiled clean (typst 0.14.1) to SVG + PNG (144 ppi ⇒ 1880×1360).
- `scripts/verify_layout.py`: ink margins ≥ 28 px on all four edges (no
  clipping), no saturated-region pixel overlap. Coarse ASCII preview
  confirmed: root chip top-center, two dl1 chips, three leaf chips, dashed
  ghost, three right panels, bottom byte strip.
- Numbers cross-checked by hand against `estimate_data_size` semantics
  (see "Verify the numbers").

## Known limitations

- Fine typography (kerning, exact edge-to-chip attachment) was verified
  programmatically, not by eye — the authoring model had no image input;
  expect to nudge `dx/dy` by a few points when refining by hand.
- The pruned ghost shows ONE missing child (bit 7); bits 1, 3–6 are also
  unset in `05` — mentioned in the panel text rather than drawn.
