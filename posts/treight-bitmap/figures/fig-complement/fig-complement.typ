// fig-complement — the inverted representation + De Morgan dispatch.
//
// Dense sets store their HOLES: a 60-of-64 set stores only {9, 25, 26, 60}
// → tree bytes 8a 02 06 10 (4 bytes). and/or dispatch on the two inverted
// flags (De Morgan) so the raw walkers never see a complement.
//
// Sources: src/crates/treight/src/bitmap.rs (Bitmap, from_sorted_iter_complemented,
//          full, complement, and/or dispatch tables + short-circuits,
//          ComplementIter); raw.rs for the raw walker names.
// Palette: Okabe-Ito; N/I distinction is textual (N/I letters everywhere),
//          never color-only.
#set page(width: 940pt, height: 640pt, margin: 0pt)
#set text(font: "DejaVu Sans", size: 10pt, fill: rgb("1a1a1a"))
#set par(leading: 0.6em)

#let ink     = rgb("1a1a1a")
#let blue    = rgb("0072b2")
#let green   = rgb("009e73")
#let vermil  = rgb("d55e00")
#let rpurple = rgb("cc79a7")
#let gray    = rgb("9a9a9a")
#let pale    = rgb("f4f4f2")

#let holes = (9, 25, 26, 60)

// one value cell of a 64-cell strip
#let vcell(i, filled, fillc) = box(
  width: 5.6pt, height: 13pt, inset: 0pt,
  stroke: 0.5pt + gray,
  fill: if filled { fillc } else { white },
)

#place(dx: 20pt, dy: 12pt)[
  #text(size: 16.5pt, weight: "bold")[Dense sets: store the holes, dispatch by De Morgan]
  #v(2pt)
  #text(size: 10pt, fill: ink.lighten(30%))[
    `Bitmap` = `RawBitmap` + `inverted: bool` · one set over universe 64 shown both ways · N = stores members, I = stores holes (the flag, not the data, flips)
  ]
]

// ---- left: one set, two representations ------------------------------------
#place(dx: 20pt, dy: 62pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 452pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[ONE SET, TWO REPRESENTATIONS]
    #v(3pt)
    #text(size: 8.4pt)[a dense set — 60 of 64 positions present (e.g. `service=web` on most rows):]
    #v(5pt)
    // members strip
    #stack(dir: ltr, spacing: 0.8pt, ..range(64).map(i => vcell(i, i not in holes, blue)))
    #v(2pt)
    #text(size: 7.6pt, fill: ink.lighten(25%))[members: 60 filled cells · holes at 9, 25, 26, 60]
    #v(8pt)
    #text(size: 8.4pt)[what an *inverted* bitmap actually stores — the 4 holes as a normal tree:]
    #v(5pt)
    // holes strip
    #stack(dir: ltr, spacing: 0.8pt, ..range(64).map(i => vcell(i, i in holes, vermil)))
    #v(2pt)
    #text(size: 7.6pt, fill: ink.lighten(25%))[the stored tree's "members" are the holes of the real set]
    #v(8pt)
    #grid(columns: (auto, 1fr), column-gutter: 8pt, align: horizon,
      text(font: "DejaVu Sans Mono", size: 12pt, weight: "bold")[
        #box(fill: rpurple.lighten(70%), inset: 2pt)[8a] #h(3pt)
        #box(fill: vermil.lighten(70%), inset: 2pt)[02] #h(3pt)
        #box(fill: vermil.lighten(70%), inset: 2pt)[06] #h(3pt)
        #box(fill: vermil.lighten(70%), inset: 2pt)[10]
      ],
      text(size: 8.2pt)[4 tree bytes: root `8a` = children {1, 3, 7}; leaves `02` (hole 9), `06` (25, 26), `10` (60) — built by `from_sorted_iter_complemented`],
    )
    #v(6pt)
    #text(size: 8.4pt)[
      • `full` stores *nothing*: an inverted descriptor over an empty tree \
      • `complement()` flips the flag — free, shares the caller's bytes
    ]
  ]
]

// ---- right: De Morgan dispatch ----------------------------------------------
#place(dx: 500pt, dy: 62pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 420pt, fill: pale)[
    #text(size: 10pt, weight: "bold")[DE MORGAN DISPATCH — `and` / `or` pick a raw walker]
    #v(5pt)
    #grid(
      columns: (auto, auto, 1fr, auto, auto, 1fr),
      column-gutter: (7pt, 7pt, 12pt), row-gutter: 4pt,
      align: (center, center, left, center, center, left),
      text(size: 8pt)[*and*], text(size: 8pt)[*A\B*], text(size: 8pt)[*raw op → result*],
      text(size: 8pt)[*or*], text(size: 8pt)[*A\B*], text(size: 8pt)[*raw op → result*],
      grid.hline(stroke: 0.5pt + gray),
      text(size: 9.5pt, font: "DejaVu Sans Mono")[N&N], text(size: 9pt)[N N], text(size: 8.4pt)[`intersect` → N],
      text(size: 9.5pt, font: "DejaVu Sans Mono")[N|N], text(size: 9pt)[N N], text(size: 8.4pt)[`union` → N],
      text(size: 9.5pt, font: "DejaVu Sans Mono")[N&I], text(size: 9pt)[N I], text(size: 8.4pt)[`A − B` → N],
      text(size: 9.5pt, font: "DejaVu Sans Mono")[N|I], text(size: 9pt)[N I], text(size: 8.4pt)[`B − A` → I],
      text(size: 9.5pt, font: "DejaVu Sans Mono")[I&N], text(size: 9pt)[I N], text(size: 8.4pt)[`B − A` → N],
      text(size: 9.5pt, font: "DejaVu Sans Mono")[I|N], text(size: 9pt)[I N], text(size: 8.4pt)[`A − B` → I],
      text(size: 9.5pt, font: "DejaVu Sans Mono")[I&I], text(size: 9pt)[I I], text(size: 8.4pt)[`union` → I],
      text(size: 9.5pt, font: "DejaVu Sans Mono")[I|I], text(size: 9pt)[I I], text(size: 8.4pt)[`intersect` → I],
    )
    #v(6pt)
    #text(size: 8.4pt)[
      De Morgan: ¬A ∧ ¬B = ¬(A ∨ B) — so intersecting two hole-sets is
      unioning their complements, and the result is naturally inverted. The
      raw walkers (`union`/`intersect`/`difference`) never see a complement;
      the flags pick which walker and which result form.
    ]
  ]
]

// ---- bottom-left: short-circuits ---------------------------------------------
#place(dx: 20pt, dy: 356pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 452pt, fill: white)[
    #text(size: 10pt, weight: "bold")[SHORT-CIRCUITS + `and_not`]
    #v(4pt)
    #grid(
      columns: (auto, 1fr),
      column-gutter: 10pt, row-gutter: 3.5pt,
      align: (left, left),
      text(size: 9pt, font: "DejaVu Sans Mono")[∅ ∧ x], text(size: 8.4pt)[= ∅ — no bytes appended],
      text(size: 9pt, font: "DejaVu Sans Mono")[U ∧ x], text(size: 8.4pt)[= x — x's bytes copied into `out`],
      text(size: 9pt, font: "DejaVu Sans Mono")[∅ ∨ x], text(size: 8.4pt)[= x — bytes copied],
      text(size: 9pt, font: "DejaVu Sans Mono")[U ∨ x], text(size: 8.4pt)[= U — nothing appended],
    )
    #v(4pt)
    #text(size: 8.4pt)[
      `and_not(a, b)` = `a.and(complement(b))` — one flag flip, then the
      dispatch above. Universe mismatches `assert_eq!` (release panics too —
      no silent mis-answer); only the empty/full short-circuits skip the check.
    ]
  ]
]

// ---- bottom-right: read-side costs -------------------------------------------
#place(dx: 500pt, dy: 356pt)[
  #box(radius: 6pt, stroke: 0.8pt + gray, inset: 9pt, width: 420pt, fill: white)[
    #text(size: 10pt, weight: "bold")[WHAT INVERSION COSTS ON READS]
    #v(4pt)
    #text(size: 8.6pt)[
      • `contains` = raw tree lookup XOR the flag (out-of-universe checked first) \
      • `len` = universe − stored-tree population \
      • `range_cardinality` = range length − stored count in range \
      • iteration over an inverted set walks the *whole universe*, skipping
        stored holes (`ComplementIter`) — O(universe), not O(tree) \
      • so: dense sets are tiny to store, exact to query point-wise, but
        expensive to iterate
    ]
  ]
]

// ---- footer --------------------------------------------------------------------
#place(dx: 20pt, dy: 604pt)[
  #box(width: 900pt)[
    #text(size: 8.4pt, fill: ink.lighten(20%))[
      *Sources:* `src/crates/treight/src/bitmap.rs` (`Bitmap`, `from_sorted_iter_complemented`, `full`, `complement`, the `and`/`or` doc tables and match arms, `ComplementIter`) · `src/crates/treight/src/ops.rs` (the raw walkers the dispatch selects).
    ]
  ]
]
