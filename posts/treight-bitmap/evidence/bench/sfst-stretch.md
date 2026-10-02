# Stretch: a real `.sfst` built from the certstream corpus through the production pipeline

Outcome: **delivered and PASSED** (2026-10-02). This is not a simulation of
the index — it IS the index `otel-ledger` would seal from this corpus.

## What was run

`treight-bench`'s `treight-sfst` bin (src/crates/treight-bench/src/bin/
treight-sfst.rs) replays data/corpus/certstream-250k.jsonl.gz through the
production functions only:

1. `Source::event_to_log_record` + `build_export_request` (batched 100/request),
2. `ng_ingest::write_request` — the ng-ingest entry: `ng_flatten::
   prepare_log_frame` → bincode → `wal::Writer` frame (`payload_format` 1),
3. `ng_index::build_sfst_file` — the otel-ledger seal entry:
   `populate_row_index` (intern + `RowIndex::row`) → `IndexWriter`.

Artifacts (local, gitignored): a 143,436,616-byte WAL and a
**55,443,918-byte `index.sfst`** over 250,000 records (build ~5.9 s).

## Verification (why the derived posting lists are THE real ones)

Checked with `sfst::IndexReader` against the derived cache
(sfst-stretch.json carries the machine-readable outcomes):

| check | result |
|---|---|
| universe: reader `total_logs()` vs derived rows | 250,000 == 250,000 ✓ |
| field table: 47 fields, per-field cardinality + tier | 0 mismatches ✓ |
| primary FST: every low-tier `key=value` entry's treight blob bytes + cardinality | **273/273 exact matches** ✓ |
| PRIM treight blob total: reader vs derived mirror | 694,392 B == 694,392 B ✓ |

The primary-FST check is the load-bearing one: the reader's `BitmapValue`
IS `remap_one_bitmap`'s output for that posting list (time-order remap +
>50%-density complement + `from_sorted_iter`), produced by the production
code path from the real WAL. Byte equality with the derived mirror pins
the whole chain — flatten → `key=value` rendering → interning → row
accumulation → time-order permutation → encode.

## Index composition (context for the blog)

- The `.sfst` totals 55.4 MB for 250K records (~226 B/record all-in:
  per-row stream batches, high-card string arenas, per-row columns, FSTs).
- The treight bitmaps the index materializes (low+mid tiers) total
  **1,792,162 B ≈ 1.75 MB — 3.2% of the file** (totals-certstream-250k).
  A roaring bitmap per value instead of treight for the same lists would
  take 3,514,248 B serialized (and 4,184,992 B heap while live).
- High-cardinality fields (2.44M lists here) carry NO per-value bitmap in
  the format at all — batch masks + the stream batches cover them — which
  is why the per-value bitmap comparison is scoped to low+mid in totals.

## Why not .sfst builds for all three corpora

One full-pipeline build suffices for the derivation-faithfulness claim
(the check is per-bitmap byte equality, not statistics). The footprint
totals (totals-*.json) already cover all three corpora exactly; repeating
the 30 s WAL+seal per corpus would add no verification power. The bin
accepts any corpus, so re-running for jetstream/mixed is a one-liner
(see environment.txt).
