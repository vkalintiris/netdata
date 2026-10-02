// fig-sfst-overview — where treight bitmaps live in an SFST index file.
//
// The user-requested centerpiece: "how a key=value filter becomes bitmap
// bytes in the file" — chunk framing (header / TOC / bodies + CRC), the
// hot prefix, and the cardinality tier routing that decides where each
// field's posting lists land:
//   low  (< 100 values/field)  → PRIM   : PrefixMap<BitmapValue>  (treight)
//   mid  (100–999)             → MF{i}  : PrefixMap<BitmapValue>  (treight)
//   high (≥ 1000)              → HF{i}  : columnar arena + u8 batch masks (NO bitmaps)
//
// Sources: src/crates/sfst/FORMAT.md (File Layout, Header, TOC, Chunk Ids,
//          PRIM/MF{i}/HF{i}/TIMS/SB{i}, Tier-Aligned IDs, Encoding);
//          src/crates/sfst/src/schema.rs (BitmapValue, HighField,
//          DEFAULT_CARDINALITY_THRESHOLD = 100);
//          src/crates/sfst/src/kv_interner.rs (tier boundaries T, 10·T);
//          src/crates/sfst/src/build.rs (canonical chunk order, hot prefix).
// Palette: Okabe-Ito; bitmap homes are marked with a tree glyph in addition
// to color; HF's "no bitmaps" is stated in text.
#set page(width: 940pt, height: 780pt, margin: 0pt)
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

// chunk row in the file map: id chip + payload text (+ optional marker)
#let chunk(id, desc, idfill: white, mark: none, bold-id: false) = grid(
  columns: (52pt, 1fr),
  column-gutter: 6pt,
  box(
    radius: 2.5pt, stroke: 0.8pt + ink, fill: idfill, inset: (x: 3pt, y: 2pt),
    align(center)[#text(size: 8.6pt, weight: "bold", font: "DejaVu Sans Mono")[#id]],
  ),
  text(size: 8.4pt)[#desc #if mark != none [#h(2pt) #mark]],
)

#place(dx: 20pt, dy: 12pt)[
  #text(size: 16.5pt, weight: "bold")[The SFST index file: where the bitmaps live]
  #v(2pt)
  #text(size: 10pt, fill: ink.lighten(30%))[
    one file = one content stream's WAL, sealed · rows are time-sorted; a posting list is the set of row positions holding a `key=value` · tier routing picks each field's on-disk home
  ]
]

// ---- left: the file, top to bottom ------------------------------------------
#place(dx: 20pt, dy: 62pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 336pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[THE FILE, TOP TO BOTTOM]
    #v(5pt)
    // header
    #box(radius: 2.5pt, stroke: 0.8pt + ink, fill: white, inset: 4pt, width: 314pt)[
      #grid(columns: (1fr, 1fr, 1fr), align: center,
        text(size: 8pt)[*Header* — 12 B], text(size: 8pt)[magic "SFST"], text(size: 8pt)[version 1 · num_chunks],
      )
      #grid(columns: (1fr,), align: center, text(size: 7.4pt, fill: ink.lighten(25%))[u32 LE fields])
    ]
    #v(3pt)
    #box(radius: 2.5pt, stroke: 0.8pt + ink, fill: white, inset: 4pt, width: 314pt)[
      #grid(columns: (1fr,), align: center,
        text(size: 8pt)[*TOC* — 12 B × (num_chunks + 1)],
        text(size: 7.4pt, fill: ink.lighten(25%))[per chunk: 4-B id + u64 LE body offset · final entry = sentinel → EOF],
      )
    ]
    #v(5pt)
    #text(size: 8pt, fill: ink.lighten(20%))[chunk bodies, in TOC order — each body = payload + trailing crc32 (u32 LE):]
    #v(4pt)
    #stack(spacing: 3.4pt,
      chunk("SUMR", [summary — *record_count* (the bitmap universe), time range], idfill: orange.lighten(70%)),
      chunk("META", [histogram, *id_ranges* (tier boundaries), schema tree, columns manifest], idfill: orange.lighten(70%)),
      chunk("TIMS", [*Vec\<i64\>* ns timestamps, chronological — parallel to the SB rows], idfill: orange.lighten(70%)),
      chunk("PRIM", [low-card FST: `PrefixMap<BitmapValue>` — every low-card `key=value` #text(fill: vermil)[🌳]], idfill: green.lighten(70%)),
      chunk("OBTS…", [optional per-row columns (OBTS TRCE SPAN FLAG DRAC PSPN DURN)], idfill: white),
      chunk("TIDX…", [traces-only structures (TIDX TBLM EVNB LNKB TRSU), optional], idfill: white),
      chunk("MF{i}", [one per mid-card field: `PrefixMap<BitmapValue>` #text(fill: vermil)[🌳]], idfill: green.lighten(70%)),
      chunk("HF{i}", [one per high-card field: columnar arena — *no bitmaps*], idfill: vermil.lighten(70%)),
      chunk("SB{i}", [stream batches (≤ 8): per-row `KvId` lists, chronological], idfill: white),
    )
    #v(6pt)
    #grid(columns: (auto, 1fr), column-gutter: 6pt,
      box(width: 3pt, height: 78pt, fill: orange, radius: 1pt)[],
      text(size: 7.8pt)[*hot prefix* — `IndexReader::open` decodes SUMR + META + PRIM eagerly; TIMS on demand. `PRIM` closes the prefix; everything after is one cold span (madvise-away-able).],
    )
    #v(2pt)
    #grid(columns: (auto, 1fr), column-gutter: 6pt,
      box(width: 3pt, height: 40pt, fill: sky, radius: 1pt)[],
      text(size: 7.8pt)[*cold region* — read only when a field is filtered/faceted or rows are materialized.],
    )
  ]
]

// ---- right top: tier routing --------------------------------------------------
#place(dx: 380pt, dy: 62pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 540pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[TIER ROUTING — how a field's posting lists reach the file]
    #v(5pt)
    #grid(
      columns: (auto, auto, 1fr),
      column-gutter: 8pt, row-gutter: 4pt,
      align: (center, left, left),
      // header row
      text(size: 8pt)[*tier*], text(size: 8pt)[*field has*], text(size: 8pt)[*on-disk home + posting-list shape*],
      grid.hline(stroke: 0.5pt + gray),
      // low
      box(fill: green.lighten(70%), radius: 2.5pt, inset: (x: 3pt, y: 1pt))[#text(size: 8.6pt, weight: "bold")[low]],
      text(size: 8.4pt)[< 100 distinct values],
      text(size: 8.4pt)[`PRIM` — one shared FST; each `key=value` → *treight bitmap* of row positions],
      // mid
      box(fill: green.lighten(70%), radius: 2.5pt, inset: (x: 3pt, y: 1pt))[#text(size: 8.6pt, weight: "bold")[mid]],
      text(size: 8.4pt)[100 – 999],
      text(size: 8.4pt)[`MF{i}` — one FST per field; each `key=value` → *treight bitmap*],
      // high
      box(fill: vermil.lighten(70%), radius: 2.5pt, inset: (x: 3pt, y: 1pt))[#text(size: 8.6pt, weight: "bold")[high]],
      text(size: 8.4pt)[≥ 1000],
      text(size: 8.4pt)[`HF{i}` — sorted `keys_blob` + `key_lens` + per-key *u8 batch mask* — *no per-value bitmaps* (they would explode); the mask says which SB batches to scan],
    )
    #v(6pt)
    #text(size: 8.4pt)[
      boundaries: `T = DEFAULT_CARDINALITY_THRESHOLD = 100`; low < T, mid = \[T, 10·T), high ≥ 10·T — a *field-level* decision made at interning time, so two indexers given the same WAL seal identical files. `KvId`s are tier-aligned: 0..low_end..mid_end..high_end (`META.id_ranges`).
    ]
  ]
]

// ---- right middle: BitmapValue anatomy ------------------------------------------
#place(dx: 380pt, dy: 306pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 540pt, fill: white)[
    #text(size: 10pt, weight: "bold")[ANATOMY OF ONE FST ENTRY — `BitmapValue`]
    #v(5pt)
    #grid(
      columns: (auto, auto, auto),
      column-gutter: 8pt, align: top,
      box(radius: 3pt, stroke: 0.9pt + ink, fill: sky.lighten(60%), inset: 6pt, width: 190pt)[
        #align(center)[
          #text(size: 9pt, weight: "bold")[`desc: treight::Bitmap`]
          #v(3pt)
          #text(size: 8pt)[universe_size (u32) · levels (u8) · inverted (bool)]
          #v(3pt)
          #text(size: 7.8pt, fill: ink.lighten(20%))[`Copy` descriptor, ~5–6 B; serde-serialized inline]
        ]
      ],
      box(radius: 3pt, stroke: 0.9pt + ink, fill: green.lighten(70%), inset: 6pt, width: 170pt)[
        #align(center)[
          #text(size: 9pt, weight: "bold")[`data: Vec<u8>`]
          #v(3pt)
          #text(size: 8pt)[the tree bytes (canonical blob)]
          #v(3pt)
          #text(size: 7.8pt, fill: ink.lighten(20%))[zero-copy on read: `PosSet` uses them as-is from the mmap]
        ]
      ],
      box(radius: 3pt, stroke: 0.9pt + ink, fill: pale, inset: 6pt, width: 128pt)[
        #align(center)[
          #text(size: 9pt, weight: "bold")[FST key]
          #v(3pt)
          #text(size: 8pt)[the full `key=value` string]
          #v(3pt)
          #text(size: 7.8pt, fill: ink.lighten(20%))[prefix-mapped, shared prefixes shared]
        ]
      ],
    )
    #v(6pt)
    #text(size: 8.4pt)[
      encoding chain for every chunk: `bincode(value)` → `zstd` (PRIM/MF at level 3, rest level 1) → append crc32 trailer. A dense value's bitmap stores its holes (`inverted = true`); `TIMS` + `SUMR.record_count` define the position universe — positions are *time-sorted row indexes*, so `and`/`or` between any two filters align by construction.
    ]
  ]
]

// ---- right bottom: the query loop ------------------------------------------------
#place(dx: 380pt, dy: 528pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 540pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[THE QUERY LOOP (read side)]
    #v(4pt)
    #text(size: 8.6pt)[
      `service.name = "api"` → PRIM (or an MF\{i\}) FST lookup → `BitmapValue` → `PosSet::from_value` (clones the tree bytes once) → `and_assign` / `or_assign` / `and_not_assign` / `range` with the other filters — *all native treight ops, no roaring anywhere on the read path* — surviving positions index into `TIMS` (time) and `SB{i}` (row materialization).
    ]
  ]
]

// ---- footer ------------------------------------------------------------------------
#place(dx: 20pt, dy: 744pt)[
  #box(width: 900pt)[
    #text(size: 8.4pt, fill: ink.lighten(20%))[
      *Sources:* `src/crates/sfst/FORMAT.md` (File Layout, Header, TOC, Chunk Ids, `PRIM`/`MF{i}`/`HF{i}`/`TIMS`/`SB{i}`, Tier-Aligned IDs, Encoding) · `src/crates/sfst/src/schema.rs` (`BitmapValue`, `HighField`, `DEFAULT_CARDINALITY_THRESHOLD = 100`) · `src/crates/sfst/src/kv_interner.rs` (T, 10·T tier boundaries) · `src/crates/sfst/src/build.rs` (canonical order, hot prefix) · `src/crates/sfst/src/index_reader.rs` (`PosSet`).
    ]
  ]
]
