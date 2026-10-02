# treight: an 8-way bit-tree bitmap for immutable indexes

*First in a series on the SFST indexing behind Netdata's OpenTelemetry (OTel) logs and traces.*

## A query, start to finish

Open one of our log index files and run a filtered count — say, everything from a busy service in a
five-minute window. What actually executes?

The file is memory-mapped. On open, the reader decodes the primary index chunk once: a finite-state
map from `key=value` strings to bitmaps. The query then resolves your filter into posting lists —
the bitmap of row positions where `service=edge` appears, the bitmap for `level=error` — and
intersects them, then clips the result to the time window by intersecting it with the bitmap of
in-window rows, then counts. Two set operations and a count, zero deserialization steps: every
bitmap in the plan is intersected and counted *as stored bytes* — union and iteration work the same
way. No secondary structure is built to answer the query
(`compile_filter`/`matched_count` in `src/crates/sfst/src/index_reader.rs`).

Those bitmaps are not roaring bitmaps, though roaring bitmaps are what built the file. They are
`treight` bitmaps — **tre**e + **eight** — a compressed set of `u32` row positions stored as an
8-way bit-tree, one byte per node. The tree bytes on disk are the same bytes the query path walks.
This post is written for software engineers and Rust programmers: why that bitmap exists, how it
works down to the byte level, and where roaring beats it.

## Contents

- [A query, start to finish](#a-query-start-to-finish)
- [The problem: posting lists over a bounded row universe](#the-problem-posting-lists-over-a-bounded-row-universe)
- [Why not just roaring?](#why-not-just-roaring)
- [treight, from the bottom up](#treight-from-the-bottom-up)
  - [Lineage: GNU idutils](#lineage-gnu-idutils)
  - [Node bytes and the tree shape](#node-bytes-and-the-tree-shape)
  - [Pre-order serialization, by hand](#pre-order-serialization-by-hand)
  - [Canonical form: the size is predictable](#canonical-form-the-size-is-predictable)
  - [The Rust API: a `Copy` descriptor over someone else's bytes](#the-rust-api-a-copy-descriptor-over-someone-elses-bytes)
  - [Dense sets: store the holes, dispatch by De Morgan](#dense-sets-store-the-holes-dispatch-by-de-morgan)
  - [One walker, four operations](#one-walker-four-operations)
- [Where it lives: the SFST index lifecycle](#where-it-lives-the-sfst-index-lifecycle)
- [treight vs roaring, honestly](#treight-vs-roaring-honestly)
- [Correctness: differential fuzzing against roaring](#correctness-differential-fuzzing-against-roaring)
- [Benchmarks](#benchmarks)
- [Limitations: when not to use treight](#limitations-when-not-to-use-treight)
- [What's next](#whats-next)

## The problem: posting lists over a bounded row universe

An inverted index over logs needs, for every `key=value` pair, the set of rows containing it. That
set is the posting list, and its natural representation is a bitmap over row positions.

Our universe has three properties that shape everything else:

- **Small and known.** Each index file holds one sealed batch of rows; the universe is the file's
  `record_count`. The corpora we benchmark against hold 250,000 logs each; each benchmark's
  universe is that corpus's exact derived row count, which lands in 8⁵ < n ≤ 8⁶ — six tree levels
  either way. The running example below uses 262,144 = 8⁶ as its universe, so the byte-level
  arithmetic is exact and the all-sevens rightmost path is the very last row of *that* universe.
- **Wild density variance.** A dominant service name can appear in most rows; a rare error code in
  a handful. One representation must be small at *both* ends and decent in the middle, because the
  index holds thousands of these lists side by side.
- **Build once, read many.** Files are immutable once sealed. The write path bulk-encodes each
  posting list from sorted data exactly once; the read path only intersects, unions, iterates, and
  counts range cardinalities. There is no such thing as an in-place posting-list edit.

That last property is the license for the whole design: a representation excellent to store and
query can afford to be mediocre to mutate — nothing mutates it.

## Why not just roaring?

First, the credit. Roaring bitmaps (Chambi, Lemire, Kaser, Godin — [arXiv:1402.6407][1], [DOI
10.1002/spe.2325][2]) are the state of the art for compressed integer sets, and they are
everywhere. A roaring bitmap partitions the u32 space into 65,536 containers
by the top 16 bits of each value; each container stores its low-16-bit members as a sorted array,
an 8 KiB bitset, or a run container — run-length encoding ([RoaringBitmap format spec][3]).
Membership is two
probes. Cardinality is a sum of per-container counts. Intersections run machine-word-parallel over
bitset containers. For mutable, in-memory sets over an unbounded u32 range, nothing else is close —
which is why our *write path uses roaring*
([Where it lives](#where-it-lives-the-sfst-index-lifecycle)).

For the storage-and-query slot, three things bite:

1. **Per-container overhead on tiny lists.** In the portable serialization, every occupied
   container costs a 4-byte `(key, cardinality)` entry plus a 4-byte offset-table entry before
   any payload, plus 2 bytes per array member ([spec][3]) — structural facts of the format, not
   measurements. Six members scattered across six 16-bit pages therefore cost 60 bytes of
   container overhead — six containers × (8 + 2) — plus the file cookie. In treight, an isolated
   member costs exactly one byte per tree level — 6 over this universe; members that share
   prefixes share bytes — and across millions of real posting lists that compounds
   (benchmarks below).
2. **Deserialize-to-query.** A roaring bitmap read from disk is a portable byte stream that must be
   decoded into owned containers before the first `contains`. We wanted the bytes on disk to *be*
   the query-ready structure: no decode step per query, no per-query allocations, and a handle you
   can hold across an mmap. (sfst's zstd-framed chunks decode once on file open; nothing
   decodes or allocates per query or set op after that.)
3. **A known universe is a resource.** Roaring's machinery manages an unbounded u32 space; ours is
   one file's row count. Every bitmap in the file shares it: the descriptor — the two-integer
   handle [The Rust API](#the-rust-api-a-copy-descriptor-over-someone-elses-bytes) defines —
   shrinks to almost nothing, and set operations can *assert* operand compatibility instead of
   negotiating it.

None of this is a case against roaring: each structure dominates the phase it is used in —
roaring accumulates, treight stores and queries. And treight carries a differential fuzzer that
uses roaring as the oracle
([Correctness](#correctness-differential-fuzzing-against-roaring)).

## treight, from the bottom up

This section builds treight from the bottom up: lineage, node bytes, serialization, and what the
four set operations share. Everything after — lifecycle, scoreboard, numbers — is consequences.

### Lineage: GNU idutils

The layout is not novel — that is a feature. It is a port of the bit-tree in GNU
[idutils][4]' `fid.c` (written by Greg McGary), which has stored "which files contain this token"
since the 1990s: the same posting-list problem, over a file-id universe. `fid.c` walks the tree
with `is_hit_1` (membership) and `skip_hits` (step over a subtree); treight's `node.rs`/`ops.rs`
mirror both (`contains_inner`, `skip_subtree` — `src/crates/treight/src/{node,ops}.rs`). One
deliberate addition: our `skip_subtree` returns the subtree's population on the way past, which is
how `len()` and range counting get their numbers.

### Node bytes and the tree shape

A universe maps to a fixed tree depth: `ceil_log8(universe_size)` levels
(`src/crates/treight/src/lib.rs`). `ceil_log8` shifts the largest index right by 3 bits at a time
until nothing is left, counting the shifts — each tree level consumes 3 bits of the value, one
base-8 digit (level `dl`: `levels − 1` at the root, 0 at the leaves), so a value's root-to-leaf
path *is* its base-8 representation, most significant digit first. A 262,144-value universe needs exactly 6 levels; every non-empty universe needs at
least one. A full u32 universe tops out at 11 (the root's digit shift, 3 × 11 = 33 bits, is one
past u32 — hence `checked_shr` in the builder, and a root digit that is always 0 there). Levels
are derived from the universe, never stored: two integers fix the whole geometry.

```
             one node byte (any level ≥ 1)
             ┌────┬────┬────┬────┬────┬────┬────┬────┐
             │ b7 │ b6 │ b5 │ b4 │ b3 │ b2 │ b1 │ b0 │
             └────┴────┴────┴────┴────┴────┴────┴────┘
               bit i ⇔ child i exists     (empty subtrees are pruned,
                                            so every stored byte is non-zero)

             leaf byte (level 0)
               bit i ⇔ value (leaf_base + i) is in the set
```

- child `c` of a node at level `dl` is selected by value bits `[3·dl, 3·dl+3)` — that is,
  `child_index(dl, v) = (v >> 3·dl) & 7`;
- that child's subtree covers `c · 8^dl … (c+1) · 8^dl − 1`;
- the bottom level's nodes are leaves: their 8 bits are 8 membership bits for 8 consecutive values.

Empty subtrees cost nothing — this is what makes sparse sets small, and the pruning is total: a
stored zero byte does not exist.

![The treight 8-way bit-tree over a drawable 512-value universe: one byte per node, leaves holding
eight value bits, a pruned ghost subtree, and the level-to-digit mapping](figures/fig-bitree/fig-bitree.svg)

### Pre-order serialization, by hand

The tree bytes are a depth-first **pre-order** walk: emit the node byte, then each present child's
subtree in ascending child order. No child offsets exist in the format. Skipping a
sibling's subtree means *walking* it — each node byte is self-delimiting because its bits say how
many child subtrees follow. You pay bytes, not pointers.

Let's serialize a posting list by hand. Universe 262,144 (6 levels); the list holds six row
positions: `{5, 7, 61, 64, 4097, 262143}`. Each value decomposes into six base-8 digits
(root-to-leaf, then leaf bit):

```
value     dl5  dl4  dl3  dl2  dl1  dl0(leaf bits)
5           0    0    0    0    0    {5,7}     ← shares the path with 7
61          0    0    0    0    7    5
64          0    0    0    1    0    0
4097        0    1    0    0    0    1
262143      7    7    7    7    7    7          ← the very last row: rightmost path
```

The serialized tree is exactly 18 bytes — hex first, then the same bytes read as a tree
(offset in the blob — the crate's name for the tree bytes):

```
81 03 01 03 81 a0 20 01 01 01 01 01 02 80 80 80 80 80

offset dl node  subtree
0x00   5  81    root                  children {0, 7}
0x01   4  03    ├─0                   children {0, 1}
0x02   3  01    │ ├─0                 child  {0}
0x03   2  03    │ │ └─0               children {0, 1}
0x04   1  81    │ │   ├─0             children {0, 7}
0x05   0  a0    │ │   │ ├─0           leaf: values 5, 7
0x06   0  20    │ │   │ └─7           leaf: value 61      (56 + 5)
0x07   1  01    │ │   └─1             child  {0}
0x08   0  01    │ │     └─0           leaf: value 64
0x09   3  01    │ └─1                 child  {0}
0x0A   2  01    │   └─0               child  {0}
0x0B   1  01    │     └─0             child  {0}
0x0C   0  02    │       └─0           leaf: value 4097    (4096 + 1)
0x0D   4  80    └─7                   child  {7}
0x0E   3  80      └─7                 child  {7}
0x0F   2  80        └─7               child  {7}
0x10   1  80          └─7             child  {7}
0x11   0  80            └─7           leaf: value 262143  (262136 + 7)
```

Verify the 18 without trusting us. An isolated value costs exactly `levels` bytes — its own
root-to-leaf chain, six over this universe — so six values with nothing shared would cost 36.
But 5, 7, and 61 share the entire five-byte root-to-parent path and split only at the leaf (two
leaf bytes for all three); 64 joins the top four bytes and adds two of its own; 4097 joins the
top two and adds four; 262,143 joins only the root and adds five. That is 5 + 2 + 2 + 4 + 5 = 18.
Equivalently, count distinct nodes per level, root to leaf — 1 + 2 + 3 + 3 + 4 + 5 — the same
18, and exactly what `estimate_data_size` computes.

The wire format adds an 8-byte header (`serialize_into`, `src/crates/treight/src/raw.rs`):

```
00 00 04 00   universe_size = 262,144   (u32 LE)
12 00 00 00   tree length   = 18        (u32 LE)
81 03 01 03 81 a0 20 01 01 01 01 01 02 80 80 80 80 80
```

26 bytes on the wire, and the levels are not even stored — `deserialize_from` re-derives it
from the universe size via `ceil_log8`.

![The same 18 bytes as a tree with every byte's ordinal, aligned over the flat wire
form](figures/fig-serialization/fig-serialization.svg)

### Canonical form: the size is predictable

The tree bytes are *canonical*: exactly the bytes needed, whichever path built it. Three
consequences:

- `estimate_data_size(universe, sorted_values)` (`src/crates/treight/src/lib.rs`) predicts the
  exact tree-byte length without building anything — it counts distinct `(level, node index)`
  pairs on the values' paths. Weigh a roaring→treight conversion before paying for it: compare
  the prediction with the roaring serialized size (sfst's seal path decides by cardinality and
  density instead — see [Where it lives](#where-it-lives-the-sfst-index-lifecycle)).
- `is_empty` is a length check on the tree bytes. A non-empty tree always holds at least one
  value, so emptiness is O(1).
- Every mutation path — bulk build, `insert`, `remove`, `remove_range`, all four set ops —
  preserves canonicality — asserted, not conventional: the fuzz target's closing check re-derives
  `estimate_data_size` after arbitrary op sequences and requires the tree bytes to still match
  exactly.

### The Rust API: a `Copy` descriptor over someone else's bytes

The descriptor is two integers — universe size and levels (re-derived on load, never serialized) —
and it is `Copy`:

```rust
pub struct RawBitmap {
    universe_size: u32,
    levels: u8,
}
```

The tree bytes are *not* in the descriptor. Every method takes them as an argument — `&[u8]` for
queries, `&mut Vec<u8>` for builds and mutations (set operations take operand slices plus an
output buffer) — and zero tree bytes is a valid, empty bitmap. Consequences Rust programmers
will recognize:

- Handing a bitmap around is handing two integers around; descriptors of the same universe are
  interchangeable, so set operations return either operand's.
- There is no owned buffer to fight over: the tree bytes can live in a `Vec`, an arena, or a slice
  of an mmap. Queries allocate nothing.
- Set operations append the result's tree bytes to a caller-provided `out` buffer, so callers
  reuse one buffer across a query plan (exactly what sfst's `PosSet` — the read-path set handle,
  [Where it lives](#where-it-lives-the-sfst-index-lifecycle) — does).

The bulk builder is a single pass with per-level scratch — no staging bitset, no sort if you
already have one:

```rust
// src/crates/treight/src/raw.rs — from_sorted_iter, core loop
for v in iter {
    for dl in (0..levels).rev() {
        let group = v.checked_shr(3 * (dl + 1)).unwrap_or(0);
        if group != prev_group[dl as usize] {
            prev_group[dl as usize] = group;
            node_pos[dl as usize] = out.len();
            out.push(0);                       // open a node byte
        }
        out[node_pos[dl as usize]] |= 1u8 << child_index(dl, v);
    }
}
```

Ascending input is a contract, not a check: a descending iterator corrupts the pre-order layout
(the contract sfst satisfies by emitting positions in ascending order — sorted vec or bitset scan,
see the lifecycle section). Cost is O(values × levels) with one byte of scratch per level.

Iteration is a stack-based DFS with at most `levels − 1` frames — 10 for a u32 universe —
consuming leaf bytes via `trailing_zeros`; its cost tracks tree nodes visited, never the
universe size.

### Dense sets: store the holes, dispatch by De Morgan

A complete 6-level tree over 262,144 values costs one byte per node of the perfect 8-ary tree —
(8⁶ − 1)/7 = 37,449 bytes — and a naively encoded "everything matches" list would pay exactly
that. A posting list holding 99% of rows should pay for its *holes* instead.
The `Bitmap` wrapper pairs the raw tree with an `inverted` flag
(`src/crates/treight/src/bitmap.rs`): sparse sets store members, dense sets store the complement,
`full` stores *zero bytes* (an inverted descriptor over an empty tree), and `complement()` is a
flag flip — free, sharing the same tree bytes.

When either side of an `and`/`or` may be inverted, the wrapper never materializes a complement:
it dispatches on the flag pair and computes over the stored trees, De Morgan-style:

| `a.and(b)` | B normal | B inverted |
|---|---|---|
| **A normal** | intersect → normal | A − B → normal |
| **A inverted** | B − A → normal | union → inverted |

| `a.or(b)` | B normal | B inverted |
|---|---|---|
| **A normal** | union → normal | B − A → inverted |
| **A inverted** | A − B → inverted | intersect → inverted |

Every cell runs a raw op over stored bytes and returns the result in whichever form is natural.
`and_not` falls out as `and(complement(other))`. Short-circuits handle the degenerate operands
(empty, full) before any walk. Notably, those paths return value-correct results without checking
universe equality, while every dispatched path asserts it — the raw ops assert unconditionally, so
a mismatched-universe bug panics in release builds too, rather than silently mis-answering.

A taste of the density trade: a 60-of-64 set stores its four holes — 9, 25, 26, 60 — as a
four-byte tree (root `8a`, leaves `02 06 10`) under an `inverted` descriptor.

![The Bitmap layer: the inverted flag flipping between members and holes, the De Morgan dispatch
tables for and/or, the empty/full short-circuits, and the read-side flips for contains, len, and
iteration](figures/fig-complement/fig-complement.svg)

### One walker, four operations

All four raw set operations are the *same* recursive walker: two readers advance in lockstep, one
subtree per child position, and the operations differ only in the leaf byte op and what happens to
a child present in just one operand (`src/crates/treight/src/ops.rs`):

| operation | leaf byte | child only in A | child only in B |
|---|---|---|---|
| union | `a \| b` | copy subtree | copy subtree |
| intersect | `a & b` | skip subtree | skip subtree |
| difference | `a & !b` | copy subtree | skip subtree |
| symmetric difference | `a ^ b` | copy subtree | copy subtree |

That is the payoff of the layout: four operations that looked like four algorithms collapse into
one three-column table. "Copy subtree" is a byte-range append — the reader skips the subtree and
the traversed bytes go to the output verbatim, no re-encoding. The pruning operators
(intersect, difference, symmetric difference) reserve the parent's node byte, back-patch it with
the surviving child bits, and pop it if nothing survived — so the output is canonical by
construction. An `intersect` on disjoint trees never writes a byte and returns the empty
descriptor; you cannot store a zero byte even by accident.

On a two-level universe of 64 values the mechanic is visible in four bytes per operand:
A = {0..7} ∪ {16} ∪ {40} is `25 ff 01 01`, B = {1, 5, 16, 17} is `05 22 03`. The intersection
back-patches the root to the surviving children and skips A's exclusive subtree
(`05 22 01` = {1, 5, 16}); the difference prunes the zero leaf, copies A's exclusive subtree,
keeps the rest (`21 dd 01`).

![The lockstep walk on a two-level tree: intersect and difference side by side, showing the
reserved node byte, the back-patch, the zero-leaf prune, and skip-versus-copy for A-only
children](figures/fig-setops/fig-setops.svg)

## Where it lives: the SFST index lifecycle

treight is a small crate — about 1,900 lines across six source files — with exactly one production
consumer, sfst, our split-FST logs-and-traces index format (the container described in
`src/crates/sfst/src/lib.rs`). The lifecycle is a clean handoff between the two bitmap libraries —
a three-row toy (`service=x`, arriving at t=37 and t=95) travels from a roaring bitmap over
insertion-order positions, through the time remap and both encode decisions, to an inverted
one-byte tree on disk:

![The index lifecycle in three stages: roaring accumulating arbitrary-order inserts, the seal-time
time remap with the density and cardinality encode decisions, and the mmap read path running
native PosSet operations](figures/fig-lifecycle/fig-lifecycle.svg)

In compressed form:

```
 WRITE PATH                                READ PATH
 ──────────                                ─────────
 WAL (write-ahead log) frames              open file
   │                                         ├─ mmap container, verify CRCs
   ▼                                         └─ decode PRIM chunk once (zstd + bincode)
 RowIndex  (insertion order)                            │
   ├─ interner: key=value → id                          ▼
   ├─ posting lists: Vec<RoaringBitmap>    filter: service=edge AND level=error
   └─ timestamps per row                     ├─ per field:  OR of value bitmaps
        │                                    ├─ fields:     AND across fields
        ▼  seal                              ├─ time window: AND range set
 TimeOrder: insertion ⇄ chronological        └─ count / iterate / facet
        │                                              │
        ▼                                              ▼
 remap_one_bitmap per key=value            PosSet { treight::Bitmap, Vec<u8> }
   ├─ dense (> universe/2)? → complement    every set op native on tree bytes
   ├─ big (≥ max(universe/64, 256))?        no roaring on the read path
   │    → Bitset scratch, not a sort
   └─ encode: treight bytes
        ▼
 BitmapValue { desc, data } → PRIM / MF{i} chunks
```

**Accumulate as roaring.** While the indexer reads the write-ahead log (WAL), every `key=value`
slot owns
a `RoaringBitmap`, and `row()` inserts the current position into each pair's bitmap
(`src/crates/sfst/src/row_index.rs`). Rows arrive in ingest order, values arrive unsorted — this
is roaring's home turf, and it is where roaring stays.

**Remap to time order.** At seal, positions become chronological so a contiguous position
range is a contiguous time window: a `TimeOrder` permutation translates every bitmap, and
`remap_one_bitmap` (`src/crates/sfst/src/build.rs`) encodes each one as treight with two
independent decisions:

```rust
// src/crates/sfst/src/build.rs — remap_one_bitmap, the two thresholds
let bitset_threshold = (universe_size as usize / 64).max(256);
// …
let desc = if card > half {
    Bitmap::from_sorted_iter_complemented(bitset.iter_zeros(universe_size), ...)
} else {
    Bitmap::from_sorted_iter(bitset.iter_ones(), ...)
};
```

- *By cardinality:* past `max(universe/64, 256)` members, the encoder switches from sorting a
  vector to setting bits in a scratch `Bitset` and scanning it ascending — O(n) sets plus a
  universe/64-word scan instead of an O(n log n) sort (the trade documented in
  `remap_one_bitmap`).
- *By density:* above half the universe, store the complement — fewer positions to serialize
  (`Bitset` in `src/crates/sfst/src/bitset.rs` is scratch only; it is not stored).

**Store as treight.** Each entry becomes a `BitmapValue { desc: treight::Bitmap, data: Vec<u8> }`
in the primary FST chunk (`PRIM` — the finite-state map the opening query decoded) or a
per-field chunk (`MF{i}`, M for mid-cardinality)
(`src/crates/sfst/FORMAT.md`). The descriptor rides in the chunk schema beside its tree bytes.
If finite-state maps are new to you, Andrew Gallant's [Transducers][5] is the classic
explanation of the family — where we would point anyone before touching this layer.
Fields are tiered at interning time by distinct-value count — low under `T`, mid in `[T, 10·T)`,
high at or above, `T = DEFAULT_CARDINALITY_THRESHOLD = 100` (`src/crates/sfst/src/kv_interner.rs`)
— and only the low and mid tiers store bitmaps. The high tier opts out entirely: thousands of
near-empty posting lists would explode the index. Its `HF{i}` chunks (H for high-cardinality)
are a columnar string arena instead: sorted `key=value` keys plus one bitmask per key over the
file's stream batches, so a scan can skip whole batches a value never appears in (`FORMAT.md`,
`src/crates/sfst/src/schema.rs`).

The file as a whole: chunk framing and CRCs, the hot prefix decoded on open, the cold span
mmap'd lazily — and the tier routing that decides which `key=value` pairs get bitmaps:

![The SFST file top to bottom: header, TOC, CRC-framed chunk bodies with a hot prefix and a cold
region, the low/mid/high tier routing, one BitmapValue's anatomy, and the read-side query
loop](figures/fig-sfst-overview/fig-sfst-overview.svg)

**Query as treight.** On the read path, every intermediate result is a `PosSet` — a treight
descriptor plus owned tree bytes. Pulling a value's bitmap clones its tree bytes once; from there
each operation writes a fresh output buffer and swaps it in:

```rust
// src/crates/sfst/src/index_reader.rs — PosSet
fn and_assign(&mut self, other: &Self) {
    let mut out = Vec::new();
    self.bitmap = self
        .bitmap
        .and(&self.data, &other.bitmap, &other.data, &mut out);
    self.data = out;
}
```

Universe discipline is structural: the writer builds every bitmap over the same universe (the
file's `record_count`), so every `PosSet` combination is legal by construction — and treight's
asserts make a violating file panic rather than mis-answer.

## treight vs roaring, honestly

The benchmarks carry the numbers; this section is the mechanism-level accounting of who wins
where.

| axis | roaring | treight |
|---|---|---|
| layout | 65,536 containers by 16-bit prefix; array / bitset / run payloads | fixed-depth 8-way bit-tree; one byte per node |
| membership | container probe + array/bitset test | root-to-leaf descent, skipping sibling subtrees |
| `len()` | sum of stored per-container cardinalities — O(containers) | full walk summing leaf bits — O(tree bytes) |
| set ops | machine-word kernels over 8 KiB bitset containers | byte-wise lockstep tree walk |
| tiny sparse lists | per-container headers dominate | ≤ levels bytes per member, prefixes shared |
| dense lists | run containers compress runs | complement: pay only for holes |
| read path | deserialize portable bytes → owned containers | stored bytes are the structure; `Copy` handle; mmap-able |
| mutation | arbitrary-order inserts, in-place | sorted bulk build; single inserts splice; `remove_range` re-encodes |
| universe | unbounded u32 | fixed at construction, asserted across operands |

**Where roaring wins.** Cardinality: roaring stores per-container counts, so `len()` touches one
stored count per container; treight's `len()` walks every stored byte — a gap the benchmarks
quantify on real lists. Point queries: a roaring probe is two indexed lookups; treight's
`contains` descends the path but skips every serialized sibling subtree before it, so cost
tracks the bytes preceding the path, not just depth (`src/crates/treight/src/raw.rs`). Bulk operations
on large operands: roaring's bitset containers AND/OR 64 bits at a time while treight walks a
byte at a time — vector-width parallelism versus a scalar byte stream. It also wins anything
mutation-heavy, anything needing the full u32 range, and — measured below — mid-density
serialized size. If your bitmaps
live in mutable memory and get queried in place, you want roaring — we do too, on the write
path.

**Where treight wins.** Footprint of small lists: no per-page overhead exists at all, and the six
members of the running example cost 18 bytes against roughly 60 of container overhead. Dense
sets via complement. The read path: no deserialization after file open, no allocation per query,
`Copy` handles, buffer-reusing append-only ops, O(1) emptiness, linear byte scans. And the exact-size property (`estimate_data_size`) turns
storage decisions into cheap predictions: you can know the on-disk cost of a conversion before
paying for it.

**Where they tie.** Both prune range counting: roaring sums container counts, treight skips
subtrees outside the range.

## Correctness: differential fuzzing against roaring

Every operation treight offers is replayed on a `RoaringBitmap` built from the same values, and
the two must agree — roaring is the oracle. The fuzz target
(`src/crates/treight/fuzz/fuzz_targets/against_roaring.rs`) generates a universe, two seed sets,
and a random op sequence (mutations, queries, the four set ops, serialize round-trip, swap
sides), applies it to both sides, and asserts after every
query op:

```rust
// fuzz/fuzz_targets/against_roaring.rs — check_equal
fn check_equal(t: &treight::RawBitmap, td: &[u8], r: &roaring::RoaringBitmap) {
    assert_eq!(t.len(td), r.len(), "len mismatch: treight={} roaring={}", t.len(td), r.len());
    assert_eq!(t.min(td), r.min(), "min mismatch");
    assert_eq!(t.max(td), r.max(), "max mismatch");
    assert_eq!(t.is_empty(td), r.is_empty(), "is_empty mismatch");

    let t_vals: Vec<u32> = t.iter(td).collect();
    let r_vals: Vec<u32> = r.iter().collect();
    assert_eq!(t_vals, r_vals, "iter mismatch");
}
```

The universes are chosen to hit the minimum and maximum value at every tree depth 1 through 8, so
level-boundary arithmetic gets exercised on all edges. And the closing check is canonicality: after
arbitrary mutations, the final blob must still be *exactly* `estimate_data_size` bytes — no dead
bytes, no padding, whichever path produced it.

Run it yourself from `src/crates/treight/fuzz`:

```
cargo fuzz run against_roaring
```

The fuzz workspace pins the same roaring fork as the main workspace (`src/crates/Cargo.toml`),
so the oracle and the production dependency are one roaring implementation. An in-crate proptest twin (`src/crates/treight/src/tests_roaring.rs`) covers the same ground
with exhaustive membership checks up to 4,096-element universes, all inverted-flag combinations
included.

## Benchmarks

All measurements come from a single shared AMD Ryzen 9 5900X (31 GB RAM): timings are criterion
medians, compared as relative orderings rather than sub-percent deltas. Tools: rustc 1.95.0 (59807616e 2026-04-14), criterion 0.5.1, 50
samples per case (1 s warm-up, 2 s measurement), one warm reader. The roaring side is
the pinned `netdata/roaring-rs` fork (branch `allocative`, commit `3ee69b1`) — the same
implementation the fuzzer checks against.

*Every number here is reproducible — the README beside this post carries the commands,
fingerprints, and raw outputs; structural constants are source-verifiable and cited inline.*

**Corpora.** Three frozen captures of real public event streams, 250,000 events each —
`certstream-250k` (Certificate Transparency log entries), `jetstream-250k` (Bluesky Jetstream
firehose events), `mixed-250k` (both interleaved by arrival, 125,000 each). These public-internet
streams contain personal data (certificate subjects, DIDs, handles); everything quoted here is
aggregate — counts, densities, byte sizes — never content. Posting lists
are derived the way the production indexer derives them, over a universe of 250,000 rows per
corpus; the derivation is in the README.

The bench covers both phases plus footprint:

- **Build and query** per corpus × density bucket, plus operand pairs; **size** as summed
  bytes and allocative heap.

**Results — footprint** (scoped to what a real SFST file stores, the low+mid tiers):

| metric (per corpus) | certstream-250k | jetstream-250k | mixed-250k |
|---|---|---|---|
| posting lists (stored low+mid tier) | 1,152 | 24,034 | 20,316 |
| total treight bytes (tree + 5 B descriptor) | 1,797,922 | 1,115,004 | 2,212,840 |
| total roaring serialized bytes | 3,514,248 | 1,538,624 | 3,353,048 |
| footprint ratio (treight / roaring) | 0.510 | 0.647 | 0.630 |
| in-memory heap, treight vs roaring | 1,797,922 vs 4,184,992 | 1,115,004 vs 5,905,000 | 2,212,840 vs 7,409,080 |

Over *all* lists (high-cardinality tier included, though a real file stores it as arenas),
treight's whole-corpus heap is 0.068 of roaring's on all three corpora (17,799,801 vs
262,065,816 on jetstream) — flat bytes versus materialized containers.

**Results — CPU** (criterion medians; bucketed rows read ≤1% / 1–50% / >50%):

| treight-vs-roaring, per corpus | certstream-250k | jetstream-250k | mixed-250k |
|---|---|---|---|
| bulk build (seal), by bucket | 2.48 / 1.71 / 0.89 | 2.32 / 3.31 / 0.71 | 2.55 / 1.77 / 1.13 |
| full iteration, by bucket | 1.98 / 1.68 / 1.07 | 2.66 / 2.56 / 1.19 | 2.21 / 1.52 / 1.09 |
| `contains` (hit), bucket range | 387–3,279× | 578–3,791× | 502–6,973× |
| `range_count`, bucket range | 44–234× | 35–264× | 80–288× |
| `len()`, bucket range | 686–16,637× | 828–19,844× | 753–10,039× |
| `and`, same-field disjoint pair | 43× | 52× | 2.6× |
| `and_not`, dense−sparse pair | 0.049× | 0.051× | 0.053× |
| `or`, cross-field pair | 4.9 ns vs 2,931 ns | 4.6 ns vs 2,987 ns | 4.8 ns vs 2,814 ns |

Mechanisms first. Bulk build loses sparse (the seal path sorts and remaps per list — timed
identically for both sides) and wins dense on two of three corpora (0.89 / 0.71; mixed 1.13);
the harness does not credit production's scratch-bitset route, so dense is if anything
understated. Iteration approaches parity as lists densify (1.07–1.19). Membership
is roaring's by three to four orders — ≈3–11 ns vs 5–28 µs per probe — and `range_count`
(35–288×) and `len()` (up to 19,844×) follow the same stored-counts-versus-walk split. The
pairs cut both ways, as the layout predicts: disjoint `and` hurts treight (2.6–52× — skipping
exclusive subtrees means walking their bytes; roaring compares container keys), dense−sparse
`and_not` is a ~20× treight win (De Morgan runs it over the sparse operand's bytes), and
cross-field `or` returns in ~5 ns when one operand is complement-stored full — zero tree bytes,
the union resolves without touching the other operand.

**Density-sliced interpretation.** Real posting lists are not uniformly dense, and
neither structure wins everywhere — sliced by density band (treight/roaring serialized-byte
ratio; list counts certstream/jetstream/mixed):

| density band | t/r ratio (cert / jet / mix) | lists (cert / jet / mix) |
|---|---|---|
| ≤0.001% | 0.347 / 0.342 / 0.345 | 2,237,404 / 1,281,602 / 1,846,968 |
| 0.001–0.01% | 0.512 / 0.683 / 0.551 | 195,307 / 65,263 / 120,089 |
| 0.01–0.1% | 0.980 / 1.275 / 1.021 | 4,903 / 3,664 / 3,991 |
| 0.1–1% | 1.028 / 1.118 / 1.062 | 182 / 181 / 317 |
| 1–10% | 0.390 / 0.763 / 0.434 | 87 / 13 / 101 |
| 10–50% | 0.689 / 0.709 / 0.792 | 34 / 6 / 26 |
| ≥50% | 0.193 / 0.258 / 0.625 | 23 / 10 / 12 |

Predictions held for two bands and flipped for one. Ultra-sparse lists (≤0.001% — the
overwhelming majority) favor treight ~3×: per-container headers dominate roaring while treight
pays a byte per level. Dense lists (≥1%) favor treight, strongest past half the universe where
complement encoding pays only for holes (23/10/3 lists per corpus complemented, strictly above
half — exactly-half lists stay direct: in mixed-250k, 9 of the ≥50% bucket's 12). At the limit, a
100%-density list costs treight zero bytes against roaring's 32,808 — `full` really is free.
But the 0.01–1% middle band is roaring's on size: treight's members × levels outweighs sharing
exactly where roaring's packed arrays are near optimal — and per-list slices near 44% density
flip to roaring too (1.07×) as bitset containers take over.

**What this does not capture.** The footprint totals count structure bytes before zstd and chunk
framing — identical treatment on both sides, but the post-compression delta is not shown. One
shared machine, one warm reader: no cold-page, memory-pressure, or concurrent-reader behavior.
We report what we can reproduce exactly, not the whole story.

## Limitations: when not to use treight

- **Mutation-heavy workloads.** Bulk build wants sorted input; single `insert` works but splices
  bytes and walks sibling subtrees; `remove_range` re-encodes through a fresh buffer. If your set
  lives and breathes inserts, use roaring — we do.
- **Huge sparse universes.** Cost grows with *members × levels*, not with the span they cover. At
  a u32-max universe, an isolated member costs 11 bytes; scattered sparse data over a giant
  universe is exactly roaring's array-container sweet spot.
- **No O(1) cardinality.** `len()` walks the tree bytes (inverted form: universe minus the walk).
  If you need constant-time counts, you need stored counts — that is a container structure.
- **Inverted iteration is O(universe).** Iterating a complement-stored bitmap walks the whole
  universe, skipping stored holes (`ComplementIter`, `src/crates/treight/src/bitmap.rs`); range
  cardinality on inverted sets subtracts within the range instead.
- **Contracts, not checks.** `from_sorted_iter` trusts ascending order (descending input corrupts
  the layout) and does not bounds-check against the universe — sfst guarantees both by
  construction. Use the same discipline or add your own validation.
- **Fixed universe.** Every operand shares one universe, asserted at operation time. Declaring the
  universe up front is a feature in an index file and a bug elsewhere.

Use treight when sets are written once from sorted data, stored compactly, and queried as
bytes; use roaring when sets accumulate in arbitrary order, mutate in place, or span an
unbounded universe.

## What's next

This bitmap is the ground floor. The series continues with the container around it — the SFST
chunk format, CRC-framed and zstd-packed — then the tiered key=value dictionaries and FST layer
behind these posting lists, the WAL-to-index seal lifecycle, and how the same machinery indexes
traces: span-id arenas, per-file trace-id blooms, and rollups. Same organizing idea throughout:
the bytes you store are the structure you query.

---

**References.**

- [1] Chambi, Lemire, Kaser, Godin — *Better bitmap performance with roaring bitmaps*
  (preprint): https://arxiv.org/abs/1402.6407
- [2] Same, Software: Practice & Experience 46(5), 2016: https://doi.org/10.1002/spe.2325
- [3] RoaringBitmap portable format specification:
  https://github.com/RoaringBitmap/RoaringFormatSpec
- [5] Andrew Gallant, *Transducers: the rarest and most beautiful of data structures*:
  https://burntsushi.net/transducers/
- [4] GNU idutils `fid.c` — the bit-tree that treight ports, by Greg McGary (Savannah cgit,
  blob 8fb6ca4c): https://cgit.git.savannah.gnu.org/cgit/idutils.git/tree/src/fid.c

**Source pointers.** treight: `src/crates/treight/src/{lib,raw,node,ops,bitmap,roaring}.rs`;
differential tests `src/crates/treight/src/tests_roaring.rs`; fuzz target
`src/crates/treight/fuzz/fuzz_targets/against_roaring.rs`. sfst: `src/crates/sfst/FORMAT.md`,
`src/crates/sfst/src/{build,row_index,bitset,index_reader,schema,kv_interner}.rs`.

Reproduce every measurement with the `README.md` beside this post.
