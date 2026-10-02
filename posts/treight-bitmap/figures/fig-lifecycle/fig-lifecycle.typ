// fig-lifecycle — the index lifecycle: roaring accumulates, treight stores.
//
// WRITE (RowIndex, insertion order)  →  SEAL (build.rs: time remap +
// density-aware encode)  →  READ (mmap + native PosSet ops).
//
// Sources: src/crates/sfst/src/row_index.rs (Phase 1 structures, insertion
//          order contract), src/crates/sfst/src/build.rs (build_into order,
//          remap_one_bitmap: bitset threshold max(universe/64, 256),
//          complement above 50%, batch_mask for high-card),
//          src/crates/sfst/src/index_reader.rs (PosSet, buffer-swap
//          convention), src/crates/sfst/src/bitset.rs (Bitset scratch).
// Palette: Okabe-Ito; roaring vs treight stages are labeled in text
//          everywhere (color is reinforcement only).
#set page(width: 940pt, height: 760pt, margin: 0pt)
#set text(font: "DejaVu Sans", size: 10pt, fill: rgb("1a1a1a"))
#set par(leading: 0.6em)

#let ink     = rgb("1a1a1a")
#let orange  = rgb("e69f00")
#let blue    = rgb("0072b2")
#let green   = rgb("009e73")
#let sky     = rgb("56b4e9")
#let vermil  = rgb("d55e00")
#let gray    = rgb("9a9a9a")
#let pale    = rgb("f4f4f2")

#let stagechip(txt, fillc, textfill: white) = box(
  radius: 3pt, stroke: 0.9pt + ink, fill: fillc, inset: (x: 5pt, y: 2.5pt),
  text(size: 9.5pt, weight: "bold", fill: textfill)[#txt],
)

#place(dx: 20pt, dy: 12pt)[
  #text(size: 16.5pt, weight: "bold")[The index lifecycle: roaring accumulates, treight stores]
  #v(2pt)
  #text(size: 10pt, fill: ink.lighten(30%))[
    right tool per phase — mutable roaring at write time, immutable zero-copy treight on disk and on the read path · worked micro-example: 3 rows, `service=x` on rows 0 and 2
  ]
]

// ---- column 1: WRITE --------------------------------------------------------
#place(dx: 20pt, dy: 64pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 288pt, fill: pale)[
    #stagechip("1 · WRITE — roaring's home turf", blue)
    #v(6pt)
    #text(size: 8.4pt)[
      the producer decodes WAL frames and calls `RowIndex::row(ts, tokens)` per record — positions number rows in *arrival* order:
    ]
    #v(5pt)
    #grid(columns: (auto, auto, auto, 1fr), column-gutter: 6pt, row-gutter: 3pt,
      align: center + horizon,
      text(size: 8pt)[*row*], text(size: 8pt)[*ts*], text(size: 8pt)[*has service=x*], [],
      grid.hline(stroke: 0.5pt + gray),
      text(size: 9pt, font: "DejaVu Sans Mono")[0], text(size: 9pt, font: "DejaVu Sans Mono")[t=37], text(size: 9pt)[●], [],
      text(size: 9pt, font: "DejaVu Sans Mono")[1], text(size: 9pt, font: "DejaVu Sans Mono")[t=12], text(size: 9pt, fill: gray)[—], text(size: 7.6pt)[(env=y)],
      text(size: 9pt, font: "DejaVu Sans Mono")[2], text(size: 9pt, font: "DejaVu Sans Mono")[t=95], text(size: 9pt)[●], [],
    )
    #v(6pt)
    #text(size: 8.4pt)[
      • one `RoaringBitmap` per interned `key=value` (`kv_bitmaps[slot]`), bit set at the arrival position \
      • timestamps arrive out of order — the file will not \
      • arbitrary-order inserts: exactly what roaring is built for
    ]
  ]
]

#place(dx: 306pt, dy: 150pt)[#text(size: 20pt, weight: "bold", fill: gray)[→]]

// ---- column 2: SEAL -----------------------------------------------------------
#place(dx: 334pt, dy: 64pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 288pt, fill: pale)[
    #stagechip("2 · SEAL — remap + density-aware encode", vermil)
    #v(6pt)
    #text(size: 8.4pt)[
      *time remap*: `time_order` sorts rows by timestamp; every position translates insertion → chronological (`to_sorted`):
    ]
    #v(4pt)
    #grid(columns: (auto, auto, auto), column-gutter: 6pt, row-gutter: 3pt,
      align: center + horizon,
      text(size: 8pt, fill: ink.lighten(20%))[insertion], text(size: 8pt, fill: ink.lighten(20%))[→ time], text(size: 8pt, fill: ink.lighten(20%))[row (ts)],
      grid.hline(stroke: 0.5pt + gray),
      text(size: 9pt, font: "DejaVu Sans Mono")[0], text(size: 9pt, font: "DejaVu Sans Mono")[1], text(size: 8pt)[row 0 (t=37)],
      text(size: 9pt, font: "DejaVu Sans Mono")[1], text(size: 9pt, font: "DejaVu Sans Mono")[0], text(size: 8pt)[row 1 (t=12)],
      text(size: 9pt, font: "DejaVu Sans Mono")[2], text(size: 9pt, font: "DejaVu Sans Mono")[2], text(size: 8pt)[row 2 (t=95)],
    )
    #v(4pt)
    #text(size: 8.4pt)[`service=x` bitmap \{0, 2\} → \{1, 2\} — ascending, as treight requires]
    #v(6pt)
    #box(radius: 3pt, stroke: 0.8pt + gray, inset: 6pt, fill: white)[
      #text(size: 8.6pt, weight: "bold")[`remap_one_bitmap` — two independent decisions]
      #v(3pt)
      #grid(columns: (auto, 1fr), column-gutter: 6pt, row-gutter: 3pt,
        align: horizon,
        text(size: 8pt)[*path, by cardinality*], text(size: 8.2pt)[card ≥ max(universe/64, 256) → scratch `Bitset` (O(n) set + ascending scan); else collect + sort a vec],
        text(size: 8pt)[*form, by density*], text(size: 8.2pt)[card > universe/2 → `from_sorted_iter_complemented` (store the holes, `inverted = true`); else `from_sorted_iter`],
      )
      #v(3pt)
      #text(size: 7.8pt, fill: ink.lighten(20%))[toy: 2 of 3 rows > 50% → complement (holes \{0\}); real corpora: universe = record_count = 262,144 = 8⁶ → 6 levels]
    ]
    #v(5pt)
    #text(size: 8.4pt)[
      result: `BitmapValue { desc, data }` → `PRIM` / `MF{i}` (FST chunks, zstd-3). High-card fields: no bitmaps — one `u8` batch mask per value (`batch_mask`).
    ]
  ]
]

#place(dx: 620pt, dy: 150pt)[#text(size: 20pt, weight: "bold", fill: gray)[→]]

// ---- column 3: READ -------------------------------------------------------------
#place(dx: 648pt, dy: 64pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 272pt, fill: pale)[
    #stagechip("3 · READ — treight's home turf", green)
    #v(6pt)
    #text(size: 8.4pt)[
      the `.sfst` file is mmapped; `IndexReader::open` decodes SUMR + META + PRIM eagerly — the bitmap bytes are already the query structure:
    ]
    #v(5pt)
    #stack(spacing: 4pt,
      box(radius: 3pt, stroke: 0.8pt + gray, inset: 5pt, fill: white)[
        #text(size: 8.4pt)[*`PosSet::from_value`* — `desc` is `Copy`; the tree bytes are cloned once, then owned by the set]
      ],
      box(radius: 3pt, stroke: 0.8pt + gray, inset: 5pt, fill: white)[
        #text(size: 8.4pt)[*`and_assign` / `or_assign` / `and_not_assign`* — native treight walkers over the two blobs; result bytes appended to a fresh `out`, then buffers swapped]
      ],
      box(radius: 3pt, stroke: 0.8pt + gray, inset: 5pt, fill: white)[
        #text(size: 8.4pt)[*`range`* — `Bitmap::from_range` (complement-aware) intersected with the filter set]
      ],
    )
    #v(5pt)
    #text(size: 8.4pt)[
      no roaring round-trip anywhere on the read path; the tree bytes are
      never decoded into another structure — contains/len/min/max walk them
      as-is, and `service=x AND level=error` is one lockstep walk over
      stored bytes.
    ]
  ]
]

// ---- bottom strip: the bytes at each stage ---------------------------------------
#place(dx: 20pt, dy: 560pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 900pt, fill: white)[
    #text(size: 10pt, weight: "bold")[THE SAME POSTING LIST AT EACH STAGE — `service=x` over the 3-row toy]
    #v(6pt)
    #grid(
      columns: (1fr, 1fr, 1fr, 1fr),
      column-gutter: 10pt,
      align: horizon,
      [
        #text(size: 8.2pt, weight: "bold")[in the WAL (arrival order)]
        #v(3pt)
        #text(size: 8pt, font: "DejaVu Sans Mono")[rows 0, 2 hold it\ timestamps t=37, t=95]
      ],
      [
        #text(size: 8.2pt, weight: "bold")[accumulated (roaring)]
        #v(3pt)
        #text(size: 8pt, font: "DejaVu Sans Mono")[\{0, 2\}]
        #v(2pt)
        #text(size: 7.6pt, fill: ink.lighten(20%))[insertion-order bits]
      ],
      [
        #text(size: 8.2pt, weight: "bold")[remapped (time order)]
        #v(3pt)
        #text(size: 8pt, font: "DejaVu Sans Mono")[\{1, 2\}]
        #v(2pt)
        #text(size: 7.6pt, fill: ink.lighten(20%))[ascending; dense (2/3) → complement decided here]
      ],
      [
        #text(size: 8.2pt, weight: "bold")[on disk (treight)]
        #v(3pt)
        #text(size: 8pt, font: "DejaVu Sans Mono")[desc\{u=3, inv=1\} + tree(holes \{0\})]
        #v(2pt)
        #text(size: 7.6pt, fill: ink.lighten(20%))[queried as-is from the mmap]
      ],
    )
  ]
]

// ---- footer -------------------------------------------------------------------
#place(dx: 20pt, dy: 724pt)[
  #box(width: 900pt)[
    #text(size: 8.4pt, fill: ink.lighten(20%))[
      *Sources:* `src/crates/sfst/src/row_index.rs` (Phase 1, insertion-order contract) · `src/crates/sfst/src/build.rs` (`build_into` stage order, `remap_one_bitmap`, `batch_mask`) · `src/crates/sfst/src/index_reader.rs` (`PosSet`, `from_value`, assign ops) · `src/crates/sfst/src/bitset.rs` (scratch `Bitset`) · `src/crates/sfst/FORMAT.md` (Encoding).
    ]
  ]
]
