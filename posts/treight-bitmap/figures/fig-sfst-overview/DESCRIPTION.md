# fig-sfst-overview — where treight bitmaps live in the SFST file

## Intent

The user-requested centerpiece: a rough-but-accurate overview of the SFST
file format showing how a `key=value` filter becomes bitmap bytes in the
file, across the low/mid/high cardinality tiers. Left: the file as it is
laid out top-to-bottom (header, TOC, chunk bodies + CRC, hot prefix vs
cold region). Right: the tier routing, one FST entry's anatomy
(`BitmapValue`), and the read-side query loop.

Placement: draft §5 (Where it lives: the SFST index lifecycle) — opens the section; also the user-requested overview.

## Accuracy notes (verified against sources — do not "fix" without them)

- ONLY low and mid tiers store treight bitmaps. `HF{i}` (high card, ≥ 1000
  values/field) is a **columnar string arena** (`keys_blob` + `key_lens` +
  per-key `u8` batch-membership mask) — schema.rs:640-642: "High-card fields
  store no bitmaps — their chunk is an arena, not an FST". The figure marks
  bitmap homes with a 🌳 glyph AND color, and states "no bitmaps" in text
  for HF.
- Tier boundaries: `T = DEFAULT_CARDINALITY_THRESHOLD = 100` (schema.rs);
  low < T, mid ∈ [T, 10·T), high ≥ 10·T (kv_interner.rs). Field-level
  decision at interning time.
- Chunk framing: 12-byte header (magic "SFST", version u32 LE = 1,
  num_chunks u32 LE); TOC = 12 B × (num_chunks + 1) entries (4-B chunk id +
  u64 LE absolute offset), final entry is the EOF sentinel; every body =
  payload + trailing crc32 (u32 LE, over stored/compressed bytes).
- Payload encoding: `bincode(standard)` → `zstd` — PRIM/MF at level 3, the
  rest at level 1 (FORMAT.md "Encoding").
- Canonical body order (producer's, not the format contract): SUMR, META,
  TIMS, PRIM, optional per-row columns, optional trace structures, MF{i},
  HF{i}, SB{i} (build.rs `build_into`).
- Hot prefix = SUMR + META + TIMS + PRIM (query open decodes SUMR+META+PRIM
  eagerly); PRIM closes it; everything after is one cold span.
- `BitmapValue { desc: treight::Bitmap, data: Vec<u8> }` — descriptor rides
  in the chunk schema (serde) beside the tree bytes; desc is `Copy`
  (universe_size u32, levels u8, inverted bool ⇒ ~5-6 B serialized).
- TIMS is `Vec<i64>` ns timestamps in the SAME chronological order as the
  SB rows; the bitmap universe is `SUMR.record_count`; positions are
  time-sorted row indexes (so cross-filter set ops align by construction).
- Read path: `PosSet::from_value` clones tree bytes once, then
  and/or/and_not/range natively — no roaring round-trip (index_reader.rs).

## Source provenance (sections of FORMAT.md used)

- FORMAT.md "File Layout" + "Header" + "Table of Contents" (framing, CRC,
  sentinel) — lines 26-97.
- FORMAT.md "Chunk Ids" (the id table, hot-prefix ordering, cold region,
  indexed ids `MF{hi}{lo}`/`HF{hi}{lo}`/`SB0{N}`) — lines 100-315.
- FORMAT.md "`PRIM` — Primary FST" (`PrefixMap<BitmapValue>`,
  time-sorted positions) — lines 481-493.
- FORMAT.md "`MF{i}` — Mid-card field FSTs" (same schema as PRIM) —
  lines 494-499.
- FORMAT.md "`HF{i}` — High-card field columnar (string arena)" —
  lines 501-537.
- FORMAT.md "`TIMS` — Per-log timestamps" — lines 539-558.
- FORMAT.md "Tier-Aligned IDs" (KvId ranges, META.id_ranges) —
  lines 592-623.
- FORMAT.md "Encoding" (bincode + zstd, level 3 for FST chunks) —
  lines 626-644.
- `src/crates/sfst/src/schema.rs:632-670` (`BitmapValue`, `HighField`),
  `:134-138` (`DEFAULT_CARDINALITY_THRESHOLD = 100`).
- `src/crates/sfst/src/kv_interner.rs:35-49,238-251` (tier boundaries).
- `src/crates/sfst/src/build.rs:1-27` (pipeline + canonical order),
  `:459-473` (hot prefix emission).
- `src/crates/sfst/src/index_reader.rs:1719-1823` (`PosSet`).

## How to regenerate

```
cd posts/treight-bitmap/figures/fig-sfst-overview
~/.cargo/bin/typst compile --format svg fig-sfst-overview.typ fig-sfst-overview.svg
~/.cargo/bin/typst compile --format png --ppi 144 fig-sfst-overview.typ fig-sfst-overview.png
```

## Manual-recreation guide (typst)

1. Fixed canvas 940 × 780 pt. Left column = the file map (a `pale` panel at
   dx 20, width 336 pt); right column = three stacked panels at dx 380,
   width 540 pt.
2. File map: two white bars for Header/TOC (fixed `width: 314pt` so they
   read as contiguous file regions), then a `#stack` of `chunk(id, desc)`
   rows — each row is a 52 pt id chip (mono bold, colored by role:
   orange = always-needed prefix, green = bitmap home, vermillion = high-card
   arena) + payload description. Bitmap homes additionally carry the 🌳
   glyph so the distinction survives greyscale.
3. Hot/cold brackets: two grids of `(3 pt colored bar, text)` — orange bar
   spanning SUMR..PRIM, sky bar for the cold suffix. Bars are thin; zoom the
   PNG when checking them.
4. Tier routing panel: a 3-column grid (tier / field has / on-disk home).
   The tier chips repeat the chunk colors. Boundaries footnote quotes T and
   10·T with the id_ranges sentence.
5. `BitmapValue` anatomy: three side-by-side boxes (desc / data / FST key)
   with the encoding-chain paragraph underneath.
6. Escapes: `\[` for literal brackets (`\[T, 10·T)`); `Vec\<i64\>` needs
   escaped angle brackets in markup; `MF\{i\}` braces escaped in code-ish
   prose.

## Verification performed

- Compiled clean (typst 0.14.1); 144 ppi PNG = 1880 × 1560; no clipping,
  no region collisions (`verify_layout.py`).
- Zoomed crops confirmed the hot/cold bracket bars render (they vanish at
  full-page preview downscale — they are only 3 pt wide).
- Every structural claim cross-checked against FORMAT.md sections listed
  above (see "Accuracy notes").

## Known limitations

- Chunk bodies are NOT drawn to byte scale (labels would be unreadable);
  order is the canonical producer order, which FORMAT.md explicitly notes
  is not the format contract.
- The optional per-row columns and trace chunks are collapsed into two
  rows ("OBTS…", "TIDX…") — the full id list is in the text.
- Programmatic verification only (no image input in the authoring
  environment).
