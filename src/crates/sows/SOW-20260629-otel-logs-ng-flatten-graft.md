# SOW-20260629-otel-logs-ng-flatten-graft - Graft the ng-flatten typed format into the production OTel logs pipeline

## Status

Status: completed

Sub-state (2026-06-29): **Stages 0–2 DONE + 2 review rounds closed** (chain `b5953dbd6b`→`7f25b26c29`
→`ccb7219d7f`→`302d568514`→`152fa8cdda`→`ec16002670`→`1a5b0ac66e`→`f231594794`→`7fe55b5457`→`eeffbc64c8`→`dd1a75b1f0`).
**Stage 3 DONE + review+consult round closed**: deleted `wal-otap`+`otel-normalize`+flatten-otel `logs`
module, `arrow_bridge`, the OTAP harness, and `sfst_indexer::index*`; dropped the `KvSink` trait
(inherent methods); doc/spec staleness fixed. vk-review verdict **PRODUCTION GRADE**. content_meta
Option→mandatory cleanup **rejected** (keep the dormant hard-failure guard). Workspace builds + all
touched-crate tests green. Decisions: **D1–D4: A**, **D5: 1**, **D6: 1**, **D7: 1**, **normalize_ids: YES**,
review: **A yes / B 1 / C no / D i / E,F defer / G fix**; **D8 (Stage 3): KvSink fork = 3** (drop the
trait, delete `wal-otap`), **delete `otel-normalize` crate = yes**, **fold perf E = no (stay deferred)**.
Implementer = assistant direct (no GLM). SOW local/untracked. Both migration unknowns verified SOUND.

Implementer (user instruction 2026-06-29): **assistant implements all changes directly**;
no GLM/external-LLM code delegation (durable preference; external LLMs only for named
reviews). Stage 0 underway: blueprinting the equivalence harness.

Branch: `sjr-tree8`. HEAD at creation: `0b37f85b82`.

## Requirements

### Purpose

The production OTel **logs** pipeline (a pre-GA PoC, never shipped to end users) should
adopt the `ng-flatten` typed, array-collapsed flattening + SFST format v9 (the typed
schema tree + per-row columns delivered in `b5953dbd6b`/`0b37f85b82`). The new approach's
core ingest+index logic is sound; the goal is to make it the agent's actual logs path
without losing the production operational shell.

### User Request

- Investigate what updating the current production pipeline with the new `ng-*` approach
  would look like; the user expects "significant work, but mostly mechanical."
- This SOW is the **drafted migration plan** for sign-off, not yet approved for execution.

### Assistant Understanding

Facts (verified by investigation; file:line in `src/crates`):
- **Production logs path = the `otel-plugin` PLUGINSD external plugin** (Corrosion/CMake
  install to `plugins.d`), a supervisor with **Ingestor / Ledger / LegacyLogs** workers.
  - Ingestor: tonic gRPC (logs+metrics+traces), multi-tenant, TLS, stream-identity +
    `content_meta` + collision + oversize handling; flattens logs via
    `otel-ingestor/src/arrow_bridge.rs`, encodes **OTAP / Apache Arrow IPC**, writes the
    `NWAL` v4 WAL (`logs_service.rs:563` → `arrow_bridge::encode`).
  - Ledger: on WAL `Closed` → `sfst_indexer::index(wal, sfst)` (`indexer.rs:100`); on query
    builds an in-memory chunk via `sfst_indexer::index_range` (`handler.rs:334`) + scans the
    active-WAL tail via `sfsq` `WalScan`; serves the `otel-logs` netdata Function.
- **Production flatten is NOT a strawman:** `arrow_bridge` already preserves value types
  and collapses *scalar* arrays to multi-valued keys (`tags=a`,`tags=b`). It differs from
  `ng-flatten` in: WAL payload (Arrow vs bincode `FlattenedRequest`), array-of-structs
  (positional `endpoints.0.host` vs collapsed `endpoints[].host`), no persisted schema
  tree, no separated per-row columns, and frame-range-ts vs per-row-ts.
- **`ng-*` are standalone experiment binaries** (`ng-ingest`, `ng-index`) + the
  **`ng-flatten` library**; zero CMake/packaging; run via `run-ingest-v8.sh` + `otel-streams`.
  Not wired into the agent.
- **The storage + query substrate is shared:** both producers converge on
  `sfst_indexer::build_and_write` / `sfst::StreamWriter` and write **format v9**. `sfsq`
  and `otel-ledger` read SFST only through the producer-agnostic `IndexReader`
  (`reader.field_table()` derived from the typed tree, `reader.rs:123-129`). The query side
  is **already compatible with ng-*-produced SFSTs** (verified).
- **Both migration unknowns verified SOUND (focused investigation):**
  - *Tail-scan parity:* `WalScan` consumes only `(ts_ns, "key=value")` from its `KvSink`
    (`wal_scan.rs`), derives nothing from Arrow structure. ng-flatten frames carry
    everything it needs. Post-migration, the sealed builder and the tail scanner read the
    **same frame** through the **same `build_kv` renderer** and the **same frozen
    `Record.ts`** → parity becomes structural (stronger than today's two-decoder agreement).
  - *On-query `index_range`:* `build_into<W: Write+Seek>` (`fst_builder.rs:349`,
    `pub(super)`) already powers both file and in-memory builds; `wal::Reader::open_range`
    is payload-agnostic; ng-flatten frames are per-frame self-contained → the in-memory
    `(Summary, Vec<u8>)` contract reproduces by swapping `open` → `open_range`.

Inferences:
- The correct strategy is **graft, not replace**: keep `otel-plugin`/workers/IPC/ledger
  lifecycle/query path; swap only the logs flatten→encode→frame→index-feed→tail-scan chain
  to `ng-flatten`. Replacing wholesale would re-implement tenancy/TLS/identity/collision/
  lifecycle/metrics+traces that production already solved — high cost, no benefit.
- Because the WAL payload changes, ingest-encode + indexer-decode + tail-scan must flip
  **together** (they share the on-WAL frame contract).

Unknowns (none structural; resolve with the user, not by investigation):
- The four product/design forks D1–D4 below (array-of-structs semantics, WAL cutover
  strategy, migration scope, exploit-richness timing).

### Acceptance Criteria

- The production logs ingest path encodes `ng-flatten` `FlattenedRequest` frames (typed,
  `tags[]`/`endpoints[].host` collapse, per-row columns), preserving the existing
  tenancy / stream-identity / `content_meta` / collision / oversize / TLS behavior.
  Verification: ingestor unit/integration tests + a live `otel-streams` feed produces a
  queryable WAL→SFST.
- Both producer call sites (`indexer.rs:100`, `handler.rs:334`) build SFSTs via the
  ng-flatten path (seal-time file build + on-query in-memory `(Summary, Vec<u8>)`).
  Verification: ledger tests; on-query chunk feature still works.
- The active-WAL tail scanner decodes ng-flatten frames and yields results **identical**
  to the sealed SFST. Verification: a new ng equivalence harness (sibling of
  `sfsq/tests/wal_equivalence.rs`) is green across filter/facet/timeline/materialize.
- Metrics + traces ingestion is unaffected (logs-only graft). Verification: their paths
  compile and their tests pass unchanged.
- The dead Arrow **logs** encode/decode path is removed (clean end state). Verification:
  reference search shows no remaining logs consumers of `arrow_bridge::encode` /
  `wal_otap` logs decode / `sfst_indexer::index*`.
- build/clippy/tests green across all touched crates; workspace builds.

## Analysis

Sources checked:
- Production map: `otel-plugin/src/{main,supervisor}.rs`, `otel-ingestor/src/{lib,logs_service,
  arrow_bridge}.rs`, `otel-ledger/src/{indexer,ledger/rpc/handler}.rs`, `wal`, `wal-otap`.
- Query map: `sfsq/src/logs/{engine,wal_scan,aggregate,page}.rs`, `sfst/src/{reader,
  index_reader}.rs`.
- ng path: `ng-flatten/src/lib.rs`, `ng-index/src/sfst_build.rs`, `sfst-indexer/src/{lib,
  fst_builder,row_index}.rs`.
- Build/packaging: `CMakeLists.txt` (Corrosion), `src/crates/Cargo.toml`.
- Timestamp behavior: `ng-ingest/src/lib.rs:93-108`, `file-registry/src/clock.rs:21-29`,
  `wal-otap/src/decode.rs:236-243`, `otel-ingestor/src/logs_service.rs:85-114`.

Current state:
- See Assistant Understanding facts.

Risks:
- Producer-format flip is atomic across 3 read points (ingest/index/tail) — staging must
  account for the coupling (see Plan).
- Tail-scan semantic parity is correctness-critical — gated by a new equivalence harness.
- Metrics+traces share `otel-ingestor`/`arrow_bridge` module space — the logs-encode
  removal must be surgically logs-scoped.

## Pre-Implementation Gate

Status: ready

Problem / root-cause model:
- The production logs pipeline persists field identity as untyped `key=value` strings with
  positional array indices and no schema tree; the `ng-flatten` approach (now SFST v9)
  persists a typed, array-collapsed tree + per-row columns. The two diverge only at the
  **producer/format layer**; the storage + query substrate is already shared and v9-
  compatible. Closing the gap = grafting the ng-flatten format into the existing workers.

Evidence reviewed:
- All citations under Assistant Understanding (file:line in `src/crates`).
- Two focused parity verifications (tail-scan; `index_range` in-memory contract) — both SOUND.

Affected contracts and surfaces:
- **Ingest:** `otel-ingestor/src/logs_service.rs:563` (encode site), `arrow_bridge.rs`
  (logs encode functions).
- **Index feed:** `otel-ledger/src/indexer.rs:100` (`sfst_indexer::index`),
  `otel-ledger/src/ledger/rpc/handler.rs:334` (`sfst_indexer::index_range`).
- **Tail scan:** `sfsq/src/logs/wal_scan.rs:111,121` (`wal_otap::decode_file/decode_range`).
- **Builder:** `ng-index/src/sfst_build.rs:103-221` (target); `sfst-indexer/src/fst_builder.rs:349`
  (`build_into`, widen to `pub`); `sfst-indexer/src/lib.rs:67,76,116` (`index/index_with_options/
  index_range` — removable once ng owns both builds).
- **WAL frame format** (logs payload: Arrow → bincode); the `wal` container is unchanged.
- **Validation:** `sfsq/tests/wal_equivalence.rs` (template for the new ng harness).
- **Unaffected (must stay so):** metrics + traces services; `otel-plugin` supervisor/IPC;
  ledger registry/retention/upload; `sfst`/`sfsq` reader+query; tenancy/identity/collision.

Clean-end-state target:
- The production OTel **logs** path runs end-to-end on `ng-flatten`: ingestor encodes
  ng-flatten frames; the WAL carries them; seal-time + on-query SFST builds and the
  active-WAL tail scan all decode the **same** ng-flatten frame through the **same**
  `build_kv` renderer; the SFST is v9 with the typed tree + per-row columns; `sfsq`/
  `otel-ledger` query it unchanged.
- Removed as redundant (i):
  - `arrow_bridge` **logs** encode (`encode` + `encode_logs_otap_batch` logs usage) — the
    logs WAL payload is now ng-flatten bincode.
  - `wal-otap` **logs** OTAP decode as consumed by the indexer + tail scan
    (`wal_scan.rs:111/121`, `sfst-indexer/lib.rs:84/123`).
  - `sfst_indexer::index` / `index_with_options` / `index_range` (`lib.rs:67/76/116`) once
    `ng-index` owns both the file and in-memory builds.
- Excluded coupled items (ii):
  - **Metrics + traces ingest/encode** — different signals on the same gRPC server; out of
    scope (logs-only graft, D3). `arrow_bridge`/`wal-otap` code paths they rely on stay;
    only the logs-specific functions are removed. Scope source: user request ("logs"
    experiment lineage) + the metrics path uses `flatten-otel`, not `arrow_bridge` logs.
  - **Exploiting** the new typed/columnar richness in queries (typed value matching,
    per-row trace/span columns) — additive; deferred to a follow-on SOW (D4). Scope source:
    this SOW *persists/migrates* the format; *using* it is separate.
  - **`ng-ingest` / `ng-index` binaries** — stay as benchmark harnesses; their *logic*
    moves into the workers via the `ng-flatten` library. Scope source: they have no agent
    wiring and serve the perf baseline.
- Reference search (run 2026-06-29, recorded):
  - `sfst_indexer::index*` call sites: `otel-ledger/src/indexer.rs:100`,
    `otel-ledger/src/ledger/rpc/handler.rs:334`; tests in `sfsq/tests/wal_equivalence.rs`
    (:317/:786/:793/:834/:840/:843). `traces_indexer.rs:9` is a doc comment (traces, excluded).
  - `wal_otap` decode consumers: `sfsq/src/logs/wal_scan.rs:111,121`,
    `sfst-indexer/src/lib.rs:84,123`. (All logs path; mapped to (i).)
  - Logs encode: `otel-ingestor/src/logs_service.rs:563` → `arrow_bridge::encode`
    (`arrow_bridge.rs:102` `encode_logs_otap_batch`).
  - `build_into`: `sfst-indexer/src/fst_builder.rs:349` (`pub(super)` → widen to `pub`).
  - Every surviving reference is mapped to (i) or (ii). No straggler outside this set.

Existing patterns to reuse:
- `ng-index/src/sfst_build.rs` (the typed-tree + per-row-column build, incl. the
  `sfst_and_ng_flatten_path_renderers_agree` test).
- `build_into<W: Write+Seek>` (sink-generic file + in-memory build, already shared).
- `wal::Reader::open_range` + `ng_flatten::decode_frame` (per-frame self-contained).
- `sfsq` `ScanSink` (reusable almost verbatim by an ng-flatten tail scanner).
- `sfsq/tests/wal_equivalence.rs` (template for the ng equivalence harness).

Risk and blast radius:
- **Low-to-moderate, low architectural risk.** The substrate + query layer are shared and
  already v9-compatible; tail parity is structural by construction. Risk is concentrated in
  (a) carrying the ingest operational logic across the encode swap and (b) the equivalence
  harness proving parity. Format break is acceptable (pre-GA, no users; per memory
  `ng-replaces-otel-poc-breaking-ok`).
- Atomic coupling: ingest-encode + indexer-decode + tail-scan share the WAL payload
  contract and flip together; a WAL cutover strategy is needed (D2).

Sensitive data handling plan:
- Only code identifiers, file:line, schema field names, synthetic values in this SOW and
  commits. Live feeds use public certstream/jetstream + synthetic producers; report
  aggregate counts only. No secrets/PII.

Implementation plan:
- See `## Plan` (staged). Finalized only after D1–D4 are decided.

Validation plan:
- New ng equivalence harness (sibling of `wal_equivalence.rs`): ng-flatten frames →
  ng tail scanner vs ng sealed build; assert identical filter/facet/timeline/materialize.
- Ingestor tests preserving tenancy/identity/collision/oversize around the new encode.
- Ledger on-query in-memory chunk build test (range parity).
- Live `otel-streams` feed → WAL → SFST → `otel-logs` Function round-trip.
- Reference search post-removal; build/clippy/tests across touched crates; external review
  (vk-review + codex) at the milestone (user-gated).

Artifact impact plan:
- AGENTS.md: no update expected (no workflow/guardrail change).
- Runtime project skills: none cover these crates (confirm `project-writing-collectors`
  has no OTel-logs ingest pointer to update).
- Specs: **add** `.agents/sow/specs/` entry for the v9 ng-flatten logs WAL frame + the
  production logs pipeline contract once grafted (this is the graduation point the prior
  SOW deferred).
- End-user/operator docs + skills: check the `otel-logs` Function / query skills for the
  array-of-structs field-name change (D1) if any examples reference `endpoints.0.host`.
- `sfst/FORMAT.md`: already v9; add a note that production logs now produce v9 ng-flatten.
- `~/mo/experiment.md`: update when grafted.
- SOW lifecycle: branch-local; durable knowledge → specs/FORMAT.md/code/tests; remove from
  git before merge (this file IS committed for handoff if the work spans sessions).

Open-source reference evidence:
- None; in-repo crates only. (`otap-df-pdata` @ v0.46.0 is an upstream dep of the Arrow
  path being removed — no new OSS reference needed.)

Open decisions:
- D1–D4 below block reaching `Status: ready`.

## Implications And Decisions

> RESOLVED 2026-06-29 — user chose **D1: A, D2: A, D3: A, D4: A**. Goal-approval gate
> satisfied; the staged plan is approved. Each decision's selected option is its
> recommended option, marked **[CHOSEN]** below.

### D1 — Array-of-structs field-name semantics (REQUIRED)

- **Context:** production renders array-of-structs with positional indices
  (`endpoints.0.host`, `endpoints.1.host`); `ng-flatten` collapses to `endpoints[].host`.
  This changes the queryable field namespace. It affects sealed + tail **identically** (no
  parity impact); the consequence is query UX + old-vs-new file comparability.
- **A. Accept the collapse** (`endpoints[].host`). *Pro:* it is the experiment's core
  improvement (no index-position explosion; matches `tags[]`); pre-GA, no users. *Con:* old
  Arrow-era files (if any retained) and any examples/docs using positional paths diverge.
- **B. Preserve positional indices** in ng-flatten for arrays-of-structs. *Pro:* byte-for-
  byte field-name continuity. *Con:* defeats the array-collapse goal; large change to
  `ng-flatten` flattening; not recommended.
- **Recommendation: A** (long-term-best; the whole point of the work; pre-GA window).
- **[CHOSEN: A]** (user, 2026-06-29). Accept `endpoints[].host`; `event_name` indexing gain accepted with it.

### D2 — WAL cutover strategy (REQUIRED)

- **Context:** the logs WAL payload changes Arrow → bincode; ingest-encode + indexer-decode
  + tail-scan flip together. In-flight old-format logs WALs must be handled.
- **A. Flag-day** — on deploy, drop/ignore pre-existing old-format logs WALs (or let them
  age out), index only new ng-flatten WALs. *Pro:* simplest; pre-GA, transient data, no
  users. *Con:* loses any unindexed in-flight logs at cutover.
- **B. Format-tag coexistence** — tag the WAL frame/header format and keep both decoders
  during a transition window so old WALs still index. *Pro:* zero loss. *Con:* keeps the
  Arrow logs decoder alive longer (delays clean-end-state); more code.
- **Recommendation: A** (surgical; pre-GA transient data makes zero-loss unnecessary) —
  unless you want a no-loss rolling deploy, then B.
- **[CHOSEN: A]** (user, 2026-06-29). Flag-day; in-flight old-format logs WALs dropped/aged
  out. The tier-3 fallback-timestamp value change is accepted with this.

### D3 — Migration scope: logs-only now (REQUIRED)

- **Context:** the ingestor serves logs + metrics + traces on one gRPC server. The ng work
  is logs-only.
- **A. Logs-only graft** (metrics + traces stay on Arrow/existing paths). *Pro:* bounded,
  matches the experiment; the operational shell is shared and untouched. *Con:* two
  formats coexist in the agent until metrics/traces are migrated separately.
- **B. All three signals.** *Pro:* one format everywhere. *Con:* much larger; ng-flatten
  has no metrics/traces flattening; out of the validated scope.
- **Recommendation: A** (the only validated scope; metrics/traces are separate future SOWs).
- **[CHOSEN: A]** (user, 2026-06-29). Logs-only graft; metrics/traces untouched.

### D4 — Exploit typed/columnar richness now, or defer (REQUIRED)

- **Context:** the graft *persists* the typed tree + per-row trace/span/flags columns;
  `sfsq` does not yet *use* them (typed value matching, per-row column querying).
- **A. Defer** to a follow-on SOW (graft = format parity only). *Pro:* keeps this SOW about
  a clean, provable migration; smaller blast radius. *Con:* the user-visible benefit lands
  later.
- **B. Include** typed query operators / per-row column querying in this SOW. *Pro:* benefit
  sooner. *Con:* expands scope well beyond the migration; additive feature work.
- **Recommendation: A** (defer; this SOW's clean end state is format parity + clean removal).
- **[CHOSEN: A]** (user, 2026-06-29). Defer richness to a follow-on SOW (Stage 4 / Follow-up Issues).

### D5 — ng-flatten tail decode location (implementation fork, surfaced during Stage 0)

- **Context:** the tail scanner feeds sfsq's private `ScanSink`; the ng-flatten frame→rows
  decode is payload-specific. sfsq depends on `wal-otap` (OTAP) but not `ng-flatten`. This
  collides with the recorded "sfsq is a neutral engine" principle ([[project_sfsq_neutral_engine]]).
- **1. Add `ng-flatten` dep + `WalScan::scan_flattened` in sfsq.** Reuses `ScanSink` +
  evaluation verbatim; the function is needed in production for Stage 2 anyway; post-migration
  sfsq's single WAL-payload coupling *moves* wal-otap→ng-flatten (not a net-new coupling).
  Neutrality principle is about `LogsQuery`/`LogsData` + the netdata wire shape (in `otel-ledger`),
  both untouched.
- **2. Abstract the tail decode behind a trait (sfsq stays payload-neutral).** Cleaner-principle,
  bigger refactor, more risk; over-engineering for a pre-GA PoC where ng-flatten is the format.
- **Recommendation: 1.** **[CHOSEN: 1]** (user, 2026-06-29).

### D6 — Timestamp/id normalization location (Stage 2; blocks the ingest edit)

- **Context:** `ng_flatten::flatten_request` reads `time_unix_nano` directly (no event→observed→
  clock fallback); production must normalize at ingest and freeze into `Record.ts`. The exact
  logic exists as **private fns in the `ng-ingest` binary** (`normalize_timestamps` + `normalize_ids`).
- **1. Promote `normalize_timestamps` + `normalize_ids` to the `ng-flatten` library (pub)**, shared
  by `ng-ingest` and the production ingestor. One source of truth, no duplication.
- **2.** Duplicate the two fns into `logs_service.rs`.
- **Recommendation: 1.** **[CHOSEN: 1]** (user, 2026-06-29).
- **normalize_ids: YES** (user, 2026-06-29) — apply both `normalize_timestamps` and
  `normalize_ids` at production ingest; `normalize_ids` is a prerequisite for clean fixed-stride
  `TRCE`/`SPAN` per-row columns and matches the validated ng pipeline + Stage-0 harness.

### D7 — Flag-day mechanism (Stage 2)

- **Context:** the recovery path (`recover_unindexed`) hard-fails startup on any index failure,
  and the WAL header doesn't tag payload format — so a leftover OTAP logs WAL would bincode-fail
  against the ng builder and brick startup. D2-A's "drop/age out" isn't achievable purely by aging.
- **1. Op-step only** (no code): on upgrade the operator clears the logs WAL+SFST data dir.
- **2. Minimal code:** skip+delete a logs WAL on decode failure in recovery.
- **Recommendation: 2.** **[CHOSEN: 1]** (user, 2026-06-29) — rationale: there are **no users and
  no pre-existing WAL/SFST data**, so no leftover OTAP WALs can exist at cutover; the brick-on-
  leftover risk is moot. Recorded as an operator note (data dir is empty in practice); no recovery
  code change. If ng-* later ships to anyone with data, revisit (track as a follow-up).

### D8 — Stage 3 OTAP-removal forks (RESOLVED 2026-06-29)

- **Context (reference search 2026-06-29):** the only polymorphic `<S: KvSink>` consumers are
  inside `wal-otap` itself (the OTAP decoder being deleted: `decode.rs:129`, `lib.rs:76/84/92`,
  `arrow_columns.rs:329`). Every ng-path use of `KvSink` is **concrete** (`RowIndex` impl at
  `row_index.rs:182`; `ScanSink` impl at `wal_scan.rs:457`; the `use wal_otap::KvSink` lines only
  bring trait methods into scope). The ng path never calls `lookup_hash` (it calls `intern(Some(hash),
  kv)` directly) → `lookup_hash` on both sinks is also dead.
- **D8.1 — KvSink home: [CHOSEN: 3]** (user, 2026-06-29). Drop the `KvSink` trait entirely; give
  `RowIndex`/`ScanSink` inherent `intern`/`row`/`finish`; remove the dead `lookup_hash`; delete the
  `wal-otap` crate. Rejected: (1) keep wal-otap as a misnamed trait-only crate (debt); (2) relocate
  KvSink into sfst-indexer (forces sfsq → indexer runtime dep, wrong direction).
- **D8.2 — Delete `otel-normalize` crate: [CHOSEN: yes]** (user, 2026-06-29). Its only caller was the
  OTAP encoder (`arrow_bridge.rs:70`), removed here; finding C (drop JSON-body parsing) already
  accepted. Remove the crate dir + workspace member + dep.
- **D8.3 — Fold deferred perf E into Stage 3: [CHOSEN: no]** (user, 2026-06-29). E (`build_sfst_range`
  unused `Metrics`) + F (`Bump` pre-size) stay in Follow-up Issues; orthogonal to removal.

### Behavioral changes to acknowledge (not separate forks; folded into the above)

- **`event_name` becomes indexed** (ng-flatten flattens it; the Arrow logs path did not) —
  a gain, no parity impact. Acknowledge under D1's "accept ng-flatten field semantics."
- **Tier-3 fallback timestamp value differs** (ng: per-record monotonic wall clock at
  ingest, frozen in `Record.ts`; Arrow: frame `ingestion_ns + row_offset` resolved at
  decode). Ordering preserved on both; affects only timestamp-less records and only across
  the format boundary. Folded into D2 (cutover).

## Plan

> Ordered stages; each is build/clippy/tests green before the next. FINALIZED only after
> D1–D4. Stage coupling: the WAL-format flip (Stage 2) is atomic across ingest/index/tail.

### Stage 0 — De-risk: ng equivalence harness (validation-first)
- Add a sibling of `sfsq/tests/wal_equivalence.rs` that writes **ng-flatten** frames
  (`ng_flatten::flatten_request` + `fill_hashes` + `encode_frame`) and asserts an ng-flatten
  tail scanner == the ng sealed build (`build_sfst`) across filter/facet/timeline/materialize.
- Clean end state: parity is provable before any production code flips. Acceptance: harness
  green; demonstrates the tail-scanner shape works.

### Stage 1 — Builder: in-memory range build (no production flip yet)
- `sfst-indexer`: widen `build_into` to `pub` (+ re-export). `ng-index`: extract the
  RowIndex-population helper from `build_sfst`; add `build_sfst_range(wal_path, range) ->
  (Summary, Vec<u8>)` using `wal::Reader::open_range`. Keep the seal-time file build.
- Clean end state: the ng builder satisfies BOTH contracts (file + in-memory range).
  Acceptance: unit tests for both; in-memory bytes open via `IndexReader`.

### Stage 2 — Frame cutover (ATOMIC: ingest + index + tail)
- Ingestor logs path: replace `arrow_bridge::encode` (`logs_service.rs:563`) with
  `ng-flatten` flatten + `encode_frame`, **preserving** tenant grouping, stream identity +
  `content_meta`, collision check, oversize handling, and the frame ts range.
- Ledger: re-point `indexer.rs:100` → ng seal build; `handler.rs:334` → `build_sfst_range`.
- Tail: replace `WalScan`'s `wal_otap::decode_range` (`wal_scan.rs:121`) with an
  ng-flatten decoder feeding the reused `ScanSink` via `build_kv` + `Record.ts`.
- Apply the D2 cutover strategy for in-flight WALs.
- Clean end state: production logs run on ng-flatten end to end. Acceptance: ng equivalence
  harness green on the production path; live `otel-streams` round-trip; metrics/traces
  unaffected.

### Stage 3 — Remove the dead Arrow logs path (clean-end-state) — DONE (2026-06-29, uncommitted)
- Remove logs-scoped `arrow_bridge` encode + `wal-otap` logs decode usage +
  `sfst_indexer::index/index_with_options/index_range` (now unused). Keep anything
  metrics/traces still use. Re-run the reference search to confirm zero logs consumers.
- Clean end state: one logs format, one renderer, no dead Arrow logs code. Acceptance:
  reference search clean; workspace build/clippy/tests green.

### Stage 4 (DEFERRED per D4-A) — exploit typed/columnar richness
- Tracked as a follow-on SOW (typed value matching, per-row trace/span column querying).

## Execution Log

### 2026-06-29
- SOW drafted from a 5-agent read-only investigation (production ingest/index/integration
  map + 2 focused parity verifications). Both unknowns (tail-scan parity; on-query
  `index_range` in-memory contract) verified SOUND. Reference search recorded. Strategy =
  graft, not replace; logs-only. Four user-owned forks (D1–D4) surfaced. Status `planning`;
  awaiting D1–D4 + plan approval (goal-approval gate) before any code.
- **User approved 2026-06-29: D1: A, D2: A, D3: A, D4: A.** Goal-approval gate satisfied;
  Status → `ready`. SOW kept local/untracked per user request. No code until the user's
  go-ahead on Stage 0.
- **Implementer = assistant directly** (user instruction 2026-06-29; no GLM/external-LLM code
  delegation — durable, recorded in project memory). **D5: 1** chosen (ng-flatten dep +
  `WalScan::scan_flattened` in sfsq).

#### Stage 0 DONE (2026-06-29) — ng tail-scan ↔ ng sealed-build equivalence harness (green)
- `sfsq` gains `WalScan::scan_flattened` / `scan_flattened_range` (`wal_scan.rs`) + a
  `FlattenedScanError` (re-exported in `logs/mod.rs`): decode ng-flatten frames via
  `wal::Reader::open[_range]` + `ng_flatten::decode_frame`, render `key=value` via
  `ng_flatten::build_kv`, drive the existing private `ScanSink`. Mirrors `ng_index::build_sfst`'s
  resource→scope→record token assembly + `Record.ts` ordering exactly (D5=1).
- Deps: `ng-flatten` (normal) + `thiserror` added to `sfsq`; `ng-index` added to workspace deps
  + sfsq dev-deps. sfsq dependents (`otel-ledger`/`otel-plugin`/`sfsq-cli`) build clean.
- New harness `sfsq/tests/ng_wal_equivalence.rs` (sibling of `wal_equivalence.rs`): 18 seeded
  ng-flatten corpora × a ~22-query matrix assert `LogsShard` parity (matched/fields/facets/
  timeline) between `scan_flattened` (tail) and `build_sfst` (sealed). Coverage guards (asserts):
  multi-frame, multi-valued `tags[]`, array-of-structs `endpoints[].host`, nested kvlist,
  polymorphic Int/Str, non-ASCII, ≥144 non-vacuous queries. Timestamps normalized in the
  fixture (event→observed→clock) mirroring `ng-ingest` since flatten doesn't normalize.
- **Result: `cargo test -p sfsq` = lib 50 + ng_wal_equivalence 1 + wal_equivalence 9, all
  pass.** Clippy clean except one `vec_init_then_push` on the query matrix, identical to the
  sibling `wal_equivalence.rs:395` (pre-existing accepted pattern). **Tail/sealed parity for
  the ng path is now proven by a green harness — the migration's biggest risk is retired.**
- Committed `7f25b26c29` (sfsq: ng-flatten WAL tail scanner + tail/sealed equivalence harness).

#### Stage 1 DONE (2026-06-29) — builder in-memory range build (code complete; not committed)
- `sfst-indexer`: `build_into` widened `pub(super)` -> `pub` (+ re-exported in `lib.rs`) so an
  alternate producer can stream an in-memory SFST through the same machinery. No logic change.
- `ng-index` `sfst_build.rs`: extracted `populate_row_index(reader, row_index, metrics)` (frame
  loop + interning + per-row columns + typed-tree attach), shared by both entry points;
  `build_sfst` (file) behaviour unchanged; new `build_sfst_range(wal_path, range) ->
  (sfst::Summary, Vec<u8>)` opens via `wal::Reader::open_range`, runs `populate_row_index`, and
  streams through `sfst_indexer::build_into` into a `Cursor<Vec<u8>>` — the ng counterpart of
  `sfst_indexer::index_range`. Re-exported from `ng_index`.
- Tests (`count.rs`): `build_sfst_range_whole_file_matches_file_build` (whole-file range build is
  byte-identical to the file build) + `build_sfst_range_split_partitions_records` (frame-aligned
  split partitions records exactly).
- **Result: sfst-indexer 7, ng-index 6, sfsq 50+1+9, otel-ledger 48 — all pass; clippy clean in
  changed files; `otel-ledger`/`otel-plugin` build.** Additive only — the two `sfst_indexer::index*`
  call sites still drive the OTAP path until Stage 2.
- Committed `ccb7219d7f` (ng-index: add build_sfst_range over flattened WALs).

#### Stage 2 DONE (2026-06-29) — atomic frame cutover (OTAP → ng-flatten for logs); NOT committed
- Decisions resolved: **D6: 1** (promote `normalize_timestamps`+`normalize_ids` to `ng-flatten`),
  **D7: 1** (op-step only — no users/data, leftover OTAP WALs can't exist), **normalize_ids: YES**.
- **ng-flatten:** promoted `normalize_timestamps` + `normalize_ids` (+ `MalformedIds`, `TRACE_ID_LEN`/
  `SPAN_ID_LEN`) to `pub` (one source of truth); added `file-registry` dep (for `MonotonicClock`).
  `ng-ingest` now calls the promoted fns (local copies removed).
- **ng-index:** added `build_sfst_file(wal_path, out, &Metrics) -> (sfst::Summary, u64)` (seal-time
  entry; skips `sole_wal_file`, reuses `populate_row_index`). Re-exported.
- **Ingest (`otel-ingestor/logs_service.rs`):** swapped `arrow_bridge::encode` → wrap group into a
  request, `normalize_timestamps`+`normalize_ids` in place (frozen `Record.ts`; cleared malformed
  ids), `flatten_request`+`fill_hashes`+`encode_frame`. Tenancy/identity/`content_meta`/collision/
  oversize/ts-range/clock all preserved. Dropped the unused `arrow_bridge` import.
- **Index feed:** `otel-ledger/indexer.rs` `sfst_indexer::index` → `ng_index::build_sfst_file`;
  `handler.rs` `sfst_indexer::index_range` → `ng_index::build_sfst_range`. `ng-index` added as an
  `otel-ledger` dep. Return shapes/downstream (chunk_cache, IndexReader::open, count cross-check)
  unchanged.
- **Tail scan:** `sfsq` `engine.rs:149` + `page.rs:462` `scan_range` → `scan_flattened_range`
  (error arms Display-only → compile unchanged).
- **Test coupling handled:** the cutover invalidated the OTAP harness's 2 run-level tests
  (`wal_data_stats/rows_*` — they fed OTAP through the now-ng `run` path). **Ported both to
  `ng_wal_equivalence.rs`** (`ng_run_stats_equal_whole_file_index`, `ng_run_rows_match_whole_file_index`
  — chunks via `build_sfst_range` + tail via `scan_flattened` through `run`, the live production
  path) and removed the dead OTAP versions + their exclusive helpers from `wal_equivalence.rs`.
- **Result: workspace builds clean (incl. `otel-plugin`); sfsq lib 50 + ng_wal_equivalence 3
  (+2 ported run-level) + wal_equivalence 7 (−2 dead); ng-index 6, sfst-indexer 7, otel-ledger 48,
  ng-flatten/ng-ingest green; clippy clean in changed files.** Production logs now run end-to-end on
  ng-flatten. OTAP logs encode/decode + `sfst_indexer::index*` are now dead (Stage 3 removes them).
- **D7 operator note (for docs at graduation):** on upgrade to the ng-flatten logs format, any
  pre-existing OTAP logs WAL/SFST data dir must be cleared (recovery hard-fails on a stale OTAP WAL).
  Moot today (no users/data); tracked as a follow-up if ng-* ships to anyone holding data.
- Committed `302d568514` (otel logs: cut the production logs pipeline over to ng-flatten).

#### Review round 1 on the cutover (2026-06-29) — vk-review (4 models) + codex; range `0b37f85b82..302d568514`
- **Outcome: cutover plumbing/parity sound (all 5 agree), but 3 production behaviors were dropped
  because the ng *experiment* flatten path was ported without 2 production pre-flatten steps + the
  identity derivation.** Each of 3 reviewers caught a different real defect; mimo/deepseek missed them.
- Validated findings + **user decisions**:
  - **A [yes] — sfsq-cli tests broken (codex + minimax CRITICAL).** OTAP fixtures fed to the ng tail
    decoder → 5/10 fail. (I had not run sfsq-cli in Stage-2 validation — honest miss.) Fixed: migrated
    fixtures to ng-flatten (`encode_ng_frame` + `ng_index::build_sfst_file`), swapped dev-deps
    (otel-ingestor/sfst-indexer → ng-flatten/ng-index), and updated one filter to the ng field name.
  - **B [1] — sealed SFSTs lost service identity (codex P1).** Builder derived `content_meta` from
    rows via `service_stream` (matches only unprefixed `service.name=`); ng emits
    `resource.attributes.service.name=` → `(unattributed)`. Fixed: `build_into`/`build_and_write` take
    `content_meta_override: Option<Vec<u8>>`; ng builders pass `Some(reader.header().content_meta)`
    (the identity the ingestor wrote) — the "lift identity to the caller" the builder comment
    anticipated. `None` keeps legacy row-derivation for OTAP/tests.
  - **C [no] — JSON-body field indexing dropped (glm HIGH).** OTAP called `otel_normalize::normalize_body`
    (parse JSON string body → structured `body.*`); ng doesn't. **User accepted the behavior change**
    (ng keeps body as-is; JSON bodies are full-text-searchable, not field-queryable). Consequence:
    `otel_normalize::normalize_body` is now fully dead → remove in Stage 3 (with the `otel-normalize`
    dep on otel-ingestor if nothing else uses it).
  - **D [i] — clock-mutex held across the normalize loop (glm/mimo MED).** Fixed via base+offset:
    `ng_flatten::normalize_timestamps(req, fallback_base_ns: u64)` (was `&mut MonotonicClock`); the
    ingestor locks once for the tick, normalizes lock-free. Bonus: ng-flatten no longer needs
    `file-registry` (dep removed). Tradeoff (accepted): synthetic fallback ts not globally unique
    across frames (only records lacking both event+observed time; tie-break deterministic).
  - **E/F [defer] — unused `Metrics` in `build_sfst_range`; `Bump::new()` vs 32 MiB `with_capacity`
    at seal (deepseek/glm/minimax MED/LOW).** Micro-perf; tracked in Follow-up Issues, not fixed.
  - **G [fix] — stale docs.** Fixed: `traces_indexer.rs` + `engine.rs` (ng_index refs), `wal_equivalence.rs`
    module doc (now the legacy-OTAP harness; live-path tests moved to `ng_wal_equivalence.rs`),
    `wal::Writer::write_frame` (dropped the OTAP tier-3 specifics — the WAL is payload-agnostic).
- **Field-namespace change surfaced (broader than D1):** ng namespaces attributes —
  record `level` → `attributes.level`, resource → `resource.attributes.*`, scope → `scope.*` — vs the
  OTAP path's bare keys. Inherent to the ng schema (the same prefixing used throughout the harness);
  pre-GA, no users. Flagged for docs/UI/saved-queries at graduation. Not a separate fork (subsumed by
  adopting ng), but recorded here for visibility.
- **No-action (per reviewers):** dead `compute_log_ts_range` fallback (expected post-cutover); OTAP
  dead code (Stage 3); flag-day/no WAL format version (= D7:1, decided). codex confirmed no
  metrics/traces breakage; tenancy/collision/oversize/lifecycle intact.
- **Validation after fixes: workspace builds clean; sfsq lib 50 + ng_wal_equivalence 3 +
  wal_equivalence 7; sfsq-cli 10+20+1; ng-index 6, sfst-indexer 7, otel-ledger 48, ng-flatten/ng-ingest
  green; clippy clean in changed files.**
- Committed `152fa8cdda` (fix service identity, ingest clock contention, ng cutover test surface).

#### Review round 2 (2026-06-29) — vk-review 4 models (no codex, user near its 5h limit); range `0b37f85b82..152fa8cdda`
- **Verdict: converged.** glm "ship it"; mimo "no correctness bugs found"; deepseek + minimax verify
  the content_meta + clock fixes are correct. No new correctness/security regressions. Remaining
  findings were polish or already-deferred (E/F).
- **Polish applied (low-risk):** (1) `ng_run_stats_equal` test now uses production cursor keys
  (`file_seq` shared, `part: Indexed(i)`) matching the sibling rows test (minimax MED test-fidelity);
  (2) added `ng-flatten` unit tests for `normalize_timestamps` + `normalize_ids` (glm MED test gap);
  (3) restored `flatten_request`'s doc (Stage-2 insert had conflated it onto `TRACE_ID_LEN`);
  (4) fixed the sfst-indexer/lib.rs double-blank artifact (minimax LOW — the actual "fmt leftover");
  (5) rustfmt'd the new `ng_wal_equivalence.rs` + wrapped two of my long lines.
- **Not changed:** `cargo fmt --check` is **not a CI gate** here (the ng-* codebase is broadly
  non-rustfmt-clean at base — diffs in many untouched files), so no blanket reformat (would be huge
  unrelated churn). E (unused Metrics) + F (Bump pre-size) stay deferred per user. `compute_log_ts_range`
  dead-fallback doc, OTAP dead code, and `intern_flattened`'s ignored hash are known/Stage-3/harmless.
- **Validation: all touched-crate tests green (incl. 2 new normalize tests); clippy clean in changed
  files.**
- Committed `ec16002670` (review round 2 polish). **Review loop closed** (user, 2026-06-29) — round 2
  reached ship-it; only deferred/known items remain.
- Committed `1a5b0ac66e` — `cargo fmt` over the 8 migration crates (formatting only, no logic), in its
  own commit so the migration's logic diffs stay readable. Scoped to migration crates; rest of the
  workspace untouched. Migration-crate fmt-check is now clean (0 diffs).

#### Stage 3 DONE (2026-06-29) — remove the dead OTAP logs path (clean-end-state); NOT committed
- Decisions: **D8.1=3** (drop `KvSink` trait, delete `wal-otap`), **D8.2=yes** (delete `otel-normalize`),
  **D8.3=no** (perf E/F stay deferred). Reference search (recorded under D8) showed the only polymorphic
  `<S: KvSink>` consumers were inside `wal-otap` (the OTAP decoder); all ng-path uses are concrete.
- **Deleted crates:** `wal-otap` (OTAP frame decode) and `otel-normalize` (JSON-body parse — finding C,
  accepted) — dirs + workspace members + workspace deps removed; absent from `Cargo.lock`.
- **Deleted files:** `otel-ingestor/src/arrow_bridge.rs` (logs OTAP encoder), `sfsq/tests/wal_equivalence.rs`
  (legacy OTAP harness, superseded by `ng_wal_equivalence.rs`).
- **`KvSink` → inherent (D8.1=3):** the trait is gone. `RowIndex` (`sfst-indexer`) keeps inherent
  `lookup_hash`/`intern`/`row` (the ng builder rides the `lookup_hash` fast path — kept; `reserve_rows`
  was dead → dropped). `ScanSink` (`sfsq`) keeps inherent `intern(kv)`/`row` (its `lookup_hash`/
  `reserve_rows`/ignored-hash param were dead → dropped). All `use wal_otap::KvSink` removed.
- **`sfst-indexer`:** removed `index`/`index_with_options`/`index_range` + `IndexResult` + `INDEX_ARENA_BYTES`;
  dropped the `IndexError::Wal`/`Decode` variants + the `From<wal_otap::ReadError>` impl (only the removed
  fns used them); dropped now-unused `wal` + `wal-otap` deps. `build_into`/`build_and_write`/`IndexError`
  (the ng path) untouched.
- **`otel-ingestor`:** dropped `otap-df-pdata` + `arrow` + `otel-normalize` deps (only `arrow_bridge`
  used them; `twox-hash`/`serde_json` kept — `iter.rs` uses them).
- **`otel-ledger`:** dropped the now-unused `sfst-indexer` dep (Stage 2 migrated its call sites to
  `ng_index`; only prose backtick mentions remained — not doc links).
- **Docs:** refreshed every stale `wal-otap`/`arrow_bridge`/`index_range` reference (module docs +
  comments in `sfst-indexer`, `sfsq`, `ng-index`, `ng-flatten`, `sfst/schema.rs`, `sfsq-cli` fixtures);
  fixed the one broken intra-doc link (`Self::scan` → `Self::scan_flattened`).
- **Validation:** `cargo build --workspace` clean; `wal-otap`/`otel-normalize`/`arrow_bridge`/`index_range`/
  `WalScanError` — zero residual references in src. Tests: sfst-indexer 9, ng-index 6, ng-flatten 7,
  sfsq 50 + ng_wal_equivalence 3, sfsq-cli 10+20+1, otel-ledger 48, otel-ingestor 112 — all pass.
  `cargo doc -p sfsq` with `-D rustdoc::broken_intra_doc_links` clean. Clippy: no new warnings in
  changed lines (the 4 in `fst_builder.rs`/`row_index.rs` are at untouched lines — pre-existing ng-*
  lints; this codebase is not clippy-clean at base, same standard as prior rounds).
- Clean end state reached: one logs format, one renderer, no dead OTAP logs code or crates.
- Committed `f231594794` (otel logs: remove the dead OTAP logs path).

#### Stage 3 review + consult round (2026-06-29) — committed
- **vk-review** (4 models, commit `f231594794`): verdict **PRODUCTION GRADE** — clean, correct,
  well-scoped; deepseek noted a net **security positive** (removed the Arrow IPC attack surface). All
  findings were LOW/MED doc/spec staleness. Applied #1–#8 (FORMAT.md:298, wal_scan/tests.rs:4 module
  doc, fst_builder.rs:419 OTAP comment, sfst_build_spike.rs:5 intern-signature comment, sfst-indexer
  Cargo description, otel-storage-substrate.md `sfst_indexer::index`→`ng_index::build_sfst_file`,
  rust-cross-crate-doc-references.md wal-otap example, dead `tempfile`/`uuid` dev-deps) + the
  verification-surfaced migration-coupled ng-ingest doc links. Rejected re-adding `reserve_rows`
  (speculative). Committed `7fe55b5457`. Workspace-wide `cargo doc -D broken_intra_doc_links` confirmed
  the migration crates clean (remaining errors are unrelated pre-existing debt in netipc/file-lifecycle).
- **vk-consult** (4 models, revim consult mode) — open-ended "what does the removal unlock". Two
  validated net-new findings beyond the review:
  - **flatten-otel `logs` module dead** (`json_from_export_logs_service_request`/`json_from_log_record`,
    ~164 lines) — orphaned by the `otel-normalize` deletion; coupled clean-end-state. **Done**: deleted
    + dropped the now-unused `opentelemetry-proto "logs"` feature. Committed `eeffbc64c8`.
  - **`content_meta_override: Option<Vec<u8>>` `None` arm vestigial** (3/4 consensus) — making it
    mandatory would cascade-remove `resolve_stream`, `RowIndex::service_stream` (+tests),
    `IndexError::MultipleStreams`/`IdentityTooLarge`, and the `otel-logs-identity` dep on sfst-indexer.
    **DECISION (user, 2026-06-29): SKIP / keep as-is.** Rationale: the `None` arm's `MultipleStreams`
    one-stream-per-file check is a defense-in-depth **hard-failure invariant guard**; it is dormant in
    production today (header authority) but valued per [[feedback_prefer_hard_failures]]. Not worth
    removing a latent invariant for API tidiness. Recorded as rejected-with-reason (Follow-up Issues).
  - Minor historical OTAP comments in `ng_wal_equivalence.rs` refreshed (doc pass). Committed
    `dd1a75b1f0`. (`schema/tests.rs:261` "as ng-index supplies it" left — still accurate.)

**Migration status (2026-06-29):** Stages 0–3 delivered; 2 review rounds (Stages 0–2) + 1 review+consult
round (Stage 3) closed. Commit chain on `sjr-tree8` (NOT pushed): `b5953dbd6b` (v9) → `7f25b26c29`
(Stage 0) → `ccb7219d7f` (Stage 1) → `302d568514` (Stage 2 cutover) → `152fa8cdda` (round-1 fixes) →
`ec16002670` (round-2 polish) → `1a5b0ac66e` (cargo fmt) → `f231594794` (Stage 3 OTAP removal) →
`7fe55b5457` (review doc fixes) → `eeffbc64c8` (dead flatten-otel logs module) → `dd1a75b1f0` (consult
doc pass). Production OTel logs run end-to-end on ng-flatten; the dead OTAP path + 3 dead crates/modules
are gone. **Remaining (tracked, not started):** Stage 4 (typed query operators / per-row column
querying), spec graduation, and the deferred E/F perf items — all in Follow-up Issues.

## Validation

**Live agent round-trip (2026-06-29) — real-use evidence, PASS.** Built the agent in place
(`netdata-build` MCP, debug profile, compiles `otel-plugin`/`otel-ingestor`/`otel-ledger`/`sfsq`/
`ng-index` from this branch), ran it (claimed + cloud-connected), pushed a deterministic **200-record
OTLP log corpus** over gRPC (`service.namespace=ngtest`, `service.name=ng-verify-svc`,
field_cardinality=10), and queried the `otel-logs` Function. Config forced a seal
(`logs_wal_max_log_entries=50`).
- **Ingest→WAL→SFST→query, full count:** query `matched=200` (all pushed records queryable).
- **Both migrated paths exercised + folded correctly:** `otel-files` showed **1 sealed SFST** (seq=1,
  100 logs, 4031 B — built via `ng_index::build_sfst_file`) **+ 1 active WAL** (seq=2, 100 entries —
  scanned via `scan_flattened`). The 200-count query merges sealed + tail = the production `run()` path.
- **Service identity preserved end-to-end** (Stage-2 B-1 fix): the stream resolved as
  `ngtest/ng-verify-svc` (ns_hash `a6d32efa9f1fd008`) on **both** the WAL and the sealed SFST — i.e. the
  WAL-header `content_meta` authority works through real ingest, not just unit tests.
- **Field namespace (D1 change) confirmed live:** queryable fields are `attributes.{host,code,level,seq}`,
  `resource.attributes.service.{name,namespace}`, `scope.{name,version}`, `event_name` (now indexed),
  `severity_text`/`severity_number`, `body` — exactly the ng-flatten namespacing.
- **Filter + facet correctness:** `severity_text=ERROR` → `matched=50`; facets on `attributes.host`/
  `attributes.code` returned the expected 10-per-value mid-cardinality breakdown (scoped to the filter).
  `severity_text` facet split 50/50/50/50; histogram bucketed all 200 across the window.
- Agent stopped cleanly (throwaway). This satisfies the acceptance criterion "a live feed produces a
  queryable WAL→SFST" and the Validation Gate's real-use-evidence requirement for Stages 0–3.

**Acceptance criteria:** ingest encodes ng-flatten frames preserving tenancy/identity/collision/oversize
✓ (live + unit); both producer call sites build via ng path ✓; tail decodes ng frames identical to sealed
✓ (`ng_wal_equivalence` + live fold); metrics/traces unaffected ✓ (compile + tests); dead Arrow logs path
removed ✓ (Stage 3, reference search clean); build/clippy/tests green across touched crates ✓.

**Clean-end-state evidence:** the recorded target (one logs format, one renderer; OTAP encode/decode +
`sfst_indexer::index*` + `KvSink` + the `wal-otap`/`otel-normalize`/`flatten-otel::logs` dead code removed)
is delivered; reference searches recorded under D8 + the review/consult rounds returned clean. One
clean-end-state simplification (content_meta Option→mandatory) was surfaced and **user-rejected with
reason** (Follow-up Issues), not silently dropped.

**Reviewer findings:** vk-review (4 models) → PRODUCTION GRADE; all findings doc-hygiene, fixed in
`7fe55b5457`/`dd1a75b1f0`. vk-consult net-new: dead `flatten-otel::logs` (fixed `eeffbc64c8`) +
content_meta (rejected). Same-failure search: all stale `wal-otap`/`arrow_bridge`/`sfst_indexer::index`
references swept from src + specs (the active SOW file itself retains them as history; removed before merge).

**Sensitive data gate:** no secrets/PII in this SOW, the specs, the commits, or commit messages —
only code identifiers, file:line, schema field names, and synthetic values (`ngtest`/`ng-verify-svc`,
`host-N`, `cNNN`). The live feed used the synthetic `otel-streams` corpus + an ephemeral local agent;
aggregate counts only. `scan-sensitive.sh` clean (173 files).

Remaining acceptance items (Stage 4 / metrics+traces) stay open per Follow-up Issues; spec graduation is
DONE (this round).

## Artifact Maintenance Gate

- **AGENTS.md:** no change — no workflow/responsibility/guardrail change (the migration is code + specs,
  not process).
- **Runtime project skills (`.agents/skills/`):** no change — none cover the OTel-logs ingest/index/query
  crates; `project-writing-collectors` has no OTel-logs storage pointer to update.
- **Specs (`.agents/sow/specs/`):** UPDATED — added `otel-logs-ng-flatten-format.md` (the v9 ng-flatten
  WAL frame + field namespace + render-parity contract; the prior SOW's deferred graduation); fixed the
  identity-authority bullet in `otel-stream-identity.md` (WAL-header `content_meta` is the authority, not
  row-derivation); refreshed the `sfst_indexer::index`→`ng_index::build_sfst_file` reference in
  `otel-storage-substrate.md` and the wal-otap example in `rust-cross-crate-doc-references.md`; README
  index updated. (`otel-offline-wal-sfst-query.md`/`otel-remote-storage-config.md`/`otel-legacy-logs-viewer.md`
  unaffected — verified no OTAP refs.)
- **End-user/operator docs:** none shipped for this pre-GA PoC. The `attributes.*` field-namespace change
  is recorded as a pre-GA graduation-docs follow-up (Follow-up Issues); no current operator doc references
  the old bare-key namespace.
- **End-user/operator skills:** no change — the public query skills (`query-netdata-{cloud,agents}`,
  `query-snmp-traps`) are signal/transport-neutral and do not encode the OTel-logs field namespace.
- **Code-adjacent format doc:** `sfst/FORMAT.md` already states v9 and now uses the correct
  "raw `(ts, key=value)` rows" wording (review fix).
- **SOW lifecycle:** branch-local; durable knowledge transferred to the spec + FORMAT.md + code/tests; the
  active SOW file is untracked (never committed) so the branch HEAD carries no SOW — CI merge-guard passes.
  Local copy preserved for handoff.

## Outcome

The production OTel **logs** pipeline runs end-to-end on the `ng-flatten` typed, array-collapsed format
with SFST v9 (typed schema tree + per-row columns): ingest encodes bincode `FlattenedRequest` frames
(tenancy/identity/collision/oversize/TLS preserved), seal-time + on-query SFST builds and the active-WAL
tail all decode the same frame through the same `build_kv` renderer, and `sfsq`/`otel-ledger` query it
unchanged. The dead OTAP/Arrow logs path is fully removed — 2 crates (`wal-otap`, `otel-normalize`), 1
module (`flatten-otel::logs`), `otel-ingestor::arrow_bridge`, the `KvSink` trait, and
`sfst_indexer::index*` are gone (~3,900 net lines deleted). Validated by unit tests, the
`ng_wal_equivalence` tail/sealed parity harness, a PRODUCTION-GRADE 4-model review, a 4-model consult, and
a live agent OTLP round-trip (200 records, sealed + tail, correct counts/facets/identity/namespace).
Delivered on `sjr-tree8`, **not pushed**; merge is a separate user-gated step.

## Lessons Extracted

- **Run `cargo doc --workspace -D rustdoc::broken_intra_doc_links`, not per-crate.** The per-crate check
  (`-p sfsq`) passed while migration-coupled links in `ng-ingest` (and pre-existing ones elsewhere) were
  broken; the workspace check surfaced them. Make workspace-wide the default after a cross-crate refactor.
- **Moving a `pub fn` across crates silently breaks plain-text + intra-doc references to it.** The
  `normalize_timestamps`/`normalize_ids` promotion (Stage 2) left dangling refs found only at Stage-3
  review — the cross-crate-doc-reference spec rule applies to *moves*, not just privatization.
- **Deleting a crate orphans its consumers' submodules.** Removing `otel-normalize` left
  `flatten-otel::logs` dead with no compiler signal (pub items); a reverse-dep sweep on each deleted crate
  belongs in the removal checklist.
- **A dormant hard-failure guard is worth keeping over API tidiness.** The `MultipleStreams`/`service_stream`
  cleanup was clean-end-state-attractive but removed a latent invariant check; recording it rejected-with-
  reason ([[feedback_prefer_hard_failures]]) is the right disposition, not silent deletion.
- **A live MCP agent round-trip is cheap, decisive validation** for a storage/ingest migration — it caught
  nothing wrong here, but it exercised the real binary + gRPC + seal + query Function in ~2.5 min and is
  stronger evidence than unit tests for the field-namespace + identity-authority changes.

## Follow-up Issues

- **content_meta_override → mandatory: REJECTED (user, 2026-06-29).** vk-consult flagged the `None` arm
  of `build_into`/`build_and_write` as vestigial (only the spike test passes `None`). Making it
  mandatory would remove `resolve_stream`/`RowIndex::service_stream`/`MultipleStreams`/`IdentityTooLarge`
  + the `otel-logs-identity` dep on sfst-indexer. **Not doing it:** the `MultipleStreams` one-stream-per-
  file check is a defense-in-depth hard-failure guard ([[feedback_prefer_hard_failures]]); dormant in
  production but kept deliberately. Revisit only if a future producer needs `None` removed.
- **Review round 1 deferred (E):** `build_sfst_range` allocates a `Metrics` it never reports on the
  on-query hot path — make `populate_row_index` take `Option<&Metrics>` (or a no-op metrics). Micro-perf.
- **Review round 1 deferred (F):** ng seal build uses `Bump::new()`; the OTAP path used a 32 MiB
  `Bump::with_capacity` — give the ng builders a pre-sized arena to cut seal-time arena growth.
- **Review round 1 deferred (F3):** no production-service-level regression test (the ng round-trip
  tests live in `ng-ingest`/harness, bypassing `NetdataLogsService::export`'s grouping/tenancy/
  collision/oversize). Add a test that exports via the production service and asserts the sealed
  SFST's `content_meta` + timestamp/id normalization. Would have caught finding B.
- **Stage 3 (C consequence): DONE** — `otel-normalize` (the whole crate, incl. `normalize_body`) and
  its `otel-ingestor` dep were removed together with the OTAP path.
- **Field-namespace change (graduation docs):** record attrs are now `attributes.*`, resource
  `resource.attributes.*`, scope `scope.*` (vs OTAP bare keys). Update operator docs / UI field
  pickers / any saved queries before GA.
- **Stage 4 (D4-A):** typed query operators + per-row trace/span column querying — follow-on SOW.
- **Metrics + traces migration (D3-A):** separate SOWs; ng-flatten has no metrics/traces
  flattening yet.
- **Spec graduation: DONE (2026-06-29).** Added `.agents/sow/specs/otel-logs-ng-flatten-format.md`
  (v9 ng-flatten WAL frame + field namespace + render-parity contract); fixed identity-authority in
  `otel-stream-identity.md`; README index updated.
