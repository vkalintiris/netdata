# sfsq readability & maintainability backlog

Consolidated refactor candidates for the `sfsq::logs` query engine (plus a
couple of adjacent `otel-ledger` items), gathered while reading the WAL-query
implementation to make it readable and maintainable.

## Ground rules

- **Blast radius is not a constraint.** The goal is the clearest long-term
  code, not the smallest diff.
- **`sfsq/tests/wal_equivalence.rs` is the safety net.** Every refactor must
  keep it green (it proves a live query == index-then-query). A red
  equivalence test is a hard stop.
- **Leaf vs load-bearing.** Self-contained types/functions are improved in
  place. Load-bearing pieces — those whose exact shape other code silently
  relies on (the cursor total order, the wire codec) — are redesigned only
  after their machinery is fully read.
- Items carry `file:line` evidence; line numbers are approximate (the code
  shifts as items land).

## Meta-note

The biggest readability debt — the cursor discriminator (`M-4`) — surfaced
only from reading the cross-cutting types. Local, function-scoped passes
miss it because it lives in the *type* shape spanning `cursor.rs`, `page.rs`,
`engine.rs`, and `wal_scan.rs`, not in any single function. Keep that in mind
when prioritizing: the cheap local wins are real, but the structural win is
the headline.

---

## Items

### Correctness / robustness

| ID | Item | Evidence | Status |
|----|------|----------|--------|
| C-1 ✅ | `PageShard::evaluate` built cursors with `timestamps.at(position).unwrap_or(0)`. A `None` would emit a `1970` timestamp that sorts to the oldest position and corrupts the page / resets the consumer window. **Verified**: `at` is `None` only when `pos >= len` (`query.rs:242`); for a well-formed file matched positions are always in range, so this never fires in normal operation. It is a silent-corruption guard for *malformed* files only. | `page.rs:96-103` | **Done (`0c001e1092`)** — returns `sfst::Error::CorruptIndex`; `paginate` skips the source with a warning, consistent with the engine's per-source degradation. |
| C-2 ✅ | `matched` was `u64` in `LogsShard` but `usize` in `LogsData`, bridged by an early `as usize` inside the engine — the only count narrowed *before* the wire boundary (facet `u32` and bucket `u64` counts are carried sfst-native in `LogsData` and cast to `usize` at the adapter, `adapter.rs:226,284-285`). | `result.rs:19`, `engine.rs:201`, `adapter.rs:176` | **Done.** Made `LogsData::matched: u64` (matches the `Bucket::counts: u64` it already carries), dropped the engine cast, and moved the `as usize` to the wire adapter next to the existing bucket casts. The narrowing now lives at the single UI boundary all counts share, not hidden mid-engine. (Not a 32-bit-safety change — the agent is 64-bit; this is consistency.) |
| C-3 ✅ | `sfst::IndexReader::materialize_rows` silently dropped positions it couldn't resolve (out-of-range / missing batch / over-local `continue`s) and zeroed a missing timestamp via `at(pos).unwrap_or(0)`. Because the caller pairs requested positions with returned rows by index, a dropped row misaligned the page (rows attached to the wrong cursors), not just a missing row. Effectively unreachable for well-formed files (selected positions are always in range), but reachable for a corrupt SFST. | `reader.rs` (`materialize_rows`) | **Done.** The three skips and the timestamp default now return `Error::CorruptIndex` instead of silently skipping, so the function yields exactly one row per position or errors; `page.rs` already propagates that into the existing empty-page collapse. Matches the row-misalignment guard already on the WAL row-scanner. |
| C-4 ⛔ | A negative-path test for C-1's `CorruptIndex` skip. **Won't do — the branch is unreachable by construction.** `PageShard::evaluate` only ever looks up positions from `matched_positions`, which clamps the match set to `[lo, hi)` where `hi = range_positions(window).1 <= timestamps.len()` (the window is resolved against the *same* timestamps chunk the lookup uses, `reader.rs:376-392`, `query.rs:248-256`). So a matched position is always `< timestamps.len()` and `at()` is always `Some` — even a forged/short timestamps chunk clamps `hi` to its own length. Reaching the branch needs a *mock* `IndexReader` (a trait abstraction we deliberately avoided), exercising provably-dead code. Documented the unreachability invariant at the guard instead. | `page.rs:104-128`, `reader.rs:376-392` | Closed (no test); guard kept as defense + commented. |
| C-7 ✅ | `otel-ledger` built the wire `columns` object in `index` order but didn't enable `serde_json`'s `preserve_order`, so `serde_json::Map` (a `BTreeMap`) serialized the keys **alphabetically**. **Verified not a live bug for the cloud UI:** `cloud-frontend` orders columns and maps row cells by each column's `index` field, not by JSON key order (`functions/useFetch/normalizers/table/index.js:279-281` + ~220-235). In the shipped `otel-plugin` binary `preserve_order` was also already on by accident (a sibling dep, `otel-ingestor`, enables it; Cargo unifies features), so insertion order was preserved regardless. | `otel-ledger/Cargo.toml`; cf. `journal-function/.../columns.rs` | **Done.** Enabled `serde_json/preserve_order` on `otel-ledger` explicitly — consistency + defense (don't rely on accidental feature unification; covers any key-order consumer such as the agent's local UI that `journal-function` guards), not a cloud-UI fix. |
| C-6 ⛔ | A claimed "stale anchor → silent row drop + optimistic `has_more` → empty load-more" bug. **Closed — not a bug** (verified, and confirmed by a 5-model consult, including the model that first raised it). The anchor is an exclusive *comparison boundary*, never materialized; every page row comes from a source evaluated in the same call, so `materialize` always finds it; `has_more` is a plain candidate count immune to a stale anchor. The only real cross-request effect is the **already-documented WAL→SFST cursor seam** (a boundary row can duplicate on backward paging, or skip, when rows share a timestamp and a WAL seals between requests) — see `wal-query-design.md`; deferred (M5 mismatch metric). Left a clarifying note in `paginate`'s doc so it isn't re-filed. | `page.rs` (`paginate`/`materialize`/`finalize_page`) | Closed (no code change); documented. |
| C-5 ✅ | **Multiple `WalTail`s sharing one `file_seq` collided in `materialize`.** The handler's old `fallback` closure emitted one `WalTail` per *failed* chunk build, all carrying `wal.seq`. Two such tails (a) collided in `tail_by_seq: HashMap<u64, &WalScan>` (last-write-wins) and (b) emitted cursors `(file_seq, Tail, position)` with overlapping per-scan insertion indices — silent row loss/corruption on the degraded path. Pre-existing (predated M-4). | `handler.rs:resolve_wal`, `page.rs:278` | **Done.** Dropped the failed-chunk→tail fallback: a chunk that fails to build/parse now makes the **whole WAL** un-queryable for that query (returns no candidates/tails; data returns via the sealed SFST after rotation, or next query for a transient failure since build errors aren't cached). This guarantees **≤1 tail per `file_seq`** (the trailing un-chunked suffix), which the cursor's `(file_seq, Part::Tail)` routing assumes — dissolving the collision by construction. Chose this (option ii) over a multi-range tail (option C) per user decision; keeps active-WAL tail row-scanning. |

### Readability / maintainability

| ID | Item | Evidence | Notes |
|----|------|----------|-------|
| M-1 ✅ | `paginate` was ~130 lines doing six jobs (partition, tail scan, SFST sort, open+evaluate+early-terminate, finalize, cold-release). Extracted named helpers so the body reads as the documented map→reduce→root→fetch pipeline. | `page.rs` | **Done.** Extracted `partition_sources`, `scan_tails`, `map_sfsts`, `open_and_evaluate_sfsts`, `build_page`, `release_cold`; `paginate` is now a ~12-line recipe. The SFST loop *was* extractable after all — the `IndexReader`-borrows-`Mapped` constraint is handled by passing `mappings` by ref + a lifetime param on the helper. Pure structure, zero behavior change. |
| M-2 ✅ | `paginate` folded with `PageShard::merge(vec![merged, shard], …)` once per source, allocating a 2-element `Vec` each time. Added `PageShard::merge_into(&mut self, other, …)` and fold in place. **Verified faithful**: `PageShard` is exactly `{cursors, has_opposite}`; `merge` = extend + OR + `order_by_closeness` + truncate, which `merge_into` replicates. | `page.rs` | **Done.** Readability win + tiny alloc saving. Does **not** reduce sort count (`merge_into` still re-orders per fold, like before); sort-once is a separate change. Parity test added; `merge` kept (all-at-once reduce, still tested). |
| M-3 ✅ | The step-1 per-source dispatch (`match` on `LogSource` → `LogsShard::evaluate` vs `WalScan::scan_range().evaluate()`, with per-arm error handling) lived at the call site. Moved it onto `LogSource::to_shard(&self, &LogsQuery) -> LogsShard`. | `engine.rs` | **Done.** Co-locates the step-1 *dispatch* + the tail scan-failure handling (not the SFST internal degrade, which stays in `LogsShard::evaluate`). Tail-failure now degrades to an empty shard (monoid identity) instead of skipping — behavior-equivalent under `merge`; added `merge_ignores_interspersed_default_shards` to prove it. Named `to_shard` (not `into_shard` — takes `&self`). Multi-LLM consult confirmed sound + advised against unifying step-1/step-2 dispatch and against a trait (over-engineering for 2 variants). |
| **M-4** ✅ | **The cursor discriminator (headline, load-bearing).** `sub_id: u32` was overloaded `0`=sealed / chunk-index / `u32::MAX`=tail; `position` meant "chronological index in an SFST" *or* "insertion index in a tail"; routing used anonymous tuple keys `(u64,u32)` and `(u64,u32,u32)`. Replaced with a typed `Part` enum centralizing the magic mapping in one documented wire conversion, plus named key types; `seq`→`file_seq`. | `cursor.rs`, `page.rs`, `engine.rs` | **Done — see "§ M-4 — agreed design" below.** Shipped as M-4a (`9addc40ab4`), M-4b (`373e2c32cc`), M-4c. Reading + multi-LLM consult (glm-5.1, minimax-m3-coder, deepseek-v4-pro:max) drove the design. Rejected: 3-way `{Sealed,Chunk,Tail}` enum (sealed/chunk-0 collide on the wire → not faithfully decodable); `Indexed(NonZeroU32)` (can't hold 0-based chunk 0 without a +1 wire shift = wire break); `chunk_id` name; `Option<NonZero>`. **Skipped** the planned `Cursor::sealed/chunk/tail` constructors — `otel-ledger` builds `SfstCandidate` (not `Cursor`), so they'd be unused; sites write `Part::Indexed(..)` directly. |
| M-5 ✅ | `NS_PER_S` was defined in two places. Centralized in `cursor.rs` (`pub(super)`), used by `page.rs` + `merge/tests.rs`. | `cursor.rs` | **Done.** |
| M-6 ✅ | `from_cursors` requires its input already sorted ascending, previously stated only in prose. Added `debug_assert!(ascending.is_sorted())` so a future caller that forgets fails loudly. | `page.rs` | **Done.** |
| M-7 ✅ | `bound` (= `limit + 1`) in `paginate` renamed to `page_bound` with a clarifying comment. Helper params stay generic `bound`. | `page.rs` | **Done.** |
| M-8 ✅ | `build_table` did column-schema building and per-row cell building (incl. the multi-value join + last-`severity_text` rule) in one body, with the per-row logic trapped in a `.map()` closure — untestable in isolation. | `otel-ledger/.../adapter.rs` | **Done.** Extracted pure helpers `build_columns`, `group_row_fields`, `build_row_cells`; `build_table` is now a thin orchestrator (signature unchanged). Output byte-identical. Added unit tests on the helpers. A 6-model consult confirmed this over a dedicated table type / module / crate (over-engineering: positional rows, one caller, no invariant). |
| M-9 ✅ | Doc nits: a note that `LogSource::Sfst`'s name tracks the inner type, not storage; a note in `handler.rs` that caller source-order is cosmetic (the engine re-partitions). | `engine.rs`, `handler.rs` | **Done.** Both comments added. |

### § M-4 — agreed design (decided)

Decided after a full read of `cursor.rs`/`page.rs` and a 3-model consult
(`/tmp/vk-consult-m4-cursor/*.md`). The wire string the consumer round-trips
(`Cursor::encode`/`decode`, `otel-ledger/.../adapter.rs:85,422`) stays
**byte-identical**; `wal_equivalence.rs` gates every step. Decisions:

1. **Discriminator type.** `Cursor.sub_id: u32` → `part: Part`, a 2-variant
   enum `enum Part { Indexed(u32), Tail }`. `Indexed(n)` covers a sealed SFST
   (`n == 0`) *and* an in-memory chunk (`n` = chunk index); the sealed-vs-chunk
   distinction is construction-site knowledge the wire cannot carry and the
   engine never needs (the only runtime branch is tail-vs-not, `page.rs:255`).
   Plain `u32`, **not** `NonZeroU32` (chunk indices are 0-based — `NonZeroU32`
   would force a +1 wire shift).
2. **Wire conversion.** `Part::to_wire()/from_wire()` own the single mapping
   (`Indexed(n) ⇄ n`, `Tail ⇄ u32::MAX`); `encode`/`decode` stay thin wrappers
   on `Cursor` in `cursor.rs` (no separate codec module). Drop the
   `SFST_SUB_ID`/`TAIL_SUB_ID` public constants.
3. **Ordering.** Keep **derived** `Ord` — with `Indexed` declared before `Tail`,
   the derived order reproduces today's `0 < … < u32::MAX` exactly. Add a
   load-bearing variant-order doc comment + extend `cursor/tests.rs` to assert
   `Indexed(0) < Indexed(MAX-1) < Tail`. (Hand-written `cmp` rejected: more
   error-prone for no gain given the test + comment.)
4. **`position`.** Stays `u32` + doc comment. Nothing interprets it
   cross-source; the enum already prevents source misrouting. (Newtypes
   `InChunk`/`InTail` rejected as over-modeling.)
5. **Construction + visibility.** Add `Cursor::sealed()/chunk()/tail()`
   constructors so `otel-ledger` never builds a `Part` directly;
   `Anchor::Timestamp` uses `Part::Tail` with the "synthetic max sorts last"
   coupling documented (`query.rs:219-224`). **Keep `Cursor` fields `pub`** —
   privatizing + accessors is a larger cross-crate read-site break, deferred as
   a separate item (adjacent to C-2).
   - **Resolved (follow-up).** A 3-model consult (`/tmp/vk-consult-cursor-api`)
     converged that this is polish — M-4 was the win. Landed **only**
     `Cursor::synthetic_max(ts)`: the `Anchor::Timestamp` site was the one
     construction with real semantic oddness (three `MAX`/`Tail` sentinels +
     a load-bearing comment), now a named `pub(super)` constructor in
     `cursor.rs` that centralizes the coupling. **Skipped** field privatization
     (`Cursor` is a `Copy` value type with no external constructor — privatizing
     while exposing `Ord`/`Eq` is incoherent, ~nil safety gain), the
     `tail`/`indexed`/`sealed`/`chunk` constructors (1 call site each, no info
     added), `is_tail()`, a wire-codec split, and `position` newtypes. Kept the
     `Indexed(u32::MAX)` guard as a `debug_assert!` for consistency with C-5.
6. **Naming + keys.** Rename `SfstCandidate.seq` / `WalTail.seq` → `file_seq`
   (match `Cursor.file_seq`, the doc-referenced name). Replace the anonymous
   `(u64,u32)` / `(u64,u32,u32)` routing tuples in `materialize` with named
   `SourceKey { file_seq, part }` / `RowKey { file_seq, part, position }`.

**Commit plan (each gated by `wal_equivalence` + clippy) — all landed:**
- **M-4a** (`9addc40ab4`) — `Part` enum + wire codec + derived-`Ord`
  comment/test + `Anchor::Timestamp` fix + migrate the construction sites
  and `SfstCandidate.sub_id` → `part`. (Constructors skipped — see above.)
- **M-4b** (`373e2c32cc`) — `SourceKey`/`RowKey` named routing structs in
  `materialize`.
- **M-4c** — `seq` → `file_seq` rename on `SfstCandidate`/`WalTail`.

### Performance (deferred / needs measurement)

| ID | Item | Evidence | Notes |
|----|------|----------|-------|
| P-1 | **WAL tail double-scan.** Each tail is decoded once in step 1 (`evaluate`) and again in step 2 (`page_shard`). The "re-open is deliberate" comment applies to SFST mmap, not to tail frame-decoding. Scan once, hand both the stats shard and the page scan to `paginate`. | `engine.rs:158`, `page.rs:303` | Known / deferred (M5). **Keep step 1 and step 2 separately computable** (distributed fan-out) — i.e. plumb a pre-scanned tail through, do *not* fuse the two evaluators. |
| P-2 | `WalScan::materialize_rows` re-stringifies every pair per row (`to_string()` ×2). Bounded by page `limit`. Consider `Cow` / pre-split storage. | `wal_scan.rs:222-224` | Bounded; measure before acting. |
| P-3 | `WalScan::evaluate` clones `fields`. **Not wasted** — the compile-error path returns it in the degraded shard, so `fields` is needed on both paths and moving the clone after compile would break the error path. The only real win is `Arc<FieldTable>` to avoid the clone outright. | `wal_scan.rs:240,249-254` | Reclassified: not a free move; needs `Arc`. |
| P-4 | `limit == 0` still opens every SFST. **Cannot blindly skip** SFST evaluation: `has_newer`/`has_older` derive from `has_opposite` across all sources, so a zero-row query still needs source evaluation for exact has-more flags. Only row *materialization* is skippable. | `page.rs:363-379`, `page.rs:187-189` | Rare edge case, and not a clean skip. |
| P-5 | Per-row distinct-token dedup uses `Vec::contains` (O(n²) in tokens). A `TokenSet` bitset already exists. | `wal_scan.rs:285-290` | Speculative without a bench. |

### Explicit non-issues (recorded so they aren't re-chased)

- ~~The two `From<…> for LogSource` impls are kept as an idiomatic
  affordance.~~ **Reversed** — they were unused dead code (every `LogSource`
  is built via the `Sfst`/`Tail` variants, e.g. `.map(LogSource::Sfst)`);
  dropped after the overall-change consult flagged them.
- `paginate`'s split allocates two small `Vec`s of references — O(n), n≈tens.
- `merge.rs` `BTreeMap` for the cross-file field union is intentional.
- `materialize`'s `HashMap` routing is the faster choice at typical page
  sizes; linear-search alternatives are micro-opt territory.

---

## Recommended iterative order

Each step lands as its own commit, gated by `wal_equivalence` + clippy.

1. **C-1** — fail-fast on the `at` lookup (robustness; consistent with M4b). Small.
2. **M-2** — `merge_into` (leaf, `page.rs`-local, zero risk).
3. **M-1** — extract `paginate` helpers (the consensus structural win).
4. **Cleanup batch** — M-5 (`NS_PER_S`), M-6 (`debug_assert`), M-7 (rename `bound`).
5. **M-3** — `into_shard` on `LogSource`.
6. **M-4** — `CursorPart` + `position` + named keys. The headline. Do **only after** reading `cursor.rs`/`page.rs` *and* explicitly deciding the cursor wire-compat story and the `sub_id` replacement shape.
7. **P-1** — WAL double-scan (perf; around or after the cursor pass). Preserve separate step-1/step-2 products for fan-out.
8. **C-2** — `u64`/`usize` count type, on its own: a coordinated engine+wire decision, not batched with trivial cleanups.
9. Remaining perf (P-2, P-5) only with a benchmark; P-3/P-4 and the nits as encountered.
10. **C-3, C-4** — low-priority follow-ups (sfst `materialize_rows` robustness; `CorruptIndex` negative-path test).

Rationale: correctness first, then the cheap local readability wins, then the
load-bearing structural refactor once the spine is understood, then perf.
