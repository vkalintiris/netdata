# Outline — treight bitmap blog post (post 1 of the SFST indexing series)

Status: awaiting Lead review. Word target for the draft: 3,500–4,300 (SOW band 3,000–4,500).
Audience: (a) software engineers, (b) Rust programmers. Voice: first-person plural. No marketing fluff.
Working title: **"treight: an 8-way bit-tree bitmap for immutable indexes"** (alt: "tree + eight").

## Numbers policy (binding for the draft)

- Measured performance/size numbers appear ONLY as `{{PLACEHOLDER: evidence-file#metric}}` until
  bench-eng's artifacts land under `posts/treight-bitmap/evidence/`; then filled verbatim from files.
- Structural constants that are verifiable against cited source files (not measurements) may be stated
  directly with attribution: `ceil_log8` ≤ 11 levels for a u32 universe; 250K-row corpus universe
  262,144 = 8⁶ → 6 levels; bitset threshold `max(universe/64, 256)`; complement above 50% density;
  8-byte wire header `[universe u32 LE][len u32 LE]`; the hand-worked example's 18 bytes.
- No personal data (DIDs, handles, cert subjects) quoted anywhere; corpus content referenced only as
  aggregates (counts, densities, sizes).

## Section plan

### 1. Hook — a query over an mmap'd index (~250 words)

Open inside a real query: two attribute filters (`service=x`, `level=error`) arrive as two posting
lists over the SAME bounded universe — the file's time-sorted row positions — and the answer is one
intersection, computed without deserializing anything. The bitmaps are intersected as stored bytes.
Framing: this post is the first in a series on the indexing machinery behind Netdata's OTel logs and
traces (the SFST file format); we start with the smallest piece, the posting-list bitmap called
`treight`.

Sources: sfst/src/index_reader.rs (`PosSet::and_assign` region), FORMAT.md `PRIM`.

### 2. The problem: posting lists over a bounded row universe (~350 words)

- What an inverted index over logs needs per `key=value` pair: a SET of row positions.
- The universe is small and known: `record_count` per file (our corpora: ~250K rows → universe
  262,144, i.e. 8⁶ — exactly 6 tree levels). Positions are `u32`, dense-bound `[0, record_count)`.
- Density variance is wild: a low-card field value can hit most rows (e.g. a dominant service name),
  a high-card value hits a handful. One representation must be small at BOTH ends and cheap in the
  middle.
- Workload shape: build once (bulk, from sorted data), read many times, mutate ~never. Immutable
  files; the query path only unions/intersects/iterates/counts.

Sources: FORMAT.md (`PRIM`, `BitmapValue`), index_reader.rs `PosSet` doc, build.rs.

### 3. Why not just roaring? (~400 words)

Credit where due (cite Chambi et al. 2016, DOI 10.1002/spe.2325 — see citation fix below — and
RoaringBitmap/RoaringFormatSpec): roaring is excellent at what it was designed for — mutable,
in-memory sets over the full u32 range. Three friction points for OUR slot:

1. Per-container overhead on tiny lists: every occupied 16-bit page costs a container header before
   payload (format spec); a 1-of-250K posting list pays that for a single member. (Exact measured
   totals → bench evidence; here we state only the structural fact.)
2. Owned, decompressed memory: a roaring bitmap read from disk must be deserialized into its own
   structure; we want the bytes on disk to BE the query-ready structure (mmap, zero-copy, no alloc
   on the read path).
3. We control the universe: roaring's machinery for unbounded u32 values is wasted when the universe
   is one file's row count and known at write time.

Punchline: we still USE roaring where it wins — accumulation at write time (see §5) — and ship a
differential fuzzer that proves treight agrees with it (§8). This is "right tool per phase", not
roaring-vs-treight tribalism.

### 4. treight, from the bottom up (~1,300 words)

#### 4.1 Lineage: GNU idutils' bit-tree

`treight = tree + eight`. The layout is a port of GNU idutils' `fid.c` (Greg McGary) — the 8-way
bit-tree idutils has used since the 1990s to store which files contain a token. `node.rs`/`ops.rs`
mirror `is_hit_1`/`skip_hits`; one deliberate addition: `skip_subtree` returns the population, not
just skipping (idutils only skips). Cite the upstream fid.c (Savannah cgit, blob 8fb6ca4c) and note
what we changed.

#### 4.2 Node bytes and the tree shape (ASCII diagram #1)

- Universe size → depth: `ceil_log8` maps 8⁶=262,144 → 6 levels; u32 never needs more than 11.
- Each internal node = ONE byte; bit i set ⇔ child i present. Empty subtrees are pruned entirely
  (no zero bytes stored — canonical form).
- Level dl consumes 3 bits of the value: `child_index(dl, v) = (v >> 3·dl) & 7`; child c of a node
  at height h spans `c << 3·(h−1)` values. Leaves ARE node bytes whose 8 bits are 8 value bits.

```
            root (dl=5)          covers 8^6 = 262,144 values
   bit7│…│bit0  each bit = "this child subtree exists"
        │
        └─ node (dl=4) … down to leaf (dl=0)
              leaf byte: 1 bit per value, 8 consecutive values
```

(Full precise diagram in the draft: node-byte bit layout + level→value-bits mapping.)

#### 4.3 Pre-order serialization + HAND-WORKED EXAMPLE (ASCII diagram #2)

Serialization is a depth-first pre-order walk: node byte first, then present children's subtrees in
ascending child order. Consequence: NO child offsets anywhere; sibling subtrees are stepped over by
walking them (`skip_subtree`) — the tree is self-delimiting.

Hand-worked example (verified by script against the algorithm; will re-verify against the crate
before final):
universe 262,144; posting list {5, 7, 61, 64, 4097, 262143} (6 row positions).
Exact tree bytes (18):

```
81 03 01 03 81 a0 20 01 01 01 01 01 02 80 80 80 80 80
```

Walk through it in the draft, byte by byte, with the annotated tree:

```
root 81 ── child0 ─> 03 ── child0 ─> 01 ── child0 ─> 03 ── child0 ─> 81 ─┬─ child0 ─> leaf a0 (5,7)
              │                                    │                    └─ child7 ─> leaf 20 (61)
              │                                    └─ child1 ─> 01 ── child0 ─> leaf 01 (64)
              └─ child1 ─> 01 ─> 01 ─> 01 ─> 01 ─> leaf 02 (4097)
     └─ child7 ─> 80 ─> 80 ─> 80 ─> 80 ─> leaf 80 (262143)   ← the max-value rightmost path
```

Wire form (`serialize_into`): `00 00 04 00 | 12 00 00 00 | <18 tree bytes>` = 26 bytes total
(8-byte header + tree). Reading it back = `deserialize_from`, which re-derives levels via
`ceil_log8(universe)` — the level count is never stored.

Also state the per-value cost intuition: an isolated value costs exactly `levels` bytes (its
root-to-leaf chain); shared prefixes are shared bytes. 6 isolated values here cost 18 bytes because
three share the leftmost path.

#### 4.4 Canonical form: exact size prediction, O(1) emptiness

- `estimate_data_size(universe, sorted_values)` predicts the EXACT blob length without building it
  (one byte per distinct (level, node-index) pair on the value paths + root). Pinned exact by
  proptest `estimate_data_size_is_exact` and by the fuzz target's closing check.
- Use: weigh a roaring→treight conversion before doing it (code sketch from lib.rs docs).
- Canonicality ⇒ `is_empty` is a blob-length check; a non-empty blob always holds ≥1 value.
- Every mutation path preserves canonical form — fuzz-asserted.

#### 4.5 The Rust API: a `Copy` descriptor over an external `&[u8]` blob (~snippet)

- `RawBitmap { universe_size, levels }` is 8-ish bytes, `Copy`; tree bytes live in a caller-owned
  `&[u8]`/`&mut Vec<u8>` passed to EVERY method. Two descriptors of the same universe are
  interchangeable; set ops return either operand's.
- The serialized form IS the in-memory form: no deserialize step on the read path; the blob can be a
  mmap slice. Zero allocation for contains/len/min/max/iter over a mapped file.
- Set-op convention: ops APPEND the result's bytes to a caller `out` buffer; caller swaps buffers
  (that's exactly what sfst's `PosSet::or_assign` does).
- Snippet (attributed `src/crates/treight/src/raw.rs`): the `from_sorted_iter` core loop —
  per-level open-a-byte / OR-the-bit, single pass, O(values × levels), no staging bitvec.

#### 4.6 Dense sets: the complement representation + De Morgan dispatch (table #1)

- `Bitmap` = `RawBitmap` + `inverted: bool`. Sparse → store members; dense → store HOLES
  (`from_sorted_iter_complemented`); `full` stores zero bytes. `complement()` is a flag flip.
- Dispatch tables (from bitmap.rs, reproduced verbatim as tables):

```
and(A,B)                or(A,B)
N&N -> intersect   N    N|N -> union       N
N&I -> A−B         N    N|I -> B−A         I
I&N -> B−A         N    I|N -> A−B         I
I&I -> union       I    I|I -> intersect   I
```

- `and_not` = `and(complement)`. Note the empty/full short-circuits and the universe-mismatch
  assert (raw ops assert unconditionally → release-mode panic, cannot silently mis-answer).

#### 4.7 Lockstep subtree walkers (table #2)

All four raw set ops are ONE recursive shape — two readers advance in lockstep, one subtree per
child position — differing only in (leaf op, A-only column, B-only column):

```
Op   leaf     A-only child   B-only child
OR   a|b      copy           copy
AND  a&b      skip           skip
SUB  a&!b     copy           skip
XOR  a^b      copy           copy
```

Pruning: OR never prunes; AND/SUB/XOR reserve a node byte, back-patch surviving child bits, pop the
byte if nothing survives (canonical output for free). Snippet: `intersect_subtree`'s match arm
(attributed ops.rs). Also: `copy_subtree` = skip + verbatim byte-range append (no re-encode).

### 5. Where it lives: the SFST index lifecycle (~400 words)

Build side (sfst/src/build.rs `remap_one_bitmap`):
1. Rows arrive in insertion order; the indexer accumulates each `key=value` posting list as an
   in-memory `RoaringBitmap` — arbitrary-order inserts, roaring's home turf.
2. At seal time rows are remapped to TIME order (the file is chronological); posting lists remap
   with them.
3. Encoding per value, two independent decisions:
   - cardinality ≥ `max(universe/64, 256)` → build through a scratch `Bitset` (O(n) set + ascending
     scan) instead of sorting a vec;
   - density > 50% → store the complement (`from_sorted_iter_complemented`).
4. On disk (`PRIM`/`MF{i}` chunks): `BitmapValue { desc: treight::Bitmap, data: Vec<u8> }` — the
   descriptor rides in the chunk schema beside its tree bytes.

Query side (index_reader.rs `PosSet`): every intermediate set is a `PosSet` (treight `Bitmap` +
owned bytes). `from_value` clones the on-disk tree bytes once, then `and_assign`/`or_assign`/
`and_not_assign`/`range`/`full` stay native — NO roaring round-trip anywhere on the read path.
ASCII diagram #3: the lifecycle (WAL → accumulate as roaring → remap → encode as treight → mmap →
PosSet ops).

### 6. treight vs roaring, honestly (~550 words + comparison table)

Comparison axes (each row cites the winning side's mechanism):
- Layout: fixed-depth 8-way byte tree vs 64K containers with typed payloads (array/bitmap/run).
- Membership: both O(depth)/O(container-lookup + array/bitmap probe); treight's `contains` skips
  sibling subtrees so cost tracks preceding bytes, not just depth (fid.c `is_hit_1` port).
- `len`: roaring O(#containers) (stored cardinalities); treight walks the whole blob summing leaf
  bits → roaring wins big sets. (Honest: treight `len` is O(blob).)
- Set ops: roaring's vectorized 64/128-bit container kernels vs treight's byte-wise lockstep walk →
  roaring wins on large operands; treight competitive when operands are small/cold (exact numbers →
  bench evidence).
- Serialized size, small lists: treight ≤ k·levels bytes for k members, no per-page overhead;
  roaring pays per-container headers (structural fact; measured totals → evidence).
- Serialized size, dense: treight complement keeps only holes; roaring run containers compress runs
  — both good, different shapes (measured → evidence).
- Read path: treight = zero deserialization, mmap-able, `Copy` handle; roaring = deserialize to
  query. treight wins cold-start/resident-set.
- Mutability: roaring inserts anywhere O(container); treight bulk-build is sort-based; single
  `insert` splices the missing chain but walks sibling subtrees; `remove_range` re-encodes →
  roaring wins mutation-heavy use. We don't have mutations on the read path; that's WHY treight fits.
- Universe: treight needs a known bound; roaring is unbounded u32.
- "Where each wins" summary table + explicit guidance sentence.

### 7. Correctness: differential fuzzing against roaring (~250 words)

- Method: same random op sequence replayed on treight `RawBitmap` and on roaring (the oracle);
  every query op asserts agreement (len/min/max/is_empty/contains/iter/range_cardinality), set ops
  compared by iteration, serialize roundtrip checked, and the final blob must still equal
  `estimate_data_size` (canonicality under arbitrary mutation).
- Universes chosen to hit min/max at every depth 1..8 (the UNIVERSES table).
- Proptest twin in `tests_roaring.rs` (≤4096 universes, exhaustive membership checks, De Morgan
  flag combinations checked against a per-value oracle).
- How to run: `cargo fuzz run against_roaring` from `src/crates/treight/fuzz`; both sides pin the
  same netdata/roaring-rs fork (branch `allocative`) so there's exactly one roaring under test.

### 8. Benchmarks (~600 words; numbers 100% from evidence files)

- Methodology: hardware (AMD Ryzen 9 5900X, 31 GB RAM — disclosed), rustc/criterion versions
  (placeholder), roaring pin (netdata/roaring-rs @ branch allocative), corpora provenance:
  certstream-250k (Certificate Transparency firehose), jetstream-250k (Bluesky Jetstream firehose),
  mixed-250k (interleaved by arrival, tagged) — real public event streams captured via the
  otel-streams source modules; counts/SHA256s in `evidence/corpora/` (README explains re-capture).
- Posting-list derivation mirrors the production flatten path (bench-eng verified; exact tool name
  and command from README).
- Both phases measured: BUILD (bulk encode from sorted positions; incremental insert stream) and
  QUERY (contains hit/miss, len, min/max, full iteration, and/or/and_not on realistic pairs, range
  counting), plus SIZE (per-corpus total treight bytes vs roaring serialized bytes over ALL posting
  lists; in-memory heap) — index-footprint totals.
- Results tables per corpus + density-sliced interpretation: which density band each side wins and
  why that matches the layout story (§6). Every table cell cites its evidence file.
- `{{PLACEHOLDER: ...}}` for all of the above until bench artifacts land.

### 9. Limitations — when NOT to use treight (~300 words)

- Mutation-heavy workloads (sorted bulk-build or splice-based inserts; `remove_range` re-encodes).
- Huge sparse universes: cost grows with members × levels, not with the interval they span
  (6 bytes/member at 250K rows; 11 at u32-max) — roaring's array containers are the better fit
  when members are few and scattered over unbounded u32.
- No rank/select acceleration structures; `len`/iteration walk stored bytes (O(blob)) — if you need
  O(container) cardinality or select(), roaring.
- Inverted-`Bitmap` iteration is O(universe) (ComplementIter walks the universe skipping stored
  holes); `range_cardinality` on inverted sets subtracts within the range.
- Ascending-input contract on bulk build (`from_sorted_iter` corrupts on descending input);
  out-of-universe values are not checked on that path — the caller (sfst) guarantees both.
- Fixed universe: universe must be known and shared; ops assert universe equality.

### 10. Series teaser (~100 words)

Next posts: the SFST container itself (chunk framing, tier-aligned IDs, the FST/prefix-map layer
that maps `key=value` → these bitmaps), the WAL→index seal lifecycle, and how the same machinery
indexes traces (span-id arenas, trace rollups).

### 11. Appendix: links + reproducibility pointer

- Source pointers (repo paths), upstream references (idutils fid.c; RoaringFormatSpec; Chambi et
  al. 2016), and "see README.md to re-capture corpora and re-run every benchmark".

## Deliverable 2: README.md (reproducibility) — plan

- Prereqs: rust toolchain (+ nightly for cargo fuzz), zstd?, disk space for corpora.
- NOTE (Lead's heads-up): on this dev box `cc` is ccache-wrapped and ccache's default cache dir is
  read-only under the sandbox — every build/bench command is written as
  `CCACHE_DIR=/tmp/nd-bp-ccache cargo ...`; include a one-line note so commands are copy-pasteable.
- Steps: (1) re-capture corpora (corpus-eng's capture tool + exact commands — NAME PLACEHOLDER
  pending Lead relay); (2) verify SHA256s/line counts against `evidence/corpora/`; (3) derive
  posting lists (bench-eng tool — NAME PLACEHOLDER); (4) run the criterion bench
  (`cargo bench` invocation with env vars, sfst/decode.rs pattern — PLACEHOLDER); (5) regenerate
  the density/size histograms; (6) run the differential fuzzer + proptests.
- Layout of `evidence/` explained (what each artifact backs).

## Open items for the Lead (in the outline-ready message)

1. CITATION FIX (SOW bug): "arXiv:1602.08767" is an unrelated astrophysics paper (verified by
   fetch). Correct citation for Chambi, Lemire, Kaser, Godin, "Better bitmap performance with
   roaring bitmaps", SPE 46(5):709–719, 2016 → DOI 10.1002/spe.2325 (preprint arXiv:1402.6407).
   I'll cite the DOI + arXiv:1402.6407; SOW line should be corrected at close.
2. Verified upstream URLs: idutils fid.c at
   https://cgit.git.savannah.gnu.org/cgit/idutils.git/tree/src/fid.c (blob 8fb6ca4c); spec at
   https://github.com/RoaringBitmap/RoaringFormatSpec.
3. Hand-worked example values {5, 7, 61, 64, 4097, 262143} over universe 262,144 are illustrative
   row positions (aggregates-safe), not quoted corpus content — confirm that satisfies the
   sensitive-data plan, or I'll restate them as anonymized positions from an evidence file.
4. Tool/command names for README still placeholders — will fill from your relay.
