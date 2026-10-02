// fig-serialization — pre-order serialization of the verified example.
//
// Posting list {5, 7, 61, 64, 4097, 262143} over universe 262,144 = 8⁶ (6 levels)
// → tree bytes 81 03 01 03 81 a0 20 01 01 01 01 01 02 80 80 80 80 80
// → wire form: 8-byte header [universe u32 LE][len u32 LE] + tree = 26 bytes.
//
// Sources: src/crates/treight/src/raw.rs (from_sorted_iter core loop,
//          serialize_into/deserialize_from wire format, serialized_size),
//          src/crates/treight/src/node.rs (pre-order layout, skip_subtree).
// Palette: Okabe-Ito (colorblind-safe). Branch identity is carried by BOTH
//          hue and the #ordinal printed on every byte — color is never the
//          only cue.
#set page(width: 940pt, height: 800pt, margin: 0pt)
#set text(font: "DejaVu Sans", size: 10pt, fill: rgb("1a1a1a"))
#set par(leading: 0.6em)

#let ink     = rgb("1a1a1a")
#let orange  = rgb("e69f00")   // #1 root
#let blue    = rgb("0072b2")   // #2-#5 left-cluster inner nodes
#let green   = rgb("009e73")   // leaves
#let sky     = rgb("56b4e9")   // #8-#9 the 64 branch
#let vermil  = rgb("d55e00")   // #10-#13 the 4097 branch
#let rpurple = rgb("cc79a7")   // #14-#18 the 262143 rightmost chain
#let gray    = rgb("9a9a9a")
#let pale    = rgb("f4f4f2")

// byte chip used in the tree rows
#let bchip(hexbyte, fillc) = box(
  radius: 3pt, stroke: 0.9pt + ink, fill: fillc, inset: (x: 4pt, y: 2.5pt),
  text(size: 11.5pt, weight: "bold", font: "DejaVu Sans Mono",
    fill: if fillc == sky or fillc == rpurple or fillc == orange { ink } else { white })[#hexbyte],
)

// one tree row: blob offset, dl level, chip, mono connector art, annotation
#let row(off, dl, hexbyte, fillc, art, note) = grid(
  columns: (26pt, 16pt, auto, auto, 1fr),
  column-gutter: 5pt,
  align: (right + horizon, center + horizon, center + horizon, left + horizon, left + horizon),
  text(size: 8.5pt, font: "DejaVu Sans Mono", fill: ink.lighten(25%))[#off],
  text(size: 8.5pt, fill: ink.lighten(25%))[#dl],
  bchip(hexbyte, fillc),
  text(size: 9.5pt, font: "DejaVu Sans Mono", fill: ink.lighten(15%))[#art],
  text(size: 8.6pt)[#note],
)

// flat wire byte cell
#let wcell(hexbyte, fillc, off) = box(
  stroke: 0.8pt + ink, inset: 0pt, radius: 2pt, width: 25pt,
  {
    box(width: 25pt, inset: (y: 3pt), fill: fillc, align(center)[
      #text(size: 10.5pt, weight: "bold", font: "DejaVu Sans Mono",
        fill: if fillc == sky or fillc == rpurple or fillc == orange or fillc == white { ink } else { white })[#hexbyte]
    ])
    box(width: 25pt, inset: (y: 1pt), align(center)[#text(size: 7pt, font: "DejaVu Sans Mono", fill: ink.lighten(25%))[#off]]
    )
  },
)

#place(dx: 20pt, dy: 12pt)[
  #text(size: 16.5pt, weight: "bold")[Serialization: a pre-order walk, no offsets anywhere]
  #v(2pt)
  #text(size: 10pt, fill: ink.lighten(30%))[
    posting list {5, 7, 61, 64, 4097, 262143} · universe 262,144 = 8⁶ → 6 levels · node byte first, then each present child's subtree in ascending child order
  ]
]

// ---- the tree (18 nodes, ordinals #1..#18) --------------------------------
#place(dx: 20pt, dy: 62pt)[
  #box(
    radius: 6pt, stroke: 0.8pt + gray, inset: (x: 10pt, y: 9pt), width: 566pt, fill: pale,
  )[
    #text(size: 10pt, weight: "bold")[THE TREE — 18 node bytes, pre-order (offset · dl)]
    #v(5pt)
    #stack(spacing: 3.2pt,
      row("0x00", "5", "81", orange,  "",               "root · children {0, 7}"),
      row("0x01", "4", "03", blue,    "├─0",            "dl4 · children {0, 1}"),
      row("0x02", "3", "01", blue,    "│  ├─0",         "dl3 · child {0}"),
      row("0x03", "2", "03", blue,    "│  │  └─0",      "dl2 · children {0, 1}"),
      row("0x04", "1", "81", blue,    "│  │     ├─0",   "dl1 · children {0, 7}"),
      row("0x05", "0", "a0", green,   "│  │     │  ├─0","leaf \[0, 8) · bits 5, 7 → values 5, 7"),
      row("0x06", "0", "20", green,   "│  │     │  └─7","leaf \[56, 64) · bit 5 → 61"),
      row("0x07", "1", "01", sky,     "│  │     └─1",   "dl1 · child {0}"),
      row("0x08", "0", "01", green,   "│  │        └─0","leaf \[64, 72) · bit 0 → 64"),
      row("0x09", "3", "01", vermil,  "│  └─1",         "dl3 · child {0}"),
      row("0x0A", "2", "01", vermil,  "│     └─0",      "dl2 · child {0}"),
      row("0x0B", "1", "01", vermil,  "│        └─0",   "dl1 · child {0}"),
      row("0x0C", "0", "02", vermil,  "│           └─0","leaf \[4096, 4104) · bit 1 → 4097"),
      row("0x0D", "4", "80", rpurple, "└─7",            "dl4 · child {7}"),
      row("0x0E", "3", "80", rpurple, "   └─7",         "dl3 · child {7}"),
      row("0x0F", "2", "80", rpurple, "      └─7",      "dl2 · child {7}"),
      row("0x10", "1", "80", rpurple, "         └─7",   "dl1 · child {7}"),
      row("0x11", "0", "80", rpurple, "            └─7","leaf \[262136, 262144) · bit 7 → 262143"),
    )
    #v(4pt)
    #text(size: 8.4pt, fill: ink.lighten(15%))[
      there are no child offsets: each byte is self-delimiting (its bits say how many subtree bodies follow); skipping a sibling means *walking* it (`skip_subtree`)
    ]
  ]
]

// ---- right column ----------------------------------------------------------
#place(dx: 612pt, dy: 62pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 308pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[VALUE → BASE-8 DIGITS]
    #v(4pt)
    #grid(
      columns: (auto, auto, auto, auto, auto, auto, auto, auto),
      column-gutter: (7pt, 4pt), row-gutter: 3pt,
      align: right + horizon,
      text(size: 8pt, fill: ink.lighten(25%))[*v*], text(size: 8pt, fill: ink.lighten(25%))[*dl5*],
      text(size: 8pt, fill: ink.lighten(25%))[*dl4*], text(size: 8pt, fill: ink.lighten(25%))[*dl3*],
      text(size: 8pt, fill: ink.lighten(25%))[*dl2*], text(size: 8pt, fill: ink.lighten(25%))[*dl1*],
      text(size: 8pt, fill: ink.lighten(25%))[*dl0*], text(size: 8pt, fill: ink.lighten(25%))[*values*],
      grid.hline(stroke: 0.5pt + gray),
      ..(
        "5", "0", "0", "0", "0", "0", "{5,7}", "—",
        "61", "0", "0", "0", "0", "7", "5", "—",
        "64", "0", "0", "0", "1", "0", "0", "—",
        "4097", "0", "1", "0", "0", "0", "1", "—",
        "262143", "7", "7", "7", "7", "7", "7", "—",
      ).map(t => text(size: 9pt, font: "DejaVu Sans Mono")[#t])
    )
    #v(4pt)
    #text(size: 8.2pt)[dl0 is the leaf *bit*; dl5..dl1 pick children. `child_index(dl, v) = (v >> 3·dl) & 7`]
  ]
]

#place(dx: 612pt, dy: 250pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 308pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[WHY 18 BYTES FOR 6 VALUES]
    #v(4pt)
    #text(size: 8.6pt)[
      • count the nodes level by level: 1 root + 2 dl4 + 3 dl3 + 3 dl2 + 4 dl1 + 5 leaves = *18* \
      • an isolated value costs exactly `levels` bytes — its root-to-leaf chain (6 here) \
      • 5, 7, 61 share the five-byte root-to-parent path and split only at the leaf \
      • 64 joins four shared nodes, adds a dl1 node + leaf (+2) \
      • 4097 joins root + dl4, adds three chain nodes + leaf `02` (+4) \
      • 262143 joins only the root: the all-sevens rightmost path, every byte `80` (+5) \
      • same 18 as `estimate_data_size`: one byte per distinct (level, node index) pair
    ]
  ]
]

#place(dx: 612pt, dy: 470pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 308pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[READING IT BACK]
    #v(4pt)
    #text(size: 8.6pt)[
      `deserialize_from` takes the header's universe size, re-derives `levels = ceil_log8(262,144) = 6`, and uses the tree bytes as-is — the serialized form IS the in-memory form. The level count is never stored.
    ]
  ]
]

// ---- the wire strip (aligned under the tree) -------------------------------
#place(dx: 20pt, dy: 608pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 900pt, fill: white)[
    #grid(
      columns: (auto, auto, 1fr),
      column-gutter: 8pt,
      text(size: 10pt, weight: "bold")[ON THE WIRE — 26 bytes],
      align(right)[#text(size: 8.6pt, fill: ink.lighten(20%))[`serialize_into`: header + verbatim tree bytes; `serialized_size` = 8 + 18]],
      [],
    )
    #v(7pt)
    #stack(dir: ltr, spacing: 2pt,
      // header
      wcell("00", white, ""), wcell("00", white, ""), wcell("04", white, ""), wcell("00", white, ""),
      box(width: 6pt)[],
      wcell("12", white, ""), wcell("00", white, ""), wcell("00", white, ""), wcell("00", white, ""),
      box(width: 10pt)[],
      // tree bytes 0x00..0x11
      wcell("81", orange, "0x00"), wcell("03", blue, "0x01"), wcell("01", blue, "0x02"),
      wcell("03", blue, "0x03"), wcell("81", blue, "0x04"), wcell("a0", green, "0x05"),
      wcell("20", green, "0x06"), wcell("01", sky, "0x07"), wcell("01", green, "0x08"),
      wcell("01", vermil, "0x09"), wcell("01", vermil, "0x0A"), wcell("01", vermil, "0x0B"),
      wcell("02", vermil, "0x0C"),
      box(width: 10pt)[],
      wcell("80", rpurple, "0x0D"), wcell("80", rpurple, "0x0E"), wcell("80", rpurple, "0x0F"),
      wcell("80", rpurple, "0x10"), wcell("80", rpurple, "0x11"),
    )
    #v(6pt)
    #grid(
      columns: (auto, auto, auto, 1fr),
      column-gutter: 10pt,
      box(width: 102pt)[#text(size: 8pt)[universe_size = 262,144 (u32 LE)]],
      box(width: 102pt)[#text(size: 8pt)[tree_len = 18 (u32 LE)]],
      box(width: 330pt)[#text(size: 8pt)[tree bytes in pre-order — blob offsets match the tree above]],
      [],
    )
  ]
]

// ---- footer -----------------------------------------------------------------
#place(dx: 20pt, dy: 756pt)[
  #box(width: 900pt)[
    #text(size: 8.4pt, fill: ink.lighten(20%))[
      *Sources:* `src/crates/treight/src/raw.rs` (`from_sorted_iter`, `serialize_into`, `deserialize_from`, `serialized_size`) · `src/crates/treight/src/node.rs` (pre-order layout, `skip_subtree`) · example matches the draft's committed dump (18 rows, offsets 0x00–0x11) and the Lead-verified flat bytes.
    ]
  ]
]
