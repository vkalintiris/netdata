# fig-serialization — the pre-order walk, byte by byte

## Intent

The blog's §"Pre-order serialization, by hand": walks the Lead-verified
example — posting list {5, 7, 61, 64, 4097, 262143} over universe 262,144
(= 8⁶, 6 levels) — showing the tree WITH its node bytes AND the flat wire
form aligned under it (blob offsets 0x00–0x11 tie every tree row to every
wire cell). The figure is the visual companion to the draft's hand-worked
dump.

Placement: draft §4.3 (Pre-order serialization, by hand).

## The verified data (do not change)

- Tree bytes (18, pre-order):
  `81 03 01 03 81 a0 20 01 01 01 01 01 02 80 80 80 80 80`
- Wire form (26 = 8-byte header + tree):
  `00 00 04 00 | 12 00 00 00 | 81 03 01 03 81 a0 20 01 01 01 01 01 02 80 80 80 80 80`
  (header: universe_size = 262,144 u32 LE; tree_len = 18 u32 LE)
- Tree shape (offset · dl · indentation = depth; offsets match the wire order):

```
0x00 5 81 root            children {0, 7}
0x01 4 03 ├─0             children {0, 1}          dl4
0x02 3 01 │  ├─0          child {0}                dl3
0x03 2 03 │  │  └─0       children {0, 1}          dl2
0x04 1 81 │  │     ├─0    children {0, 7}          dl1
0x05 0 a0 │  │     │  ├─0 leaf [0, 8)      → 5, 7
0x06 0 20 │  │     │  └─7 leaf [56, 64)    → 61
0x07 1 01 │  │     └─1    child {0}                dl1
0x08 0 01 │  │        └─0 leaf [64, 72)    → 64
0x09 3 01 │  └─1          child {0}                dl3
0x0A 2 01 │     └─0       child {0}                dl2
0x0B 1 01 │        └─0    child {0}                dl1
0x0C 0 02 │           └─0 leaf [4096, 4104) → 4097
0x0D 4 80 └─7             child {7}                dl4
0x0E 3 80    └─7          child {7}                dl3
0x0F 2 80       └─7       child {7}                dl2
0x10 1 80          └─7    child {7}                dl1
0x11 0 80             └─7 leaf [262136, 262144) → 262143
```

Row labels (blob offset + dl level) and the 18-row shape match the
committed draft dump exactly (writer's dump at commit 9b744ee5ec;
independently re-derived against the flat bytes).

## Source provenance

- `src/crates/treight/src/raw.rs:59-115` — `from_sorted_iter` (how these
  bytes are produced: per-level open-a-byte / OR-the-bit, single pass).
- `src/crates/treight/src/raw.rs:273-312` — `serialize_into` /
  `deserialize_from` / `serialized_size` (the 8-byte header; levels
  re-derived by `ceil_log8`, never stored).
- `src/crates/treight/src/node.rs:1-11,26-45` — pre-order layout (node byte
  first, present children ascending; `skip_subtree` = walking, not offsetting).
- `src/crates/treight/src/lib.rs:88-149` — `estimate_data_size` (the
  6 + 0 + 1 + 2 + 4 + 5 = 18 accounting in the side panel).

## How to regenerate

```
cd posts/treight-bitmap/figures/fig-serialization
~/.cargo/bin/typst compile --format svg fig-serialization.typ fig-serialization.svg
~/.cargo/bin/typst compile --format png --ppi 144 fig-serialization.typ fig-serialization.png
```

## Manual-recreation guide (typst)

1. Fixed canvas 940 × 800 pt, margin 0; everything absolutely placed.
2. Branch colors (Okabe-Ito) carry the tree↔wire mapping, and every byte
   ALSO prints its blob offset (0x00..0x11) so the mapping survives
   greyscale printing: root `81` orange #e69f00 · left-cluster inner
   #0072b2 blue · leaves #009e73 green · 64-branch #56b4e9 sky ·
   4097-branch #d55e00 vermillion · 262143 chain #cc79a7 reddish purple.
   Light fills (sky, rpurple, orange) get dark text; dark fills get white
   text. The row label scheme (offset + dl) matches the draft's committed
   dump so figure and prose read identically.
3. The tree is a `#stack` of `row(...)` grids: column 1 = ordinal,
   column 2 = byte chip (`bchip`: rounded mono box), column 3 = connector
   art in mono (`├─0`, `│  └─7` …), column 4 = annotation. Depth is faked
   by the connector string's leading spaces — to add a node, extend its
   connector and insert the row in stack order (pre-order = the order the
   bytes appear).
4. Wire cells (`wcell`): 25 pt-wide boxes, hex on top / blob offset below
   (mono 7 pt); header cells white with no offset. The strip is a
   `#stack(dir: ltr)` — 4 + 4 header cells, 10pt spacer, 18 tree cells,
   another spacer, 5 purple cells; the labels grid underneath names each
   group.
5. Literal `[` in prose must be escaped `\[` (e.g. `leaf \[0, 8)`) or typst
   opens a nested content block and the file breaks.
6. If you change the example set, regenerate: the digit table, the 18 rows,
   the wire strip, and the "why 18 bytes" panel — all follow from
   decomposing each value into base-8 digits (the table at top right).

## Verification performed

- Compiled clean (typst 0.14.1); 144 ppi PNG = 1880 × 1600.
- `verify_layout.py`: no edge clipping, no region collisions; zoomed crop of
  the wire strip confirmed 8 header + 18 colored cells with the expected
  color runs (4 blue, 3 green, 4 vermillion, 5 purple) and spacers.
- Byte-for-byte match with the Lead-verified flat bytes and wire header;
  tree independently re-derived twice (see NOTE above).

## Known limitations

- Connector art alignment (the `│` glyphs lining up under a chip) is
  mono-font dependent; DejaVu Sans Mono is set explicitly, but hand-tuning
  the connector strings is normal if you change fonts.
- Fine typography verified programmatically only (no image input in the
  authoring environment).
