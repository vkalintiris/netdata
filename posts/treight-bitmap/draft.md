# treight: trading CPU for bytes and allocations in a bitmap

*First in a series on the indexing behind Netdata's OpenTelemetry logs. This post covers only the bitmap.*

An inverted index over logs stores, for every `key=value` pair, the set of rows that contain it. That set is a
posting list, and the usual way to store one is a compressed bitmap. The usual compressed bitmap is
[roaring][1].

We wrote our own, `treight` (**tre**e + **eight**): an 8-way bit-tree stored as one byte per node. We had two
reasons:

1. **Size.** Most of our posting lists are tiny, and roaring's fixed per-bitmap and per-container overhead
   dominates them.
2. **Allocations.** A roaring bitmap in memory is a vector of containers plus one heap block per container.
   Thousands of small bitmaps mean tens of thousands of small allocations of varying size. A treight bitmap is
   one flat byte slice, which can live in an arena, a reused buffer, or a memory map.

Both reasons hold up when measured on real posting lists: the trees are 24–33% smaller than roaring's
serialized form, and loading them leaves 2–4× fewer live heap blocks. They also have a cost, and it is large:
treight's set operations are 13× to over 900× slower than roaring's on the same data. This post explains how
treight works, shows the measurements, and explains where that CPU cost comes from.

## The setting

Three properties of the workload shaped the design:

- **A small, known universe.** Each index file holds one sealed batch of rows. The bitmap universe is that
  file's row count, typically hundreds of thousands, never all of `u32`.
- **Write once, read many.** Posting lists are built once, from sorted positions, when the file is sealed. After
  that they are only read: intersected, unioned, counted, iterated.
- **Mostly tiny lists.** We measured two public event streams, 250,000 events each: Certificate Transparency
  entries (certstream) and the Bluesky Jetstream firehose. Taking only fields with fewer than 1,000 distinct
  values, jetstream yields 24,034 posting lists across 2,048 fields. Their median cardinality is **1**, and
  20,919 of them hold two rows or fewer. Certstream is denser (1,152 lists across 33 fields, median cardinality
  25), but it still has a long tail of rare values.

## What roaring costs on a tiny list

Roaring ([Chambi, Lemire, Kaser, Godin][1]) splits the `u32` space into 65,536 chunks by the top 16 bits of
each value. Each non-empty chunk is a container that stores its low 16 bits as a sorted array, an 8 KiB bitset,
or a list of runs. Membership is two lookups, cardinality is a sum of stored counts, and bitset intersections
run 64 bits at a time. For mutable sets, and for general-purpose use, it is excellent. We use it ourselves
to accumulate posting lists while ingesting.

For a list with a single member, the [portable format][2] costs 18 bytes: a 4-byte cookie, a 4-byte container
count, 4 bytes of key and cardinality, a 4-byte offset, and 2 bytes of payload.

In memory, the roaring-rs crate we use (a Netdata fork, commit `3ee69b1`) represents a bitmap as
`Vec<Container>`, where each container's payload is its own `Vec<u16>`, boxed bitset, or run vector. So a
loaded single-member bitmap is 2 heap allocations totalling 42 bytes: a 40-byte container slot and a 2-byte
array. Every non-empty bitmap needs at least two allocations, plus one more per additional container.

The C library, CRoaring, has an answer for the allocation problem: `roaring_bitmap_portable_deserialize_frozen`
and `roaring_bitmap_frozen_view` build read-only views over a buffer, including a memory-mapped one. roaring-rs
has no such view. The closest thing it offers is `intersection_with_serialized_unchecked`, which intersects a
loaded bitmap with a serialized one and decodes fewer containers, but still allocates. treight was written for
Rust.

## treight from the bottom up

### Lineage

The layout is not new. treight ports the bit-tree from GNU [idutils][3]' `fid.c`, written by Greg McGary, where
it is called `tree8`. idutils has used it for decades to store "which files contain this token". That is the same posting-list problem over a
universe of file ids. treight's subtree walker mirrors idutils' `skip_hits`, with one addition: it returns the
number of values it skipped, which is how treight computes `len()` and range counts.

### Node bytes

The universe fixes the tree depth: `ceil(log8(universe))` levels, six for 250,000 rows. Each level consumes 3
bits of the value, so a value's root-to-leaf path is its base-8 representation, most significant digit first.

- **Inner node:** one byte. Bit `i` set means child `i` exists.
- **Leaf:** one byte. Bit `i` set means value `leaf_base + i` is in the set.
- **Empty subtrees are pruned.** Every stored byte is non-zero.

![The treight 8-way bit-tree over a 512-value universe: one byte per node, leaves holding eight value bits, a
pruned subtree, and the level-to-digit mapping](figures/fig-bitree/fig-bitree.svg)

Why eight children? A byte is the smallest unit you can address without bit-packing, and eight children are
exactly what a byte's bits can describe. The trade-off is easy to reason about: an isolated member costs one
node per level. At 250,000 rows that is 6 levels × 1 byte = 6 bytes with 8-way nodes. With 16-way nodes
(`u16` masks) it would be 5 levels × 2 bytes = 10 bytes. Narrower nodes would cost less per member, but they
are no longer byte-aligned.

### Serialization, by hand

The tree is stored as a depth-first pre-order walk: a node byte, then each present child's subtree in ascending
order. There are no pointers or offsets. Each node byte says which children follow, and the depth comes from
the universe, so the stream parses itself.

Take a universe of 262,144 (8⁶, so the arithmetic is exact) and six members, `{5, 7, 61, 64, 4097, 262143}`.
Written as base-8 digits from the root down:

```
value    l5 l4 l3 l2 l1  leaf bit
5         0  0  0  0  0  5
7         0  0  0  0  0  7     same leaf byte as 5
61        0  0  0  0  7  5
64        0  0  0  1  0  0
4097      0  1  0  0  0  1
262143    7  7  7  7  7  7     the last value: the rightmost path
```

The tree is 18 bytes:

```
81 03 01 03 81 a0 20 01 01 01 01 01 02 80 80 80 80 80

81  root          children {0, 7}
03  ├─0           children {0, 1}
01  │ ├─0         child {0}
03  │ │ └─0       children {0, 1}
81  │ │   ├─0     children {0, 7}
a0  │ │   │ ├─0   leaf: 5, 7
20  │ │   │ └─7   leaf: 61
01  │ │   └─1     child {0}
01  │ │     └─0   leaf: 64
01  │ └─1         child {0}
01  │   └─0       child {0}
01  │     └─0     child {0}
02  │       └─0   leaf: 4097
80  └─7           child {7}
80    └─7         child {7}
80      └─7       child {7}
80        └─7     child {7}
80          └─7   leaf: 262143
```

To check the count, add up distinct nodes per level, from root to leaf: 1 + 2 + 3 + 3 + 4 + 5 = 18. Values that
share a path prefix share its bytes. The standalone wire form adds an 8-byte header (universe size and tree
length, both `u32`), for 26 bytes in total. The level count is not stored; it is derived from the universe.

Roaring stores the same set in 36 bytes. With a universe of 262,144, all six values fall into just two
containers. That is 8 bytes of header, 16 bytes of per-container entries, and 12 bytes of payload. Loaded with
roaring-rs, it is 3 allocations and 92 bytes. Loaded with treight, it is one 18-byte allocation, or none if the
bytes are read in place.

![The same 18 bytes drawn as a tree, aligned over the flat wire
form](figures/fig-serialization/fig-serialization.svg)

### Canonical form

Every path that produces a tree produces exactly the bytes it needs, with no padding or dead space: the bulk
builder, `insert`, `remove`, `remove_range`, and all four set operations. As a result:

- `estimate_data_size(universe, sorted_values)` returns the exact size without building anything. It counts
  distinct `(level, node)` pairs on the values' paths. You can compare it with roaring's `serialized_size()` and
  decide whether a conversion is worth doing before you do it.
- `RawBitmap::is_empty()` is a length check.
- The fuzzer enforces it: after a random sequence of operations, the final tree must be exactly
  `estimate_data_size` bytes long.

## The API: a `Copy` descriptor over someone else's bytes

This is where the allocation reason becomes concrete. A treight bitmap is a descriptor plus bytes, and the
descriptor does not own the bytes:

```rust
#[derive(Copy, Clone)]
pub struct RawBitmap {
    universe_size: u32,
    levels: u8,
}

impl RawBitmap {
    pub fn contains(&self, data: &[u8], value: u32) -> bool;
    pub fn len(&self, data: &[u8]) -> u64;
    pub fn intersect(&self, a: &[u8], other: &RawBitmap, b: &[u8], out: &mut Vec<u8>) -> RawBitmap;
    // ...
}
```

What follows from that:

- **Storage is the caller's choice.** The tree bytes can be a `Vec<u8>`, a slice of one large arena holding
  thousands of bitmaps, or a slice of a memory map. Zero bytes is a valid, empty bitmap.
- **Queries never allocate.** `contains`, `len`, `range_cardinality`, `min`, `max`, and iteration all read the
  slice. The iterator keeps its depth-first stack in a fixed array of at most 10 frames.
- **Set operations append to a caller-supplied buffer.** Reuse one buffer across a query and, once it has grown
  to size, set operations make no allocations at all.
- **Descriptors are interchangeable.** Two descriptors with the same universe are identical, so a set operation
  can return either operand's.

The bulk builder is a single pass over ascending values. It keeps one open node per level, remembered as a
group id and an output position:

```rust
for v in iter {
    for dl in (0..levels).rev() {
        let group = v.checked_shr(3 * (dl + 1)).unwrap_or(0);
        if group != prev_group[dl as usize] {   // entered a new node at this level
            prev_group[dl as usize] = group;
            node_pos[dl as usize] = out.len();
            out.push(0);
        }
        out[node_pos[dl as usize]] |= 1u8 << child_index(dl, v);
    }
}
```

The input must be in ascending order. This is a contract, not a check: descending input corrupts the layout.

## Dense sets: store the holes

A tree holding every row of a 262,144-row universe would cost (8⁶ − 1) / 7 = 37,449 bytes. A posting list that
matches 99% of rows should pay for the 1% it misses, not for the 99%.

The `Bitmap` wrapper adds an `inverted` flag. Dense sets store their complement. `full()` is an inverted
descriptor over an empty tree, which costs zero bytes. `complement()` flips the flag and touches no bytes. For
example, a 60-of-64 set stores its four holes (9, 25, 26, 60) as a 4-byte tree: `8a 02 06 10`.

Set operations never build a complement. They pick the operation by De Morgan's laws, based on the two flags:

| `a AND b` | b stored directly | b stored inverted |
|---|---|---|
| **a stored directly** | intersect → direct | a − b → direct |
| **a stored inverted** | b − a → direct | union → inverted |

`OR` has the mirror-image table, and `and_not(b)` is `and(complement(b))`.

![The inverted flag, the De Morgan dispatch tables for AND and OR, and the empty/full
shortcuts](figures/fig-complement/fig-complement.svg)

## One walker, four operations

All four raw set operations are the same recursive walk. Two readers advance through their trees in lockstep,
one child slot at a time. The operations differ only in the byte operation applied at the leaves and in what
happens to a child present in only one tree:

| operation | leaf byte | child only in A | child only in B |
|---|---|---|---|
| union | `a \| b` | copy | copy |
| intersection | `a & b` | skip | skip |
| difference | `a & !b` | copy | skip |
| symmetric difference | `a ^ b` | copy | copy |

"Copy" appends the subtree's bytes to the output verbatim, without re-encoding. Intersection, difference and
symmetric difference can produce empty subtrees. For those, the walker reserves the parent's node byte, fills it
in with the children that survived, and removes it if none did. The output is canonical by construction.

On a 64-value universe: A = {0..7, 16, 40} is `25 ff 01 01` and B = {1, 5, 16, 17} is `05 22 03`. Then A ∩ B is
`05 22 01` and A − B is `21 dd 01`. In the difference, A's child 5 is copied whole, and child 2 (`01 & !03`)
becomes zero and is pruned.

![Intersection and difference walked side by side: the reserved node byte, the fill-in, the pruned zero leaf, and
skip versus copy](figures/fig-setops/fig-setops.svg)

This table also explains treight's main weakness. Look at the "skip" entries. There are no offsets in the
format, so skipping a subtree means reading every byte in it. Intersecting a 6-byte list with a 20 KB list
reads all 20 KB.

## Measurements

Setup:

- **Lists:** every posting list from fields with fewer than 1,000 distinct values, in both corpora. Positions
  are in chronological order, and treight stores lists denser than half the universe as complements, as our
  index does.
- **Roaring:** run-optimized (`optimize()`) before serializing, which is what a careful roaring user would
  store.
- **Allocations:** counted by a global allocator wrapper.
- **Timings:** fastest of 7 runs, release build, one thread, on a shared AMD Ryzen 9 5900X. Compare orderings,
  not decimals.
- **Cross-check:** loading checks that each list round-trips identically, and every workload checks that both
  libraries produce the same total count.
- **Code and data:** the tool is `treight-perkv` in `src/crates/treight-bench`, and its raw output, including a
  per-list CSV, is in `posts/treight-bitmap/evidence/perkv/`.

### Size

| | jetstream | certstream |
|---|---|---|
| posting lists (fields) | 24,034 (2,048) | 1,152 (33) |
| roaring, serialized | 1,311,699 B | 2,668,315 B |
| treight, tree bytes | 994,834 B (**0.76×**) | 1,792,162 B (**0.67×**) |
| treight, standalone wire form | 1,211,140 B (0.92×) | 1,802,530 B (0.68×) |
| lists where treight's wire form is smaller | 22,433 of 24,034 | 656 of 1,152 |
| zstd level 3, wire forms concatenated | 327,405 vs 348,623 B (0.94×) | 600,795 vs 807,158 B (0.74×) |

Three things in this table are easy to miss:

- **Headers matter as much as encoding.** For jetstream lists below 0.01% density (fewer than 25 rows, 23,030
  lists), treight's tree bytes are 0.40× roaring's.
  Its 8-byte standalone header (plus a flag byte) eats most of that gain. Inside an index file the universe is
  shared, so a bitmap needs only its tree and a length. Roaring's cookie and container count are part of its
  format.
- **The middle density band belongs to roaring.** For lists between 0.01% and 1% density, treight's trees are
  3–33% larger than roaring's serialized form. Treight pays a full root-to-leaf path for every isolated member, while roaring's 2-byte array entries
  are close to optimal there. Treight wins at both ends: below 0.01% (0.40–0.64×) and above 1% (0.50–0.84×),
  where full leaf bytes are cheap and, above 50%, complements pay only for the holes.
- **Compression narrows the gap.** Run zstd over the concatenated wire forms and treight's advantage drops from
  8% to 6% on jetstream and from 32% to 26% on certstream.

### Allocations

Loading every list from its stored bytes into a queryable value. Treight is shown with one `Vec` per list, the
least favorable option for it; an arena needs one block, and reading in place needs none.

| | jetstream roaring | jetstream treight | certstream roaring | certstream treight |
|---|---|---|---|---|
| live heap blocks afterwards | 54,318 | 24,029 | 4,592 | 1,140 |
| live heap bytes afterwards | 2,088,480 | 994,834 | 2,769,000 | 1,792,162 |
| allocator calls, including temporary ones | 102,393 | 24,029 | 6,941 | 1,140 |
| time to load all lists | 1.87 ms | 0.44 ms | 0.66 ms | 0.08 ms |

The roaring live-block counts are exactly one block per bitmap plus one per container: 24,034 + 30,284 on
jetstream. The allocator-call counts are higher because roaring-rs's deserializer also allocates temporary
buffers for the container headers and offsets. Treight's 24,029 is one block per non-empty list: the 5 lists
that match every row are complemented to an empty tree and need no bytes.

Allocations during queries matter as much as allocations at load time. A roaring `&` returns a new bitmap: an
allocation for its container vector, plus roughly one per container both operands share. Running each
set-operation workload below over every jetstream list, roaring made between 8,411 and 38,424 allocator calls.
Treight made 1 (AND) or 17 (OR, two buffers) on the first pass while its reused buffers grew, and zero on every
pass after that.

We measured the cause of fragmentation (many small, short- and long-lived allocations of varying sizes), not
fragmentation itself. Fragmentation depends on the allocator and on the rest of the process.

### CPU

Every workload runs over every list. The totals are in milliseconds:

| workload (jetstream, 24,034 lists) | treight | roaring | roaring allocations |
|---|---|---|---|
| AND with a 2% time window, then `len` | 44.1 | 0.50 | 8,411 |
| `range_cardinality` over the same 2% window | 4.8 | 0.15 | 0 |
| AND with a 50% time window, then `len` | 968 | 2.8 | 36,457 |
| AND with a ~10%-density list, then `len` | 1,375 | 1.5 | 38,424 |
| OR all values of each field, then `len` | 88.1 | 6.9 | 31,813 |

| workload (certstream, 1,152 lists) | treight | roaring | roaring allocations |
|---|---|---|---|
| AND with a 2% time window, then `len` | 11.2 | 0.30 | 1,503 |
| `range_cardinality` over the same 2% window | 8.4 | 0.08 | 0 |
| AND with a 50% time window, then `len` | 64.2 | 2.5 | 3,757 |
| AND with a ~10%-density list, then `len` | 92.6 | 1.2 | 3,765 |
| OR all values of each field, then `len` | 88.5 | 4.2 | 225 |

Treight is 13× to over 900× slower. The worst cases have the same shape: a tiny list intersected with a
large one. A single-member jetstream list is 6 bytes. The 10%-density partner is 22,471 bytes, and the walker
reads all of it, because every child present only in the partner must be skipped byte by byte. Roaring compares
16-bit container keys and touches only the containers both sides share.

Point operations show the same pattern. On the sampled lists in our criterion benchmarks, a treight `contains`
hit takes 4–18 µs (misses on dense lists up to 40 µs), against about 3–10 ns for roaring. The reason is the same: to reach a value's path, the walker reads
every sibling subtree stored before it. `len()` reads the whole tree, while roaring sums its stored per-container
counts.

Two smaller observations:

- **Count instead of materializing.** For the same answer over the 2% window, `range_cardinality` is 9× faster
  than AND-then-`len` on jetstream, because it reads only the list and never touches a window tree or builds an
  intersection. On certstream, whose lists are larger, the gain is about 1.3×. Over the 50% window the gains
  are 200× and 7.6×.
- **Range counting costs what the list costs.** It does not descend into subtrees that lie entirely outside or
  entirely inside the range, but it still has to read their bytes to step over them. The cost therefore depends
  on the list's size, not the window's: the 50% window took 4.8 ms on jetstream and 8.4 ms on certstream, the
  same as the 2% window.

Speculation, not built: subtree byte lengths stored above some size threshold would make skips O(1), at the
cost of some bytes and the canonical-size property. This is the obvious lever if CPU becomes the bottleneck.

## Correctness: fuzzing against roaring

Roaring is also treight's test oracle. A `cargo fuzz` target generates a universe, two seed sets, and a random
sequence of operations: mutations, queries, all four set operations, serialization round-trips, and operand
swaps. It applies each operation to a raw treight bitmap and to a `RoaringBitmap` with the same contents, and
every query must give the same answer on both. After seeding and at the end, `len`, `min`, `max`, `is_empty`,
and the full iteration must all match. The universe sizes are the smallest and largest for every tree depth
from 1 to 8, so level-boundary arithmetic is exercised at both edges. The final check is canonical form:
whatever happened along the way, the final tree must be exactly `estimate_data_size` bytes.

The fuzzer drives the raw layer only. The complement layer and its De Morgan dispatch are covered by an in-crate
property-test suite that checks each operation against roaring on universes up to 4,096 values, with exhaustive
membership checks and random inverted flags on both operands.

## When not to use treight

- **CPU-bound query paths.** If set operations or membership tests dominate your latency, roaring is faster by
  one to three orders of magnitude on this data.
- **Mutable sets.** Bulk building needs sorted input. `insert` works, but it walks and splices bytes. Removing a
  range rebuilds the tree.
- **Huge sparse universes.** Cost grows with members × levels. With a full `u32` universe, an isolated member
  costs 11 bytes. Roaring's arrays cost 2 bytes per member plus container overhead.
- **Counting.** `len()` reads the whole tree. A complemented bitmap's emptiness check does too, and `and`/`or`
  run that check before every operation that involves a complemented operand.
- **Iterating complemented sets.** Iteration walks the whole universe, skipping the stored holes.
- **Unchecked input.** The builder trusts that input is ascending and within the universe. Loading trusts that
  the bytes are canonical. Validate untrusted input yourself.

Use treight when bitmaps are written once from sorted data, stored by the thousand, and read rarely enough that
bytes and allocations matter more than nanoseconds. Use roaring otherwise. We still use roaring for every bitmap
that changes.

## What's next

The next post covers the index file around these bitmaps: how it is laid out, which fields get bitmaps at all,
and how queries combine them.

---

**References**

- [1] Chambi, Lemire, Kaser, Godin, *Better bitmap performance with Roaring bitmaps*, Software: Practice &
  Experience 46(5), 2016. Preprint: https://arxiv.org/abs/1402.6407
- [2] Roaring portable format specification: https://github.com/RoaringBitmap/RoaringFormatSpec
- [3] GNU idutils `fid.c`: https://cgit.git.savannah.gnu.org/cgit/idutils.git/tree/src/fid.c

**Source.** treight: `src/crates/treight/`. Fuzz target: `src/crates/treight/fuzz/fuzz_targets/against_roaring.rs`.
Measurements: `src/crates/treight-bench/src/bin/treight-perkv.rs`, output in
`posts/treight-bitmap/evidence/perkv/`. CRoaring frozen views: `include/roaring/roaring.h` in
https://github.com/RoaringBitmap/CRoaring.
