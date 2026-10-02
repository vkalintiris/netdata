// fig-setops — two trees walked in lockstep: AND (and SUB), one walker shape.
//
// Universe 64 = 8² (2 levels). A = {0..7} ∪ {16} ∪ {40} → bytes 25 ff 01 01.
// B = {1, 5, 16, 17} → bytes 05 22 03.
// A ∩ B = 05 22 01 = {1, 5, 16}   (leaf AND, A-only child skipped)
// A − B = 21 dd 01 = {0,2,3,4,6,7} ∪ {40}  (leaf SUB; zero leaf pruned; A-only child copied)
//
// Sources: src/crates/treight/src/ops.rs (intersect_subtree,
//          difference_subtree, the four-walker table, copy_subtree),
//          src/crates/treight/src/raw.rs (intersect/difference wrappers).
// Palette: Okabe-Ito; redundancy: ordinals/hex text, dashed = skipped,
//          ×-marks = pruned — color is never the only cue.
#set page(width: 940pt, height: 710pt, margin: 0pt)
#set text(font: "DejaVu Sans", size: 10pt, fill: rgb("1a1a1a"))
#set par(leading: 0.6em)

#let ink     = rgb("1a1a1a")
#let orange  = rgb("e69f00")
#let blue    = rgb("0072b2")
#let green   = rgb("009e73")
#let sky     = rgb("56b4e9")
#let vermil  = rgb("d55e00")
#let rpurple = rgb("cc79a7")
#let gray    = rgb("9a9a9a")
#let pale    = rgb("f4f4f2")

#let rootchip(hexbyte) = box(
  radius: 4pt, stroke: 1pt + ink, fill: orange, inset: (x: 6pt, y: 3pt),
  text(size: 13pt, weight: "bold", font: "DejaVu Sans Mono")[#hexbyte],
)
#let leafchip(hexbyte, range, vals, fillc, tag: "", dashed: false, textfill: white) = box(
  radius: 4pt,
  stroke: (paint: ink, thickness: 0.9pt, dash: if dashed { "dashed" } else { none }),
  fill: fillc, inset: 4pt, width: 76pt,
  align(center)[
    #text(size: 12pt, weight: "bold", font: "DejaVu Sans Mono", fill: textfill)[#hexbyte]
    #v(-2pt)
    #text(size: 7.4pt, fill: textfill)[#range · #vals]
    #if tag != "" [#v(-1pt) #text(size: 7pt, weight: "bold", fill: vermil)[#tag]]
  ],
)

#place(dx: 20pt, dy: 12pt)[
  #text(size: 16.5pt, weight: "bold")[Set operations: two readers advance in lockstep, one subtree per child slot]
  #v(2pt)
  #text(size: 10pt, fill: ink.lighten(30%))[
    universe 64 = 8² → 2 levels (root + leaves) · A = {0..7} ∪ {16} ∪ {40} · B = {1, 5, 16, 17} · both sides non-empty, same universe (asserted)
  ]
]

// ---- A -----------------------------------------------------------------
#place(dx: 20pt, dy: 66pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 8pt, width: 272pt, fill: pale)[
    #align(center)[
      #text(size: 9.5pt, weight: "bold")[A · bytes `25 ff 01 01`]
      #v(4pt)
      #rootchip("25")
      #v(2pt)
      #grid(columns: (1fr, 1fr, 1fr), column-gutter: 4pt, align: center,
        leafchip("ff", "\[0, 8)", "{0..7}", blue),
        leafchip("01", "\[16, 24)", "{16}", blue, tag: "only in A"),
        leafchip("01", "\[40, 48)", "{40}", blue, tag: "only in A"),
      )
      #v(2pt)
      #text(size: 7.6pt, fill: ink.lighten(20%))[root `25` = children {0, 2, 5}]
    ]
  ]
]
// edges A
#place(dx: 60pt, dy: 132pt)[#line(start: (0pt, 0pt), end: (-8pt, 52pt), stroke: 1pt + ink)]
#place(dx: 60pt, dy: 132pt)[#line(start: (0pt, 0pt), end: (86pt, 52pt), stroke: 1pt + ink)]
#place(dx: 60pt, dy: 132pt)[#line(start: (0pt, 0pt), end: (180pt, 52pt), stroke: 1pt + ink)]
#place(dx: 38pt, dy: 152pt)[#text(size: 8pt)[#box(fill: pale, inset: 0.5pt)[0]]]
#place(dx: 136pt, dy: 152pt)[#text(size: 8pt)[#box(fill: pale, inset: 0.5pt)[2]]]
#place(dx: 226pt, dy: 152pt)[#text(size: 8pt)[#box(fill: pale, inset: 0.5pt)[5]]]

#place(dx: 298pt, dy: 108pt)[#text(size: 22pt, weight: "bold")[∩]]

// ---- B -----------------------------------------------------------------
#place(dx: 324pt, dy: 66pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 8pt, width: 272pt, fill: pale)[
    #align(center)[
      #text(size: 9.5pt, weight: "bold")[B · bytes `05 22 03`]
      #v(4pt)
      #rootchip("05")
      #v(2pt)
      #grid(columns: (1fr, 1fr, 1fr), column-gutter: 4pt, align: center,
        leafchip("22", "\[0, 8)", "{1, 5}", green),
        leafchip("03", "\[16, 24)", "{16, 17}", green),
        box(height: 34pt)[#align(center + horizon)[#text(size: 7.6pt, fill: gray)[no child 5:\
                                                                  pruned at build]]],
      )
      #v(2pt)
      #text(size: 7.6pt, fill: ink.lighten(20%))[root `05` = children {0, 2}]
    ]
  ]
]
// edges B
#place(dx: 364pt, dy: 132pt)[#line(start: (0pt, 0pt), end: (-8pt, 52pt), stroke: 1pt + ink)]
#place(dx: 364pt, dy: 132pt)[#line(start: (0pt, 0pt), end: (86pt, 52pt), stroke: 1pt + ink)]
#place(dx: 342pt, dy: 152pt)[#text(size: 8pt)[#box(fill: pale, inset: 0.5pt)[0]]]
#place(dx: 440pt, dy: 152pt)[#text(size: 8pt)[#box(fill: pale, inset: 0.5pt)[2]]]

#place(dx: 602pt, dy: 108pt)[#text(size: 22pt, weight: "bold")[=]]

// ---- result (AND) --------------------------------------------------------
#place(dx: 628pt, dy: 66pt)[
  #box(radius: 6pt, stroke: 1.1pt + vermil, inset: 8pt, width: 272pt, fill: white)[
    #align(center)[
      #text(size: 9.5pt, weight: "bold")[A ∩ B · bytes `05 22 01`]
      #v(4pt)
      #rootchip("05")
      #v(2pt)
      #grid(columns: (1fr, 1fr, 1fr), column-gutter: 4pt, align: center,
        leafchip("22", "\[0, 8)", "{1, 5}", vermil),
        leafchip("01", "\[16, 24)", "{16}", vermil),
        box(height: 34pt)[#align(center + horizon)[#box(stroke: (paint: gray, thickness: 0.8pt, dash: "dashed"), radius: 3pt, inset: 3pt)[#text(size: 7.4pt, fill: gray)[child 5 skipped ×]]]],
      )
      #v(2pt)
      #text(size: 7.6pt, fill: ink.lighten(20%))[= {1, 5, 16}]
    ]
  ]
]
// edges result
#place(dx: 668pt, dy: 132pt)[#line(start: (0pt, 0pt), end: (-8pt, 52pt), stroke: 1pt + ink)]
#place(dx: 668pt, dy: 132pt)[#line(start: (0pt, 0pt), end: (86pt, 52pt), stroke: 1pt + ink)]
#place(dx: 646pt, dy: 152pt)[#text(size: 8pt)[#box(fill: white, inset: 0.5pt)[0]]]
#place(dx: 744pt, dy: 152pt)[#text(size: 8pt)[#box(fill: white, inset: 0.5pt)[2]]]

// ---- the lockstep walk table ----------------------------------------------
#place(dx: 20pt, dy: 262pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 572pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[THE LOCKSTEP WALK — what happens at each child slot of the root]
    #v(5pt)
    #grid(
      columns: (auto, auto, auto, 1fr, 1fr),
      column-gutter: 8pt, row-gutter: 4.5pt,
      align: (center, center, center, left, left),
      text(size: 8pt)[*child*], text(size: 8pt)[*in A*], text(size: 8pt)[*in B*],
      text(size: 8pt)[*AND: leaf `a & b`*], text(size: 8pt)[*SUB: leaf `a & !b`*],
      grid.hline(stroke: 0.5pt + gray),
      text(size: 9pt, font: "DejaVu Sans Mono")[0], text(size: 9pt)[●], text(size: 9pt)[●],
      text(size: 8.6pt)[`ff & 22` → `22` ✓ kept], text(size: 8.6pt)[`ff & !22` → `dd` ✓ kept],
      text(size: 9pt, font: "DejaVu Sans Mono")[2], text(size: 9pt)[●], text(size: 9pt)[●],
      text(size: 8.6pt)[`01 & 03` → `01` ✓ kept], text(size: 8.6pt)[`01 & !03` → `00` ✗ *pruned (byte popped)*],
      text(size: 9pt, font: "DejaVu Sans Mono")[5], text(size: 9pt)[●], text(size: 9pt, fill: gray)[—],
      text(size: 8.6pt)[A-only: `a.skip_subtree()` — subtree dropped], text(size: 8.6pt)[A-only: `copy_subtree` verbatim — bit set],
      text(size: 9pt, font: "DejaVu Sans Mono")[1,3,4,6,7], text(size: 9pt, fill: gray)[—], text(size: 9pt, fill: gray)[—],
      text(size: 8.6pt, fill: ink.lighten(25%))[in neither operand — nothing], text(size: 8.6pt, fill: ink.lighten(25%))[same],
    )
    #v(5pt)
    #text(size: 8.6pt)[
      result root bytes: AND back-patches children {0, 2} → `05`; SUB back-patches {0, 5} → `21` (child 2's leaf computed to zero, so its bit is never set).
    ]
  ]
]

// ---- right: one walker, four operations ------------------------------------
#place(dx: 612pt, dy: 262pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 308pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[ONE WALKER, FOUR OPERATIONS]
    #v(5pt)
    #grid(
      columns: (auto, auto, auto, auto),
      column-gutter: 10pt, row-gutter: 4pt,
      align: center + horizon,
      text(size: 8pt)[*op*], text(size: 8pt)[*leaf*], text(size: 8pt)[*A-only child*], text(size: 8pt)[*B-only child*],
      grid.hline(stroke: 0.5pt + gray),
      text(size: 9pt, font: "DejaVu Sans Mono")[OR], text(size: 9pt, font: "DejaVu Sans Mono")[`a | b`], text(size: 8.6pt)[copy], text(size: 8.6pt)[copy],
      text(size: 9pt, font: "DejaVu Sans Mono")[AND], text(size: 9pt, font: "DejaVu Sans Mono")[`a & b`], text(size: 8.6pt)[skip], text(size: 8.6pt)[skip],
      text(size: 9pt, font: "DejaVu Sans Mono")[SUB], text(size: 9pt, font: "DejaVu Sans Mono")[`a & !b`], text(size: 8.6pt)[copy], text(size: 8.6pt)[skip],
      text(size: 9pt, font: "DejaVu Sans Mono")[XOR], text(size: 9pt, font: "DejaVu Sans Mono")[`a ^ b`], text(size: 8.6pt)[copy], text(size: 8.6pt)[copy],
    )
    #v(6pt)
    #text(size: 8.4pt)[
      • both readers advance in lockstep; each call consumes exactly one subtree from each \
      • OR never prunes (a union of non-empty trees is non-empty) \
      • AND/SUB/XOR *reserve* a zero node byte in `out`, walk the children, then *back-patch* the surviving child bits — none survived ⇒ *pop* the byte (canonical output for free) \
      • `copy_subtree` = skip + verbatim byte-range append (no re-encode)
    ]
  ]
]

// ---- bottom: the two results ------------------------------------------------
#place(dx: 20pt, dy: 508pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 900pt, fill: white)[
    #grid(columns: (1fr, 1fr), column-gutter: 16pt,
      [
        #text(size: 10pt, weight: "bold")[A ∩ B]
        #v(4pt)
        #text(font: "DejaVu Sans Mono", size: 13pt, weight: "bold")[
          #box(fill: orange.lighten(70%), inset: 2pt)[05] #h(4pt)
          #box(fill: vermil.lighten(70%), inset: 2pt)[22] #h(4pt)
          #box(fill: vermil.lighten(70%), inset: 2pt)[01]
        ]
        #v(3pt)
        #text(size: 8.4pt)[= {1, 5, 16} — child 5 skipped, root back-patched to {0, 2}]
      ],
      [
        #text(size: 10pt, weight: "bold")[A − B]
        #v(4pt)
        #text(font: "DejaVu Sans Mono", size: 13pt, weight: "bold")[
          #box(fill: orange.lighten(70%), inset: 2pt)[21] #h(4pt)
          #box(fill: vermil.lighten(70%), inset: 2pt)[dd] #h(4pt)
          #box(fill: vermil.lighten(70%), inset: 2pt)[01]
        ]
        #v(3pt)
        #text(size: 8.4pt)[= {0, 2, 3, 4, 6, 7} ∪ {40} — zero leaf pruned, child 5 copied, root back-patched to {0, 5}]
      ],
    )
  ]
]

// ---- footer -------------------------------------------------------------------
#place(dx: 20pt, dy: 660pt)[
  #box(width: 900pt)[
    #text(size: 8.4pt, fill: ink.lighten(20%))[
      *Sources:* `src/crates/treight/src/ops.rs` (`intersect_subtree`/`difference_subtree`, the four-walker table at the `// The four set-op walkers` comment, `copy_subtree`) · `src/crates/treight/src/raw.rs` (`intersect`, `difference`: empty-blob short-circuits + universe `assert_eq!`).
    ]
  ]
]
