# nlogql Evaluator Plan

Implementation plan for the query evaluator that takes a parsed
LogQL AST and produces query results against an SFST data source.
Builds on the parser landed in `nlogql-implementation-plan.md`.

## Goal

A binary `nlogql-query` that accepts:

```
nlogql-query <SFST-file> '<logql-expression>'
```

and prints matching log lines (for log-path queries) or time-series
samples (for metric-path queries) as JSON, one record per line.

The same evaluator code is later wired into `otel-ledger`'s
`otel-logs` function-call handler. The CLI exists primarily as a
fast feedback loop during development and as a permanent debug
tool.

## Scope

End-to-end query execution against a **single SFST file**. No
multi-file fan-out, no remote fetch, no catalog awareness, no WAL
scan. Those layers join later, behind the same `Backend` trait.

## Confirmed decisions

- **Library + thin binary.** `nlogql-eval` is a new library crate;
  `nlogql-query` is a small binary in the same crate's `bin/` (or
  a separate `nlogql-cli` crate). The library is the integration
  surface for `otel-ledger`'s future function-call use.
- **AST → IR lowering.** The AST mirrors Loki's grammar; the IR is
  for execution. Lowering normalizes equivalent surface forms,
  catches type errors once, and shrinks the evaluator's surface
  area.
- **`Backend` trait abstraction**, modelled on the parallel
  PromQL experiment's `pql::Backend`. Two impls land in this
  plan: `MemBackend` (testing) and `SfstBackend` (production).
  `RemoteSfstBackend` (catalog-driven, multi-file) is out of scope
  for this plan.
- **JSON output, NDJSON streamed.** One record per line. We don't
  buffer entire result sets. Format follows Loki's response shape
  closely enough for tooling reuse, but isn't a bug-for-bug match.
- **No optimizer.** First evaluator is straightforward — scan,
  filter, aggregate. Query rewrites (constant folding, predicate
  pushdown) come later as a separate plan if needed.
- **License**: GPL-3.0-or-later, matching the workspace and
  `nlogql`.

## Pushbacks against the obvious

- **"Build it directly into otel-ledger."** Tempting (it's where
  this code eventually lives) but couples the evaluator to a
  running agent, which makes iteration expensive. The CLI gives
  us a tight test-and-debug loop. The same library code merges
  back into otel-ledger when ready.
- **"Skip the IR, walk the AST directly."** The AST has many
  equivalent surface forms (e.g. pipeline-before vs pipeline-after
  RANGE in log range expressions; `==`/`=` both yielding `Eq`).
  Walking the AST means handling every surface form in the
  evaluator. The IR collapses these to one canonical shape, halving
  the evaluator's branch count.
- **"Build all backends now."** Premature. `MemBackend` for tests
  and `SfstBackend` for one file gives us full evaluator coverage;
  multi-file and remote can layer on once the trait shape is
  validated.

## Architecture sketch

```
                           ┌───────────────┐
        LogQL string  ───> │  nlogql       │  parse()
                           │  (parser)     │
                           └───────┬───────┘
                                   ▼ Expr (AST)
                           ┌───────────────┐
                           │  lowering     │  lower()
                           │  (in eval)    │
                           └───────┬───────┘
                                   ▼ Plan (IR)
                           ┌───────────────┐
                           │  evaluator    │  eval()
                           │  (in eval)    │  ◄──── Backend trait
                           └───────┬───────┘             │
                                   ▼                     ├─ MemBackend (tests)
                           QueryResult                   └─ SfstBackend (prod)
                                   ▼
                           ┌───────────────┐
                           │  output       │  ndjson
                           │  (in eval)    │
                           └───────────────┘
                                   ▼
                                  stdout
```

## Split rationale

Four phases, mirroring the parser plan's structure. Each SOW lands
as one self-contained commit named `nlogql-eval: SOW-N — short title`.

## Phase D — Lowering (AST → IR)

Goal: a typed `Plan` IR that the evaluator can consume without
revisiting AST quirks.

### SOW-D1 — Scaffold `nlogql-eval` crate

- Cargo.toml under `src/crates/nlogql-eval/`
- Register in workspace
- `lib.rs` with module skeleton: `plan`, `lower`, `eval`, `storage`,
  `output`, `error`
- Crate-level docs with the architecture diagram
- A `Plan` enum stub, `lower()` placeholder
- Builds clean with `cargo check`.

### SOW-D2 — Plan IR for log path

- `LogPlan { selector, filters: Vec<LogFilterStage>, output: LineOutput }`
- `LogFilterStage` collapses line filters + parser stages +
  label filters into one ordered sequence.
- Selector matchers normalized: `=`/`!=` against string,
  `=~`/`!~` against compiled regex (regex stays opaque in IR;
  compilation happens at eval start).
- Tests: lower a small set of log-path AST queries; assert IR
  shape.

### SOW-D3 — Plan IR for metric path

- `MetricPlan` with variants for range aggregation, vector
  aggregation, binary op, label_replace, vector literal.
- `RangeAggPlan { op: RangeOp, range_ns, offset_ns, log_plan,
  parameter, grouping, unwrap }`
- `VectorAggPlan { op, parameter, grouping, inner: Box<MetricPlan> }`
- `BinaryPlan { op, lhs, rhs, modifier }`
- Type-check at lower time:
  - `topk`/`bottomk`/`approx_topk` require parameter.
  - Other vector ops reject parameter (closes the leniency in
    EXPECTED_FAILS.md).
  - `quantile_over_time` requires parameter in [0, 1].
- Tests for each variant + each rejection case.

### SOW-D4 — `LowerError`, doc the boundaries

- Error type for lowering failures (semantic errors caught here).
- Public `lower(input: Expr) -> Result<Plan, LowerError>`.
- Document the AST/IR boundary in `nlogql-eval`'s crate docs.
- Test coverage: 95%+ of lowering branches.

## Phase E — Storage backend

Goal: a `Backend` trait that the evaluator drives, with two
implementations (one for tests, one for SFST).

### SOW-E1 — `Backend` trait

- `trait Backend { fn matching_streams(selector) -> impl Iterator<Item=StreamMeta>; fn lines_for(stream, time_range) -> impl Iterator<Item=LogLine>; }`
- `StreamMeta { labels: Vec<(String, String)>, signature: u64 }`
  (signature = hash of labels for dedup / join keys; mirrors the
  pql experiment's `SeriesMeta`).
- `LogLine { timestamp_ns: i64, line: String, labels: Vec<(K, V)> }`
- The trait stays small in this SOW; we extend it as the
  evaluator needs more (e.g., for `count`-style queries that
  don't need line text).

### SOW-E2 — `MemBackend`

- An in-memory `Vec<(StreamMeta, Vec<LogLine>)>`.
- Builder API for tests: `MemBackend::builder().stream(labels).line(ts, text)...`.
- Tests for selector matching, time-range filtering, regex matcher.
- This is the workhorse for evaluator unit tests in Phase F.

### SOW-E3 — `SfstBackend`

- Wrap `sfst::Reader`: open one `.sfst` file, expose its streams.
- Translate selector matchers to FST index probes.
- Yield `LogLine`s lazily.
- Tests against a small synthetic SFST file fixture.

## Phase F — Evaluator

Goal: walk a `Plan` IR with a `Backend` impl and produce results.

### SOW-F1 — Log-path evaluator (selector + line filters)

- `eval_log(plan: LogPlan, backend: &impl Backend) -> impl Iterator<Item=LogLine>`
- Resolve streams via backend, iterate lines, apply line-filter
  stages (`|=`, `!=`, `|~`, `!~`, `|>`, `!>`, `or`-chains, `ip()`).
- Tests via MemBackend with handful of streams + ~50 log lines.

### SOW-F2 — Label parsers (json / logfmt / regexp / pattern / unpack)

- Parsing inside the evaluator: each stage produces a label
  set, which subsequent stages can filter on.
- JSON parser via `serde_json::Value::pointer` for projection
  paths.
- Logfmt parser hand-rolled (small) or via the `logfmt` crate
  if license-compatible.
- Regex parser via the `regex` crate, capturing named groups.
- Pattern via a small handwritten matcher mirroring Loki's
  pattern syntax (`<label>` / `<_>`).
- Unpack via JSON expansion of a designated field.
- Tests with multi-stage pipelines.

### SOW-F3 — Label filters

- Apply `LabelFilter::String / Ip / Numeric / Duration / Bytes`
  against the current label set.
- And/Or compose. Parens already collapse via the AST.
- Tests covering each value type.

### SOW-F4 — Format stages

- `line_format`: render Go-template-style string against
  current labels + line. Use the `gtmpl` crate or implement a
  small interpreter for the subset Loki uses.
- `label_format`: rename labels and/or compute new values via
  template.
- Tests with realistic templates.

### SOW-F5 — Structural stages (drop / keep / decolorize)

- `drop`/`keep` mutate the label set.
- `decolorize` strips ANSI escape codes from line text.
- Tests.

### SOW-F6 — Metric path: range aggregations

- `eval_metric(MetricPlan) -> impl Iterator<Item=Sample>`
- `Sample { timestamp_ns, labels, value: f64 }`.
- Range aggregation: bucket lines into windows, fold per window.
- All 15 range ops with their semantics:
  - `rate` = count / range_seconds
  - `count_over_time` = count
  - `bytes_rate`, `bytes_over_time` over byte-typed unwrap
  - `*_over_time` family
  - `absent_over_time` returns 1 if no lines, else nothing
  - `quantile_over_time` needs unwrap
- Tests per op via MemBackend.

### SOW-F7 — Metric path: vector aggregations

- Group by `grouping` (labels), apply op (sum/avg/min/max/...).
- `topk`/`bottomk`/`approx_topk` with parameter.
- `sort`/`sort_desc`.
- Tests with multi-stream MemBackend inputs.

### SOW-F8 — Metric path: binary ops

- Scalar-scalar, scalar-vector, vector-vector arithmetic.
- Vector matching (`on(...)` / `ignoring(...)`).
- Cardinality (`group_left` / `group_right`).
- `bool` modifier for comparison ops.
- `label_replace` and `vector(N)`.
- Tests covering all 15 binops + modifier combinations.

## Phase G — CLI binary

### SOW-G1 — `nlogql-query` binary

- Argv parsing: `<sfst-file>` `<query>`; flag for time range
  (`--start`, `--end`) defaulting to "all".
- Open SFST, parse + lower query, eval, stream NDJSON to stdout.
- Exit codes: 0 on success, 1 on parse/lower error, 2 on I/O error.
- README under the crate describing example usage.
- End-to-end test: build a synthetic SFST, run the binary against
  a query, assert stdout.

### SOW-G2 — Documentation pass

- Crate-level docs in `nlogql-eval` listing the public surface.
- Wire up doctests for the top-level functions.
- Add a "How to read a query plan" debugging section explaining
  `Plan` Debug output.

## Out of scope (separate plans)

- **Multi-file evaluation.** Discover relevant SFSTs via catalog,
  fan out scans, merge results. Comes after the single-file path
  is solid.
- **Remote-fetch SFSTs.** Read from object storage, with caching.
- **Optimizer.** Constant folding, predicate / projection pushdown
  into the backend.
- **Live tailing.** Streaming queries against actively-written
  WALs.
- **Function-call wiring.** Integrating into otel-ledger's
  `otel-logs` handler. This is the eventual home but lands as a
  separate plan once the library is stable.

## Estimation

Per-SOW effort, "session" ≈ half a day:

- D1: 0.5 — scaffolding
- D2: 1.5 — log path is straightforward
- D3: 2 — metric path has more types to model
- D4: 1
- E1: 1
- E2: 1
- E3: 2 — SFST integration may surface surprises
- F1: 1.5
- F2: 2.5 — parser implementations each take real care
- F3: 1
- F4: 2 — Go-template-ish render
- F5: 0.5
- F6: 2 — 15 range ops with quirks
- F7: 2 — vector aggs incl. parameter forms
- F8: 2.5 — vector matching + label_replace
- G1: 1.5
- G2: 1

Total: ~25 sessions. Less than the parser plan's 20 because we're
reusing more existing code (sfst, regex, serde_json) and the IR
shape is well-understood from the pql experiment.

## Naming convention

Commit subjects: `nlogql-eval: SOW-N — short title`.
The `-N` is digits within phase, e.g. `D2`, `F6`.
Plan amendments land here as `**Amended YYYY-MM-DD:**` lines.

## Open questions

These should be resolved before SOW-D1:

1. **One crate or two?** Add a `bin/` to `nlogql-eval`, or a
   separate `nlogql-cli` crate? Single-crate is simpler;
   separate-crate keeps the library dependency lean (no clap,
   no serde for stdout formatting). Lean: single crate with a
   feature-gated `bin` target.

2. **Template engine for `line_format` / `label_format`?** Loki
   uses Go's `text/template` semantics. Options:
   - `gtmpl` crate (Go-template subset, last published 2024)
   - Hand-roll a small interpreter for the `{{ .label }}` subset
     Loki actually uses
   - Defer entirely — return raw line unchanged in SOW-F4 and
     fold in real templating later.

3. **Time-range source.** Where does the `[start, end)` window
   come from for non-metric queries? CLI flag with defaults?
   Backend-derived (the SFST's natural time range)? Both with
   CLI overriding? Lean: CLI flag with `--start`/`--end` defaulting
   to the SFST's full extent.

4. **Output format compatibility.** Match Loki's
   `/loki/api/v1/query` JSON shape exactly, or roll our own?
   Lean: NDJSON one-record-per-line first (simple, easy to
   compose), Loki-shape JSON later if/when needed for tooling.
