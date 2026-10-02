# TRACED: the production path from OTLP LogRecord to indexed field=value pairs

Verified 2026-10-02 by bench-eng against the working tree (branch `bp`).
This is the path the blog's "realistic posting lists" claim rests on; the
derivation tool (`src/crates/treight-bench`) CALLS the production functions
for every stage that shapes a posting list, and mirrors only the trivial
final accumulation (semantics proved identical below).

## The pipeline, end to end

1. **Event → OTLP LogRecord** — `src/crates/otel-streams/src/lib.rs:21`
   `trait Source::event_to_log_record(event, raw_json)`:
   - certstream: `certstream.rs:117` — body = the FULL raw WS message as an
     OTLP kvlist (explodes into `body.*`), `event_name=certificate_update`,
     `severity_number=9`/`severity_text=INFO`, `time_unix_nano = seen * 1e9`.
   - jetstream: `jetstream.rs:191` — promoted keys become
     `attributes.bluesky.*` (did, event.kind, commit.{operation,collection,
     rkey,rev,cid}, identity.handle, account.{active,status}); body = raw
     event minus promoted keys (explodes into `body.*`);
     `time_unix_nano = time_us * 1000`.
   The corpus files persist exactly the `(event, raw_json)` inputs this
   function consumes (lossless: the conversion reads nothing else).

2. **Batching** — `otel-streams/src/runner.rs:42` maps every event through
   `event_to_log_record`; `src/crates/otel-streams/src/otel.rs:70
   build_export_request(records, SERVICE_NAME, None, SCOPE_NAME,
   SCOPE_VERSION)` groups ≤100 records/request (production default,
   `args.rs`) with resource `service.name=<source>` (no namespace) and the
   source's scope name/version. Batch boundaries do not affect posting-list
   contents (rows and interning are order-global) — mirrored anyway.

3. **Normalize + flatten** — `src/crates/ng-flatten/src/logs.rs:411
   prepare_log_frame` (the single recipe shared by `ng-ingest` and the
   production otel-ingestor logs service):
   - `normalize_log_request` (logs.rs:206): resolve `time_unix_nano`
     (event time → observed time → synthesized fallback; both sources
     always set event time, so rows are deterministic), clear malformed
     trace/span ids, rewrite JSON-object STRING bodies into kvlists (never
     fires here: bodies arrive as kvlists already).
   - `flatten_log_request` → `Flattener::flatten_resource` /
     `flatten_scope` / `flatten_record` (`ng-flatten/src/common.rs:489/502/
     532`): one entry per typed leaf — `severity_number`, `severity_text`,
     `event_name`, `body.*` (nested objects as `.`, arrays collapsed to
     `[]`), `attributes.*`, `resource.attributes.*`, `scope.name/version/
     attributes.*`. `trace_id`/`span_id`/`flags`/timestamps are per-row
     columns, NOT indexed facets.
   - bincode → WAL frame (`LOG_FRAME_PAYLOAD_FORMAT = 1`, logs.rs:45).

4. **Seal-time intern + rows** — `src/crates/ng-index/src/sfst_build.rs:172
   populate_row_index` (called by `otel-ledger` via `build_sfst_file`):
   decodes each frame, renders every entry's `key=value` with
   `ng_flatten::build_kv` (canonical rendering: strings raw, ints decimal,
   doubles decimal, bools true/false, bytes lowercase hex, Null empty —
   common.rs:830), interns by full string (xxhash64 fast path with string
   re-check — identical identity to a `HashMap<kv-string>`), and feeds
   `sfst::RowIndex::row(ts, resource++scope++record tokens)`.

5. **Posting-list accumulation** — `src/crates/sfst/src/row_index.rs:234
   RowIndex::row`: the row lands at the next insertion-order position
   (0,1,2,… in arrival order) and each token's `RoaringBitmap::insert(pos)`
   fires. **THE POSTING LISTS**: one roaring bitmap per distinct
   `key=value` string, positions = arrival-order row indexes, universe =
   row count. This step is mirrored by `treight_bench::Derivation`
   (a `HashMap<kv, ascending positions>`); the mirror's identity matches the
   interner's by construction (both dedup the same full strings), and its
   positions match because rows are appended in the same order the frames
   are read.

6. **Encode at seal** — `src/crates/sfst/src/build.rs:614 remap_one_bitmap`
   (per posting list):
   - remap insertion→chronological order via `TimeOrder`
     (`row_index.rs:297 build_time_order`, stable sort of positions by the
     per-row timestamps);
   - encode route by cardinality: bitset (set + scan) at
     `card >= max(universe/64, 256)`, else collect+sort — identical output
     bytes either way;
   - **complement by density**: `card > universe/2` stores the ZEROS via
     `Bitmap::from_sorted_iter_complemented` (treight/src/bitmap.rs:85).
   Where the bitmaps land on disk depends on the field's cardinality tier
   (`sfst/src/kv_interner.rs:239-253`, threshold 100 —
   `CARDINALITY_THRESHOLD` in ng-index/sfst_build.rs:87):
   - **low** (<100 distinct values): primary FST (`PRIM`) — treight per value;
   - **mid** ([100,1000)): per-field FST chunks (`MF**`) — treight per value;
   - **high** (>=1000): **NO per-value bitmap at all** — `HighField`
     string arena + per-value u8 stream-batch mask (`build.rs:204
     build_high_card_chunks`, schema.rs `HighField`). Per-row values are
     recoverable from the stream batches.
   The footprint totals therefore report both scopes: `sfst_materialized`
   (low+mid, what the real index stores as bitmaps) and `all_lists` (the
   pure "bitmap per value" comparison).

## What the derivation mirrors vs calls

| stage | production | derivation |
|---|---|---|
| event→LogRecord | `Source::event_to_log_record` | **called** (otel-streams path dep) |
| batching/envelope | `build_export_request` | **called** |
| normalize+flatten | `prepare_log_frame` → bincode frame | **called**, incl. the `decode_log_frame` round-trip |
| kv rendering | `build_kv` + per-frame `tree.path` | **called** (same fns as `populate_row_index`) |
| interning | `KeyValueInterner` (hash fast path + string recheck) | mirrored: `HashMap<full string, slot>` — same identity |
| posting-list accumulation | `RowIndex::row` → `RoaringBitmap::insert(pos)` | mirrored: `Vec<u32>` per slot, same positions |
| time-order remap | `build_time_order` | mirrored (same stable sort), permutation cached |
| treight encode | `remap_one_bitmap` | mirrored bit-for-bit in the footprint/bench paths (complement >50%, ascending contract) |

Not mirrored (no posting-list observability): the WAL bytes, FST/tier
chunk layout, id translation, and per-row columns. The `.sfst` stretch
build (below, when run) re-enters the pipeline at step 4 for real
(`ng_index::build_sfst_file`) and cross-checks the mirror end-to-end.

## Known determinism caveats

- `observed_time_unix_nano` is wall-clock at conversion time — carried as a
  stored column only; it can never influence a posting list or the
  time-order remap (both key off the event-data `time_unix_nano`).
- jetstream bodies strip promoted keys before the kvlist explode —
  faithful to production (`strip_promoted_keys`, jetstream.rs:159).
- The mixed corpus interleaves two `service.name` streams in one file; the
  real `otel-ledger` would derive per-(namespace,name) streams → separate
  SFST files per source. The mixed derivation intentionally keeps ONE
  universe (the arrival- interleaved row space) so cross-source posting
  lists (e.g. `resource.attributes.service.name` at ~50% density) are
  measurable; the two single-source corpora cover the per-stream shape.

## For the record: concurrency during the 2026-10-02 per-corpus re-run window (~21:50–22:10)

Precise answer: **YES — concurrent CPU work was present, including mine.**

- During the certstream and jetstream benches (~21:50–22:00): this agent ran
  two full `cargo clippy --all-targets` compilation passes over the
  dependency tree (wal, sfst, journal-common, rustls, tokio, …) while the
  benches executed — visible in the session log at ~21:52–21:57.
- During the mixed bench (~22:00–22:10): this agent re-ran `treight-derive`
  and `treight-footprint` for certstream and jetstream (stats/totals
  regeneration) concurrently with the running bench.
- Teammate activity (diagram-eng chart scripting, writer/lead work) is
  likely in the same window per the Lead's relays, but cannot be verified
  from bench artifacts; the earlier 21:11 bench attempt also died mid-run
  in a period of heavy machine contention.
- Consequence: that pass is disclosed as SUITE VALIDATION (correctness,
  selection, and sanity asserts — all of which are timing-independent).
  The PUBLISHED numbers come from the exclusive pass recorded in
  environment.txt (loadavg-snapshotted, teammates frozen).
