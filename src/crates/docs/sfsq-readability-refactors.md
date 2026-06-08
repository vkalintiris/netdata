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
| C-2 | `matched` is `u64` in `LogsShard` but `usize` in `LogsData`, bridged by `as usize`. Truncates on 32-bit (theoretical for a 64-bit server). | `aggregate.rs:30`, `result.rs:19`, `engine.rs:190` | **API-facing — not trivial cleanup.** `LogsData::matched` is public and the wire `Items` uses `usize`; `u64` is the cleaner long-term type, but treat it as a coordinated engine+wire decision. Do **not** batch with trivial cleanups. |
| C-3 | `sfst::IndexReader::materialize_rows` silently drops positions on inconsistency (out-of-range / missing batch / over-local `continue`s) and zeroes the timestamp via `at(pos).unwrap_or(0)`. The sfst-crate analog of the M4b `WalScan::materialize_rows` fix. **Lower priority than C-1**: it's the *fetch* path (wrong displayed value, not corrupted ordering), the `unwrap_or(0)` is already guarded by the `pos >= total` check, and for any selected position `PageShard::evaluate` (post-C-1) already validated its timestamp — so it's effectively unreachable in the query path. Fixing properly means `materialize_rows -> Result` threaded through `page.rs` materialize (which collapses the whole page on failure). | `reader.rs:261-273` | Low priority; robustness, not a live bug. |
| C-4 | No negative-path test for C-1's `CorruptIndex` skip. Forge a corrupt SFST (mismatched matched-positions vs timestamps chunk), assert `PageShard::evaluate` errors and `paginate` skips the source while other sources still contribute. Needs test infra (forged file or a mock reader). | `page.rs:97-113` | Follow-up to C-1; non-trivial test infra. |

### Readability / maintainability

| ID | Item | Evidence | Notes |
|----|------|----------|-------|
| M-1 ✅ | `paginate` was ~130 lines doing six jobs (partition, tail scan, SFST sort, open+evaluate+early-terminate, finalize, cold-release). Extracted named helpers so the body reads as the documented map→reduce→root→fetch pipeline. | `page.rs` | **Done.** Extracted `partition_sources`, `scan_tails`, `map_sfsts`, `open_and_evaluate_sfsts`, `build_page`, `release_cold`; `paginate` is now a ~12-line recipe. The SFST loop *was* extractable after all — the `IndexReader`-borrows-`Mapped` constraint is handled by passing `mappings` by ref + a lifetime param on the helper. Pure structure, zero behavior change. |
| M-2 ✅ | `paginate` folded with `PageShard::merge(vec![merged, shard], …)` once per source, allocating a 2-element `Vec` each time. Added `PageShard::merge_into(&mut self, other, …)` and fold in place. **Verified faithful**: `PageShard` is exactly `{cursors, has_opposite}`; `merge` = extend + OR + `order_by_closeness` + truncate, which `merge_into` replicates. | `page.rs` | **Done.** Readability win + tiny alloc saving. Does **not** reduce sort count (`merge_into` still re-orders per fold, like before); sort-once is a separate change. Parity test added; `merge` kept (all-at-once reduce, still tested). |
| M-3 | The step-1 per-source dispatch (`match` on `LogSource` → `LogsShard::evaluate` vs `WalScan::scan_range().evaluate()`, with per-arm error handling) lives at the call site. Move it onto `LogSource` (e.g. `fn into_shard(&self, &LogsQuery) -> LogsShard`). The existing `match`es are already exhaustive (a new variant already fails to compile), so the real benefit is **centralizing the per-source error/degrade policy** rather than duplicating it across call sites. | `engine.rs:154-168` | Natural extension of the `LogSource` refactor (`c595973479`). |
| **M-4** | **The cursor discriminator (headline, load-bearing).** `sub_id: u32` is overloaded `0`=sealed / chunk-index / `u32::MAX`=tail; `position` means "chronological index in an SFST" *or* "insertion index in a tail"; routing uses anonymous tuple keys `(u64,u32)` and `(u64,u32,u32)`. Replace with a `CursorPart` enum (Sealed / Chunk(n) / Tail) that centralizes the magic mapping and the sealed-xor-active invariant in one documented wire conversion, plus a named key type instead of the tuples. Settle the `seq` vs `Cursor.file_seq` naming split here too. | `cursor.rs:33-36`, `cursor.rs:21`, `page.rs:211,219` | **Load-bearing — do after fully reading `cursor.rs`/`page.rs`.** Rejected alternatives: `chunk_id` (wrong for sealed/tail), `Option<NonZero>` (chunk indices are 0-based). Tracked in memory `deferred-readability-refactors`. |
| M-5 | `NS_PER_S` defined in more than one place. Centralize and re-export. | `page.rs:28`, `merge/tests.rs` | Trivial hygiene. |
| M-6 | `from_cursors` requires its input already sorted ascending, stated only in prose. Add `debug_assert!(ascending.is_sorted())` so a future caller that forgets fails loudly. | `page.rs:114` | One-liner; same "fail fast" spirit as M4b. |
| M-7 | `bound` (= `limit + 1`, the page-candidate bound) is an ambiguous name. Rename `page_bound` / `candidate_bound`. | `page.rs:~293` | Trivial. |
| M-8 | `build_table` computes a field→values map, then reads it twice (per-column cell + the dedicated `severity_text` cell). Split into named helpers for testability. **Adjacent — `otel-ledger`, not `sfsq`.** | `otel-ledger/.../adapter.rs:388-425` | Minor; out of the `sfsq` focus. |
| M-9 | Doc nits: a one-line note that `LogSource::Sfst`'s name tracks the inner type, not storage; a note in `handler.rs` that caller source-order is cosmetic (the engine re-partitions). | `engine.rs:107-112`, `handler.rs:~220` | Skip unless touching those lines. |

### Performance (deferred / needs measurement)

| ID | Item | Evidence | Notes |
|----|------|----------|-------|
| P-1 | **WAL tail double-scan.** Each tail is decoded once in step 1 (`evaluate`) and again in step 2 (`page_shard`). The "re-open is deliberate" comment applies to SFST mmap, not to tail frame-decoding. Scan once, hand both the stats shard and the page scan to `paginate`. | `engine.rs:158`, `page.rs:303` | Known / deferred (M5). **Keep step 1 and step 2 separately computable** (distributed fan-out) — i.e. plumb a pre-scanned tail through, do *not* fuse the two evaluators. |
| P-2 | `WalScan::materialize_rows` re-stringifies every pair per row (`to_string()` ×2). Bounded by page `limit`. Consider `Cow` / pre-split storage. | `wal_scan.rs:222-224` | Bounded; measure before acting. |
| P-3 | `WalScan::evaluate` clones `fields`. **Not wasted** — the compile-error path returns it in the degraded shard, so `fields` is needed on both paths and moving the clone after compile would break the error path. The only real win is `Arc<FieldTable>` to avoid the clone outright. | `wal_scan.rs:240,249-254` | Reclassified: not a free move; needs `Arc`. |
| P-4 | `limit == 0` still opens every SFST. **Cannot blindly skip** SFST evaluation: `has_newer`/`has_older` derive from `has_opposite` across all sources, so a zero-row query still needs source evaluation for exact has-more flags. Only row *materialization* is skippable. | `page.rs:363-379`, `page.rs:187-189` | Rare edge case, and not a clean skip. |
| P-5 | Per-row distinct-token dedup uses `Vec::contains` (O(n²) in tokens). A `TokenSet` bitset already exists. | `wal_scan.rs:285-290` | Speculative without a bench. |

### Explicit non-issues (recorded so they aren't re-chased)

- The two `From<…> for LogSource` impls are currently unused but kept as an
  idiomatic affordance (decision made).
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
