# nlogql Implementation Plan

Implementation plan for `nlogql` — Netdata's LogQL parser. Built from
scratch against Loki's grammar as the semantics oracle. The reference
yacc grammar lives at `~/repos/loki/pkg/logql/syntax/syntax.y`.

## Scope

Parser only for this plan. The pipeline is **string → AST**. Lowering,
IR design, and evaluator come in follow-up plans, after the parser is
solid enough that downstream design can rely on it.

## Confirmed decisions

- **Parser library:** `chumsky = "0.13"`. Combinator composition pays
  off when we extend LogQL with Netdata-specific operators later.
- **No separate lexer.** chumsky parses straight from chars. A token
  layer can be added later if needed for syntax highlighting or
  IDE integration.
- **License:** GPL-3.0-or-later. Matches the workspace and is the
  same posture the parallel PromQL experiment took.
- **Reference, not source.** Loki is AGPL-3.0; we read `syntax.y`,
  `ast.go`, `lex.go`, and the `parser_test.go` corpus as a *spec*,
  but copy no code. Local-only verbatim copies of those files live
  at `~/.cache/nlogql-loki-reference/` so we have a stable reference
  during development. They are never committed or redistributed.
  A cleaned vendored copy of the third-party (MIT) `logql` crate
  lives at `~/repos/crates/logql/` for AST shape comparison only.
- **Errors via `chumsky::error::Rich<char>`**, mapped to our
  `ParseError` at the public-API boundary. We do not depend on
  `ariadne` — consumers can add it if they want CLI rendering.
- **Full-input consumption is non-negotiable.** Every `parse()` call
  ends with `.then_ignore(end())`. No silent suffix-drop.
- **Source spans on every AST node.** Byte ranges into the input
  string, captured via `MapExtra::span()`.

## Pushbacks against the vendored `logql` crate

The audit identified three structural defects that this rewrite
exists to fix:

1. **No precedence** — `Expr::parse` was right-associative regardless
   of operator. Pratt climber with Loki's yacc precedence is mandatory.
2. **Silent partial-parse** — `IResult<&str, T>` left callers to
   verify EOF. We enforce it in the top-level entry-point.
3. **No spans, no `Display`, no useful errors** — all addressed at
   the foundation rather than retrofitted.

## Split rationale

Three phases mirroring the natural fault line in LogQL itself: the
**log path** (returns log lines) and the **metric path** (returns
time series) are largely independent, joined only at the log-range
expression. A quality phase wraps them up.

Each SOW is one focused work session — small enough to land as a
self-contained commit named `nlogql: SOW-NN — short title`.
The SOW boundary is a green build with new tests; not "the whole
parser works."

Why this is split into ~18 SOWs and not one bundled PR:

- Each SOW exercises a different part of the grammar — easy to
  bisect a regression.
- Parser bugs love to compound: a fix to label-filter parsing might
  break line-filter parsing if both landed together. Small SOWs
  catch this fast.
- The team can review productions in lexical order matching `syntax.y`.

## Phase A — Log Query Path

Goal: parse any LogQL query that returns log lines, end-to-end.

### SOW-01 — Lexical primitives

Lexical bits that every other production uses. Land them once.

- `text::ident()` adapters for LogQL identifier rules.
- String literals: `"..."`, `'...'`, backtick `` `...` ``. Escape
  handling matches Loki's `lex.go`.
- Numeric literals: int, float (incl. scientific notation), hex.
- Duration literals: `5s`, `10m`, `1h30m`, `7d` — see `lex.go:lexDuration`.
- Byte literals: `100B`, `1KiB`, `1MB` — see `lex.go:lexBytes`.
- Comments: `# ...` to end of line.
- Whitespace helper that also eats comments.

AST: nothing yet; these are leaf parsers exported for re-use.

Reference: `syntax.y:50-90` (token block), `lex.go:80-300`.

### SOW-02 — Stream selector and matchers

Smallest meaningful LogQL query: `{app="foo"}`.

- AST: `MatcherOp { Eq, NotEq, Match, NotMatch }`, `Matcher { name, op, value, span }`, `StreamSelector { matchers, span }`.
- Parsers: `{`, all four matcher ops, comma-separated matcher list, `}`.
- Top-level `parse()` accepts `{app="foo", env=~"prod.*"}` and rejects malformed inputs with span-located errors.

Reference: `syntax.y:matchers,matcher`, `ast.go:MatchersExpr,VectorSelector`.

### SOW-03 — Line filters

The "grep on logs" operators: `|=`, `!=`, `|~`, `!~`, plus the pattern variants `|>`, `!>` (Loki 2.9+).

- AST: `LineFilterOp` (6 variants), `LineFilter { op, value, ip_pattern, span }`, `LineFilters(Vec<LineFilter>)`.
- `or`-chaining: `|= "a" or "b"` is one filter with multiple values.
- IP variant: `|= ip("10.0.0.0/8")`.
- Top-level `parse()` accepts `{app="foo"} |= "error" !~ "noise"`.

Reference: `syntax.y:lineFilter`, `ast.go:LineFilterExpr`.

### SOW-04 — Parser stages

The "extract fields" operators: `| json`, `| logfmt`, `| regexp "..."`, `| pattern "..."`, `| unpack`.

- AST: `ParserStage` enum with sub-variants for each.
- Plain forms (`| json`) and expression forms (`| json a="b.c"`, `| logfmt a, b="c"`).
- Parser flags: `--strict`, `--keep-empty`.
- Top-level `parse()` accepts `{app="foo"} | json | line_format "{{.user}}"` (line_format added in SOW-06; this case checks composition).

Reference: `syntax.y:labelParser,jsonExpressionParser,logfmtExpressionParser`, `ast.go:LabelParserExpr`.

### SOW-05 — Label filters

Filter after parsing extracted fields: `| status = 200`, `| latency > 100ms`, `| size > 1KiB`, `| ip = ip("10.0.0.0/8")`, plus `and`/`or`/parens.

- AST: `LabelFilter` enum with `String`, `Regex`, `Numeric`, `Duration`, `Bytes`, `Ip`, `Compound(And/Or)` variants.
- All comparison ops (`=`, `!=`, `<`, `<=`, `>`, `>=`, `==`).
- Top-level `parse()` handles compound expressions with parens.

Reference: `syntax.y:labelFilter`, `ast.go:LabelFilterExpr`.

### SOW-06 — Line format + label format

Rewrite output stages: `| line_format "{{ .ip }}"`, `| label_format new=old, x="{{ .y }}"`.

- AST: `LineFormatStage { template, span }`, `LabelFormatStage { items: Vec<LabelFormatItem>, span }`.
- Go template strings: treat the contents as opaque strings (we don't parse Go templates; that's an evaluator concern).
- `label_format` keyword consumption is enforced (the vendored crate's bug).

Reference: `syntax.y:lineFormatExpr,labelFormatExpr`, `ast.go:LineFmtExpr,LabelFmtExpr`.

### SOW-07 — Structural stages

The "trim the result" stages: `| drop`, `| keep`, `| distinct`, `| decolorize`.

- AST: four small enum variants with span.
- Comma-separated label lists for `drop` / `keep`.

Reference: `syntax.y:dropLabelsExpr,keepLabelsExpr,decolorizeExpr`, `ast.go:DropLabelsExpr,KeepLabelsExpr,DecolorizeExpr,DistinctFilterExpr`.

**Exit criterion for Phase A:** any well-formed LogQL query that
returns log lines parses successfully. Includes pipeline composition
across all stage kinds. ~80% of real-world LogQL queries by volume.

## Phase B — Metric Query Path

Goal: parse any LogQL query that returns time series.

### SOW-08 — Log range expression

The bridge from log-path to metric-path: `{app="foo"} |= "x" [5m]` and `[5m] offset 10m`.

- AST: `LogRangeExpr { selector, pipeline, range, offset, span }`.
- Range duration parsing reuses SOW-01 primitives.
- `offset` modifier with negative durations.

Reference: `syntax.y:logRangeExpr,offsetExpr`, `ast.go:LogRange,OffsetExpr`.

### SOW-09 — Range aggregations

The 15 `*_over_time` family: `rate`, `count_over_time`, `bytes_rate`, `bytes_over_time`, `avg/sum/min/max/stdvar/stddev/quantile/first/last/absent_over_time`, `rate_counter`.

- AST: `RangeOp` enum (15 variants), `RangeAggregationExpr { op, log_range, grouping, parameter, span }`.
- `grouping` (`by(...)`/`without(...)`) and parameter for `quantile_over_time`.
- Special form: `quantile_over_time(0.99, {...} | unwrap latency [5m])` — needs unwrap; depends on SOW-10 land order. We accept it syntactically and validate semantics at lower time.

Reference: `syntax.y:rangeAggregationExpr,rangeOp`, `ast.go:RangeAggregationExpr`.

### SOW-10 — Unwrap expression

The unwrap stage and its conversion ops: `| unwrap latency`, `| unwrap duration(latency)`, `| unwrap bytes(size)`, `| unwrap duration_seconds(latency)`.

- AST: `UnwrapExpr { conv_op: Option<ConvOp>, identifier, post_filters: Option<LabelFilters>, span }`.
- Lives inside the log-range expression; lifecycle is tied to SOW-08.

Reference: `syntax.y:unwrapExpr,convOp`, `ast.go:UnwrapExpr`.

### SOW-11 — Vector aggregations

The 12 vector-level operators: `sum`, `avg`, `min`, `max`, `stddev`, `stdvar`, `count`, `bottomk`, `topk`, `sort`, `sort_desc`, `approx_topk`.

- AST: `VectorOp` enum, `VectorAggregationExpr { op, expr, grouping, parameter, span }`.
- Parameter form: `topk(5, ...)`, `approx_topk(5, ...)`.
- Grouping syntax mirrors SOW-09's.

Reference: `syntax.y:vectorAggregationExpr,vectorOp`, `ast.go:VectorAggregationExpr`.

### SOW-12 — Binary operators with Pratt precedence

The 15 binops: `+`, `-`, `*`, `/`, `%`, `^`, `==`, `!=`, `>`, `>=`, `<`, `<=`, `and`, `or`, `unless`.

- AST: `BinaryExpr { op, lhs, rhs, modifier, span }`, `BinaryOp` enum, `BinaryModifier { bool, matching: Option<VectorMatching>, group: Option<GroupSide>, include: Option<Labels> }`.
- Precedence table direct from `syntax.y:90-95`:
  - `or` (1, left)
  - `and unless` (2, left)
  - `== != > < >= <=` (3, left)
  - `+ -` (4, left)
  - `* / %` (5, left)
  - `^` (6, right)
- Use `chumsky::pratt::pratt()`.
- Tests must assert tree shape for `1 + 2 * 3`, `2 * 3 + 1`, `2 ^ 3 ^ 2` (right-assoc), etc.

Reference: `syntax.y:binOpExpr,boolModifier,onOrIgnoringModifier,binOpModifier`, `ast.go:BinOpExpr`.

### SOW-13 — Misc metric expressions

The leftovers: `label_replace(...)`, `vector(N)`, bare literal expressions.

- AST: `LabelReplaceExpr`, `VectorExpr(f64)`, literal expression.
- `label_replace` has six string-argument slots — straightforward but tedious.

Reference: `syntax.y:labelReplaceExpr,vectorExpr,literalExpr`, `ast.go:LabelReplaceExpr,VectorExpr,LiteralExpr`.

### SOW-14 — Subqueries

The `[range:step]` form: `rate({app="foo"}[5m])[10m:1m]`.

- AST: `SubqueryExpr { expr, range, step, offset, span }`.
- Step is optional: `[10m:]` defaults step to the global resolution.

Reference: `syntax.y:logRangeExpr` (subquery rule), `ast.go:RangeAggregationExpr` (Loki overloads — read carefully).

**Exit criterion for Phase B:** any well-formed LogQL query
parses, including metric queries with full operator precedence.
Feature parity with Loki's grammar (modulo `variants(...)` which is
Loki-experimental and out of scope).

## Phase C — Quality

Goal: ship-ready parser.

### SOW-15 — Error messages

Add `.labelled()` calls at every production. Custom error formatter
that produces Loki-style messages: `parse error at line N, col M:
expected X, found Y`. Map `Rich<char>` into our `ParseError` with
labelled context. Round-trip through chumsky's
`Rich::reason()` and `Rich::expected()`.

### SOW-16 — Compliance pass

By this point every prior SOW has hand-ported test inputs from
`~/.cache/nlogql-loki-reference/parser_test.go` into nlogql's own
Rust test modules (with nlogql's expected AST, not Loki's). This
SOW is the **gap audit**: walk through Loki's full `ParseTestCases`
slice one more time, identify any cases we missed (productions
covered by Phases A and B but with edge-case inputs we didn't
include), and either add them to our Rust tests or document them
in `nlogql/EXPECTED_FAILS.md` if we deliberately choose not to
support that case.

Target: every well-formed Loki test input either has a
matching nlogql test or an explicit entry in `EXPECTED_FAILS.md`
with a one-line reason. No silent gaps.

### SOW-17 — `Display` impls + docs

Implement `Display` on every AST node such that
`parse(input)?.to_string()` produces canonical LogQL. Verify with
a round-trip property test against the corpus.

Crate-level docs with a worked example. Module-level docs for each
of `ast`, `parser`, `span`, `error`.

### SOW-18 — Fuzz

`cargo-fuzz` harness with two properties:

1. **No-panic:** any byte string either parses or returns
   `Result::Err(_)`. No panics, no unbounded recursion, no infinite
   loops.
2. **Round-trip:** `parse(display(parse(input)?)) == parse(input)?`
   for any input that parses.

Run until the corpus rate plateaus or a crash is found.

**Exit criterion for Phase C:** > 95% Loki corpus pass rate,
zero fuzz panics over a 1-hour run, full `Display` round-trip,
documentation complete.

## Out of Scope

These are downstream of the parser. Separate plans cover each:

- **Lowering / IR.** Plan AST → typed IR similar to the pql
  experiment's `Plan` IR. Catches type errors once at lower time.
- **Evaluator.** Plan IR → actual log/metric output. Will lift
  the pql experiment's `Backend` / `BackendQuery` trait pattern
  to a LogQL-shaped `LogBackend` / `MetricBackend`.
- **Backend impls.** `SfstBackend` (production), `MemBackend`
  (testing). Possibly a `RemoteSfstBackend` for catalog-sourced
  remote SFSTs.
- **Output formatting.** Serialization to Loki-compatible JSON
  for the `otel-logs` function-call response.
- **Optimizer.** Query rewrites (constant folding, filter
  pushdown).

## Estimation

Rough per-SOW effort (a "session" is one focused stretch of work,
~half a day):

- SOW-01: 1 session — lots of small parsers, none deep.
- SOW-02: 1 session — small AST, well-defined.
- SOW-03: 1 session — or-chaining is the only tricky bit.
- SOW-04: 2 sessions — many sub-variants, parser flags.
- SOW-05: 1.5 sessions — and/or/paren composition.
- SOW-06: 1 session.
- SOW-07: 0.5 sessions — almost trivial.
- SOW-08: 1 session.
- SOW-09: 1.5 sessions — 15 ops, grouping.
- SOW-10: 0.5 sessions.
- SOW-11: 1 session.
- SOW-12: 2 sessions — Pratt setup + precedence test matrix.
- SOW-13: 0.5 sessions.
- SOW-14: 1 session.
- SOW-15: 1.5 sessions — error message phrasing iterates.
- SOW-16: 2 sessions — corpus extraction, runner, gap doc.
- SOW-17: 1 session.
- SOW-18: 1 session — initial harness; runs onwards autonomously.

Total: ~20 sessions of focused work.

## Naming convention

Commit subjects: `nlogql: SOW-NN — short title`.
Branch naming: caller's preference (no convention enforced).
Plan amendments land here, in this file, with a `**Amended YYYY-MM-DD:**`
prefix on the changed lines.
