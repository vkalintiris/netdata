# Rehydration: treight blog post

Read this first when resuming work on the treight blog post in a fresh session.

## Goal

Publish an accurate, readable engineering blog post about the `treight` bitmap crate
(`src/crates/treight/`).

## Current state

- `draft.md` has been fully rewritten. It is waiting for the user's review.
- Local commits on branch `bp`. Nothing has been pushed.
  - `36e234617f`: `treight-perkv` measurement tool and evidence
  - `4ce6404954`: live heap block/byte accounting; evidence regenerated
  - `3096a7060a`: rewritten `draft.md`, plus README step 4b
- An independent internal reviewer fact-checked every number and claim in the draft against code and evidence.
  All findings are fixed.
- The local SOW (gitignored, not committed) is `.agents/sow/q/current/SOW-20261012-treight-blog-rewrite.md`. It is
  `in-progress` and stays open until the user accepts the post.

## User decisions (binding)

1. **Scope: the bitmap only.** The SFST file format gets its own future post. Do not pull SFST internals into this
   one.
2. **The user's two reasons for building treight:**
   - small serialized size
   - fewer heap allocations, which means less memory fragmentation

   The post is built around these, plus an honest account of the CPU cost.
3. Overwrite `draft.md` in place. Git keeps the history.
4. Target length: about 3,000 words of prose.
5. Describe existing code as it is. Do not present unplanned work as planned.
6. Use an independent internal fact-check review (a subagent, not an external model) before declaring the post
   done.
7. **Keep all intermediate code, scripts, programs and results in this worktree. Never write under `/tmp`.**
8. Use the certstream and jetstream corpora for measurements, per `key=value`, low- and mid-cardinality fields
   only (fewer than 1,000 distinct values per field).

## Why the previous draft was rejected

The previous draft was written by another LLM. It is preserved in git history before `3096a7060a`. Its problems:

- **Wrong roaring size claim.** It said the running example costs "60 bytes of container overhead". The real
  figures are 36 bytes for roaring (2 containers in a 262,144-value universe) against 26 bytes for treight's wire
  form.
- **Contradicted the code it quoted.** It said `PosSet` reuses buffers, while the `PosSet` snippet it showed
  allocates a fresh `Vec` on every operation.
- **An unmeasured thesis.** It argued for a "zero deserialization" read path that was never measured. It also
  buried the CPU losses.
- **Poor writing.** An audit-report voice, file paths in nearly every paragraph, defensive hedging, repetition,
  and about 5,900 words.

## Key facts (verified)

### Sizes and allocations

- **Single-member roaring list:** 18 B serialized. Loaded with roaring-rs, it is 2 live blocks totalling 42 B
  (a 40-byte `Container` slot and a 2-byte array). treight stores the same list as a 6-byte tree.
- **Six-member running example** (`{5, 7, 61, 64, 4097, 262143}`, universe 262,144): the tree is 18 B and the
  treight wire form is 26 B. Roaring serializes it to 36 B; loaded, it is 3 blocks totalling 92 B.
- **roaring-rs load cost:** one block per bitmap plus one per container. Its deserializer also makes temporary
  allocations for container headers and offsets.
- **CRoaring** has read-only views over a buffer (`roaring_bitmap_portable_deserialize_frozen`,
  `roaring_bitmap_frozen_view`). roaring-rs has none. Its nearest feature, `intersection_with_serialized_unchecked`,
  still allocates.
- **idutils:** `fid.c` was written by Greg McGary; the structure is called `tree8` there. It was confirmed from the
  file header.

### Why treight is slow

- The format stores no child offsets, so skipping a subtree means reading every byte of it.
- An intersection therefore costs roughly bytes(A) + bytes(B).
- `contains` reads every sibling subtree stored before the target value.
- `len()` reads the whole tree.
- For a complemented bitmap, `is_empty` also reads the whole tree, and `and`/`or` call it first.

### Test coverage

- The fuzzer (`fuzz/fuzz_targets/against_roaring.rs`) drives `RawBitmap` only.
- The complement layer is covered only by the proptests in `tests_roaring.rs`, on universes up to 4,096 values.

### Headline numbers

Source: `evidence/perkv/*.md`.

| | jetstream (24,034 lists) | certstream (1,152 lists) |
|---|---|---|
| tree vs roaring (optimized) | 0.76× | 0.67× |
| standalone wire vs roaring | 0.92× | 0.68× |
| zstd-3 of wire forms | 0.94× | 0.74× |
| live heap blocks after load | 24,029 vs 54,318 | 1,140 vs 4,592 |
| AND with a ~10% list + `len`, all lists | 1,375 ms vs 1.5 ms | 92.6 ms vs 1.2 ms |

- **Density bands** (tree vs roaring): treight wins below 0.01% (0.40–0.64×) and above 1% (0.50–0.84×). Roaring
  wins between 0.01% and 1%, where treight is 3–33% larger.
- **CPU range:** treight is 13× to over 900× slower across the workloads.
- **Point lookups** (criterion results in `evidence/bench/results.json`): a `contains` hit takes 4–18 µs for
  treight (misses up to 40 µs) against about 3–10 ns for roaring.

## Files

- `draft.md`: the post.
- `README.md`: reproduction steps. Step 4b covers the per-`key=value` tool.
- `evidence/perkv/<corpus>-250k.{json,md,csv.gz}`: output of the per-`key=value` tool.
- `evidence/bench/`: older criterion results and footprint totals, produced by the previous effort.
  - Only `results.json` is still cited, for the `contains` numbers.
  - Note: its `or` and `and_not` pair benchmarks hit shortcut paths on full sets and are not representative.
- `figures/`: the post uses `fig-bitree`, `fig-serialization`, `fig-complement` and `fig-setops`.
  - The `chart-*` figures belong to the old draft and are now unused.
  - `fig-sfst-overview` and `fig-lifecycle` are kept for the future SFST post.
- Stale material from the old effort, left for the user to decide on: `outline.md`, `style-notes.md`, `review/`.
- `src/crates/treight-bench/src/bin/treight-perkv.rs`: the measurement tool.
  - A counting global allocator records allocator calls, live blocks and live bytes.
  - Every workload asserts that both libraries produce the same totals.

## Reproduce the measurements

```sh
cd src/crates
cargo build --release -p treight-bench --bin treight-perkv
for c in certstream jetstream; do
  target/release/treight-perkv --derived ../../data/derived/$c-250k \
      --out ../../posts/treight-bitmap/evidence/perkv/$c-250k
done
```

- Derived caches live under `data/derived/` (gitignored).
- To rebuild them, see README step 3.
- Timings come from a shared machine: compare orderings, not decimals. If you regenerate, re-check every number in
  `draft.md` against the new output.

## Open items (user decisions pending)

1. **The user reviews the post.** Then revise, or close the SOW (`Status: completed`, then move it to `q/done/`).
2. **A potential code win, reported only.** `matched_count` in `src/crates/sfst/src/index_reader.rs` does an AND
   with a time range followed by `len()`. Using `range_cardinality` gives the same answer 9–200× faster on this
   data. Nothing has been changed.
3. **A labeled idea in the post.** Storing subtree byte lengths above a size threshold would make skips O(1). It
   is not built and not planned.
4. **Stale files.** Decide whether to delete or keep the unused charts, `outline.md`, `style-notes.md` and
   `review/`.
5. **Pushing and publishing.** Both need explicit user authorization.
