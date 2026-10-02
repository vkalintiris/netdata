# treight blog — figures

All figures for the treight blog post. Static diagrams are typst (user's
preferred tool); data charts are scripted matplotlib reading bench evidence.
Every figure directory contains the source, SVG + PNG exports, and a
DESCRIPTION.md with intent, source provenance (file:line), and
manual-recreation instructions for hand-refinement.

## Regenerate everything

```
cd posts/treight-bitmap/figures
for d in fig-*; do (cd "$d" && ~/.cargo/bin/typst compile --format svg "$d.typ" "$d.svg" \
  && ~/.cargo/bin/typst compile --format png --ppi 144 "$d.typ" "$d.png"); done
MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-density.py     # etc. per chart
```

## Static figures (typst 0.14.1)

| Figure | Purpose | Sources |
|---|---|---|
| `fig-sfst-overview` | The user-requested centerpiece: the SFST file top-to-bottom (header/TOC/bodies+CRC, hot prefix vs cold region) and how `key=value` posting lists land per cardinality tier — low→`PRIM`, mid→`MF{i}` as treight `BitmapValue`s; high→`HF{i}` columnar arena with u8 batch masks, **no bitmaps**; `TIMS`/`SUMR.record_count` define the position universe. | `sfst/FORMAT.md` (File Layout, Header, TOC, Chunk Ids, PRIM/MF{i}/HF{i}/TIMS, Tier-Aligned IDs, Encoding); `sfst/src/schema.rs` (`BitmapValue`, `HighField`, T=100); `sfst/src/kv_interner.rs` (T, 10·T); `sfst/src/build.rs`; `sfst/src/index_reader.rs` (`PosSet`) |
| `fig-bitree` | The 8-way bit-tree anatomy: one byte per node (bit i = child i present), leaves as value-bit bytes, level→digit mapping (`child_index`), pruned empty subtrees, canonical form. Universe 512 drawable example. | `treight/src/lib.rs` (`ceil_log8`, `estimate_data_size`); `treight/src/node.rs`; `treight/src/raw.rs` (`from_sorted_iter`) |
| `fig-serialization` | The pre-order walk of the Lead-verified example: {5, 7, 61, 64, 4097, 262143} over 262,144 → 18 tree bytes + the 26-byte wire form, tree rows and wire cells tied by blob offsets 0x00–0x11 (labeling matches the draft's committed dump). | `treight/src/raw.rs` (`from_sorted_iter`, `serialize_into`/`deserialize_from`); `treight/src/node.rs` (pre-order, `skip_subtree`); `treight/src/lib.rs` (`estimate_data_size`) |
| `fig-setops` | Two trees walked in lockstep (AND + SUB on a 64-value universe): leaf byte ops, A-only skip vs copy, reserve-node-byte / back-patch / prune, and the verbatim four-walker table. | `treight/src/ops.rs` (`intersect_subtree`, `difference_subtree`, `copy_subtree`, four-walker table); `treight/src/raw.rs` (wrappers, asserts) |
| `fig-complement` | Dense sets store their holes (60-of-64 → 4 tree bytes); the De Morgan dispatch tables for `and`/`or`; short-circuits; `and_not` = `and(complement)`; inverted read costs (O(universe) iteration). | `treight/src/bitmap.rs` (`Bitmap`, dispatch tables + match arms, `ComplementIter`) |
| `fig-lifecycle` | The index lifecycle: WAL arrival order → roaring accumulation → seal-time time remap → density-aware encode (`max(universe/64, 256)` bitset threshold; complement > 50%) → on-disk `BitmapValue` → mmap'd native `PosSet` ops. | `sfst/src/row_index.rs`; `sfst/src/build.rs` (`remap_one_bitmap`, `batch_mask`); `sfst/src/index_reader.rs` (`PosSet`); `sfst/src/bitset.rs` |

## Data charts (matplotlib, scripted)

| Chart | Purpose | Evidence input |
|---|---|---|
| `chart-density` | Posting-list density histogram per corpus (log-x buckets incl. the >50% complement band). | `evidence/bench/stats.json` per corpus |
| `chart-footprint` | Per-corpus total bytes, treight vs roaring serialized (both scopes: SFST-materialized low+mid, and all lists). | `evidence/bench/totals.json` |
| `chart-ops` | Per-op CPU comparison, treight vs roaring (grouped bars; per-position/per-list normalizations from the bench). | `evidence/bench/results.json` |
| `chart-size-vs-density` | Scatter of per-posting-list size ratio (treight/roaring) vs density, colored by complement choice. | `evidence/bench/totals.json` → `per_list` |

Charts read `posts/treight-bitmap/evidence/bench/` when present and fall
back to clearly-marked synthetic sample data otherwise (the scripts print
which mode they ran in; synthetic mode also stamps the plot title).
Schemas: `evidence/bench/schema.md` (authored by bench-eng).

## Conventions

- Palette: Okabe-Ito (colorblind-safe, monochrome-printable). Color is
  never the only cue — ordinals/offsets, ●/×/∅ glyphs, dashed borders, and
  text labels carry the same distinctions.
- Blog-ready exports: SVG primary, PNG fallback at 144 ppi.
- `scripts/verify_layout.py FILE.png` — text-channel layout QA (edge
  clipping, region collisions, ASCII preview); used on every export.
- The authoring model had no image input; visual QA was programmatic
  (clipping/collision/preview) — expect minor hand-nudges when refining.
