# SOW-20260629-datafusion-sfst-stage-a - SQL over a single SFST via DataFusion (Stage A)

## Status

Status: in-progress

Sub-state: Stage A + column-direct reads (D3=B) implemented and validated against
`out-v9.sfst`. User froze the SFST format (2026-06-29), closing Stage B; the
column-direct optimization needs no format change (KvId ranges already in META).
8 integration tests + 2 sfst unit tests + doctest green; clippy clean. See
`## Validation`. Awaiting user direction (Stage C typed pushdown / Stage D facet
aggregation / `materialize_fields` batching / graduate).

## Requirements

### Purpose

Prove an end-to-end DataFusion integration that runs SQL over **one** SFST file,
with real pushdown (projection, equality/time filters, LIMIT, ordering,
statistics). This is an **experiment** to learn the integration contract before
deciding on any SFST format change. Production v9 / the ng-index seal path are
NOT touched.

### User Request

> "Crate location should be here, ie. under ~/repos/nd/sjr-tree8/src/crates."
> "Let's focus on stage A only. Once we finish stage A, we will be in a much
> better position to make further informed explorations."

Stage A = an Arrow-native `TableProvider` + leaf `ExecutionPlan` over a single
existing v9 SFST. Stage B (experimental columnar value layout) is explicitly
**out of scope** for now. Design doc: `~/mo/datafusion-sfst.md`.

### Assistant Understanding

Facts:

- DataFusion 54.0.0 is the target (source ref at `~/repos/crates/datafusion`,
  published crate `datafusion = "54"`). Extension point settled in the design
  doc: custom `TableProvider` + hand-written leaf `ExecutionPlan` (NOT
  `ListingTable`, which is columnar-file-bound).
- The SFST read surface exists today (`sfst::IndexReader`): `tree()` /
  `field_table()` (schema), `compile_filter`/`matched_positions`/
  `range_positions` (filter + time pushdown), `load_timestamps` + the per-row
  column chunks (already columnar), `materialize_rows` (attribute
  reconstruction), `facets`/`timeline` (Stage D, not now).
- Measured fixture `~/repos/tmp/ng/out-v9.sfst` (80.2 MiB, 500K records):
  `HF` high-card index 68.2%, `SB` stream batches 25.7%, `MF` 1.7%, `PRIM`
  1.4%. Stage A reads this file as-is; no format change.
- Workspace `src/crates/Cargo.toml`, edition 2024, resolver 2. No arrow /
  datafusion deps present yet. `release` profile is `lto="fat"` +
  `codegen-units=1`.

Inferences:

- Stage A's value is the **integration + pushdown contract**, not raw scan
  speed; a correct (possibly row-materialized) attribute path is acceptable for
  Stage A and the column-direct optimization can be a later stage (D3 decision).
- Per-row scalar columns (TIMS/OBTS/TRCE/SPAN/FLAG/DRAC) are already columnar →
  map to typed Arrow arrays directly, independent of the attribute decision.

Unknowns:

- Whether `datafusion = "54"`'s transitive deps (arrow, sqlparser, prost) clash
  with workspace pins (`prost = 0.14`, `tonic = 0.14`, `zstd = 0.13`). Resolved
  by a trial `cargo build` of the new crate; recorded before `ready`.

### Acceptance Criteria

- A new crate under `src/crates/` exposes a `TableProvider` over one SFST file;
  `SELECT … FROM sfst WHERE …` runs through a `SessionContext` and returns
  correct rows. Verified by integration tests on `out-v9.sfst`.
- Projection pushdown: a projected query reads/builds only the requested columns
  (verified by test + `EXPLAIN`).
- Filter pushdown: equality-on-field and time-range predicates are pushed
  (`supports_filters_pushdown` reports them; `EXPLAIN` shows no redundant
  `FilterExec` for the Exact set). Correctness verified against a
  `materialize_rows` oracle for the same predicate.
- LIMIT + output-ordering (timestamp) advertised and honored.
- `statistics()` returns row count + time-range bounds.
- Results match an independent oracle (the existing `IndexReader` query path) on
  a suite of queries.

## Decisions (REVISIT after Stage A works)

User delegated all five forks to assistant judgement on 2026-06-29 ("follow your
own judgement"), with one requirement: **record them so we revisit once we have
something working.** Each is a deliberate Stage-A simplification, not a permanent
choice.

- **D1 — Crate name → `sfst-datafusion`.** Bridge crate naming. Revisit: only if
  a broader role emerges. Low stakes.
- **D2 — DataFusion dep → published `datafusion = "54"` (crates.io).** Portable
  and committable; the `~/repos/crates/datafusion` checkout stays a read-only
  reference. Revisit: only if we need an unreleased DataFusion patch.
- **D3 — Attribute columns → ✅ RESOLVED to column-direct (option B), no format
  change.** Was option A (`materialize_rows`); measured A at ~6s to project one
  high-card column over 500K rows (root cause: `build_string_table` eagerly
  decodes all 38 HF chunks). Replaced with a new sfst primitive
  `IndexReader::materialize_field(field, positions)` that decodes only the
  projected field's chunk (KvId range derived from META cardinalities via
  `high_kv_id` — no format change; the freeze holds). Result: single-column
  high-card projection **6.27s → 395ms (~16×)**, `…text` 5.89s → 186ms (~32×);
  count(*)/timestamp-only unchanged (~27ms, no attribute decode). Validated for
  value-parity vs `materialize_rows` (sfst unit tests low + multi-valued;
  integration test high-card on the real fixture). Remaining inefficiency
  (tracked, not blocking): `SELECT *` re-scans the stream batches once per
  high-card field (20.4s → 6.07s only) — a batched `materialize_fields` sharing
  one SB pass would fix it; only matters for very wide projections.
- **D4 — Schema shape → flat dotted-path columns (option A).** One Arrow column
  per leaf path (`derive_scalar_kinds` types; multi-valued → `List<Utf8>`).
  Revisit: nested `Struct`/`List` (option B) if SQL ergonomics or nested access
  demand it.
- **D5 — Pushdown exactness → `Exact` for `col = 'value'` + timestamp range;
  `Inexact` for everything else (option A).** Safe: DataFusion re-checks the
  Inexact set. Revisit: promote LIKE/regex/numeric predicates to `Exact` as part
  of Stage C (typed-predicate pushdown), with semantic-match tests.

## Analysis

Sources checked:

- `sfst/FORMAT.md`, `sfst/src/index_reader.rs`, `sfst/src/query.rs`,
  `sfst/examples/inspect.rs` (prior art: schema walk + chunk reads).
- `src/crates/Cargo.toml` (workspace wiring).
- DataFusion 54 recon (custom_datasource.rs, table.rs, execution_plan.rs) per
  `~/mo/datafusion-sfst.md`.

Current state:

- No DataFusion/Arrow integration exists. `sfsq` is the current query engine
  (wire-neutral); it is NOT DataFusion-aware and stays untouched.

Risks:

- **Build weight:** datafusion 54 is a large dep tree; workspace release is
  `lto=fat`. Mitigation: build/test the experiment crate in dev profile; it is a
  leaf crate, so it does not slow other crates' builds.
- **Dep conflicts:** arrow/prost/zstd version skew vs workspace pins. Mitigation:
  trial build before `ready`; the new crate may pin its own arrow via datafusion
  re-exports to avoid touching `[workspace.dependencies]`.
- **Pushdown correctness:** claiming `Exact` when SFST semantics differ (regex
  anchoring, type coercion) would drop DataFusion's re-check and return wrong
  rows. Mitigation: claim `Exact` only for equality + time-range in Stage A;
  everything else `Inexact` (DataFusion re-filters).

## Pre-Implementation Gate

Status: ready

Problem / root-cause model:

- Not a bug. New experimental capability: expose one SFST as a DataFusion table
  so SQL + DataFusion's optimizer can drive the existing SFST index.

Evidence reviewed:

- SFST format + reader surface (above). DataFusion extension-point recon in
  `~/mo/datafusion-sfst.md`. Measured fixture breakdown via
  `cargo run --example inspect -- sections out-v9.sfst`.

Affected contracts and surfaces:

- NEW crate `src/crates/sfst-datafusion`. Adds it to workspace `members`.
- `sfst` crate: ADDED one public method `IndexReader::materialize_field(field,
  positions) -> Vec<Vec<String>>` (additive; no behavior change to existing
  APIs; covered by new sfst unit tests). This is the column-direct read the
  Stage A gate pre-authorized.
- No change to the v9 on-disk format, ng-index, sfsq, otel-ledger, WAL. Format
  is frozen per user decision.

Clean-end-state target:

- A self-contained experiment crate that turns one SFST into a queryable
  DataFusion table with the Stage A pushdowns wired. No production wiring.
- Removed as redundant (i): none (net-new crate).
- Excluded coupled items (ii): Stage B/C/D (columnar layout, typed-predicate
  pushdown, aggregation pushdown) — out of scope by explicit user instruction;
  tracked in `~/mo/datafusion-sfst.md` staging.
- Reference search: N/A (no path/contract replaced).

Existing patterns to reuse:

- `sfst::IndexReader` query surface; `sfst/examples/inspect.rs` schema walk;
  DataFusion `custom_datasource.rs` provider+plan skeleton.

Risk and blast radius: see Analysis Risks. Blast radius is one new leaf crate +
one `members` line; reversible.

Sensitive data handling plan:

- Fixture is a synthetic bluesky/certstream dataset (no secrets/PII). SOW cites
  field paths and sizes only; no raw record values beyond public schema names.

Implementation plan (pending D1–D5):

1. Scaffold crate (D1 name), add `datafusion` dep (D2 source), trial build to
   clear the dep-conflict unknown.
2. Schema: map `SchemaTree`/`field_table` → Arrow `SchemaRef` (D4 flat vs
   nested). Per-row scalar columns → typed Arrow directly.
3. `TableProvider`: `schema`, `table_type`, `statistics`,
   `supports_filters_pushdown` (D5 exact set), `scan`.
4. Leaf `ExecutionPlan`: `PlanProperties` (schema, single partition,
   output_ordering=timestamp), `execute` → `RecordBatchStream`.
5. Attribute column build (D3 row-materialize+pivot vs column-direct).
6. Integration tests vs `IndexReader` oracle on `out-v9.sfst`; `EXPLAIN`
   assertions for pushdown.

Validation plan:

- `cargo test -p <crate>`; oracle-comparison suite; `EXPLAIN` snapshot tests for
  projection/filter/limit pushdown; manual `SELECT` smoke queries.

Artifact impact plan:

- AGENTS.md: no change (experiment crate, no workflow change).
- Runtime project skills: no change.
- Specs: no new spec until the experiment graduates; if Stage A reveals a
  durable contract worth keeping, record it then. Reason: experiment.
- End-user/operator docs: none (no user-facing feature).
- End-user/operator skills: none.
- SOW lifecycle: branch-local; durable design memory lives in
  `~/mo/datafusion-sfst.md` (local, not committed) until graduation; delete SOW
  from git before any merge.

Open decisions: none. D1–D5 resolved by assistant judgement (delegated by user
2026-06-29); recorded under `## Decisions (REVISIT after Stage A works)`.

## Validation

Acceptance criteria — all met, verified on `~/repos/tmp/ng/out-v9.sfst` (500,772
records):

- TableProvider + SQL: `count(*)` over a `SessionContext` returns the exact
  `record_count` (test `count_star_matches_record_count`).
- Projection pushdown: `SELECT timestamp ... LIMIT 10` yields one column / 10
  rows; the scan builds only projected columns and skips attribute
  materialization entirely when no attribute column is requested
  (`projection_emits_only_requested_column`).
- Filter pushdown (D5): string-eq and time-window counts match an
  `IndexReader`/`Filter::select` oracle (`string_eq_pushdown_matches_oracle`,
  `time_window_pushdown_matches_oracle`); `EXPLAIN` shows the Exact time
  predicate removes `FilterExec` (`exact_pushdown_drops_filter_exec`).
- LIMIT honored; output-ordering advertised → `ORDER BY timestamp` produces no
  `SortExec` (`order_by_timestamp_needs_no_sort`).
- Statistics: `num_rows` Exact + `timestamp` column min/max bounds.
- Timestamp pivot faithful to TIMS head (`timestamps_match_oracle_head`).

Commands:

- `cargo build -p sfst-datafusion` — clean (datafusion 54 + arrow 59 resolve
  with no version clash against workspace pins; the dep tree is leaf-local).
- `cargo clippy -p sfst-datafusion --all-targets` — clean (the one warning is
  pre-existing in unrelated `file-registry`).
- `SFST_DF_FIXTURE=~/repos/tmp/ng/out-v9.sfst cargo test -p sfst-datafusion`
  — 7 integration tests + 1 doctest pass. Tests skip cleanly when the env var
  is unset/missing, so the suite is green on machines without the fixture.

Honest limitations (each a recorded revisit item, NOT a hidden gap):

- D3=A: when ≥1 attribute column is projected, all attributes of the matched
  rows are decoded row-major via `materialize_rows`; the scan avoids only the
  Arrow array *build* for unprojected columns, and skips materialization
  entirely for count(*)/timestamp-only. Column-direct attribute reads are the
  primary revisit (D3=B / Stage B).
- D5: equality pushdown is string-only; numeric/range/LIKE/regex stay with
  DataFusion (Inexact/Unsupported). Tighten in Stage C.
- Single partition (`UnknownPartitioning(1)`); no intra-file parallelism.
- Multi-valued (`[]`) columns are `List<Utf8>` regardless of element kind (D4).

Artifact maintenance: experiment crate; no AGENTS.md / skill / spec / docs
changes (no production behavior change). Durable design memory lives in
`~/mo/datafusion-sfst.md` (local). SOW remains branch-local and uncommitted.

## Validation addendum — column-direct reads (D3=B)

- New sfst primitive `IndexReader::materialize_field` (index_reader.rs): decodes
  only the projected field's chunk. KvId range from META cardinalities
  (`high_kv_id`), no format change.
- sfst unit tests (self-contained, no fixture needed):
  `materialize_field_matches_materialize_rows` (low tier + absent field),
  `materialize_field_multivalued` (multi-valued). 84 sfst lib tests pass.
- Integration `high_card_field_values_match_oracle`: high-card SB-scan path
  returns byte-identical values to the `materialize_rows` oracle on the real
  fixture (validates the riskiest path on real data).
- Mid tier shares the low-tier bitmap-scatter mechanism (different chunk source
  only); low + high coverage exercises both mechanisms.
- Bench (dev profile, `out-v9.sfst`): single high-card column 6.27s→395ms
  (~16×), `…text` 5.89s→186ms (~32×); count(*)/ts-only ~27ms; `SELECT *`
  20.4s→6.07s (per-high-field SB re-scan; batched `materialize_fields` would
  close this — tracked follow-up).
- clippy clean (`-p sfst -p sfst-datafusion --all-targets`).

Artifact maintenance addendum: sfst gained a public method → its rustdoc covers
it; no spec change (the on-disk format/contract is unchanged — this is a reader
optimization). FORMAT.md untouched (correct: no format change).

Follow-ups (tracked, not blocking):
- `materialize_fields(&[field], positions)` sharing one SB scan across projected
  high-card fields (fixes wide-projection / `SELECT *` cost).
- Stage C: typed/numeric/range predicate pushdown (exploit `SchemaTree`).
- Stage D: facet/timeline aggregation pushdown via a custom OptimizerRule.

## Update — shared-scan batching + SQL CLI (2026-06-29)

- `materialize_field` is now a thin wrapper over a new `materialize_fields(&[field],
  positions)` that resolves all projected high-card fields in ONE stream-batch
  scan (the tracked wide-projection follow-up — DONE). `SELECT *` LIMIT 100000:
  6.07s → 4.92s; the remainder is inherent (decode 38 value arenas + build ~700
  columns), not redundant work. Single/few-column projections unchanged (fast).
- Added `sfst-datafusion/examples/sql.rs`: `cargo run -p sfst-datafusion --example
  sql -- <file.sfst> "<SQL>"` registers the file as table `logs` and prints
  results. Demonstrated a real `GROUP BY ... WHERE ... ORDER BY ... LIMIT` over
  the fixture end-to-end.
- Final state: clippy clean (sfst + sfst-datafusion); 84 sfst lib tests; 8
  integration + 1 doctest pass. sfst public surface gained `materialize_field` +
  `materialize_fields` (additive, unit-tested). On-disk format unchanged.

Remaining optional work (NOT done; offered to user): Stage C (typed/numeric/range
predicate pushdown) and Stage D (facet/timeline aggregation pushdown via a custom
OptimizerRule — would make GROUP BY/date_bin instant from precomputed bitmaps).

## Stage D — facet aggregation pushdown (2026-06-29)

Goal: answer `SELECT <field>, COUNT(*) FROM logs [WHERE …] GROUP BY <field>` from
SFST's precomputed facet bitmaps instead of scanning rows.

Mechanism (DF 54): `SfstFacetRule: OptimizerRule` (ApplyOrder::BottomUp) detects
the pattern over an `SfstTable` and rewrites the `Aggregate` to a leaf
`Extension(SfstFacetNode)`; `SfstExtensionPlanner` (wired via `SfstQueryPlanner`
on the SessionState) plans it to `SfstFacetExec`, which calls
`IndexReader::facets`. New entry point `sfst_datafusion::session_context()` wires
the rule + planner; plain `SessionContext::new()` still runs correctly via the
normal plan. New module `src/aggregate.rs`.

Correctness guardrails (fall back to the normal plan unless ALL hold):
- group column is a low/mid **scalar** field (not high-card — `facets()` errors;
  not `List`/`[]` — SQL GROUP BY on a list ≠ per-value facet). Tier added to
  `ColumnSpec` so this is a plan-time decision (never an execution-time error).
- aggregate is exactly `COUNT(*)` (empty/literal args, not distinct, unfiltered).
- every `WHERE` conjunct (gathered from BOTH the Filter node and pushed
  `scan.filters`) translates to an SFST predicate, and none constrains the group
  column (facets exclude their own selection).
- NULL group: `facets()` omits rows lacking the field; for a scalar field that
  count is exactly `matched_count − Σ(facet counts)`, emitted as a NULL-key row
  so results equal the normal plan.

No SFST format change; no new sfst API needed (facets()/matched_count() sufficed;
the only sfst-side fact required — per-chunk KvId range — was already derivable
from META, and tier came from the existing field_table).

Validation:
- `facet_pushdown_matches_normal_plan`: pushed result == plain-SessionContext
  result (the normal aggregation oracle), incl. the NULL group; EXPLAIN shows
  the `SfstFacet` node fired.
- `facet_pushdown_falls_back`: high-card group column and WHERE-on-group-column
  both keep the normal plan (no `SfstFacet` in EXPLAIN).
- Bench: `GROUP BY` low-card field 64.7ms (normal) → 21.6ms (pushed), identical
  results. Modest absolute win because eligible (low/mid) fields are already
  cheap to aggregate; the advantage is big-O (O(distinct values) vs O(rows)),
  widening with row count.
- clippy clean; 84 sfst lib + 10 integration + doctest pass.

Follow-ups (tracked): `date_bin`/timeline GROUP BY pushdown (the 2-D time×value
grid via `IndexReader::timeline`); multi-field GROUP BY.

## Stage D.2 — timeline (date_bin) pushdown (2026-06-29)

Extends the aggregation pushdown to time bucketing:
- 1-D: `SELECT date_bin(<interval>, timestamp), COUNT(*) GROUP BY 1` → new sfst
  primitive `IndexReader::timeline_totals(filter, grid)` (per-bucket matched
  counts; the field-less analogue of `matched_count`).
- 2-D: `SELECT date_bin(...), <field>, COUNT(*) GROUP BY 1, 2` → existing
  `IndexReader::timeline(field, filter, grid)` (time × value grid; `unset`
  becomes the per-bucket NULL group).

`SfstFacetRule` now also recognises `date_bin(<interval>, timestamp[, origin])`:
parses the `IntervalMonthDayNano` stride (rejects month-based / variable-width),
aligns a `Grid` to date_bin's `origin + k*stride` boundaries sized to the file's
time range (clamped by any WHERE time window, capped at 1e6 buckets → else fall
back), and emits `SfstTimelineNode` → `SfstTimelineExec`. Column order follows
the Aggregate's group_expr (date_bin and value column positions tracked). Same
fallback rules as facets (value field low/mid scalar, WHERE translatable and not
on the value field, pure COUNT(*)).

sfst additions this stage: `timeline_totals` (the only new API; `timeline`
already existed). `SfstTable::ts_bounds()` (datafusion-side accessor). No format
change.

Validation:
- `timeline_1d_pushdown_matches_normal` + `timeline_2d_pushdown_matches_normal`:
  pushed result == normal plan (the date_bin bucket-alignment correctness check),
  incl. the 2-D per-bucket NULL group; EXPLAIN shows the `SfstTimeline` node.
- Bench: 1-D `date_bin('10s')` 38.5ms (normal) → 18.1ms (pushed).
- clippy clean; 84 sfst lib + 12 integration + doctest pass.

Remaining follow-up: multi-field (non-time) GROUP BY (poor fit for the facet
index; stays a correct fallback).
