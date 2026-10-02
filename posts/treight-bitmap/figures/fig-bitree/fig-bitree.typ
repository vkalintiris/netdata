// fig-bitree — the treight 8-way bit-tree: one byte per node.
//
// Sources: src/crates/treight/src/lib.rs (ceil_log8, crate docs),
//          src/crates/treight/src/node.rs (Node, child_index/child_offset docs).
// Palette: Okabe-Ito (colorblind-safe); redundancy: shape (solid vs dashed),
//          weight (bold), and glyph marks (●) — color is never the only cue.
#set page(width: 940pt, height: 680pt, margin: 0pt)
#set text(font: "DejaVu Sans", size: 10pt, fill: rgb("1a1a1a"))
#set par(leading: 0.62em)

#let ink     = rgb("1a1a1a")
#let orange  = rgb("e69f00")   // structural nodes (root tier)
#let sky     = rgb("56b4e9")   // highlighted value path
#let green   = rgb("009e73")   // leaves
#let vermil  = rgb("d55e00")   // value bits / accents
#let gray    = rgb("9a9a9a")   // pruned / ghost
#let pale    = rgb("f4f4f2")   // panel background

// --- helpers ---------------------------------------------------------------

// One bit cell for the anatomy panels. top = label above, bot = label below,
// on = whether the bit is set (accent fill + dot).
#let bitcell(top, on, accent: sky, bot: "") = box(
  stroke: 0.7pt + ink, width: 17pt, height: 21pt, inset: 0pt,
  align(center + horizon)[
    #v(1.5pt)
    #text(size: 7.3pt, fill: ink.lighten(20%))[#top]
    #v(1pt)
    #if on [
      #box(fill: accent, radius: 1.5pt, width: 7pt, height: 7pt)[#align(center + horizon)[#text(size: 6pt, fill: white)[●]]]
    ] else [
      #box(stroke: 0.7pt + gray, radius: 1.5pt, width: 7pt, height: 7pt)[]
    ]
    #v(0.5pt)
    #text(size: 7.3pt)[#bot]
    #v(0.5pt)
  ],
)

// A tree node chip: hex byte + small caption.
#let nodechip(hexbyte, caption, fillc, border: ink, dash: none, textfill: ink) = box(
  radius: 4pt, stroke: (paint: border, thickness: 1pt, dash: dash),
  fill: fillc, inset: 5pt,
  align(center)[
    #text(size: 15pt, weight: "bold", font: "DejaVu Sans Mono", fill: textfill)[#hexbyte]
    #v(-2pt)
    #text(size: 7.6pt)[#caption]
  ],
)

// --- title -------------------------------------------------------------------
#place(dx: 20pt, dy: 14pt)[
  #text(size: 17pt, weight: "bold")[treight: an 8-way bit-tree — one byte per node]
  #v(2pt)
  #text(size: 10pt, fill: ink.lighten(30%))[
    universe 512 = 8³ → levels = ceil_log8(512) = 3 · example set {5, 9, 173} · highlighted: the path of v = 173 = 0o255
  ]
]

// --- left rail: levels --------------------------------------------------------
#place(dx: 20pt, dy: 92pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 8pt, width: 128pt, fill: pale)[
    #align(center)[
      #text(size: 9.5pt, weight: "bold")[LEVEL (dl)]
      #v(2pt)
      #text(size: 7.8pt)[level counts up from the leaf; child_index(dl, v) = (v >> 3·dl) & 7]
      #v(6pt)
      #grid(
        columns: (1fr, 1fr, 1.25fr), rows: 13pt, align: center + horizon,
        [*dl*], [*height*], [*value bits*],
        grid.hline(stroke: 0.5pt + gray),
      )
      #grid(
        columns: (1fr, 1fr, 1.25fr), rows: 13pt, align: center + horizon,
        [dl2], [3], [\[6, 8)],
        [dl1], [2], [\[3, 6)],
        [dl0], [1], [\[0, 3)],
      )
      #v(5pt)
      #grid(
        columns: (1.2fr, 1fr), rows: 12pt, align: center + horizon,
        [root covers], [512 values],
        [a dl1 node], [64 values],
        [a leaf], [8 values],
      )
      #v(5pt)
      #text(size: 7.8pt)[height = subtree height in node bytes; child c of a height-h node starts at c · 8^(h−1)]
    ]
  ]
]

// --- main tree ----------------------------------------------------------------
#place(dx: 300pt, dy: 96pt)[#nodechip("05", "root · children {0, 2}", orange)]
#place(dx: 172pt, dy: 216pt)[#nodechip("03", "dl1 node 0 · children {0, 1}", sky)]
#place(dx: 428pt, dy: 216pt)[#nodechip("20", "dl1 node 2 · child {5}", sky)]

#place(dx: 96pt,  dy: 336pt)[#nodechip("20", "leaf [0, 8) · bit 5 → 5", green)]
#place(dx: 250pt, dy: 336pt)[#nodechip("02", "leaf [8, 16) · bit 1 → 9", green)]
#place(dx: 404pt, dy: 336pt)[#nodechip("20", "leaf [168, 176) · bit 5 → 173", vermil, border: vermil, textfill: white)]

// pruned ghost under root bit 7
#place(dx: 560pt, dy: 216pt)[
  #box(
    radius: 4pt, stroke: (paint: gray, dash: "dashed", thickness: 1pt),
    inset: 5pt, width: 86pt,
    align(center)[
      #text(size: 15pt, weight: "bold", fill: gray)[∅]
      #v(-2pt)
      #text(size: 7.6pt, fill: gray)[pruned: root bit 7 = 0 #h(1pt) no subtree — no byte]
    ],
  )
]

// edges root -> dl1
#place(dx: 300pt, dy: 158pt)[#line(start: (38pt, 0pt), end: (-96pt, 58pt), stroke: 1.2pt + ink)]
#place(dx: 300pt, dy: 158pt)[#line(start: (38pt, 0pt), end: (74pt, 58pt), stroke: 3.4pt + sky)]
#place(dx: 300pt, dy: 158pt)[#line(start: (38pt, 0pt), end: (288pt, 58pt), stroke: (paint: gray, thickness: 1pt, dash: "dashed"))]

// edge labels root -> dl1
#place(dx: 222pt, dy: 174pt)[#text(size: 9pt)[#box(fill: white, inset: 1pt)[0]]]
#place(dx: 374pt, dy: 174pt)[#text(size: 9pt, weight: "bold", fill: vermil)[#box(fill: white, inset: 1pt)[2]]]
#place(dx: 572pt, dy: 174pt)[#text(size: 9pt, fill: gray)[#box(fill: white, inset: 1pt)[7]]]

// edges dl1 node0 -> its two leaves
#place(dx: 210pt, dy: 278pt)[#line(start: (0pt, 0pt), end: (-70pt, 58pt), stroke: 1.2pt + ink)]
#place(dx: 210pt, dy: 278pt)[#line(start: (0pt, 0pt), end: (104pt, 58pt), stroke: 1.2pt + ink)]
#place(dx: 150pt, dy: 294pt)[#text(size: 9pt)[#box(fill: white, inset: 1pt)[0]]]
#place(dx: 300pt, dy: 294pt)[#text(size: 9pt)[#box(fill: white, inset: 1pt)[1]]]

// edge dl1 node2 -> leaf (highlighted)
#place(dx: 468pt, dy: 278pt)[#line(start: (0pt, 0pt), end: (-6pt, 58pt), stroke: 3.4pt + sky)]
#place(dx: 446pt, dy: 294pt)[#text(size: 9pt, weight: "bold", fill: vermil)[#box(fill: white, inset: 1pt)[5]]]

// path annotation under the highlighted leaf
#place(dx: 380pt, dy: 416pt)[
  #box(radius: 3pt, stroke: 0.9pt + vermil, inset: 4pt)[
    #text(size: 8.8pt)[v = 173 = 0o255 → root digit *2* → dl1 digit *5* → leaf bit *5* (value 168 + 5)]
  ]
]

// pre-order bytes strip under the tree
#place(dx: 96pt, dy: 478pt)[
  #box(radius: 4pt, stroke: 0.8pt + gray, inset: 7pt, fill: pale)[
    #text(size: 8.8pt)[serialized tree (pre-order walk of this example):]
    #v(4pt)
    #text(font: "DejaVu Sans Mono", size: 13pt, weight: "bold")[
      #box(fill: orange.lighten(70%), inset: 2pt)[05] #h(4pt)
      #box(fill: sky.lighten(60%), inset: 2pt)[03] #h(4pt)
      #box(fill: green.lighten(70%), inset: 2pt)[20] #h(4pt)
      #box(fill: green.lighten(70%), inset: 2pt)[02] #h(4pt)
      #box(fill: sky.lighten(60%), inset: 2pt)[20] #h(4pt)
      #box(fill: green.lighten(70%), inset: 2pt)[20]
    ]
    #v(3pt)
    #text(size: 8.2pt)[root · dl1 node 0 · its two leaves · dl1 node 2 · its leaf — 3 values, 6 bytes: shared prefixes are shared bytes (3 × levels = 9 naive)]
  ]
]

// --- right panels ---------------------------------------------------------------
#place(dx: 660pt, dy: 92pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 260pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[NODE BYTE ANATOMY]
    #v(3pt)
    #text(size: 8.4pt)[bit *i* set ⇔ child *i* present. Root byte `05`:]
    #v(6pt)
    #grid(
      columns: (auto,) * 8, column-gutter: 1.5pt,
      bitcell("7", false, accent: orange), bitcell("6", false, accent: orange),
      bitcell("5", false, accent: orange), bitcell("4", false, accent: orange),
      bitcell("3", false, accent: orange), bitcell("2", true, accent: orange),
      bitcell("1", false, accent: orange), bitcell("0", true, accent: orange),
    )
    #v(4pt)
    #text(size: 8.4pt)[children 0 and 2 exist → the only subtrees stored. Bits 1, 3–6 = 0 → those subtrees are pruned before they reach the file.]
  ]
]

#place(dx: 660pt, dy: 274pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 260pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[LEAF BYTE = 8 VALUE BITS]
    #v(3pt)
    #text(size: 8.4pt)[the bottom level's bytes are leaves: bit *b* ⇔ value base + b. Leaf \[168, 176) = `20`:]
    #v(6pt)
    #grid(
      columns: (auto,) * 8, column-gutter: 1.5pt,
      bitcell("168", false, accent: vermil), bitcell("169", false, accent: vermil),
      bitcell("170", false, accent: vermil), bitcell("171", false, accent: vermil),
      bitcell("172", false, accent: vermil), bitcell("173", true, accent: vermil),
      bitcell("174", false, accent: vermil), bitcell("175", false, accent: vermil),
    )
    #v(4pt)
    #text(size: 8.4pt)[● at 173 only → this byte is exactly the set {173} over its 8 values.]
  ]
]

#place(dx: 660pt, dy: 456pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 260pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[CANONICAL FORM]
    #v(3pt)
    #text(size: 8.4pt)[
      • every stored byte is non-zero: empty subtrees are pruned, never written as zero bytes \
      • a non-empty blob always holds ≥ 1 value → `is_empty` is a length check \
      • u32 universes never need more than 11 levels (`ceil_log8`); 8⁶ = 262,144 → 6 levels
    ]
  ]
]

// --- footer ---------------------------------------------------------------------
#place(dx: 20pt, dy: 636pt)[
  #text(size: 8.4pt, fill: ink.lighten(20%))[
    *Sources:* `src/crates/treight/src/lib.rs` (`ceil_log8`, crate docs) · `src/crates/treight/src/node.rs` (`Node`, `child_index`, `child_offset`).
  ]
]
